use crate::{
    guards::ShardLockGuard,
    shard_policies::ShardPolicyKind,
    task_locals::{get_shard_ind, get_shard_policy, get_shift, get_task_node, set_shard_ind},
    task_node::{TaskNode, TaskNodePtr},
};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::{Cell, UnsafeCell},
    fmt::{Debug, Write},
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};
use tokio::{runtime::Handle, sync::Notify, task::yield_now};

// Enum used in try_acquire_shard to determine if task
// is enqueue or dequeue
#[derive(Debug, PartialEq, Eq)]
enum Acquire {
    Enqueue,
    Dequeue,
    EnqueueFull
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

    // These are used in the singled threaded
    // Tokio runtime cases to optimize on context
    // switching
    /// signals that there is a job to enqueue on the buffer
    job_space_notif: Notify,
    /// signals that there is a job posted to dequeue on the buffer
    job_post_notif: Notify,


    pub(crate) job_space_shard_notifs: Box<[Notify]>,
    pub(crate) job_post_shard_notifs: Box<[Notify]>,

    pub(crate) deq_fin_taken: Cell<bool>,

    // The fields below are for CFT policy
    /// The head of the TaskNode linked list for CFT
    pub(crate) head: AtomicPtr<TaskNode>,
    /// Signals whether the assigner is spawned already or not
    pub(crate) assigner_spawned: AtomicBool,
    /// Signals whether the assigner is terminated or not
    pub(crate) assigner_terminate: AtomicBool,
}

// An inner ring buffer to contain the items, enqueue and dequeue index for ShardedRingBuf struct
#[derive(Debug, Default)]
struct InnerRingBuffer<T> {
    /// Box containing the content of the buffer
    items: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Where to enqueue at in the Box
    // enqueue_index: Cell<usize>,
    enqueue_index: AtomicUsize,
    /// Where to dequeue at in the Box
    // dequeue_index: Cell<usize>,
    dequeue_index: AtomicUsize
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
            // enqueue_index: Cell::new(0),
            // dequeue_index: Cell::new(0),
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
            job_space_notif: Notify::new(),
            job_post_notif: Notify::new(),

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

            deq_fin_taken: Cell::new(false),

