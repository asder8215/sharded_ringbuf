use crossbeam_utils::CachePadded;
use std::{
    cell::UnsafeCell,
    fmt::{Debug, Write},
    mem::MaybeUninit,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;

// Enum used in try_acquire_shard to determine if task
// is enqueue or dequeue
#[derive(Debug, PartialEq, Eq)]
enum Acquire {
    Enqueue,
    EnqueueFull { batch_count: usize },
    Dequeue,
}

/// A sharded ring (circular) buffer struct that can only be used in an *async environment*.
#[derive(Debug)]
pub struct ShardedRingBuf<T> {
    /// Total capacity of the buffer
    capacity: AtomicUsize,
    /// Used to determine which shard a thread-task pair should work on
    /// CachePadded to prevent false sharing
    shard_locks: Box<[CachePadded<AtomicBool>]>,
    /// Multiple InnerRingBuffer structure based on num of shards
    /// CachePadded to prevent false sharing
    inner_rb: Box<[CachePadded<InnerRingBuffer<T>>]>,
    /// Poison signal of the ring buffer
    poisoned: AtomicBool,

    /// Used as a monotonic increasing counter to determine which
    /// shard should an enqueuer task place an item in
    /// Cache Padded to prevent false sharing if spawning an enqueuer
    /// task happens across multiple threads
    shard_enq: CachePadded<AtomicUsize>,

    /// Used as a monotonic increasing counter to determine which
    /// shard should a dequeuer task place an item in
    /// Cache Padded to prevent false sharing if spawning an dequeuer
    /// task happens across multiple threads
    shard_deq: CachePadded<AtomicUsize>,

    pub(crate) job_space_shard_notifs: Box<[Notify]>,
    pub(crate) job_post_shard_notifs: Box<[Notify]>,
}

// An inner ring buffer to contain the items, enqueue and dequeue index for ShardedRingBuf struct
#[derive(Debug, Default)]
struct InnerRingBuffer<T> {
    /// Box containing the content of the buffer
    items: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Where to enqueue at in the Box
    enqueue_index: AtomicUsize,
    /// Where to dequeue at in the Box
    dequeue_index: AtomicUsize,
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
        }
    }

    /// Helper function to see if a given index inside this buffer does
    /// indeed contain a valid item. Used in Drop Trait.
    #[inline(always)]
    fn is_item_in_shard(&self, item_ind: usize) -> bool {
        let enqueue_ind = self.enqueue_index.load(Ordering::Relaxed) % self.items.len();
        let dequeue_ind = self.dequeue_index.load(Ordering::Relaxed) % self.items.len();

        if enqueue_ind > dequeue_ind {
            item_ind < enqueue_ind && item_ind >= dequeue_ind
        } else if enqueue_ind < dequeue_ind {
            item_ind >= dequeue_ind || item_ind < enqueue_ind
        } else {
            false
        }
    }
}

impl<T> ShardedRingBuf<T> {
    /// Instantiates ShardedRingBuf
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(s * c_s) where s is the number of shards and c_s
    /// is the capacity per shard (space usage also depends on T)
    pub fn new(capacity: usize, shards: usize) -> Arc<Self> {
        assert!(capacity > 0, "Capacity must be positive");
        assert!(shards > 0, "Shards must be positive");
        assert!(
            capacity >= shards,
            "Capacity of buffer must be greater or equal to number of requested shards"
        );
        let capacity_per_shard = capacity / shards;
        let mut remainder = capacity % shards;
        Arc::new(Self {
            capacity: AtomicUsize::new(capacity),
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
                        vec.push(CachePadded::new(InnerRingBuffer::new(
                            capacity_per_shard + 1,
                        )));
                        remainder -= 1;
                    }
                }
                vec.into_boxed_slice()
            },
            poisoned: AtomicBool::new(false),
            shard_enq: CachePadded::new(AtomicUsize::new(0)),
            shard_deq: CachePadded::new(AtomicUsize::new(0)),
            job_post_shard_notifs: {
                let mut vec = Vec::with_capacity(shards);
                for _ in 0..shards {
                    vec.push(Notify::new());
                }
                vec.into_boxed_slice()
            },

