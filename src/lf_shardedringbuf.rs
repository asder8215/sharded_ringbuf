use crate::task_local_spawn::{get_shard_ind, set_shard_ind};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::{Cell, UnsafeCell},
    fmt::Debug,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

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
/// See the [Wikipedia article](https://en.wikipedia.org/wiki/Circular_buffer) for more info.
#[derive(Debug)]
pub struct LFShardedRingBuf<T> {
    capacity: usize,
    shards: usize,
    max_capacity_per_shard: usize,
    // Used to determine which shard a thread should work on
    // CachePadded to prevent false sharing
    shard_jobs: Box<[CachePadded<ShardJob>]>,
    // Multiple InnerRingBuffer structure based on num of shards
    // CachePadded to prevent false sharing
    inner_rb: Box<[CachePadded<InnerRingBuffer<T>>]>,
}

#[derive(Debug)]
struct ShardJob {
    occupied: AtomicBool,   // occupied status of shard
    job_count: Cell<usize>, // how many jobs are in shard
}

// An inner ring buffer to contain the items, enqueue, and dequeue index for LFShardedRingBuf struct
#[derive(Debug, Default)]
struct InnerRingBuffer<T> {
    items: Box<[UnsafeCell<Option<T>>]>,
    enqueue_index: Cell<usize>,
    dequeue_index: Cell<usize>,
}

impl ShardJob {
    fn new() -> Self {
        ShardJob {
            occupied: AtomicBool::new(false),
            job_count: Cell::new(0),
        }
    }
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
            capacity: (capacity as f64 / shards as f64).ceil() as usize * shards,
            shards,
            max_capacity_per_shard: (capacity + shards - 1) / shards,
            shard_jobs: {
                let mut vec = Vec::with_capacity(shards);
                for _i in 0..shards {
                    vec.push(CachePadded::new(ShardJob::new()));
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
        }
    }

    /// Acquire the specific shard
    #[inline(always)]
    fn acquire_shard(&self, shard_ind: usize) -> bool {
        self.shard_jobs[shard_ind]
            .occupied
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release the specific shard
    #[inline(always)]
    fn release_shard(&self, shard_ind: usize) {
        self.shard_jobs[shard_ind]
            .occupied
            .store(false, Ordering::Release);
    }

    /// Helper function for a thread to acquire a specific shard within
    /// self.shard_jobs for enqueuing or dequeuing purposes. It iterates
    /// in a ring buffer like manner per thread to give each shard equal weight.
    /// Yielding is done through exponential backoff + random jitter
    /// (capped at 20 ms) so that this function isn't fully occupying the CPU at
    /// all times.
    ///
    /// Time Complexity: O(s_t) where s_t comes from the consideration below:
    ///
    /// The time complexity of this depends on number of enquerer and
    /// dequerer threads there are and shard count; ideally, you would have similar
    /// number of enquerer and dequerer threads with the number of shards being
    /// greater than or equal to max(enquerer thread count, dequeurer thread count)
    /// so that each thread can find a shard to enqueue or dequeue off from
    ///
    /// Space Complexity: O(1)
    async fn try_acquire_shard(&self, acquire: Acquire) -> usize {
        /*
         * Threads start off with a random shard_ind or
         * user provided initial shard ind value (+ 1 % self.shards)
         * before going around a circle in the ring buffer
         * Fortunately no funny games can be played with SHARD_INDEX
         * because of the % self.shards ;)
         * If it's a poison task, then it will go try to find
         * a shard to just enqueue a None item in there
         */
        let mut current = match acquire {
            Acquire::Poison => 0,
            _ => {
                match get_shard_ind() {
                    Some(val) => {
                        let val = (val + 1) % self.shards;
                        set_shard_ind(val);
                        val
                    } // look at the next shard
                    None => {
                        let val = frand(0..self.shards);
                        set_shard_ind(val);
                        val
                    } // init rand shard for thread to look at
                }
            }
        };

        let mut spins = 0;
        let mut attempt: i32 = 0;

        loop {
            if self.acquire_shard(current) {
                if match acquire {
                    Acquire::Enqueue => !self.is_shard_full(current),
                    Acquire::Dequeue => !self.is_shard_empty(current),
                    Acquire::Poison => !self.is_shard_full(current),
                } {
                    if acquire != Acquire::Poison {
                        // make sure that the shard index value is set to the
                        // next index it should look at instead of starting
                        // from its previous state
                        set_shard_ind((current + 1) % self.shards);
                    }
                    break;
                } else {
                    self.release_shard(current);
                }
            }

            current = (current + 1) % self.shards;

            spins += 1;
            // yield only once the enquerer or dequerer thread has went one round through
            // the shard_job buffer
            if spins % self.shards == 0 {
                // yielding is done through exponential backoff + random jitter
                // max wait of 20ms; jitter allows the threads to wake up at
                // different ms timings
                let backoff_ms = (1u64 << attempt.min(5)).min(20) as usize;
                let jitter = frand(0..=backoff_ms) as u64;
                tokio::time::sleep(Duration::from_millis(jitter)).await;
                attempt = attempt.saturating_add(1); // Avoid overflow
            }
        }
        current
    }

    /// Increment num of jobs in shard by 1 and
    /// Release the occupation status of this shard atomically
    #[inline(always)]
    fn release_and_increment_shard(&self, shard_ind: usize) {
        self.shard_jobs[shard_ind]
            .job_count
            .set(self.shard_jobs[shard_ind].job_count.get() + 1);
        self.shard_jobs[shard_ind]
            .occupied
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

        self.enqueue_in_shard(current, item);
        self.release_and_increment_shard(current);
    }

    /// Adds an item of type T to the RingBuffer, *blocking* the thread until there is space to add the item.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub async fn enqueue(&self, item: T) {
        self.enqueue_item(Some(item)).await;
    }

