use crate::{
    ShardPolicy,
    task_locals::{get_shard_ind, get_shard_policy, get_shift, set_shard_ind},
};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::{UnsafeCell},
    fmt::Debug,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::task::yield_now;

// Enum used in try_acquire_shard to determine if task
// is enqueue or dequeue
#[derive(Debug, PartialEq, Eq)]
enum Acquire {
    Enqueue,
    Dequeue,
}

/// A sharded ring (circular) buffer struct that can only be used in an *async environment*.
#[derive(Debug)]
pub struct LFShardedRingBuf<T> {
    // Used to determine which shard a thread-task pair should work on
    // CachePadded to prevent false sharing
    shard_locks: Box<[CachePadded<AtomicBool>]>,
    // Multiple InnerRingBuffer structure based on num of shards
    // CachePadded to prevent false sharing
    inner_rb: Box<[CachePadded<InnerRingBuffer<T>>]>,
    poisoned: AtomicBool,
}

// An inner ring buffer to contain the items, enqueue, and dequeue index for LFShardedRingBuf struct
#[derive(Debug, Default)]
struct InnerRingBuffer<T> {
    items: Box<[UnsafeCell<MaybeUninit<T>>]>,
    enqueue_index: AtomicUsize,
    dequeue_index: AtomicUsize,
    job_count: AtomicUsize,
}

/// Implements the InnerRingBuffer functions
impl<T> InnerRingBuffer<T> {
    /// Instantiates the InnerRingBuffer
    #[inline(always)] // inline because this could be called often
    fn new(capacity: usize) -> Self {
        InnerRingBuffer {
            items: {
                let mut vec = Vec::with_capacity(capacity);
                for _i in 0..capacity {
                    vec.push(UnsafeCell::new(MaybeUninit::uninit()));
                }
                vec.into_boxed_slice()
            },
            enqueue_index: AtomicUsize::new(0),
            dequeue_index: AtomicUsize::new(0),
            job_count: AtomicUsize::new(0),
        }
    }
}

impl<T> LFShardedRingBuf<T> {
    /// Instantiates the LFShardedRingBuf.
    ///
    /// Note: The capacity of this buffer will always be rounded up to
    /// the next positive integer that is divisible by the provided shards.
    /// The provided shard value can only be a *positive* integer.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(s * c_s) where s is the number of shards and c_s
    /// is the capacity per shard (space usage also depends on T)
    pub fn new(capacity: usize, shards: usize) -> Self {
        assert!(capacity > 0, "Capacity must be positive");
        assert!(shards > 0, "Shards must be positive");
        assert!(
            capacity >= shards,
            "Capacity of buffer must be greater or equal to number of requested shards"
        );
        let capacity_per_shard = capacity / shards;
        let mut remainder = capacity % shards;
        Self {
            shard_locks: {
                let mut vec = Vec::with_capacity(shards);
                for _i in 0..shards {
                    vec.push(CachePadded::new(AtomicBool::new(false)));
                }
                vec.into_boxed_slice()
            },
            inner_rb: {
                let mut vec = Vec::with_capacity(shards);
                for _ in 0..shards {
                    if remainder == 0 {
                        vec.push(CachePadded::new(InnerRingBuffer::new(capacity_per_shard)));
                    } else {
                        vec.push(CachePadded::new(InnerRingBuffer::new(capacity_per_shard + 1)));
                        remainder -= 1;
                    }
                }
                vec.into_boxed_slice()
            },
            poisoned: AtomicBool::new(false),
        }
    }

    /// Returns the number of shards used in LFShardedRingBuf
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    pub fn get_num_of_shards(&self) -> usize {
        self.shard_locks.len()
    }

    /// Returns the capacity of a specific shard in LFShardedRingBuf
    /// or None if provided an invalid shard index
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    pub fn get_shard_capacity(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            debug_assert!(shard_ind >= self.shard_locks.len(), "Invalid shard index");
            return None;
        }