            job_space_shard_notifs: {
                let mut vec = Vec::with_capacity(shards);

                for _ in 0..shards {
                    vec.push(Notify::new());
                }
                vec.into_boxed_slice()
            },
        })
    }

    /// Returns the number of shards used in this buffer
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_num_of_shards(&self) -> usize {
        self.shard_locks.len()
    }

    /// Returns the number of shards used in this buffer in
    /// an async manner
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline]
    pub async fn async_get_num_of_shards(&self) -> usize {
        self.shard_locks.len()
    }

    /// Returns the capacity of a specific shard in this buffer
    /// or None if provided an invalid shard index
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_shard_capacity(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        Some(self.inner_rb[shard_ind].items.len())
    }

    /// Returns the capacity of a specific shard in this buffer in
    /// an async manner or None if provided an invalid shard index
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline]
    pub async fn async_get_shard_capacity(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        Some(self.inner_rb[shard_ind].items.len())
    }

    /// Returns the total capacity of this buffer
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_total_capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Returns the total capacity of this buffer in an async manner
    ///
    /// Time Complexity: O(1)
    ///
    ///
    /// Space Complexity: O(1)
    #[inline]
    pub async fn async_get_total_capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Acquire the specific shard
    #[inline(always)]
    fn acquire_shard(&self, shard_ind: usize) -> bool {
        self.shard_locks[shard_ind]
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release the occupation status of this shard atomically
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
    async fn try_acquire_shard(&self, acquire: Acquire, shard_ind: usize) {
        loop {
            // if poisoned and empty, get out of this loop
            if self.poisoned.load(Ordering::Relaxed) && self.is_shard_empty(shard_ind) {
                break;
            }

            if self.acquire_shard(shard_ind) {
                /*
                 * We need to acquire the shard first to get a stable view of how many items
                 * are on the shard.
                 */
                if match acquire {
                    // for some reason checking if the shard is empty to enqueue full gives pretty stable
                    // timing performances. the con of doing this way is that you're not using the full
                    // benefit of enqueuing as much as you can into this shard buffer, but the pro is
                    // that even in a partial filled state, you let the dequeuer immediately remove the
                    // items here
                    // Acquire::EnqueueFull {batch_count: _} => self.is_shard_empty(current),

                    // checking if the shard is unable to contain a batch gives different range of timing
                    // performance (sometimes better, sometimes worse). My assumption is that it
                    // has to do with cache hits and misses. that's probs the con here, but the pro
                    // is that you're able to enqueue the most out of this shard buffer.
                    Acquire::EnqueueFull { batch_count } => {
                        !self.is_shard_full_batch(shard_ind, batch_count)
                    }
                    Acquire::Enqueue => !self.is_shard_full(shard_ind),
                    Acquire::Dequeue => !self.is_shard_empty(shard_ind),
                } {
                    break;
                } else {
                    /*
                     * If the shard is full/empty for enqueue/dequeue operation,
                     * then release the lock in a relaxed manner
                     */
                    self.shard_locks[shard_ind].store(false, Ordering::Relaxed);
                    if matches!(acquire, Acquire::Dequeue) {
                        self.job_space_shard_notifs[shard_ind].notify_one();
                        self.job_post_shard_notifs[shard_ind].notified().await;
                        continue;
                    } else {
                        self.job_post_shard_notifs[shard_ind].notify_one();
                        self.job_space_shard_notifs[shard_ind].notified().await;
                        continue;
                    }
                }
            }

            // You don't need this part; you only need to send a notif on release
            // of the shard
            // if matches!(acquire, Acquire::Dequeue) {
            //     self.job_post_shard_notifs[shard_ind].notified().await;
            // } else {
            //     self.job_space_shard_notifs[shard_ind].notified().await;
            // }
        }
        // current
    }

    /// Grab inner ring buffer shard, enqueue the item, update the enqueue index
    #[inline(always)]
    fn enqueue_item_in_shard(&self, shard_ind: usize, item: T) {
        let inner = &self.inner_rb[shard_ind];
        // we use fetch add here because we want to obtain the previous value
        // to dequeue while also incrementing this counter (separate load and store
        // incurs more cost)
        let enqueue_index = inner.enqueue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();
        let item_cell = inner.items[enqueue_index].get();
        // SAFETY: Only one thread will perform this operation and write to this
        // item cell
        unsafe {
            (*item_cell).write(item);
        }
    }

    /// Helper function to add an Option item to the RingBuffer
    /// This is necessary so that the ring buffer can be poisoned with None values
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    async fn enqueue_item(&self, item: T, shard_ind: usize) {
        // acquire shard
        self.try_acquire_shard(Acquire::Enqueue, shard_ind).await;

        // If poisoned, do not enqueue this item.
        if self.poisoned.load(Ordering::Relaxed) {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }

        self.enqueue_item_in_shard(shard_ind, item);
        self.release_shard(shard_ind);
        self.job_post_shard_notifs[shard_ind].notify_one();
    }

    /// Adds an item of type T to the ring buffer at a provided shard. If the user
    /// provides a shard index greater than the existing number of shards in the
    /// buffer, it will perform wrap around (% number of existing shards).
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a slot in a shard
    /// (this is usually pretty fast)
    ///
    /// Space complexity: O(1)
    pub async fn enqueue_in_shard(&self, item: T, shard_ind: usize) {
        if self.poisoned.load(Ordering::Relaxed) {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }
        let shard_ind = shard_ind % self.get_num_of_shards();
        self.enqueue_item(item, shard_ind).await;
    }

    /// Adds an item of type T to the ring buffer. It uses the ring buffer's shard_enq
    /// field and mods it with the number of existing shards for the buffer to determine
    /// which shard this enqueue operation will occur at. As a result, if you have multiple
    /// shards and one enqueuer task repeatedly using enqueue(), it will sweep across the
    /// shards and place an item in each.
    ///
    /// If you intend to have an enqueuer task map to a specific shard, use enqueue_in_shard()
    /// for more control.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a slot in the shard.
    /// (this is usually pretty fast)
    ///
    /// Space complexity: O(1)
    pub async fn enqueue(&self, item: T) {
        let shard_ind = self.shard_enq.fetch_add(1, Ordering::Relaxed) % self.get_num_of_shards();
        self.enqueue_item(item, shard_ind).await;
    }

    /// Adds a list of T items to the ring buffer at a provided shard. If the user
    /// provides a shard index greater than the existing number of shards in the
    /// buffer, it will perform wrap around (% number of existing shards).
    ///
    /// Note: The number of items you can batch can only go up to the capacity of the shard.
    /// Beyond that will cause a panic.
    ///
    /// Time Complexity: O(s_t * n) where s_t is the time it takes to acquire a shard
    /// and n is the number of items you are enqueuing
    ///
    /// Space Complexity: O(1)
    pub async fn enqueue_full_in_shard(&self, items: Vec<T>, shard_ind: usize) {
        let shard_ind = shard_ind % self.get_num_of_shards();
        let items_len = items.len();
        let shard_len = self.inner_rb[shard_ind].items.len();
        assert!(
            items_len <= shard_len,
            "Cannot batch more than {shard_len} at shard index {shard_ind}"
        );

        // If we have multiple shards or multiple worker threads,
        // we need to use locking
        self.try_acquire_shard(
            Acquire::EnqueueFull {
                batch_count: items.len(),
            },
            shard_ind,
        )
        .await;
        if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
            return;
        }

        for item in items {
            self.enqueue_item_in_shard(shard_ind, item);
        }

        self.release_shard(shard_ind);
        self.job_post_shard_notifs[shard_ind].notify_one();
    }

    /// Adds a list of T items to the ring buffer. It uses the ring buffer's shard_enq
    /// field and mods it with the number of existing shards for the buffer to determine
    /// which shard this enqueue operation will occur at. As a result, if you have multiple
    /// shards and one enqueuer task repeatedly using enqueue(), it will sweep across the
    /// shards and place an item in each.
    ///
    /// Note: The number of items you can batch can only go up to the capacity of the shard.
    /// Beyond that will cause a panic.
    ///
    /// Time Complexity: O(s_t * n) where s_t is the time it takes to acquire a shard
    /// and n is the number of items you are enqueuing
    ///
    /// Space Complexity: O(1)
    pub async fn enqueue_full(&self, items: Vec<T>) {
        let shard_ind = self.shard_enq.fetch_add(1, Ordering::Relaxed) % self.get_num_of_shards();
        let items_len = items.len();
        let shard_len = self.inner_rb[shard_ind].items.len();
        assert!(
            items_len <= shard_len,
            "Cannot batch more than {shard_len} at shard index {shard_ind}"
        );

        // If we have multiple shards or multiple worker threads,
        // we need to use locking
        self.try_acquire_shard(
            Acquire::EnqueueFull {
                batch_count: items.len(),
            },
            shard_ind,
        )
        .await;
        if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
            return;
        }

        for item in items {
            self.enqueue_item_in_shard(shard_ind, item);
        }

        self.release_shard(shard_ind);
        self.job_post_shard_notifs[shard_ind].notify_one();
    }

    /// Grab the inner ring buffer shard, dequeue the item, update the dequeue index
    #[inline(always)]
    fn dequeue_item_in_shard(&self, shard_ind: usize) -> T {
        let inner = &self.inner_rb[shard_ind];
        // we use fetch add here because we want to obtain the previous value
        // to dequeue while also incrementing this counter (separate load and store
        // incurs more cost)
        let dequeue_index = inner.dequeue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();

        let item_cell = inner.items[dequeue_index].get();

        unsafe { (*item_cell).assume_init_read() }
    }

    /// Retrieves an item of type T from the ring buffer. If the user
    /// provides a shard index greater than the existing number of shards in the
    /// buffer, it will perform wrap around (% number of existing shards). On a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a slot in the shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn dequeue_in_shard(&self, shard_ind: usize) -> Option<T> {
        let shard_ind = shard_ind % self.get_num_of_shards();
        self.try_acquire_shard(Acquire::Dequeue, shard_ind).await;

        if self.poisoned.load(Ordering::Relaxed) && self.is_shard_empty(shard_ind) {
            return None;
        }

        let item = self.dequeue_item_in_shard(shard_ind);
        self.release_shard(shard_ind);
        self.job_space_shard_notifs[shard_ind].notify_one();

        Some(item)
    }

    /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn dequeue(&self) -> Option<T> {
        let shard_ind = self.shard_deq.fetch_add(1, Ordering::Relaxed) % self.get_num_of_shards();
        self.try_acquire_shard(Acquire::Dequeue, shard_ind).await;

        if self.poisoned.load(Ordering::Relaxed) && self.is_shard_empty(shard_ind) {
            return None;
        }

        let item = self.dequeue_item_in_shard(shard_ind);
        self.release_shard(shard_ind);
        self.job_space_shard_notifs[shard_ind].notify_one();

        Some(item)
    }

    /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn dequeue_full_in_shard(&self, shard_ind: usize) -> Option<Vec<T>> {
        let shard_ind = shard_ind % self.get_num_of_shards();
        self.try_acquire_shard(Acquire::Dequeue, shard_ind).await;

        if self.poisoned.load(Ordering::Relaxed) && self.is_shard_empty(shard_ind) {
            return None;
        }

        let mut vec_items = Vec::new();

        while !self.is_shard_empty(shard_ind) {
            let item = self.dequeue_item_in_shard(shard_ind);
            vec_items.push(item);
        }

        self.release_shard(shard_ind);
        self.job_space_shard_notifs[shard_ind].notify_one();

        Some(vec_items)
    }

    /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub async fn dequeue_full(&self) -> Option<Vec<T>> {
        let shard_ind = self.shard_deq.fetch_add(1, Ordering::Relaxed) % self.get_num_of_shards();
        self.try_acquire_shard(Acquire::Dequeue, shard_ind).await;

        if self.poisoned.load(Ordering::Relaxed) && self.is_shard_empty(shard_ind) {
            return None;
        }

        let mut vec_items = Vec::new();

        while !self.is_shard_empty(shard_ind) {
            let item = self.dequeue_item_in_shard(shard_ind);
            vec_items.push(item);
        }

        self.release_shard(shard_ind);
        self.job_space_shard_notifs[shard_ind].notify_one();

        Some(vec_items)
    }

    /// Sets the poison flag of the ring buffer to true. This will prevent enqueuers
    /// from enqueuing anymore jobs if this method is called while enqueues are occuring.
    /// However you can use this if you want graceful exit of dequeuers tasks completing
    /// all available jobs enqueued first before exiting.
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn poison(&self) {
        self.poisoned.store(true, Ordering::Relaxed);
    }

    /// Sets the poison flag of the ring buffer to true in an async manner.
    /// Keep in mind that async context switches tasks, so if you want to
    /// poison after all enqueuer tasks are completed, use [Self::poison]
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline]
    pub async fn async_poison(&self) {
        self.poisoned.store(true, Ordering::Relaxed);
    }

    /// Clears the poison flag
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn clear_poison(&self) {
        self.poisoned.store(false, Ordering::Relaxed);
    }

    /// Clears the poison flag in an async manner
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline]
    pub async fn async_clear_poison(&self) {
        self.poisoned.store(false, Ordering::Relaxed);
    }

    /// Checks to see if the buffer is poisoned
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    /// Checks to see if the buffer is poisoned in an async manner
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline]
    pub async fn async_is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    /// Notifies one dequeuer task assigned to each shard.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(1)
    ///
    /// Important Note: When you want all dequeuer tasks to terminate
    /// gracefully after the buffer has been poisoned, you need to
    /// spawn a "notifier_task" that looks like this.
    /// ```
    /// let notifier_task: JoinHandle<()> = tokio::spawn({
    ///     let rb_clone: Arc<ShardedRingBuf<T>> = rb.clone();
    ///     async move {
    ///         loop {
    ///             for i in 0..rb_clone.get_num_of_shards() {
    ///              rb_clone.notify_pin_shard(i)
    ///             }
    ///             yield_now().await;
    ///          }
    ///     }
    /// })
    /// ```
    /// You can then terminate this task with `.abort()` later on after ensuring
    /// that all dequeuer tasks have terminated. The unfortunate part is that graceful
    /// termination of dequeuer tasks in a nonblocking manner for this buffer relies on
    /// whether the async runtime you are using has an equivalent `yield_now().await` function.
    #[inline(always)]
    pub fn notify_dequeuer_in_shard(&self, shard_ind: usize) {
        assert!(
            shard_ind < self.get_num_of_shards(),
            "Shard index must be within the number of shards that exist"
        );
        self.job_post_shard_notifs[shard_ind].notify_one();
    }

    /// Clears the buffer back to an empty state
    ///
    /// Note: This function is not safe to use within a multithreaded
    /// multitask environment. If you need to clear while performing concurrent
    /// tasks, use [Self::async_clear]
    ///
    /// Time Complexity: O(s * c_s) where s is the num of shards,
    /// c_s is the capacity per shard
    ///
    /// Space complexity: O(1)
    pub fn clear(&self) {
        // reset each shard's inner ring buffer
        for shard in 0..self.shard_locks.len() {
            let inner = &self.inner_rb[shard];
            let mut drop_index = inner.dequeue_index.load(Ordering::Relaxed) % inner.items.len();
            let stop_index = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued. Note here that this method uses Relaxed loads.
                unsafe {
                    // ptr::drop_in_place((*self.inner_rb[shard].items[drop_index].get()).as_mut_ptr())
                    (*self.inner_rb[shard].items[drop_index].get()).assume_init_drop()
                }
                drop_index = (drop_index + 1) % self.inner_rb[shard].items.len();
            }
            self.inner_rb[shard]
                .enqueue_index
                .store(0, Ordering::Relaxed);
            self.inner_rb[shard]
                .dequeue_index
                .store(0, Ordering::Relaxed);
        }
    }

    /// Checks if all shards are empty
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
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_shard_empty(&self, shard_ind: usize) -> bool {
        let inner = &self.inner_rb[shard_ind];
        // use these values as monotonic counter than indices
        let (enq_ind, deq_ind) = (
            inner.enqueue_index.load(Ordering::Relaxed),
            inner.dequeue_index.load(Ordering::Relaxed),
        );
        let jobs = enq_ind.wrapping_sub(deq_ind);
        jobs == 0
    }

    /// Checks if all shards are full
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
        let inner = &self.inner_rb[shard_ind];
        let item_len = inner.items.len();
        // use these values as monotonic counter than indices
        let (enq_ind, deq_ind) = (
            inner.enqueue_index.load(Ordering::Relaxed),
            inner.dequeue_index.load(Ordering::Relaxed),
        );
        let jobs = enq_ind.wrapping_sub(deq_ind);
        jobs == item_len
    }

    /// Checks to see if a specific shard is able to support batching a
    /// certain number of items
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_shard_full_batch(&self, shard_ind: usize, batch_count: usize) -> bool {
        let inner = &self.inner_rb[shard_ind];
        let item_len = inner.items.len();
        // use these values as monotonic counter than indices
        let (enq_ind, deq_ind) = (
            inner.enqueue_index.load(Ordering::Relaxed),
            inner.dequeue_index.load(Ordering::Relaxed),
        );
        let jobs = enq_ind.wrapping_sub(deq_ind);
        item_len - jobs < batch_count
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
    pub fn get_enq_ind_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // grab enq val
        let inner = &self.inner_rb[shard_ind];
        let enq_ind = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();

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
    pub fn get_deq_ind_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // grab deq ind val
        let inner = &self.inner_rb[shard_ind];
        let deq_ind = inner.dequeue_index.load(Ordering::Relaxed) % inner.items.len();

        Some(deq_ind)
    }

    /// Gets the total number of jobs within a shard. Returns None if the shard
    /// index is invalid.
    ///
    /// Note: This is all done in a RELAXED manner, so this function
    /// may give stale value at the time or invocation
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_job_count_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        let inner = &self.inner_rb[shard_ind];
        let (enq_count, deq_count) = (
            inner.enqueue_index.load(Ordering::Relaxed),
            inner.dequeue_index.load(Ordering::Relaxed),
        );
        let jobs = enq_count.wrapping_sub(deq_count);
        Some(jobs)
    }

    /// Gets the total number of jobs within each shard.
    ///
    /// Note: This is all done in a RELAXED manner, so this function
    /// may give stale value at the time or invocation
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(s)
    #[inline(always)]
    pub fn get_job_count_total(&self) -> Vec<usize> {
        let mut count = Vec::new();

        for shard in &self.inner_rb {
            let (enq_count, deq_count) = (
                shard.enqueue_index.load(Ordering::Relaxed),
                shard.dequeue_index.load(Ordering::Relaxed),
            );
            let jobs = enq_count.wrapping_sub(deq_count);
            count.push(jobs);
        }
        count
    }

    /// Helper function to see if a given index inside of a shard does
    /// indeed contain a valid item
    #[inline(always)]
    fn is_item_in_shard(&self, item_ind: usize, shard_ind: usize) -> bool {
        let inner = &self.inner_rb[shard_ind];
        let enqueue_ind = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();
        let dequeue_ind = inner.dequeue_index.load(Ordering::Relaxed) % inner.items.len();

        if enqueue_ind > dequeue_ind {
            item_ind < enqueue_ind && item_ind >= dequeue_ind
        } else if enqueue_ind < dequeue_ind {
            item_ind >= dequeue_ind || item_ind < enqueue_ind
        } else {
            false
        }
    }

    /// Returns a clone of an item within the buffer or
    /// None if the shard index/item index is invalid or if there exists
    /// no item inside that position
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
    /// and this function *must* be used in a single threaded manner
    ///
    /// Note: This is all done in a RELAXED manner, so this function is not
    /// thread-safe; you should only use this function in a single-threaded
    /// context
    ///
    /// Time Complexity: O(T_t)
    ///
    /// Space Complexity: O(T_s)
    ///
    /// Where O(T_t) and O(T_s) is the time and space complexity required to
    /// clone the internals of the T object itself
    pub fn clone_item_at_shard(&self, item_ind: usize, shard_ind: usize) -> Option<T>
    where
        T: Clone,
    {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        if item_ind >= self.inner_rb[shard_ind].items.len() {
            return None;
        }

        // clone item in shard
        let inner = &self.inner_rb[shard_ind];
        // SAFETY: We know for certain there's an item inside the ring buffer if
        // item index is in [dequeue ind, enqueue ind) + wraparound
        unsafe {
            if self.is_item_in_shard(item_ind, shard_ind) {
                let val_ref = (*inner.items[item_ind].get()).assume_init_ref();
                Some(val_ref.clone())
            } else {
                None
            }
        }
    }

    /// Returns a clone of a specific InnerRingBuffer shard or
    /// None if the shard index is invalid
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
    /// and this function *must* be used in a single threaded manner
    ///
    /// Note: This is all done in a RELAXED manner, so this function is not
    /// thread-safe; you should only use this function in a single-threaded
    /// context
    ///
    /// Time Complexity: O(c_s * O(T_t))
    ///
    /// Space Complexity: O(c_s * O(T_s))
    ///
    /// Where c_s is the capacity in a shard O(T_t) and O(T_s) is the time and
    /// space complexity required to clone the internals of the T object itself
    pub fn clone_items_at_shard(&self, shard_ind: usize) -> Option<Box<[Option<T>]>>
    where
        T: Clone,
    {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // clone items in shard
        let inner = &self.inner_rb[shard_ind];
        let items = {
            let mut vec = Vec::with_capacity(inner.items.len());

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

        Some(items)
    }

    /// Returns a clone of the entire buffer in its current state
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
    /// and this function *must* be used in a single threaded manner
    ///
    /// Note: This is all done in a RELAXED manner, so this function is not
    /// thread-safe; you should only use this function in a single-threaded
    /// context
    ///
    /// Time Complexity: O(s * c_s * O(T_t))
    ///
    /// Space Complexity: O(s * c_s * O(T_s))
    ///
    /// Where s is the number of shards, c_s is the capacity in a shard
    /// and O(T_t) and O(T_s) is the time and space complexity required
    /// to clone the internals of the T object itself
    pub async fn clone_items(&self) -> Box<[Box<[Option<T>]>]>
    where
        T: Clone,
    {
        let inner = &self.inner_rb;
        let mut vec = Vec::with_capacity(inner.len());

        // Clone items in each shard
        for shard_ind in 0..self.inner_rb.len() {
            let shard = &inner[shard_ind];
            let items = {
                let mut shard_vec = Vec::with_capacity(shard.items.len());

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

        vec.into_boxed_slice()
    }

    /// Print out the content inside the a specific shard of the buffer.
    /// If nothing is printed out, that means an invalid shard index was
    /// provided.
    ///
    /// Note: This function is NOT thread safe. You should not use this
    /// function within a multithreaded async situation
    ///
    /// Time Complexity: O(c_s * s_t) where c_s is the capacity of the shard
    /// and s_t is how long it takes to acquire the shard
    ///
    /// Space Complexity: O(1)
    pub fn print_shard(&self, shard_ind: usize)
    where
        T: Debug,
    {
        if shard_ind >= self.shard_locks.len() {
            return;
        }

        let mut print_buffer = String::new();
        let inner_shard = &self.inner_rb[shard_ind];

        write!(print_buffer, "[").unwrap();

        // SAFETY: Only print values in between [dequeue_ind, enqueue_ind) + wraparound
        // otherwise print <uninit> as a placeholder value
        for i in 0..inner_shard.items.len() {
            if i == inner_shard.items.len() - 1 {
                if self.is_item_in_shard(i, shard_ind) {
                    let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                    write!(print_buffer, "{val_ref:?}").unwrap();
                } else {
                    write!(print_buffer, "<uninit>").unwrap();
                }
            } else if self.is_item_in_shard(i, shard_ind) {
                let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                write!(print_buffer, "{val_ref:?}, ").unwrap();
            } else {
                write!(print_buffer, "<uninit>, ").unwrap();
            }
        }

        write!(print_buffer, "]").unwrap();

        // Print the buffer!
        print!("{print_buffer}");
    }

    /// Print out the content inside the entire buffer
    ///
    /// Note: This function is NOT thread safe. You should not use this
    /// function within a multithreaded async situation
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space Complexity: O(1)
    pub fn print(&self)
    where
        T: Debug,
    {
        let mut print_buffer = String::new();
        write!(print_buffer, "[").unwrap();

        // Neat thing here is that this for loop owns the guards, so it'll drop the
        // guard for me when it goes to the next iteration
        for shard_ind in 0..self.shard_locks.len() {
            let inner_shard = &self.inner_rb[shard_ind];
            write!(print_buffer, "[").unwrap();

            // SAFETY: Only print values in between [dequeue_ind, enqueue_ind) + wraparound
            // otherwise print None as a placeholder value
            for i in 0..inner_shard.items.len() {
                if i == inner_shard.items.len() - 1 {
                    if self.is_item_in_shard(i, shard_ind) {
                        let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                        write!(print_buffer, "{val_ref:?}").unwrap();
                    } else {
                        write!(print_buffer, "<uninit>").unwrap();
                    }
                } else if self.is_item_in_shard(i, shard_ind) {
                    let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                    write!(print_buffer, "{val_ref:?}, ").unwrap();
                } else {
                    write!(print_buffer, "<uninit>, ").unwrap();
                }
            }

            write!(print_buffer, "]").unwrap();
        }

        write!(print_buffer, "]").unwrap();

        // Print the buffer!
        print!("{print_buffer}");
    }
}

unsafe impl<T> Sync for ShardedRingBuf<T> {}
unsafe impl<T> Send for ShardedRingBuf<T> {}

unsafe impl<T> Sync for InnerRingBuffer<T> {}
unsafe impl<T> Send for InnerRingBuffer<T> {}

// Destructor trait created for InnerRingBuffer<T> to clean up
// memory allocated onto MaybeUninit<T> when ShardedRingBuf<T>
// goes out of scope
impl<T> Drop for InnerRingBuffer<T> {
    fn drop(&mut self) {
        for item_ind in 0..self.items.len() {
            if self.is_item_in_shard(item_ind) {
                let ptr = self.items[item_ind].get();
                unsafe {
                    (*ptr).assume_init_drop();
                }
            }
        }
    }
}
