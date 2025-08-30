use crate::{
    shard_policies::ShardPolicyKind,
    task_locals::{get_shard_ind, get_shard_policy, get_shift, get_task_node, set_shard_ind},
    task_node::{TaskNode, TaskNodePtr},
};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::UnsafeCell,
    fmt::{Debug, Write},
    mem::MaybeUninit,
    ptr::{self, null},
    sync::atomic::{AtomicBool, AtomicI8, AtomicPtr, AtomicU8, AtomicUsize, Ordering},
};
use tokio::{sync::Notify, task::yield_now};

// Enum used in try_acquire_shard to determine if task
// is enqueue or dequeue
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Acquire {
    Enqueue,
    Dequeue,
}

/// A sharded ring (circular) buffer struct that can only be used in an *async environment*.
#[derive(Debug)]
pub struct MLFShardedRingBuf<T> {
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

    pub(crate) job_space_shard_notifs: Box<[Notify]>,
    pub(crate) job_post_shard_notifs: Box<[Notify]>,

    // The fields below are for CFT policy
    /// The head of the TaskNode linked list for CFT
    pub(crate) head: AtomicPtr<TaskNode>,
    /// Signals whether the assigner is spawned already or not
    pub(crate) assigner_spawned: AtomicBool,
    /// Signals whether the assigner is terminated or not
    // pub(crate) assigner_terminate: AtomicBool,
    pub assigner_terminate: AtomicBool,
}

// An inner ring buffer to contain the items, enqueue and dequeue index for ShardedRingBuf struct
#[derive(Debug, Default)]
struct InnerRingBuffer<T> {
    /// Box of Slots containing the content of the buffer
    /// Cache Padded to avoid false sharing
    items: Box<[CachePadded<Slot<T>>]>,
    /// Where to enqueue at in the Box
    enqueue_index: CachePadded<AtomicUsize>,
    /// Where to dequeue at in the Box
    dequeue_index: AtomicUsize,
}

// An inner ring buffer to contain the items, enqueue and dequeue index for ShardedRingBuf struct
#[derive(Debug)]
struct Slot<T> {
    /// contains the actual item
    item: UnsafeCell<MaybeUninit<Option<T>>>,
    /// 0: empty, 1: full, 2: in progress (deq), 3: in progress (enq)
    state: AtomicU8
}

/// Implements the Slot functions
impl<T> Slot<T> {
    #[inline(always)]
    fn new() -> Self {
        Slot { 
            item: UnsafeCell::new(MaybeUninit::uninit()), 
            state: AtomicU8::new(0), 
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
                    vec.push(CachePadded::new(Slot::new()));
                }
                vec.into_boxed_slice()
            },
            enqueue_index: CachePadded::new(AtomicUsize::new(0)),
            dequeue_index: AtomicUsize::new(0),
        }
    }
}

impl<T> MLFShardedRingBuf<T> {
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

