use crossbeam_utils::CachePadded;
use std::{
    cell::UnsafeCell,
    fmt::{Debug, Write},
    mem::MaybeUninit,
    ptr::{self},
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

// Enum used in try_acquire_slot to determine if task
// is enqueue or dequeue
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Acquire {
    Enqueue,
    Dequeue,
}

/// A sharded ring (circular) buffer struct that can only be used in an *async environment*.
///
/// The key difference about this struct is that it is mostly lock free (hence MLF) because
/// all the sharded ring buffer uses a lock free queue underneath the hood. The non-lock free
/// part comes from using Tokio's Notify to wake up sleeping tasks that are not able to enqueue
/// or dequeue anything in the buffer due to fullness/emptiness. Tokio's Notify uses a Mutex
/// in its waitlist of wakers.
///
/// In theory, a fully lock-free multi-ringbuffer data structure is possible
/// with a lock free waitlist of wakers (think of lock free unbounded FIFO queue using a doubly linked list
/// or even a stack-based style using a lock free singly linked list).
/// I would imagine this would be faster than using Mutex<Waitlist> because, in most cases,
/// you would be able to add a push a new waiter at the head of the queue whilst popping a waiter from the back
/// end of the queue when you notify one waiter. However  
#[derive(Debug)]
pub struct MLFShardedRingBuf<T> {
    /// Total capacity of the buffer
    capacity: AtomicUsize,
    /// Multiple InnerRingBuffer structure based on num of shards
    /// CachePadded to prevent false sharing
    inner_rb: Box<[CachePadded<InnerRingBuffer<T>>]>,

    pub(crate) job_space_shard_notifs: Box<[Notify]>,
    pub(crate) job_post_shard_notifs: Box<[Notify]>,
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
    state: AtomicU8,
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
        }
    }

    /// Returns the number of shards used in this buffer
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_num_of_shards(&self) -> usize {
        self.inner_rb.len()
    }

    /// Returns the capacity of a specific shard in this buffer
    /// or None if provided an invalid shard index
    ///
    /// Time Complexity: O(1)
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn get_shard_capacity(&self, shard_ind: usize) -> Option<usize> {
        if shard_ind >= self.inner_rb.len() {
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

    async fn try_acquire_slot(&self, acquire: Acquire, shard_ind: usize) -> (usize, usize) {
        loop {
            let slot_taken = self.take_slot(acquire, shard_ind).await;
            if let Some(slot) = slot_taken {
                return slot;
            }

            if matches!(acquire, Acquire::Dequeue) {
                self.job_post_shard_notifs[shard_ind].notified().await;
            } else {
                self.job_space_shard_notifs[shard_ind].notified().await;
            }
        }
    }

    async fn take_slot(&self, acquire: Acquire, shard_ind: usize) -> Option<(usize, usize)> {
        let inner = &self.inner_rb[shard_ind];
        if matches!(acquire, Acquire::Enqueue) {
            let enq_ind = inner.enqueue_index.fetch_add(1, Ordering::AcqRel) % inner.items.len();
            loop {
                let acquire_slot = inner.items[enq_ind].state.compare_exchange(
                    0,
                    3,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                );
                match acquire_slot {
                    Ok(_) => return Some((shard_ind, enq_ind)),
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
                let acquire_slot = inner.items[deq_ind].state.compare_exchange(
                    1,
                    2,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                );
                match acquire_slot {
                    Ok(_) => return Some((shard_ind, deq_ind)),
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
                self.job_space_shard_notifs[shard_ind].notify_one();
            }
            Acquire::Enqueue => {
                inner.items[slot_ind].state.store(1, Ordering::Relaxed);
                self.job_post_shard_notifs[shard_ind].notify_one();
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
    #[inline(always)]
    async fn enqueue_item(&self, item: Option<T>, shard_ind: usize) {
        // acquire shard
        let current = self.try_acquire_slot(Acquire::Enqueue, shard_ind).await;
        self.enqueue_in_slot(current.0, current.1, item);
        self.release_slot(current.0, current.1, Acquire::Enqueue);
    }

    /// Adds an item of type T to the RingBuffer, *blocking* the thread until there is space to add the item.
    ///
    /// Time Complexity: O(s_t) where s_t is the time it takes to acquire a shard
    ///
    /// Space complexity: O(1)
    pub(crate) async fn enqueue(&self, item: T, shard_ind: usize) {
        let shard_ind = shard_ind % self.get_num_of_shards();
        self.enqueue_item(Some(item), shard_ind).await;
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
    #[inline(always)]
    pub(crate) async fn dequeue(&self, shard_ind: usize) -> Option<T> {
        let shard_ind = shard_ind % self.get_num_of_shards();
        let current = self.try_acquire_slot(Acquire::Dequeue, shard_ind).await;
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
    pub async fn poison_at_shard(&self, shard_ind: usize) {
        assert!(
            shard_ind < self.get_num_of_shards(),
            "Shard index must be within the number of shards that exist"
        );
        // let inner = &self.inner_rb[shard_ind];
        // let enq_ind = (inner.enqueue_index.fetch_add(1, Ordering::AcqRel)) % inner.items.len();
        // self.enqueue_in_slot(shard_ind, enq_ind, None);
        // inner.items[enq_ind].state.store(1, Ordering::Relaxed);
        let slot = self.take_slot(Acquire::Enqueue, shard_ind).await;
        if let Some(slot) = slot {
            self.enqueue_in_slot(slot.0, slot.1, None);
            self.release_slot(slot.0, slot.1, Acquire::Enqueue);
        }
    }

    /// Notifies one dequeuer task assigned to each shard.
    ///
    /// Time Complexity: O(s) where s is the number of shards
    ///
    /// Space Complexity: O(1)
    #[inline(always)]
    pub fn notify_pin_shard(&self, shard_ind: usize) {
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
        for shard in 0..self.inner_rb.len() {
            let inner = &self.inner_rb[shard];
            let mut drop_index = inner.dequeue_index.load(Ordering::Acquire) % inner.items.len();
            let stop_index = inner.enqueue_index.load(Ordering::Acquire) % inner.items.len();
            while drop_index != stop_index {
                // SAFETY: This will only clear out initialized values that have not
                // been dequeued. Note here that this method uses Relaxed loads.
                unsafe {
                    ptr::drop_in_place(
                        (*self.inner_rb[shard].items[drop_index].item.get()).as_mut_ptr(),
                    )
                }
                self.inner_rb[shard].items[drop_index]
                    .state
                    .store(0, Ordering::Relaxed);
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
        for i in 0..self.inner_rb.len() {
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
        for i in 0..self.inner_rb.len() {
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
        if shard_ind >= self.inner_rb.len() {
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
        if shard_ind >= self.inner_rb.len() {
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
        if shard_ind >= self.inner_rb.len() {
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
        if shard_ind >= self.inner_rb.len() {
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
        if shard_ind >= self.inner_rb.len() {
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
        if shard_ind >= self.inner_rb.len() {
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
        for shard_ind in 0..self.inner_rb.len() {
            let inner_shard = &self.inner_rb[shard_ind];
            write!(print_buffer, "[").unwrap();

            // SAFETY: Only print values in between [dequeue_ind, enqueue_ind) + wraparound
            // otherwise print None as a placeholder value
            for i in 0..inner_shard.items.len() {
                if i == inner_shard.items.len() - 1 {
                    if self.is_item_in_shard(i, shard_ind) {
                        let val_ref =
                            unsafe { (*inner_shard.items[i].item.get()).assume_init_ref() };
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