            head: AtomicPtr::new(ptr::null_mut()),
            assigner_spawned: AtomicBool::new(false),
            assigner_terminate: AtomicBool::new(false),
        }
    }

    /// Assigner task uses this to get the head of the linked list in a
    /// relaxed manner
    /// Private to the crate because a user should NOT have access
    /// to any of these TaskNodes
    #[inline(always)]
    pub(crate) fn get_head_relaxed(&self) -> TaskNodePtr {
        TaskNodePtr(self.head.load(Ordering::Relaxed))
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
            ShardPolicyKind::RandomAndSweep => frand(0..shard_count),
            ShardPolicyKind::Cft => {
                // What CFT relies on for its starting index is what's written
                // to this specific task's TaskNodePtr by the assigner task
                let task_node = unsafe { &*get_task_node().0 };
                match acquire {
                    Acquire::Dequeue => {
                        while !task_node.is_assigned.load(Ordering::Relaxed) {
                            if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
                                return 0;
                            }
                            yield_now().await;
                        }
                        task_node.shard_ind.load(Ordering::Relaxed)
                    },
                    _ => {
                        while !task_node.is_assigned.load(Ordering::Relaxed) {
                            yield_now().await;
                        }
                        task_node.shard_ind.load(Ordering::Relaxed)
                    }
                }
            }
            // Both SweepBy and ShiftBy use the same method of getting its starting
            // index
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
            if self.poisoned.load(Ordering::Relaxed) {
                if matches!(get_shard_policy(), ShardPolicyKind::Pin) && self.is_shard_empty(get_shard_ind().expect("This shard index should be guaranteed here") % shard_count) {
                    break;
                }
                else if self.is_empty() {
                    break;
                }
            }

            if self.acquire_shard(current) {
                /*
                 * We need to acquire the shard first to get a stable view of how many items
                 * are on the shard.
                 */
                if match acquire {
                    Acquire::EnqueueFull => self.is_shard_empty(current),
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
                     * If the shard is full/empty for enqueue/dequeue operation,
                     * then release the lock in a relaxed manner
                     */
                    self.shard_locks[current].store(false, Ordering::Relaxed);
                    if matches!(acquire, Acquire::Dequeue) {
                        if matches!(get_shard_policy(), ShardPolicyKind::Pin) {
                            self.job_space_shard_notifs[current].notify_one();
                            self.job_post_shard_notifs[current].notified().await;
                            continue;
                        }
                    } else {
                        if matches!(get_shard_policy(), ShardPolicyKind::Pin) {
                            self.job_post_shard_notifs[current].notify_one();
                            self.job_space_shard_notifs[current].notified().await;
                            continue;
                        }
                    }
                    // any other policies go through a yield_now() approach.
                }
            }

            if matches!(get_shard_policy(), ShardPolicyKind::Pin) {
                // yield_now().await;
                if matches!(acquire, Acquire::Dequeue) {
                    self.job_post_shard_notifs[current].notified().await;
                }
                else {
                    self.job_space_shard_notifs[current].notified().await;
                }
            } else if !matches!(get_shard_policy(), ShardPolicyKind::Cft) {
                // Move to the next index to check if item can be placed inside
                current = (current + get_shift()) % shard_count;
                spins += get_shift();

                // yield only once the enqueuers or dequeuers task has went one round through
                // the shard_job buffer
                if spins >= shard_count {
                    if matches!(get_shard_policy(), ShardPolicyKind::ShiftBy) {
                        current = (current + 1) % shard_count;
                    }
                    spins = 0;
                    yield_now().await;
                }
            } else {
                // The dequeuer needs to be updated/reassigned if its pairing
                // enqueuer was completed before yielding here
                let task_node = unsafe { &*get_task_node().0 };
                if matches!(acquire, Acquire::Dequeue)
                    && task_node.is_assigned.load(Ordering::Relaxed)
                {
                    current = unsafe { &*get_task_node().0 }
                        .shard_ind
                        .load(Ordering::Relaxed);
                }
                yield_now().await;
            }
        }
        current
    }

    /// Grab inner ring buffer shard, enqueue the item, update the enqueue index
    #[inline(always)]
    fn enqueue_in_shard(&self, shard_ind: usize, item: T) {
        let inner = &self.inner_rb[shard_ind];

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
    async fn enqueue_item(&self, item: T) {
        // If we have multiple shards and multiple threads, we need to use
        // shard locking
        if self.get_num_of_shards() != 1 || Handle::current().metrics().num_workers() != 1 {
            // acquire shard
            let current = self.try_acquire_shard(Acquire::Enqueue).await;

            // If poisoned, do not enqueue this item.
            if self.poisoned.load(Ordering::Relaxed) {
                println!("Can't enqueue anymore. Ring buffer is poisoned.");
                return;
            }

            self.enqueue_in_shard(current, item);
            self.release_shard(current);
        }
        // Otherwise, the user is likely going through a single dequeuer route
        // (MPSC or SPSC), so we can just approach this in a lock free slot
        // based manner, context switching if shard is full
        else {
            // If poisoned, do not enqueue this item.
            if self.poisoned.load(Ordering::Relaxed) {
                println!("Can't enqueue anymore. Ring buffer is poisoned.");
                return;
            }
            loop {
                let inner = &self.inner_rb[0];
                let enq_counter = inner.enqueue_index.load(Ordering::Relaxed);
                let deq_counter = inner.enqueue_index.load(Ordering::Relaxed);
                let jobs = enq_counter.wrapping_sub(deq_counter);

                if jobs != inner.items.len() {
                    let enqueue_index = inner.enqueue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();
                    let item_cell = inner.items[enqueue_index].get();
                    unsafe {
                        (*item_cell).write(item);
                    }
                    self.job_post_notif.notify_one();
                    return;
                }

                // we're using a notify for the enqueuer
                // it minimizes the number times we context switch
                self.job_post_notif.notify_waiters();
                self.job_space_notif.notified().await;
            }
        }
    }

    /// Adds an item of type T to the RingBuffer, *blocking* the thread until there is space to add the item.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub(crate) async fn enqueue(&self, item: T) {
        if self.poisoned.load(Ordering::Relaxed) {
            println!("Can't enqueue anymore. Ring buffer is poisoned.");
            return;
        }
        self.enqueue_item(item).await;
    }

    /// Grab the inner ring buffer shard, dequeue the item, update the dequeue index
    #[inline(always)]
    fn dequeue_in_shard(&self, shard_ind: usize) -> T {
        let inner = &self.inner_rb[shard_ind];
        // we use fetch add here because we want to obtain the previous value
        // to dequeue while also incrementing this counter (separate load and store
        // incurs more cost)
        let dequeue_index = inner.dequeue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();

        let item_cell = inner.items[dequeue_index].get();

        // SAFETY: Only one thread will perform this operation
        // And it's guaranteed that an item will exist here
        let item = unsafe { (*item_cell).assume_init_read() };

        // SAFETY: We just copied the item by value, so we no longer need to hold
        // this item in memory
        unsafe {
            ptr::drop_in_place((*inner.items[dequeue_index].get()).as_mut_ptr());
        }
        item
    }

    /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub(crate) async fn dequeue(&self) -> Option<T> {
        // If we have multiple shards or multiple worker threads,
        // we need to use locking
        if self.get_num_of_shards() != 1 || Handle::current().metrics().num_workers() != 1 {
            let current = self.try_acquire_shard(Acquire::Dequeue).await;
            // if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
            //     return None;
            // }
            if self.poisoned.load(Ordering::Relaxed) {
                if matches!(get_shard_policy(), ShardPolicyKind::Pin) && self.is_shard_empty(get_shard_ind().expect("This shard index should be guaranteed here") % self.get_num_of_shards()) {
                    return None;
                }
                else if self.is_empty() {
                    return None;
                }
            }
            let item = self.dequeue_in_shard(current);
            self.release_shard(current);

            Some(item)
        }
        // Otherwise, the user is likely going through a single dequeuer route
        // (MPSC or SPSC), so we can just approach this in a lock free slot
        // based manner, context switching if no job is available
        else {
            loop {
                if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
                    return None;
                }

                let inner = &self.inner_rb[0];
                let enq_counter = inner.enqueue_index.load(Ordering::Relaxed);
                let deq_counter = inner.enqueue_index.load(Ordering::Relaxed);

                let jobs = enq_counter.wrapping_sub(deq_counter);

                if jobs != 0 {
                    let dequeue_index = inner.dequeue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();
                    // SAFETY: Only one thread will claim this slot and perform this operation
                    // And it's guaranteed that an item will exist here
                    let item = unsafe { (*inner.items[dequeue_index].get()).assume_init_read() };
                    // SAFETY: We just copied the item by value, so we no longer need to hold
                    // this item in memory
                    unsafe {
                        ptr::drop_in_place((*inner.items[dequeue_index].get()).as_mut_ptr());
                    }
                    self.job_space_notif.notify_one();
                    return Some(item);
                }
                yield_now().await;
            }
        }
    }

    /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub(crate) async fn dequeue_full(&self) -> Option<Vec<T>> {
        // If we have multiple shards or multiple worker threads,
        // we need to use locking
        if self.get_num_of_shards() != 1 || Handle::current().metrics().num_workers() != 1 {
            let current = self.try_acquire_shard(Acquire::Dequeue).await;
            // if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
            //     return None;
            // }

            if self.poisoned.load(Ordering::Relaxed) {
                if matches!(get_shard_policy(), ShardPolicyKind::Pin) && self.is_shard_empty(get_shard_ind().expect("This shard index should be guaranteed here") % self.get_num_of_shards()) {
                    return None;
                }
                else if self.is_empty() {
                    return None;
                }
            }
            let mut vec_items = Vec::new();

            while !self.is_shard_empty(current) {
                let item = self.dequeue_in_shard(current);
                vec_items.push(item);
            }

            self.release_shard(current);
            Some(vec_items)
        }
        // Otherwise, the user is likely going through a single dequeuer route
        // (MPSC or SPSC), so we can just approach this in a lock free slot
        // based manner, context switching if no job is available
        else {
            loop {
                if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
                    return None;
                }

                self.job_post_notif.notified().await;

                let inner = &self.inner_rb[0];
                let mut vec_items = Vec::new();
                // we use this loop here because it's possible for multiple looping dequeuers to exist
                // inside 1 shard which means we need to keep re-acquiring the enq_counter and deq_counter
                // to make sure there's any jobs left inside for it to be truly lock free slot-based
                loop {
                    if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
                        return None;
                    }
                    let enq_counter = inner.enqueue_index.load(Ordering::Relaxed);
                    let deq_counter = inner.enqueue_index.load(Ordering::Relaxed);
                    let jobs = enq_counter.wrapping_sub(deq_counter);

                    if jobs != 0 {
                        let dequeue_index = inner.dequeue_index.fetch_add(1, Ordering::Relaxed) % inner.items.len();
                        // SAFETY: Only one thread will claim this slot and perform this operation
                        // And it's guaranteed that an item will exist here
                        let item =
                            unsafe { (*inner.items[dequeue_index].get()).assume_init_read() };
                        // SAFETY: We just copied the item by value, so we no longer need to hold
                        // this item in memory
                        unsafe {
                            ptr::drop_in_place((*inner.items[dequeue_index].get()).as_mut_ptr());
                        }
                        vec_items.push(item);
                    } else {
                        break;
                    }
                }

                if !vec_items.is_empty() {
                    self.job_space_notif.notify_waiters();
                    return Some(vec_items);
                }
            }
        }
    }

        /// Retrieves an item of type T from the RingBuffer if an item exists in the buffer.
    /// If the ring buffer is set with a poisoned flag or received a poison pill,
    /// this method will return None.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    pub(crate) async fn enqueue_full(&self, items: Vec<T>) {
        // If we have multiple shards or multiple worker threads,
        // we need to use locking
        if self.get_num_of_shards() != 1 || Handle::current().metrics().num_workers() != 1 {
            let current = self.try_acquire_shard(Acquire::EnqueueFull).await;
            if self.poisoned.load(Ordering::Relaxed) && self.is_empty() {
                return;
            }
            // let mut vec_items = Vec::new();

            // while !self.is_shard_empty(current) {
            //     let item = self.dequeue_in_shard(current);
            //     vec_items.push(item);
            // }

            for item in items {
                self.enqueue_in_shard(current, item);
            }

            self.release_shard(current);
            // Some(vec_items)
        }
        // Otherwise, the user is likely going through a single dequeuer route
        // (MPSC or SPSC), so we can just approach this in a lock free slot
        // based manner, context switching if no job is available
        // else {
        //     // If poisoned, do not enqueue this item.
        //     if self.poisoned.load(Ordering::Relaxed) {
        //         println!("Can't enqueue anymore. Ring buffer is poisoned.");
        //         return;
        //     }
        //     loop {
        //         let inner = &self.inner_rb[0];
        //         let enq_counter = inner.enqueue_index.get();
        //         let deq_counter = inner.dequeue_index.get();
        //         let jobs = enq_counter.wrapping_sub(deq_counter);

        //         for item in items {
        //             let enqueue_index = inner.enqueue_index.get() % inner.items.len();
        //             inner.enqueue_index.set(inner.enqueue_index.get() + 1);
        //             let item_cell = inner.items[enqueue_index].get();
        //             unsafe {
        //                 (*item_cell).write(item);
        //             }
        //             self.job_post_notif.notify_one();
        //             return;
        //         }

        //         // we're using a notify for the enqueuer
        //         // it minimizes the number times we context switch
        //         self.job_post_notif.notify_waiters();
        //         self.job_space_notif.notified().await;
        //     }
        // }
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
        if self.get_num_of_shards() == 1 && Handle::current().metrics().num_workers() == 1 {
            self.job_post_notif.notify_waiters();
        } 
        // else {
        //     for notify in &self.job_post_shard_notifs {
        //         notify.notify_one();
        //     }
        // }
    }

    #[inline(always)]
    pub fn notify_pin_shard(&self, shard_ind: usize) {
        assert!(shard_ind < self.get_num_of_shards(), "Shard index must be within the number of shards that exist");
        // while self.deq_fin_taken.get() {}
        self.job_post_shard_notifs[shard_ind].notify_one();
        // self.job_post_shard_notifs[shard_ind].notify_waiters();
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
            let mut drop_index = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();
            let stop_index = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued. Note here that this method uses Relaxed loads.
                unsafe {
                    ptr::drop_in_place((*self.inner_rb[shard].items[drop_index].get()).as_mut_ptr())
                }
                drop_index = (drop_index + 1) % self.inner_rb[shard].items.len();
            }
            self.inner_rb[shard].enqueue_index.store(0, Ordering::Relaxed);
            self.inner_rb[shard].dequeue_index.store(0, Ordering::Relaxed);
        }
    }

    /// Clears the buffer back to an empty state
    ///
    /// Note: If you plan to clear the buffer from a single thread or after
    /// all tasks are completed, then you should use [Self::clear]
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space complexity: O(1)
    pub async fn async_clear(&self) {
        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        let mut guards = Vec::new();
        for shard in 0..self.shard_locks.len() {
            let guard = ShardLockGuard::acquire(&self.shard_locks[shard]).await;
            guards.push(guard);
        }

        // reset each shard's inner ring buffer
        for (shard_ind, _guard) in guards.into_iter().enumerate() {
            let inner = &self.inner_rb[shard_ind];
            let mut drop_index = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();
            let stop_index = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued.
                unsafe {
                    ptr::drop_in_place(
                        (*self.inner_rb[shard_ind].items[drop_index].get()).as_mut_ptr(),
                    )
                }
                drop_index = (drop_index + 1) % self.inner_rb[shard_ind].items.len();
            }
            self.inner_rb[shard_ind].enqueue_index.store(0, Ordering::Relaxed);
            self.inner_rb[shard_ind].dequeue_index.store(0, Ordering::Relaxed);
        }
    }

    /// Checks if all shards are empty in an async manner
    ///
    /// Time Complexity: O(s * s_t) where s is the number of shards
    /// and s_t is the time it takes to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_is_empty(&self) -> bool {
        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        let mut guards = Vec::new();
        for shard in 0..self.shard_locks.len() {
            let guard = ShardLockGuard::acquire(&self.shard_locks[shard]).await;
            guards.push(guard);
        }

        for (shard_ind, _guard) in guards.into_iter().enumerate() {
            if !self.is_shard_empty(shard_ind) {
                return false;
            }
        }
        // if it got to this point, then indeed it was empty at this point
        true
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
        let (enq_ind, deq_ind) = (inner.enqueue_index.load(Ordering::Relaxed), inner.enqueue_index.load(Ordering::Relaxed));
        let jobs = enq_ind.wrapping_sub(deq_ind);
        jobs == 0
    }

    /// Checks to see if a specific shard is empty in an async manner
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_is_shard_empty(&self, shard_ind: usize) -> bool {
        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

        let inner = &self.inner_rb[shard_ind];
        // use these values as monotonic counter than indices
        let (enq_ind, deq_ind) = (inner.enqueue_index.load(Ordering::Relaxed), inner.enqueue_index.load(Ordering::Relaxed));
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

    /// Checks if all shards are full in an async manner
    ///
    /// Time Complexity: O(s * s_t) where s is the number of shards and
    /// s_t is the time to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_is_full(&self) -> bool {
        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        let mut guards = Vec::new();
        for shard in 0..self.shard_locks.len() {
            let guard = ShardLockGuard::acquire(&self.shard_locks[shard]).await;
            guards.push(guard);
        }

        for (shard_ind, _guard) in guards.into_iter().enumerate() {
            if !self.is_shard_full(shard_ind) {
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
        let (enq_ind, deq_ind) = (inner.enqueue_index.load(Ordering::Relaxed), inner.enqueue_index.load(Ordering::Relaxed));
        let jobs = enq_ind.wrapping_sub(deq_ind);
        jobs == item_len
    }

    /// Checks to see if a specific shard is full in an async manner
    ///
    /// Time Complexity: O(s_t) where s_t is the time to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_is_shard_full(&self, shard_ind: usize) -> bool {
        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

        let inner = &self.inner_rb[shard_ind];
        let item_len = inner.items.len();
        // use these values as monotonic counter than indices
        let (enq_ind, deq_ind) = (inner.enqueue_index.load(Ordering::Relaxed), inner.enqueue_index.load(Ordering::Relaxed));
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

    /// Get the enqueue index within a shard in an async manner.
    /// Returns None if the shard index is invalid.
    ///
    /// Time Complexity: O(s_t) where s_t is the time to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_get_enq_ind_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

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
        let deq_ind = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();

        Some(deq_ind)
    }

    /// Get the dequeue index within a shard in an async manner.
    /// Returns None if the shard index is invalid.
    ///
    /// Time Complexity: O(s_t) where s_t is the time to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_get_deq_ind_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

        // grab deq ind val
        let inner = &self.inner_rb[shard_ind];
        let deq_ind = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();

        Some(deq_ind)
    }

    /// Gets the total number of jobs within a shard. Returns None if the shard
    /// index is invalid.
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
        let (enq_count, deq_count) = (inner.enqueue_index.load(Ordering::Relaxed), inner.enqueue_index.load(Ordering::Relaxed));
        let jobs = enq_count.wrapping_sub(deq_count);
        Some(jobs)
    }

    /// Gets the total number of jobs within a shard in an async manner.
    /// Returns None if the shard index is invalid.
    ///
    /// Time Complexity: O(s_t) where s_t is the time to acquire a shard
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub async fn async_get_job_count_at_shard(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

        let inner = &self.inner_rb[shard_ind];
        let (enq_count, deq_count) = (inner.enqueue_index.load(Ordering::Relaxed), inner.enqueue_index.load(Ordering::Relaxed));
        let jobs = enq_count.wrapping_sub(deq_count);
        Some(jobs)
    }

    /// Gets the total number of jobs within each shard.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(s)
    #[inline(always)]
    pub fn get_job_count_total(&self) -> Vec<usize> {
        let mut count = Vec::new();

        for shard in &self.inner_rb {
            let (enq_count, deq_count) = (shard.enqueue_index.load(Ordering::Relaxed), shard.dequeue_index.load(Ordering::Relaxed));
            let jobs = enq_count.wrapping_sub(deq_count);
            count.push(jobs);
        }
        count
    }

    /// Gets the total number of jobs within each shard in an async manner.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(s)
    #[inline(always)]
    pub async fn async_get_job_count_total(&self) -> Vec<usize> {
        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        let mut guards = Vec::new();
        for shard in 0..self.shard_locks.len() {
            let guard = ShardLockGuard::acquire(&self.shard_locks[shard]).await;
            guards.push(guard);
        }

        let mut count = Vec::new();
        // Neat thing here is that this for loop owns the guards, so it'll drop the
        // guard for me when it goes to the next iteration
        for (shard_ind, _guard) in guards.into_iter().enumerate() {
            let shard = &self.inner_rb[shard_ind];
            let (enq_count, deq_count) = (shard.enqueue_index.load(Ordering::Relaxed), shard.dequeue_index.load(Ordering::Relaxed));
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
        let dequeue_ind = inner.enqueue_index.load(Ordering::Relaxed) % inner.items.len();

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

    /// Returns a clone of an item within the buffer in an async manner or
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
    pub async fn async_clone_item_at_shard(&self, item_ind: usize, shard_ind: usize) -> Option<T>
    where
        T: Clone,
    {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        if item_ind >= self.inner_rb[shard_ind].items.len() {
            return None;
        }

        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

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

    /// Returns a clone of a specific InnerRingBuffer shard in an async manner or
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
    pub async fn async_clone_items_at_shard(&self, shard_ind: usize) -> Option<Box<[Option<T>]>>
    where
        T: Clone,
    {
        if shard_ind >= self.shard_locks.len() {
            return None;
        }

        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

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

    /// Returns a clone of the entire buffer in its current state in an async manner
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
    pub async fn async_clone_items(&self) -> Box<[Box<[Option<T>]>]>
    where
        T: Clone,
    {
        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        let mut guards = Vec::new();
        for shard in 0..self.shard_locks.len() {
            let guard = ShardLockGuard::acquire(&self.shard_locks[shard]).await;
            guards.push(guard);
        }

        let inner = &self.inner_rb;
        let mut vec = Vec::with_capacity(inner.len());

        // Clone items in each shard
        // Neat thing is that this for loop owns the guards, so it'll drop the
        // guard for me when it goes to the next iteration
        for (shard_ind, _guard) in guards.into_iter().enumerate() {
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

    /// Print out the content inside the a specific shard of the buffer in an
    /// async manner. If nothing is printed out, that means an invalid shard
    /// index was provided
    ///
    /// Time Complexity: O(c_s * s_t) c_s is the capacity of the shard
    /// and s_t is how long it takes to acquire the shard
    ///
    /// Space Complexity: O(1)
    pub async fn async_print_shard(&self, shard_ind: usize)
    where
        T: Debug,
    {
        if shard_ind >= self.shard_locks.len() {
            return;
        }

        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        // acquire shard in a cancel safe manner
        ShardLockGuard::acquire(&self.shard_locks[shard_ind]).await;

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

    /// Print out the content inside the entire buffer in an async manner
    ///
    /// Time Complexity: O(s * c_s * s_t) where s is the num of shards,
    /// c_s is the capacity per shard, and s_t is how long it takes to
    /// acquire shard(s)
    ///
    /// Space Complexity: O(1)
    pub async fn async_print(&self)
    where
        T: Debug,
    {
        // Acquire all shards
        // CANCEL SAFETY: When a future is aborted, it puts false back into the lock
        let mut guards = Vec::new();
        for shard in 0..self.shard_locks.len() {
            let guard = ShardLockGuard::acquire(&self.shard_locks[shard]).await;
            guards.push(guard);
        }

        let mut print_buffer = String::new();
        write!(print_buffer, "[").unwrap();

        // Neat thing here is that this for loop owns the guards, so it'll drop the
        // guard for me when it goes to the next iteration
        for (shard_ind, _guard) in guards.into_iter().enumerate() {
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

// Destructor trait for ShardedRingBuf just in case it gets dropped and its
// list of AtomicPtrs are not fully cleaned up yet.
impl<T> Drop for ShardedRingBuf<T> {
    fn drop(&mut self) {
        let mut current = self.head.load(Ordering::Relaxed);
        // SAFETY: Once I see null, I know all the nodes have been freed
        while !current.is_null() {
            unsafe {
                let next = (*current).next.load(Ordering::Relaxed);
                drop(Box::from_raw(current));
                current = next;
            }
        }
    }
}

// Destructor trait created for InnerRingBuffer<T> to clean up
// memory allocated onto MaybeUninit<T> when ShardedRingBuf<T>
// goes out of scope
impl<T> Drop for InnerRingBuffer<T> {
    fn drop(&mut self) {
        for item_ind in 0..self.items.len() {
            if self.is_item_in_shard(item_ind) {
                let ptr = self.items[item_ind].get();
                if !ptr.is_null() {
                    // SAFETY: I know that if the ptr isn't null, that means there is
                    // an item here, so we drop both the heap memory allocation and the
                    // item allocation
                    unsafe {
                        let _ = Box::from_raw(ptr as *mut T);
                    }
                } else {
                    // SAFETY: If the ptr is null, then there's no item here, but we still
                    // need to drop the heap allocation for the MaybeUninit
                    // Because MaybeUninit<T> doesn't implement drop itself on T,
                    // then that's okay to use a Box here to just drop the MaybeUninit.
                    unsafe {
                        let _ = Box::from_raw(ptr);
                    }
                }
            }
        }
    }
}
