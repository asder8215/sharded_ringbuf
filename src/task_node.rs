use std::{
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize},
};

/// This is to denote whether the task you are spawning
/// is an enqueuer or a dequeur. You use this in `spawn_with_cft()`
/// or `rt_spawn_with_cft`
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TaskRole {
    Enqueue,
    Dequeue,
}

/// The assigner task uses this to traverse in a singular lock-free LinkedList manner to
/// pair up enqueuer and dequeuer task together by shard index. It is responsible
/// for cleaning up the LinkedList when the enqueuer/dequeuer says they are done.
///
/// The enqueuer and dequeuer task uses this to find out what shard index to perform its
/// operations at. It is responsible for adding itself to the head of the LinkedList in
/// LFShardedRingBuf and marking that it's done.
///
/// If the LFShardedRingBuf is dropped, the LFShardedRingBuf is responsible for cleaning up any
/// leftover allocated TaskNodes that possibly the assigner task has not had the chance to clean up
/// in the event that the assigner task is terminated early. As a result, there should be no memory
/// leaks when the program finishes.
///
/// Made all the field pub(crate) because too lazy to make getter and setter methods for these :p
/// Besides, it's private to the crate so no misuse possible by user (though I might organize this
/// later down the line)
#[derive(Debug)]
pub(crate) struct TaskNode {
    /// A static role of what the Task is
    pub(crate) role: TaskRole,
    /// If the task is completed (assigner reads, enq/deq writes)
    pub(crate) is_done: AtomicBool,
    /// If the task is paired with deq/enq (assigner writes, enq/deq reads)
    pub(crate) is_paired: AtomicBool,
    /// Whether the shard_ind is written or not (assigner writes, enq/deq reads)
    pub(crate) is_assigned: AtomicBool,
    /// The shard index that a task will look at (assigner writes, enq/deq reads)
    pub(crate) shard_ind: AtomicUsize,
    /// The next TaskNode
    pub(crate) next: AtomicPtr<TaskNode>,
}

impl TaskNode {
    /// Instantiates a new TaskNode
    pub(crate) fn new(role: TaskRole) -> Self {
        Self {
            role,
            is_done: AtomicBool::new(false),
            is_paired: AtomicBool::new(false),
            is_assigned: AtomicBool::new(false),
            shard_ind: AtomicUsize::new(0), // this 0 is just a placeholder value, enqueuer/dequeuer should only read here if is_assigned is set
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}
impl PartialEq for TaskNode {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}
/// Testing Drop for TaskNode
impl Drop for TaskNode {
    fn drop(&mut self) {}
}
unsafe impl Send for TaskNode {}
unsafe impl Sync for TaskNode {}

/// This TaskNodePtr is used to wrap around
/// *mut TaskNode because *mut objects are not
/// Send or Sync (but they are Copy! which is
/// the most important part here)
#[derive(Debug, Clone, Copy)]
pub(crate) struct TaskNodePtr(pub *mut TaskNode);
unsafe impl Send for TaskNodePtr {}
unsafe impl Sync for TaskNodePtr {}
impl PartialEq for TaskNodePtr {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for TaskNodePtr {}
impl std::hash::Hash for TaskNodePtr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.0 as usize).hash(state);
    }
}
