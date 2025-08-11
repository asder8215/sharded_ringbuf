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
    pub(crate) static TASK_NODE: Cell<TaskNodePtr>;
}

// FIXME: change unwrap_or_else to expect
#[inline(always)]
pub(crate) fn get_shard_ind() -> Option<usize> {
    SHARD_INDEX
        .try_with(|cell| cell.get())
        .expect("SHARD_INDEX is not initialized")
}

#[inline(always)]
pub(crate) fn set_shard_ind(val: usize) {
    SHARD_INDEX
        .try_with(|cell| {
            cell.set(Some(val));
        })
        .expect("SHARD_INDEX is not initialized");
}

#[inline(always)]
pub(crate) fn get_shard_policy() -> ShardPolicyKind {
    SHARD_POLICY
        .try_with(|cell| cell.get())
        .expect("SHARD_POLICY is not initialized")
}

#[inline(always)]
pub(crate) fn get_shift() -> usize {
    SHIFT
        .try_with(|cell| cell.get())
        .expect("SHIFT is not initialized")
}

#[inline(always)]
pub(crate) fn get_task_node() -> TaskNodePtr {
    TASK_NODE
        .try_with(|ptr| ptr.get())
        .expect("TASK_NODE is not initialized")
}

#[inline(always)]
pub(crate) fn set_task_node(task_ptr: TaskNodePtr) {
    TASK_NODE
        .try_with(|ptr| {
            ptr.set(task_ptr);
        })
        .expect("TASK_NODE is not initialized")
}