        Some(self.inner_rb[shard_ind].items.len())
    }

    /// Returns the total capacity of LFShardedRingBuf
    ///
    /// Time Complexity: O(s) where s is the number of shards used in this buffer
    ///
    /// Space Complexity: O(1)
    pub fn get_total_capacity(&self) -> usize {
        let mut count = 0;
        for shard in &self.inner_rb {
            count += shard.items.len()
        }
        count
    }

    /// Acquire the specific shard
    #[inline(always)]
    fn acquire_shard(&self, shard_ind: usize) -> bool {
        self.shard_locks[shard_ind]
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release the specific shard
    #[inline(always)]
    fn release_shard(&self, shard_ind: usize) {
        self.shard_locks[shard_ind].store(false, Ordering::Release);
    }

    /// Helper function for a task to acquire a specific shard within
    /// self.shard_locks for enqueuing or dequeuing purposes. Shard acquisition
    /// is done based on the policy placed inside SHARD_POLICY
    ///
    /// Time Complexity: O(s_t) where s_t comes from the consideration below:
    ///
    /// The time complexity of this depends on number of enqueuers and
    /// dequeuers tasks there are, shard count, and shard policies;
    /// ideally, you would have similar number of enqueuers and dequeuers tasks
    /// with the number of shards being greater than or equal to
    /// max(enqueuers task count, dequeurer task count)
    /// so that each task can find a shard to enqueue or dequeue off from
    ///
    /// Space Complexity: O(1)
    async fn try_acquire_shard(&self, acquire: Acquire) -> usize {
        /*
         * Tasks start off with a random shard_ind or
         * user provided initial shard ind value % self.shards
         * before going around a circle
         */
        let shard_count = self.shard_locks.len();
        let mut current = match get_shard_policy() {
            ShardPolicy::RandomAndSweep => frand(0..shard_count),
            _ => {
                match get_shard_ind() {
                    Some(val) => val % shard_count, // user provided index
                    None => {
                        let val = frand(0..shard_count);
                        set_shard_ind(val);
                        val
                    } // init rand shard for task to look at
                }
            }
        };

        let mut spins = 0;

        loop {
            // if poisoned and empty, get out of this loop
            if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
                break;
            }

            if self.acquire_shard(current) {
                /*
                 * Note here we don't check if the shard is full or empty first
                 * That's because these shard checks are done in an eventual memory
                 * consistent state, which means for safety, we need to acquire the
                 * shard first. Otherwise we may be acquiring a shard that was
                 * incorrectly seen as non empty or non full (or vice versa).
                 * and that could lead to addition/subtraction overflow on
                 * enqueue_ind, dequeue_ind, and job_count in InnerRingBuffer<T>
                 */
                if match acquire {
                    Acquire::Enqueue => !self.is_shard_full(current),
                    Acquire::Dequeue => !self.is_shard_empty(current),
                } {
                    // make sure that the shard index value is set to the
                    // next index it should look at instead of starting
                    // from its previous state
                    set_shard_ind((current + get_shift()) % shard_count);
                    break;
                } else {
                    /*
                     * Because a successful thread-task pair who can enqueue/dequeue
                     * will eventually release the shard_lock using Release
                     * memory ordering, all other threads who come here and
                     * acquire the shard only to realize that the shard is full
                     * as an enqueuer or empty as a dequeuer, can just drop this lock
                     * immediately in a memory safe ordering manner.
                     */
                    self.shard_locks[current].store(false, Ordering::Relaxed);
                }
            }

            // Move to the next index to check if item can be placed inside
            current = (current + get_shift()) % shard_count;
            spins += get_shift();

            // yield only once the enqueuers or dequeuers task has went one round through
            // the shard_job buffer
            if spins >= shard_count {
                if matches!(get_shard_policy(), ShardPolicy::ShiftBy { .. }) {
                    current = (current + 1) % shard_count;
                }
                spins = 0;
                yield_now().await;
            }
        }
        current
    }

    /// Increment num of jobs in shard by 1 and
    /// Release the occupation status of this shard atomically
    #[inline(always)]
    fn release_and_increment_shard(&self, shard_ind: usize) {
        // doing this relaxed loading, release storing is faster
        // than fetch_add + it's guaranteed that there's one writer
        // modifying this shard's job_count
        self.inner_rb[shard_ind].job_count.store(
            self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed) + 1,
            Ordering::Release,
        );
        self.shard_locks[shard_ind].store(false, Ordering::Release);
    }

    /// Grab inner ring buffer shard, enqueue the item, update the enqueue index
    #[inline(always)]
    fn enqueue_in_shard(&self, shard_ind: usize, item: T) {
        let inner = &self.inner_rb[shard_ind];
        // Ordering Relaxed can be used safely because this thread-task pair is the
        // only modifier of this index
        let enqueue_index = inner.enqueue_index.load(Ordering::Relaxed);
        let item_cell = inner.items[enqueue_index].get();
        // SAFETY: Only one thread will perform this operation and write to this
        // item cell
        unsafe {
            (*item_cell).write(item);
        }
        inner
            .enqueue_index
            .store((enqueue_index + 1) % inner.items.len(), Ordering::Relaxed);
    }

    /// Helper function to add an Option item to the RingBuffer
    /// This is necessary so that the ring buffer can be poisoned with None values
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    async fn enqueue_item(&self, item: T) {
        // acquire shard
        let current = self.try_acquire_shard(Acquire::Enqueue).await;

        // If poisoned, do not enqueue this item.
        if self.poisoned.load(Ordering::Relaxed) {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }

        self.enqueue_in_shard(current, item);
        self.release_and_increment_shard(current);
    }

    /// Adds an item of type T to the RingBuffer, *blocking* the thread until there is space to add the item.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub async fn enqueue(&self, item: T) {
        if self.poisoned.load(Ordering::Relaxed) {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }
        self.enqueue_item(item).await;
    }

    /// Decrement num of jobs in shard by 1 and
    /// Release the occupation status of this shard atomically
    #[inline(always)]
    fn release_and_decrement_shard(&self, shard_ind: usize) {
        // doing this relaxed loading, release storing is faster
        // than fetch_sub + it's guaranteed that there's one writer
        // modifying this shard's job_count
        self.inner_rb[shard_ind].job_count.store(
            self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed) - 1,
            Ordering::Release,
        );
        self.shard_locks[shard_ind].store(false, Ordering::Release);
    }

    /// Grab the inner ring buffer shard, dequeue the item, update the dequeue index
    #[inline(always)]
    fn dequeue_in_shard(&self, shard_ind: usize) -> T {
        let inner = &self.inner_rb[shard_ind];
        let dequeue_index = inner.dequeue_index.load(Ordering::Relaxed);
        // SAFETY: Only one thread will perform this operation
        // And it's guaranteed that an item will exist here
        let item = unsafe { (*inner.items[dequeue_index].get()).assume_init_read() };
        inner
            .dequeue_index
            .store((dequeue_index + 1) % inner.items.len(), Ordering::Relaxed);
        item
    }

    /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn dequeue(&self) -> Option<T> {
        let current = self.try_acquire_shard(Acquire::Dequeue).await;
        if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
            return None;
        }
        let item = self.dequeue_in_shard(current);
        self.release_and_decrement_shard(current);
        Some(item)
    }

    /// Sets the poison flag of the ring buffer to true. This will prevent enqueuers
    /// from enqueuing anymore jobs if this method is called while enqueues are occuring.
    /// However you can use this if you want graceful exit of dequeuers tasks completing
    /// all available jobs enqueued first before exiting.
    #[inline(always)]
    pub fn poison(&self) {
        self.poisoned.store(true, Ordering::Relaxed);
    }

    /// Clears the poison flag.
    #[inline(always)]
    pub fn clear_poison(&self) {
        self.poisoned.store(false, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    /// Clears the LFShardedRingBuf back to an empty state.
    ///
    /// Note: This is a sync function and should be used in a sync
    /// manner. If you need to clear while performing concurrent
    /// operations, use [Self::concurrent_clear]
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space complexity: O(1)
    pub fn clear(&self) {
        // reset each shard's inner ring buffer
        for shard in 0..self.shard_locks.len() {
            let mut drop_index = self.inner_rb[shard].dequeue_index.load(Ordering::Relaxed);
            let stop_index = self.inner_rb[shard].enqueue_index.load(Ordering::Relaxed);
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued. *Note again* this method is not thread safe.
                unsafe {
                    ptr::drop_in_place((*self.inner_rb[shard].items[drop_index].get()).as_mut_ptr())
                }
                drop_index = (drop_index + 1) % self.inner_rb[shard].items.len();
            }
            self.inner_rb[shard]
                .enqueue_index
                .store(0, Ordering::Relaxed);
            self.inner_rb[shard]
                .dequeue_index
                .store(0, Ordering::Relaxed);
            self.inner_rb[shard].job_count.store(0, Ordering::Relaxed);
        }
    }

    /// Clears the LFShardedRingBuf back to an empty state.
    ///
    /// Note: This is an async function and should be used in an async
    /// manner. If you plan to clear the buffer from a single thread or after
    /// all tasks are completed, then you should use [Self::clear()]
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space complexity: O(1)
    pub async fn concurrent_clear(&self) {
        // reset each shard's inner ring buffer
        for shard in 0..self.shard_locks.len() {
            let mut drop_index = self.inner_rb[shard].dequeue_index.load(Ordering::Acquire);
            let stop_index = self.inner_rb[shard].enqueue_index.load(Ordering::Acquire);
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued. *Note again* this method is not thread safe.
                unsafe {
                    ptr::drop_in_place((*self.inner_rb[shard].items[drop_index].get()).as_mut_ptr())
                }
                drop_index = (drop_index + 1) % self.inner_rb[shard].items.len();
            }
            self.inner_rb[shard]
                .enqueue_index
                .store(0, Ordering::Release);
            self.inner_rb[shard]
                .dequeue_index
                .store(0, Ordering::Release);
            self.inner_rb[shard].job_count.store(0, Ordering::Release);
        }
    }

    /// Checks each shard in LFShardedRingBuf to see if it's empty.
    /// Note that this ring buffer checks emptiness in an eventually
    /// consistent manner and does not acquire the shard, so it's possible
    /// that an enqueuer task can enqueue an item while this check is occurring.
    /// It's recommended to use this only after all enqueuing operations have
    /// occurred.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        for i in 0..self.shard_locks.len() {
            if !self.is_shard_empty(i) {
                return false;
            }
        }
        // if it got to this point, then indeed it was empty at this point
        true
    }

    /// Checks to see if a specific shard is empty
    #[inline(always)]
    pub fn is_shard_empty(&self, shard_ind: usize) -> bool {
        self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed) == 0
    }

    /// Checks each shard in LFShardedRingBuf to see if it's full.
    /// Note that this ring buffer checks fullness in an eventually
    /// consistent manner and does not acquire the shard, so it's possible
    /// that a dequeuer task can dequeue an item while this check is occurring.
    /// It's recommended to use this while dequeuing operations are not
    /// occurring or have all completed.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        for i in 0..self.shard_locks.len() {
            if !self.is_shard_full(i) {
                return false;
            }
        }

        // if it got to this point, all the shards were indeed full
        true
    }

    /// Checks to see if a specific shard is full
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_shard_full(&self, shard_ind: usize) -> bool {
        self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed)
            == self.inner_rb[shard_ind].items.len()
    }

    /// Gets the enqueue index within a shard. Returns None if the shard
    /// index is invalid.
    ///
    /// Note the enqueue index is accessed in a Relaxed memory
    /// ordering manner.
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_enq_ind_for_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // grab enq val
        let inner = &self.inner_rb[shard_ind];
        let enq_ind = inner.enqueue_index.load(Ordering::Relaxed);

        Some(enq_ind)
    }

    /// Get the dequeue index within a shard. Returns None if the shard
    /// index is invalid.
    ///
    /// Note the dequeue index is claimed in a Relaxed memory
    /// ordering manner.
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_deq_ind_for_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // grab deq ind val
        let inner = &self.inner_rb[shard_ind];
        let deq_ind = inner.dequeue_index.load(Ordering::Relaxed);

        Some(deq_ind)
    }

    /// Gets the total number of jobs within a shard. Returns None if the shard
    /// index is invalid.
    ///
    /// Note that the job count is claimed in a Relaxed memory ordering manner.
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_job_count_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        Some(self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed))
    }

    /// Helper function to see if a given index inside of a shard does
    /// indeed contain a valid item
    #[inline(always)]
    fn is_item_in_shard(&self, item_ind: usize, shard_ind: usize) -> bool {
        let enqueue_ind = self.inner_rb[shard_ind]
            .enqueue_index
            .load(Ordering::Relaxed);
        let dequeue_ind = self.inner_rb[shard_ind]
            .dequeue_index
            .load(Ordering::Relaxed);

        if enqueue_ind > dequeue_ind {
            item_ind < enqueue_ind && item_ind >= dequeue_ind
        } else if enqueue_ind < dequeue_ind {
            item_ind >= dequeue_ind || item_ind < enqueue_ind
        } else {
            false
        }
    }

    /// Returns a clone of the item within the LFShardedRingBuf or
    /// None if the shard index/item index is invalid or if there exists
    /// no item inside that position
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
    ///
    /// Time Complexity: O(s_t * T_t)
    ///
    /// Space Complexity: O(T_s)
    ///
    /// Where O(T_t) and O(T_s) is the time and space complexity required to
    /// clone the internals of the T object itself and s_t is the time it takes
    /// to acquire the shard
    pub async fn get_item_in_shard(&self, item_ind: usize, shard_ind: usize) -> Option<T>
    where
        T: Clone,
    {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        if item_ind >= self.inner_rb[shard_ind].items.len() {
            return None;
        }

        // spin trying to grab the shard to ensure proper
        // snapshot of items, enqueue, and dequeue indices in
        // shard
        while !self.acquire_shard(shard_ind) {
            yield_now().await;
        }

        // clone item in shard
        let inner = &self.inner_rb[shard_ind];
        // SAFETY: We know for certain there's an item inside the ring buffer if
        // item index is in [dequeue ind, enqueue ind) + wraparound
        let item = unsafe {
            if self.is_item_in_shard(item_ind, shard_ind) {
                let val_ref = (*inner.items[item_ind].get()).assume_init_ref();
                Some(val_ref.clone())
            } else {
                None
            }
        };

        // release shard
        self.release_shard(shard_ind);

        item
    }

    /// Returns a clone of a specific InnerRingBuffer shard or
    /// None if the shard index is invalid
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
    ///
    /// Time Complexity: O(c_s * s_t * O(T_t))
    ///
    /// Space Complexity: O(c_s * O(T_s))
    ///
    /// Where c_s is the capacity in a shard O(T_t) and O(T_s) is the time and
    /// space complexity required to clone the internals of the T object itself,
    /// and s_t is the time it takes to acquire the shard
    pub async fn rb_items_at_shard(&self, shard_ind: usize) -> Option<Box<[Option<T>]>>
    where
        T: Clone,
    {
        if shard_ind >= self.shard_locks.len() {
            debug_assert!(shard_ind >= self.shard_locks.len(), "Invalid shard index");
            return None;
        }

        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            yield_now().await;
        }

        // clone items in shard
        let inner = &self.inner_rb[shard_ind];
        let items = {
            let mut vec = Vec::new();

            // SAFETY: We know for certain there's an item inside the ring buffer if
            // item index is in [dequeue ind, enqueue ind) + wraparound
            for i in 0..inner.items.len() {
                if self.is_item_in_shard(i, shard_ind) {
                    let val_ref = unsafe { (*inner.items[i].get()).assume_init_ref() };
                    vec.push(Some(val_ref.clone()))
                } else {
                    vec.push(None);
                }
            }

            vec.into_boxed_slice()
        };

        // release shard
        self.release_shard(shard_ind);

        Some(items)
    }

    /// Returns a clone of the LFShardedRingBuf in its current state
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
    ///
    /// Time Complexity: O(s * c_s * s_t * O(T_t))
    ///
    /// Space Complexity: O(s * c_s * O(T_s))
    ///
    /// Where s is the number of shards, c_s is the capacity in a shard,
    /// s_t is the take it takes to acquire the shard, and O(T_t) and O(T_s)
    /// is the time and space complexity required to clone the internals of
    /// the T object itself
    pub async fn rb_items(&self) -> Box<[Box<[Option<T>]>]>
    where
        T: Clone,
    {
        // acquire shards to obtain proper snapshot
        for i in 0..self.shard_locks.len() {
            while !self.acquire_shard(i) {
                yield_now().await;
            }
        }

        let mut vec = Vec::new();
        let inner = &self.inner_rb;

        // clone items in each shard
        for shard_ind in 0..inner.len() {
            let shard = &inner[shard_ind];
            let items = {
                let mut shard_vec = Vec::new();

                // SAFETY: We know for certain there's an item inside the ring buffer if
                // item index is in [dequeue ind, enqueue ind) + wraparound
                for i in 0..shard.items.len() {
                    if self.is_item_in_shard(i, shard_ind) {
                        let val_ref = unsafe { (*shard.items[i].get()).assume_init_ref() };
                        shard_vec.push(Some(val_ref.clone()))
                    } else {
                        shard_vec.push(None);
                    }
                }

                shard_vec.into_boxed_slice()
            };
            vec.push(items);
        }

        // release shard
        for i in 0..self.shard_locks.len() {
            self.release_shard(i);
        }

        vec.into_boxed_slice()
    }

    /// Print out the content inside the LFShardedRingBuf
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space Complexity: O(1)
    pub async fn print_buffer(&self)
    where
        T: Debug,
    {
        // sping to acquire shard
        for i in 0..self.shard_locks.len() {
            while !self.acquire_shard(i) {
                yield_now().await;
            }
        }

        // Print items out and release shard
        for shard in 0..self.shard_locks.len() {
            let inner_shard = &self.inner_rb[shard];
            print!("Shard {shard}: ");
            print!("[");

            // SAFETY: Only print values in between [dequeue_ind, enqueue_ind) + wraparound
            // otherwise print None as a placeholder value
            for i in 0..inner_shard.items.len() {
                if self.is_item_in_shard(i, shard) {
                    let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                    print!("{val_ref:?}, ");
                } else {
                    print!("None, ");
                }
            }

            print!("]");
            println!();
            self.release_shard(shard);
        }
    }
}

// Both InnerRingBuffer and LFShardedRingBuf should be safe for
// Sync traits
unsafe impl<T: Send> Sync for InnerRingBuffer<T> {}
unsafe impl<T: Send> Sync for LFShardedRingBuf<T> {}

// Destructor trait created for InnerRingBuffer<T> to clean up
// memory allocated onto MaybeUninit<T> when LFShardedRingBuf<T>
// goes out of scope
impl<T> Drop for InnerRingBuffer<T> {
    fn drop(&mut self) {
        let capacity = self.items.len();
        let mut drop_index = self.dequeue_index.load(Ordering::Relaxed);
        let stop_index = self.enqueue_index.load(Ordering::Relaxed);

        while drop_index != stop_index {
            unsafe { ptr::drop_in_place((*self.items[drop_index].get()).as_mut_ptr()) }
            drop_index = (drop_index + 1) % capacity;
        }
    }
}
