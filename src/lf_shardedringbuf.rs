use crate::{
    ShardPolicy,
    task_locals::{get_shard_ind, get_shard_policy, get_shift, set_shard_ind},
};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::{Cell, UnsafeCell},
    fmt::Debug,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::task::yield_now;

// Enum used in try_acquire_shard to determine if task
// is enqueue or dequeue
#[derive(Debug, PartialEq, Eq)]
enum Acquire {
    Enqueue,
    Dequeue,
    Poison,
}

/// A sharded ring (circular) buffer struct that can only be used in a *multi-threaded environment*,
/// using a [BoxedSlice] of CachePadded<InnerRingBuffers> under the hood.
#[derive(Debug)]
pub struct LFShardedRingBuf<T> {
    shards: usize,
    max_capacity_per_shard: usize,
    // Used to determine which shard a thread-task pair should work on
    // CachePadded to prevent false sharing
    shard_locks: Box<[CachePadded<AtomicBool>]>,
    // Multiple InnerRingBuffer structure based on num of shards
    // CachePadded to prevent false sharing
    inner_rb: Box<[CachePadded<InnerRingBuffer<T>>]>,
    poisoned: Cell<bool>,
}

// An inner ring buffer to contain the items, enqueue, and dequeue index for LFShardedRingBuf struct
#[derive(Debug, Default)]
struct InnerRingBuffer<T> {
    items: Box<[UnsafeCell<Option<T>>]>,
    enqueue_index: Cell<usize>,
    dequeue_index: Cell<usize>,
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
                    vec.push(UnsafeCell::new(None));
                }
                vec.into_boxed_slice()
            },
            enqueue_index: Cell::new(0),
            dequeue_index: Cell::new(0),
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
        Self {
            // currently don't need this line
            // capacity: (capacity as f64 / shards as f64).ceil() as usize * shards,
            shards,
            max_capacity_per_shard: (capacity + shards - 1) / shards,
            shard_locks: {
                let mut vec = Vec::with_capacity(shards);
                for _i in 0..shards {
                    vec.push(CachePadded::new(AtomicBool::new(false)));
                }
                vec.into_boxed_slice()
            },
            inner_rb: {
                let mut vec = Vec::with_capacity(shards);
                for _i in 0..shards {
                    vec.push(CachePadded::new(InnerRingBuffer::new(
                        (capacity + shards - 1) / shards,
                    )));
                }
                vec.into_boxed_slice()
            },
            poisoned: Cell::new(false),
        }
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
        self.shard_locks[shard_ind]
            .store(false, Ordering::Release);
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
         * 
         * The following statement below is DEPRECATED:
         * If it's a poison task, then it will go try to find
         * a shard to just enqueue a None item in there 
         */
        let mut current = match acquire {
            Acquire::Poison => 0,
            _ => {
                match get_shard_policy() {
                    ShardPolicy::RandomAndSweep => frand(0..self.shards),
                    _ => {
                        match get_shard_ind() {
                            Some(val) => val % self.shards, // user provided index
                            None => {
                                let val = frand(0..self.shards);
                                set_shard_ind(val);
                                val
                            } // init rand shard for task to look at
                        }
                    }
                }
            }
        };

        let mut spins = 0;

        loop {
            // if poisoned and empty, get out of this loop
            if self.poisoned.get() && self.is_empty() {
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
                    Acquire::Poison => !self.is_shard_full(current),
                } {
                    if acquire != Acquire::Poison {
                        // make sure that the shard index value is set to the
                        // next index it should look at instead of starting
                        // from its previous state
                        set_shard_ind((current + get_shift()) % self.shards);
                    }
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
                        self.shard_locks[current]
                            .store(false, Ordering::Relaxed);
                }
            }

            // Move to the next index to check if item can be placed inside
            if acquire != Acquire::Poison {
                current = (current + get_shift()) % self.shards;
                spins += get_shift();
            } else {
                current = (current + 1) % self.shards;
                spins += 1;
            }

            // yield only once the enqueuers or dequeuers task has went one round through
            // the shard_job buffer
            if spins >= self.shards {
                if acquire != Acquire::Poison
                    && matches!(get_shard_policy(), ShardPolicy::ShiftBy { .. })
                {
                    current = (current + 1) % self.shards;
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
        self.inner_rb[shard_ind]
            .job_count
            .store(self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed) + 1, Ordering::Release);
        self.shard_locks[shard_ind]
            .store(false, Ordering::Release);
    }

    /// Grab inner ring buffer shard, enqueue the item, update the enqueue index
    #[inline(always)]
    fn enqueue_in_shard(&self, shard_ind: usize, item: Option<T>) {
        let inner = &self.inner_rb[shard_ind];
        let enqueue_index = inner.enqueue_index.get();
        let item_cell = inner.items[enqueue_index].get();
        unsafe {
            *item_cell = item;
        }
        inner
            .enqueue_index
            .set((enqueue_index + 1) % self.max_capacity_per_shard);
    }

    /// Helper function to add an Option item to the RingBuffer
    /// This is necessary so that the ring buffer can be poisoned with None values
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    async fn enqueue_item(&self, item: Option<T>) {
        // acquire shard
        let current = match item {
            Some(_) => self.try_acquire_shard(Acquire::Enqueue).await,
            None => self.try_acquire_shard(Acquire::Poison).await,
        };

        // If poisoned, do not enqueue this item.
        if self.poisoned.get() {
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
        if self.poisoned.get() {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }
        self.enqueue_item(Some(item)).await;
    }

    /// Decrement num of jobs in shard by 1 and
    /// Release the occupation status of this shard atomically
    #[inline(always)]
    fn release_and_decrement_shard(&self, shard_ind: usize) {
        // doing this relaxed loading, release storing is faster
        // than fetch_sub + it's guaranteed that there's one writer
        // modifying this shard's job_count
        self.inner_rb[shard_ind]
            .job_count
            .store(self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed) - 1, Ordering::Release);
        self.shard_locks[shard_ind]
            .store(false, Ordering::Release);
    }

    /// Grab the inner ring buffer shard, dequeue the item, update the dequeue index
    #[inline(always)]
    fn dequeue_in_shard(&self, shard_ind: usize) -> Option<T> {
        let inner = &self.inner_rb[shard_ind];
        let dequeue_index = inner.dequeue_index.get();
        let item = unsafe { (*inner.items[dequeue_index].get()).take() };
        inner
            .dequeue_index
            .set((dequeue_index + 1) % self.max_capacity_per_shard);
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
        if self.poisoned.get() && self.is_empty() {
            return None;
        }
        let item = self.dequeue_in_shard(current);
        self.release_and_decrement_shard(current);
        item
    }

    #[deprecated = "Use the poison() method instead to terminate dequeuers"]
    /// Poisons the RingBuffer such that you can use this to denote when dequeuers tasks
    /// are done with working. Use this ONLY if you are fine with early termination of
    /// dequeuer tasks because it's possible for valid jobs to exist in the ring buffer
    /// that have not been dequeued yet.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub async fn poison_deq(&self) {
        if self.poisoned.get() {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }
        let _ = self.enqueue_item(None).await;
    }

    /// Sets the poison flag of the ring buffer to true. This will prevent enqueuers
    /// from enqueuing anymore jobs if this method is called while enqueues are occuring.
    /// However you can use this if you want graceful exit of dequeuers tasks completing
    /// all available jobs enqueued first before exiting.
    #[inline(always)]
    pub async fn poison(&self) {
        self.poisoned.set(true);
    }

    /// Clears the poison flag.
    #[inline(always)]
    pub async fn clear_poison(&self) {
        self.poisoned.set(false);
    }

    /// Clears the LFShardedRingBuf back to an empty state.
    ///
    /// Note: This is a sync function and should be used in a sync
    /// manner. Do not use this in an async task.
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space complexity: O(1)
    pub fn clear(&self) {
        // reset each shard's inner ring buffer
        for shard in 0..self.shards {
            for i in 0..self.max_capacity_per_shard {
                unsafe {
                    *self.inner_rb[shard].items[i].get() = None;
                }
            }
            self.inner_rb[shard].enqueue_index.set(0);
            self.inner_rb[shard].dequeue_index.set(0);
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
    fn is_shard_empty(&self, shard_ind: usize) -> bool {
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
    #[inline(always)]
    pub fn is_shard_full(&self, shard_ind: usize) -> bool {
        self.inner_rb[shard_ind].job_count.load(Ordering::Relaxed) == self.max_capacity_per_shard
    }

    /// Checks the next enqueue index within the LFShardedRingBuf
    ///
    /// Time Complexity: O(s_t) where s_t is how long it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn next_enqueue_index_for_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shards {
            println!("Invalid shard index");
            return None;
        }

        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            yield_now().await;
        }

        // grab enq val
        let inner = &self.inner_rb[shard_ind];
        let enq_ind = inner.enqueue_index.get();

        // release shard
        self.release_shard(shard_ind);

        Some(enq_ind)
    }

    /// Checks the next dequeue index within the LFShardedRingBuf
    ///
    /// Time Complexity: O(s_t) where s_t is how long it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn next_dequeue_index_for_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shards {
            println!("Invalid shard index");
            return None;
        }

        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            yield_now().await;
        }

        // grab deq ind val
        let inner = &self.inner_rb[shard_ind];
        let deq_ind = inner.dequeue_index.get();

        // release shard
        self.release_shard(shard_ind);
        Some(deq_ind)
    }

    /// Returns a clone of the item within the LFShardedRingBuf
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
    pub async fn get_item_in_shard(&self, item_index: usize, shard_ind: usize) -> Option<T>
    where
        T: Clone,
    {
        if shard_ind >= self.shards {
            println!("Invalid shard index");
            return None;
        }

        if item_index >= self.max_capacity_per_shard {
            println!("Invalid item index");
            return None;
        }

        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            yield_now().await;
        }

        // clone item in shard
        let inner = &self.inner_rb[shard_ind];
        let item = unsafe { (*inner.items[item_index].get()).clone() };

        // release shard
        self.release_shard(shard_ind);

        item
    }

    /// Returns a clone of a specific InnerRingBuffer shard in its current state
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
        if shard_ind >= self.shards {
            println!("Invalid shard index");
            return None;
        }

        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            yield_now().await;
        }

        // clone items in shard
        let inner = &self.inner_rb[shard_ind];
        let items = unsafe {
            let mut vec = Vec::new();
            for item in &inner.items {
                vec.push((*item.get()).clone());
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
        // acquire shards
        for i in 0..self.shard_locks.len() {
            while !self.acquire_shard(i) {
                yield_now().await;
            }
        }

        let mut vec = Vec::new();
        let inner = &self.inner_rb;

        // clone items in each shard
        for shard in inner {
            let items = unsafe {
                let mut shard_vec = Vec::new();
                for item in &shard.items {
                    shard_vec.push((*item.get()).clone());
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
        for shard in 0..self.shards {
            let inner = &self.inner_rb[shard];
            print!("Shard {shard}: ");
            print!("[");
            for item in &inner.items {
                unsafe {
                    print!("{:?}, ", *item.get());
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
