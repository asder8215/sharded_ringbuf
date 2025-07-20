use crate::{shard_policies::ShardPolicyKind, task_node::TaskNodePtr};
use std::cell::Cell;
use tokio::task_local;

task_local! {
    /// initial shard index of task
    pub(crate) static SHARD_INDEX: Cell<Option<usize>>;
    /// how much to shift the task's shard index by
    pub(crate) static SHIFT: Cell<usize>;
    /// shard policy for buffer
    pub(crate) static SHARD_POLICY: Cell<ShardPolicyKind>;
    /// The enqueuer/dequeuer task has a reference to its node
    // pub(crate) static TASK_NODE: Cell<CachePadded<TaskNodePtr>>;
    pub(crate) static TASK_NODE: Cell<TaskNodePtr>;
    // The enqueuer/dequeuer task has a reference to its node
    // I tried this out to have `spawn_with_cft()` be cancel-free by having the
    // responsibility of freeing and changing the list assigned to the TaskNode
    // but this runs into the problem of needing to make 1) a doubly linkedlist
    // and 2) making that doubly linked list handle ABA and how to use epoch reclamation
    // 3) how do you lock both of your adjacent pair in a manner that won't cause
    // deadlocks? (remember task locals only can work with themselves.)
    // It's really, really troublesome to do this, but I think it's possible
    // Maybe in the future I'll make a cancel free smart task policy for this.
    // pub(crate) static TASK_NODE: Cell<Box<TaskNode>>;

}

// FIXME: change unwrap_or_else to expect
#[inline(always)]
pub(crate) fn get_shard_ind() -> Option<usize> {
    SHARD_INDEX
        .try_with(|cell| cell.get())
        .unwrap_or_else(|_| panic!("SHARD_INDEX is not initialized. Use `.spawn_buffer_task()`."))
}

#[inline(always)]
pub(crate) fn set_shard_ind(val: usize) {
    SHARD_INDEX
        .try_with(|cell| {
            cell.set(Some(val));
        })
        .unwrap_or_else(|_| panic!("SHARD_INDEX is not initialized. Use `.spawn_buffer_task()`."));
}

#[inline(always)]
pub(crate) fn get_shard_policy() -> ShardPolicyKind {
    SHARD_POLICY
        .try_with(|cell| cell.get())
        .unwrap_or_else(|_| panic!("SHARD_POLICY is not initialized. Use `.spawn_buffer_task()`."))
}

#[inline(always)]
pub(crate) fn get_shift() -> usize {
    SHIFT
        .try_with(|cell| cell.get())
        .unwrap_or_else(|_| panic!("SHIFT is not initialized. Use `.spawn_buffer_task()`."))
}

#[inline(always)]
pub(crate) fn get_task_node() -> TaskNodePtr {
    // TASK_NODE.try_with(|ptr| *ptr.get()).unwrap_or_else(|_| {
    //     panic!("TASK_NODE is not initialized. Use `.spawn_buffer_task()` with CFT shard policy.")
    // })
    TASK_NODE.try_with(|ptr| ptr.get()).unwrap_or_else(|_| {
        panic!("TASK_NODE is not initialized. Use `.spawn_with_cft()` with CFT shard policy.")
    })
}

#[inline(always)]
// pub(crate) fn set_task_node(task_ptr: CachePadded<TaskNodePtr>) {
pub(crate) fn set_task_node(task_ptr: TaskNodePtr) {
    TASK_NODE
        .try_with(|ptr| {
            ptr.set(task_ptr);
        })
        .unwrap_or_else(|_| {
            panic!("TASK_NODE is not initialized. Use `.spawn_with_cft()` with CFT shard policy.")
        })
}
