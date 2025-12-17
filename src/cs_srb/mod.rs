pub mod guards;
use crate::cs_srb::guards::ShardLockGuard;
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

/// This enum is used to declare whether you are enqueuing
/// or dequeuing an item for `acquire_shard_guard()`
#[derive(Debug, PartialEq, Eq)]
enum Acquire {
    Enqueue,
    Dequeue,
}

/// A sharded ring (circular) buffer struct that can only be used in an *async environment*.
#[derive(Debug)]
pub struct CSShardedRingBuf<T> {
    /// Total capacity of the buffer
    capacity: AtomicUsize,
    /// Used to determine which shard a thread-task pair should work on
    /// CachePadded to prevent false sharing
    shard_locks: Box<[CachePadded<AtomicBool>]>,
    /// Multiple InnerRingBuffer structure based on num of shards
    /// CachePadded to prevent false sharing
    inner_rb: Box<[CachePadded<InnerRingBuffer<T>>]>,
    /// Used to notify enqueuer tasks in a way that
    /// won't cause busy spinning
    job_space_shard_notifs: Box<[CachePadded<Notify>]>,
    /// Used to notify dequeuer tasks in a way that
    /// won't cause busy spinning
    job_post_shard_notifs: Box<[CachePadded<Notify>]>,
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
    /// Poison signal of the ring buffer
    poisoned: AtomicBool,
    /// If instantiating buffer with `new_with_enq_num`, we use this to
    /// note how many enqueue tasks will be spawned with this buffer
    ///
    /// This is useful for single enqueuer task - multiple dequeuer tasks
    /// situation
    enq_task_count: Option<usize>,
}

// An inner ring buffer to contain the items, enqueue and dequeue index for ShardedRingBuf struct
#[derive(Debug, Default)]
pub(crate) struct InnerRingBuffer<T> {
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
    pub fn is_item_in_shard(&self, item_ind: usize) -> bool {
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

impl<T> CSShardedRingBuf<T> {
    /// Instantiates CSShardedRingBuf
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
                    vec.push(CachePadded::new(Notify::new()));
                }
                vec.into_boxed_slice()
            },