    /// Returns the total capacity of this buffer
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_total_capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    async fn try_acquire_slot(&self, acquire: Acquire) -> (usize, usize) {
        /*
         * Tasks start off with a random shard_ind or
         * user provided initial shard ind value % self.shards
         * before going around a circle
         */
        let shard_count = self.shard_locks.len();
        let mut slot_ind = 0;
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
                                return (0, 0);
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
            let slot_taken = self.take_slot(acquire, current).await;
            if let Some(slot) = slot_taken {
                set_shard_ind(current);
                current = slot.0;
                slot_ind = slot.1;
                break;
            }

            if matches!(get_shard_policy(), ShardPolicyKind::Pin) {
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
        (current, slot_ind)
    }

    #[inline(always)]
    async fn take_slot(&self, acquire: Acquire, shard_ind: usize) -> Option<(usize, usize)> {
        let inner = &self.inner_rb[shard_ind];
        if matches!(acquire, Acquire::Enqueue) {
            let enq_ind = inner.enqueue_index.fetch_add(1, Ordering::AcqRel) % inner.items.len();
            loop {
                let acquire_slot = inner.items[enq_ind].state.compare_exchange(0, 3, Ordering::AcqRel, Ordering::Relaxed);
                match acquire_slot {
                        Ok(_) => {
                            return Some((shard_ind, enq_ind))
                        },
                        Err(actual) => {
                            if actual == 2 {
                                self.job_space_shard_notifs[shard_ind].notified().await;
                            } else {
                                self.job_post_shard_notifs[shard_ind].notify_one();
                                self.job_space_shard_notifs[shard_ind].notified().await;
                            }
                        }
                    }
                }
            } else {
            let deq_ind = inner.dequeue_index.fetch_add(1, Ordering::AcqRel) % inner.items.len();
            loop {
                let acquire_slot = inner.items[deq_ind].state.compare_exchange(1, 2, Ordering::AcqRel, Ordering::Relaxed);
                match acquire_slot {
                    Ok(_) => {
                        return Some((shard_ind, deq_ind))
                    },
                    Err(actual) => {
                        if actual == 3 {
                            self.job_post_shard_notifs[shard_ind].notified().await;
                        } else {
                            self.job_space_shard_notifs[shard_ind].notify_one();
                            self.job_post_shard_notifs[shard_ind].notified().await;
                        }
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn release_slot(&self, shard_ind: usize, slot_ind: usize, acquire: Acquire) {
        let inner = &self.inner_rb[shard_ind];
        match acquire {
            Acquire::Dequeue => {
                inner.items[slot_ind].state.store(0, Ordering::Relaxed);
                if matches!(get_shard_policy(), ShardPolicyKind::Pin) {
                    self.job_space_shard_notifs[shard_ind].notify_one();
                }
            },
            Acquire::Enqueue => {
                inner.items[slot_ind].state.store(1, Ordering::Relaxed);
                if matches!(get_shard_policy(), ShardPolicyKind::Pin) {
                    self.job_post_shard_notifs[shard_ind].notify_one();
                }
            }
        }
    }

    /// Grab inner ring buffer shard, enqueue the item, update the enqueue index
    #[inline(always)]
    fn enqueue_in_slot(&self, shard_ind: usize, slot_ind: usize, item: Option<T>) {
        let inner = &self.inner_rb[shard_ind];

        let item_cell = inner.items[slot_ind].item.get();
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
    async fn enqueue_item(&self, item: Option<T>) {
        // acquire shard
        let current = self.try_acquire_slot(Acquire::Enqueue).await;
        self.enqueue_in_slot(current.0, current.1, item);
        self.release_slot(current.0, current.1, Acquire::Enqueue);
    }

    /// Adds an item of type T to the RingBuffer, *blocking* the thread until there is space to add the item.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub(crate) async fn enqueue(&self, item: T) {
        // if self.poisoned.load(Ordering::Relaxed) {
        //     println!("Can't enqueue anymore. Ring buffer is poisoned.");
        //     return;
        // }
        self.enqueue_item(Some(item)).await;
    }

    /// Grab the inner ring buffer shard, dequeue the item, update the dequeue index
    #[inline(always)]
    fn dequeue_in_slot(&self, shard_ind: usize, slot_ind: usize) -> Option<T> {
        let inner = &self.inner_rb[shard_ind];

        let item_cell = inner.items[slot_ind].item.get();

        // SAFETY: Only one thread will perform this operation
        // And it's guaranteed that an item will exist here
        let item = unsafe { (*item_cell).assume_init_read() };

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
        let current = self.try_acquire_slot(Acquire::Dequeue).await;
        let item = self.dequeue_in_slot(current.0, current.1);
        self.release_slot(current.0, current.1, Acquire::Dequeue);
        item
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
    pub fn poison_at_shard(&self, shard_ind: usize) {
        assert!(shard_ind < self.get_num_of_shards(), "Shard index must be within the number of shards that exist");
        let inner = &self.inner_rb[shard_ind];
        let enq_ind = (inner.enqueue_index.fetch_add(1, Ordering::AcqRel)) % inner.items.len();
        self.enqueue_in_slot(shard_ind, enq_ind, None);
        inner.items[enq_ind].state.store(1, Ordering::Relaxed);
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
    pub fn notify_pin_shard(&self, shard_ind: usize) {
        assert!(shard_ind < self.get_num_of_shards(), "Shard index must be within the number of shards that exist");
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
            let mut drop_index = inner.dequeue_index.load(Ordering::Acquire) % inner.items.len();
            let stop_index = inner.enqueue_index.load(Ordering::Acquire) % inner.items.len();
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued. Note here that this method uses Relaxed loads.
                unsafe {
                    ptr::drop_in_place((*self.inner_rb[shard].items[drop_index].item.get()).as_mut_ptr())
                }
                self.inner_rb[shard].items[drop_index].state.store(0, Ordering::Relaxed);
                drop_index = (drop_index + 1) % self.inner_rb[shard].items.len();
            }
            self.inner_rb[shard]
                .enqueue_index
                .store(0, Ordering::Release);
            self.inner_rb[shard]
                .dequeue_index
                .store(0, Ordering::Release);
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
            inner.enqueue_index.load(Ordering::Acquire),
            inner.dequeue_index.load(Ordering::Acquire),
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
            inner.enqueue_index.load(Ordering::Acquire),
            inner.dequeue_index.load(Ordering::Acquire),
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
                let val_ref = (*inner.items[item_ind].item.get()).assume_init_ref();
                if let Some(val_ref) = val_ref {
                    Some(val_ref.clone())
                } else {
                    None
                }
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
                    let val_ref = unsafe { (*inner.items[i].item.get()).assume_init_ref() };
                    if let Some(val_ref) = val_ref {
                        vec.push(Some(val_ref.clone()))
                    } else {
                        vec.push(None)
                    }
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
    pub fn clone_items(&self) -> Box<[Box<[Option<T>]>]>
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
                        let val_ref = unsafe { (*shard.items[i].item.get()).assume_init_ref() };
                        if let Some(val_ref) = val_ref {
                            shard_vec.push(Some(val_ref.clone()));
                        } else {
                            shard_vec.push(None);
                        }
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
                    let val_ref = unsafe { (*inner_shard.items[i].item.get()).assume_init_ref() };
                    write!(print_buffer, "{val_ref:?}").unwrap();
                } else {
                    write!(print_buffer, "<uninit>").unwrap();
                }
            } else if self.is_item_in_shard(i, shard_ind) {
                let val_ref = unsafe { (*inner_shard.items[i].item.get()).assume_init_ref() };
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
                        let val_ref = unsafe { (*inner_shard.items[i].item.get()).assume_init_ref() };
                        write!(print_buffer, "{val_ref:?}").unwrap();
                    } else {
                        write!(print_buffer, "<uninit>").unwrap();
                    }
                } else if self.is_item_in_shard(i, shard_ind) {
                    let val_ref = unsafe { (*inner_shard.items[i].item.get()).assume_init_ref() };
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

unsafe impl<T> Sync for MLFShardedRingBuf<T> {}
unsafe impl<T> Send for MLFShardedRingBuf<T> {}

unsafe impl<T> Sync for InnerRingBuffer<T> {}
unsafe impl<T> Send for InnerRingBuffer<T> {}

// Destructor trait for ShardedRingBuf just in case it gets dropped and its
// list of AtomicPtrs are not fully cleaned up yet.
impl<T> Drop for MLFShardedRingBuf<T> {
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