    /// Decrement num of jobs in shard by 1 and
    /// Release the occupation status of this shard atomically
    #[inline(always)]
    fn release_and_decrement_shard(&self, shard_ind: usize) {
        self.shard_jobs[shard_ind]
            .job_count
            .set(self.shard_jobs[shard_ind].job_count.get() - 1);
        self.shard_jobs[shard_ind]
            .occupied
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
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn dequeue(&self) -> Option<T> {
        let current = self.try_acquire_shard(Acquire::Dequeue).await;
        let item = self.dequeue_in_shard(current);
        self.release_and_decrement_shard(current);
        item
    }

    /// Poisons the RingBuffer such that you can use this to denote when dequerer threads
    /// are done with working.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub async fn poison_deq(&self) {
        let _ = self.enqueue_item(None).await;
    }

    /// Clears the LFShardedRingBuf back to an empty state.
    ///
    /// To clear the RingBuffer *only* when it is *poisoned*, see [Self::clear_poison].
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space complexity: O(1)
    pub async fn clear(&self) {
        let mut attempt = 0;
        // acquire shards
        for i in 0..self.shard_jobs.len() {
            while !self.acquire_shard(i) {
                if attempt < 10 {
                    std::hint::spin_loop();
                    attempt += 1;
                } else {
                    tokio::task::yield_now().await;
                    attempt = 0;
                }
            }
        }

        // reset each shard's inner ring buffer
        for shard in 0..self.shards {
            for i in 0..self.max_capacity_per_shard {
                unsafe {
                    *self.inner_rb[shard].items[i].get() = None;
                }
            }
            self.inner_rb[shard].enqueue_index.set(0);
            self.inner_rb[shard].dequeue_index.set(0);
            self.shard_jobs[shard].job_count.set(0);
        }

        // release shards
        for i in 0..self.shard_jobs.len() {
            self.release_shard(i);
        }
    }

    /// Checks whether the LFShardedRingBuf is empty or not
    ///
    /// Time Complexity: O(s * s_t) where s_t is how long it takes to acquire shard(s)
    ///
    /// Space Complexity: O(1)
    pub async fn is_empty(&self) -> bool {
        let mut attempt = 0;
        // acquire shards
        for i in 0..self.shard_jobs.len() {
            while !self.acquire_shard(i) {
                if attempt < 10 {
                    std::hint::spin_loop();
                    attempt += 1;
                } else {
                    tokio::task::yield_now().await;
                    attempt = 0;
                }
            }
        }

        let res = self
            .shard_jobs
            .iter()
            .all(|shard| shard.job_count.get() == 0);

        // release shards
        for i in 0..self.shard_jobs.len() {
            self.release_shard(i);
        }

        res
    }

    #[inline(always)]
    pub fn is_shard_empty(&self, shard_ind: usize) -> bool {
        self.shard_jobs[shard_ind].job_count.get() == 0
    }

    /// Checks whether the LFShardedRingBuf is full or not
    ///
    /// Time Complexity: O(s * s_t) where s_t is how long it takes to acquire shard(s)
    ///
    /// Space Complexity: O(1)
    pub async fn is_full(&self) -> bool {
        let mut attempt = 0;
        // acquire shard
        for i in 0..self.shard_jobs.len() {
            while !self.acquire_shard(i) {
                if attempt < 10 {
                    std::hint::spin_loop();
                    attempt += 1;
                } else {
                    tokio::task::yield_now().await;
                    attempt = 0;
                }
            }
        }

        let res = self
            .shard_jobs
            .iter()
            .all(|shard| shard.job_count.get() == self.max_capacity_per_shard);

        // release shards
        for i in 0..self.shard_jobs.len() {
            self.release_shard(i);
        }

        res
    }

    #[inline(always)]
    pub fn is_shard_full(&self, shard_ind: usize) -> bool {
        self.shard_jobs[shard_ind].job_count.get() == self.max_capacity_per_shard
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

        let mut attempt = 0;
        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            if attempt < 10 {
                std::hint::spin_loop();
                attempt += 1;
            } else {
                tokio::task::yield_now().await;
                attempt = 0;
            }
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

        let mut attempt = 0;
        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            if attempt < 10 {
                std::hint::spin_loop();
                attempt += 1;
            } else {
                tokio::task::yield_now().await;
                attempt = 0;
            }
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

        let mut attempt = 0;
        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            if attempt < 10 {
                std::hint::spin_loop();
                attempt += 1;
            } else {
                tokio::task::yield_now().await;
                attempt = 0;
            }
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

        let mut attempt = 0;
        // spin trying to grab the shard
        while !self.acquire_shard(shard_ind) {
            if attempt < 10 {
                std::hint::spin_loop();
                attempt += 1;
            } else {
                tokio::task::yield_now().await;
                attempt = 0;
            }
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
        let mut attempt = 0;
        // acquire shards
        for i in 0..self.shard_jobs.len() {
            while !self.acquire_shard(i) {
                if attempt < 10 {
                    std::hint::spin_loop();
                    attempt += 1;
                } else {
                    tokio::task::yield_now().await;
                    attempt = 0;
                }
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
        for i in 0..self.shard_jobs.len() {
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
        let mut attempt = 0;
        // sping to acquire shard
        for i in 0..self.shard_jobs.len() {
            while !self.acquire_shard(i) {
                if attempt < 10 {
                    std::hint::spin_loop();
                    attempt += 1;
                } else {
                    tokio::task::yield_now().await;
                    attempt = 0;
                }
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