            job_space_shard_notifs: {
                let mut vec = Vec::with_capacity(shards);

                for _ in 0..shards {
                    vec.push(CachePadded::new(Notify::new()));
                }
                vec.into_boxed_slice()
            },
            enq_task_count: None,
        })
    }

    /// Instantiates ShardedRingBuf with a note on how many max enqueue tasks
    /// will be spawned with this buffer at a time
    ///
    /// This is useful for situations where you have enq_num < shards and you
    /// want to evenly distribute dequeuer tasks notification onto the enqueue
    /// tasks
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(s * c_s) where s is the number of shards and c_s
    /// is the capacity per shard (space usage also depends on T)
    pub fn new_with_enq_num(capacity: usize, shards: usize, enq_num: usize) -> Arc<Self> {
        assert!(capacity > 0, "Capacity must be positive");
        assert!(shards > 0, "Shards must be positive");
        assert!(enq_num > 0, "Enqueue task count must be positive");
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
                    vec.push(CachePadded::new(Notify::new()));
                }
                vec.into_boxed_slice()
            },

            job_space_shard_notifs: {
                let mut vec = Vec::with_capacity(shards);

                for _ in 0..enq_num {
                    vec.push(CachePadded::new(Notify::new()));
                }
                vec.into_boxed_slice()
            },
            enq_task_count: Some(enq_num),
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
    /// This will return None if you provide it a shard index that
    /// is invalid/not within the bound of shards created in this
    /// buffer
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

    /// Returns the total capacity of this buffer
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_total_capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Acquire the specific shard
    #[inline(always)]
    fn acquire_shard(&self, shard_ind: usize) -> bool {
        self.shard_locks[shard_ind]
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Acquires a lock on the shard for either enqueue or dequeue purposes.
    /// If the buffer was poisoned, the following effects will happen:
    /// - Enqueue: return None immediately
    /// - Dequeue: only returns None if the shard is *empty*
    ///
    /// Time Complexity: O(s_t) where s_t is how long it takes to acquire the shard
    #[inline]
    async fn acquire_shard_guard(
        &self,
        acquire: Acquire,
        shard_ind: usize,
    ) -> Option<ShardLockGuard<'_, T>> {
        let enq_shard_ind = match self.enq_task_count {
            Some(enq_num) => shard_ind % enq_num,
            None => shard_ind,
        };
        loop {
            if matches!(acquire, Acquire::Enqueue) {
                if self.poisoned.load(Ordering::Relaxed) {
                    return None;
                }
            } else {
                // if poisoned and empty, get out of this loop
                if self.poisoned.load(Ordering::Relaxed) && self.is_shard_empty(shard_ind) {
                    return None;
                }
            }

            if self.acquire_shard(shard_ind) {
                /*
                 * We need to acquire the shard first to get a stable view of how many items
                 * are on the shard.
                 */
                if match acquire {
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
                        self.job_space_shard_notifs[enq_shard_ind].notify_one();
                        self.job_post_shard_notifs[shard_ind].notified().await;
                    } else {
                        self.job_post_shard_notifs[shard_ind].notify_one();
                        self.job_space_shard_notifs[enq_shard_ind].notified().await;
                    }
                    continue;
                }
            }
        }
        if matches!(acquire, Acquire::Dequeue) {
            Some(ShardLockGuard::acquire_shard_guard(
                &self.shard_locks[shard_ind],
                &self.job_space_shard_notifs[enq_shard_ind],
                &self.inner_rb[shard_ind],
            ))
        } else {
            Some(ShardLockGuard::acquire_shard_guard(
                &self.shard_locks[shard_ind],
                &self.job_post_shard_notifs[shard_ind],
                &self.inner_rb[shard_ind],
            ))
        }
    }

    /// Given a `ShardLockGuard<'_, T>`, it enqueues an item inside the shard.
    ///
    /// You must obtain a ShardLockGuard<'_, T> from either `enqueue_guard()`
    /// or `enqueue_guard_in_shard()`
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn enqueue(&self, item: T, shard_guard: ShardLockGuard<'_, T>) {
        let inner = shard_guard.shard;
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

    /// Obtain a guard on a specific shard the ring buffer for the purpose of
    /// enqueuing. Returns None if the buffer was poisoned.
    ///
    /// Note: This function will PANIC if you specify a shard index out of
    /// bound from the number of shards that you have.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    #[inline(always)]
    pub async fn enqueue_guard_in_shard(&self, shard_ind: usize) -> Option<ShardLockGuard<'_, T>> {
        self.acquire_shard_guard(Acquire::Enqueue, shard_ind).await
    }

    /// Obtain a guard on a shard the ring buffer for the purpose of enqueuing.
    ///
    /// Returns None if the buffer was poisoned.
    ///
    /// Note: It obtains the shard in a circular manner, so for example, at the beginning,
    /// it obtains shard 0, then shard 1, the shard 2, ... (all modded with the number
    /// of shards that exists in this buffer)
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    #[inline(always)]
    pub async fn enqueue_guard(&self) -> Option<ShardLockGuard<'_, T>> {
        let shard_ind = self.shard_enq.fetch_add(1, Ordering::Relaxed) % self.get_num_of_shards();
        self.acquire_shard_guard(Acquire::Enqueue, shard_ind).await
    }

    /// Given a `ShardLockGuard<'_, T>`, it dequeues an item inside the shard.
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    fn dequeue_item(&self, shard_guard: ShardLockGuard<'_, T>) -> T {
        let inner = shard_guard.shard;
        // we use fetch add here because we want to obtain the previous value
        // to dequeue while also incrementing this counter (separate load and store
        // incurs more cost)
        let dequeue_index = inner.dequeue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();
        let item_cell = inner.items[dequeue_index].get();
        unsafe { (*item_cell).assume_init_read() }
    }

    /// Performs both the operation of obtaining a guard on a shard in the ring buffer
    /// and dequeuing an item off from the buffer. This can be done like together because
    /// due to inherent cancellation safety (data either stays in buffer on cancellation
    /// or it gets taken out without touching an await point).
    ///
    /// Returns None if the buffer was poisoned and the shard contains no more items.
    ///  
    /// Note: This function will PANIC if you specify a shard index out of
    /// bound from the number of shards that you have.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn dequeue_in_shard(&self, shard_ind: usize) -> Option<T> {
        let shard_guard = self.acquire_shard_guard(Acquire::Dequeue, shard_ind).await;
        shard_guard.map(|guard| self.dequeue_item(guard))
    }

    /// Performs both the operation of obtaining a guard on a shard in the ring buffer
    /// and dequeuing an item off from the buffer. This can be done like together because
    /// due to inherent cancellation safety (data either stays in buffer on cancellation
    /// or it gets taken out without touching an await point).
    ///
    /// Returns None if the buffer was poisoned and the shard contains no more items.
    ///
    /// Note: It obtains the shard in a circular manner, so for example, at the beginning,
    /// it obtains shard 0, then shard 1, the shard 2, ... (all modded with the number
    /// of shards that exists in this buffer)
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn dequeue(&self) -> Option<T> {
        let shard_ind = self.shard_deq.fetch_add(1, Ordering::Relaxed) % self.get_num_of_shards();
        let shard_guard = self.acquire_shard_guard(Acquire::Dequeue, shard_ind).await;

        shard_guard.map(|guard| self.dequeue_item(guard))
    }

    /// Sets the poison flag of the ring buffer to true. This will prevent enqueuers
    /// from enqueuing anymore jobs if this method is called while enqueues are occuring.
    /// This allows for graceful exit of dequeuers tasks as it completes all available jobs
    /// enqueued first.
    ///
    /// Important Note: To ensure all dequeuer tasks terminate
    /// gracefully after the buffer has been poisoned, you need to
    /// call on `notify_dequeuer_in_shard()` on each shard with a
    /// dequeuer task pinned to it (and multiple times if there are
    /// multiple dequeuer task in a shard).
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn poison(&self) {
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

    /// Checks to see if the buffer is poisoned
    /// in a relaxed manner.
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    /// Notifies one dequeuer task assigned to each shard.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(1)
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
    /// multitask environment.
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
    /// may give stale value at the time of invocation
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
    /// may give stale value at the time of invocation
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

    /// Returns a clone of an item within the buffer or
    /// None if the shard index/item index is invalid or if there exists
    /// no item inside that position
    ///
    /// The T object inside the ring buffer *must* implement the Clone trait
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
            if inner.is_item_in_shard(item_ind) {
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
                if inner.is_item_in_shard(i) {
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
                    if shard.is_item_in_shard(i) {
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
                if inner_shard.is_item_in_shard(i) {
                    let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                    write!(print_buffer, "{val_ref:?}").unwrap();
                } else {
                    write!(print_buffer, "<uninit>").unwrap();
                }
            } else if inner_shard.is_item_in_shard(i) {
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

        for shard_ind in 0..self.shard_locks.len() {
            let inner_shard = &self.inner_rb[shard_ind];
            write!(print_buffer, "[").unwrap();

            // SAFETY: Only print values in between [dequeue_ind, enqueue_ind) + wraparound
            // otherwise print None as a placeholder value
            for i in 0..inner_shard.items.len() {
                if i == inner_shard.items.len() - 1 {
                    if inner_shard.is_item_in_shard(i) {
                        let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                        write!(print_buffer, "{val_ref:?}").unwrap();
                    } else {
                        write!(print_buffer, "<uninit>").unwrap();
                    }
                } else if inner_shard.is_item_in_shard(i) {
                    let val_ref = unsafe { (*inner_shard.items[i].get()).assume_init_ref() };
                    write!(print_buffer, "{val_ref:?}, ").unwrap();
                } else {
                    write!(print_buffer, "<uninit>, ").unwrap();
                }
            }

            write!(print_buffer, "]").unwrap();
        }

        write!(print_buffer, "]").unwrap();

        print!("{print_buffer}");
    }
}

unsafe impl<T> Sync for CSShardedRingBuf<T> {}
unsafe impl<T> Send for CSShardedRingBuf<T> {}

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
