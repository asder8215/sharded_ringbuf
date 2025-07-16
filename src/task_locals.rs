use crate::{
    shard_policies::ShardPolicyKind,
    task_node::{TaskNode, TaskNodePtr},
};
use crossbeam_utils::CachePadded;
use std::{
    cell::{Cell, UnsafeCell},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicPtr, Ordering},
    },
};
use tokio::task_local;

task_local! {
    pub(crate) static SHARD_INDEX: Cell<Option<usize>>;           // initial shard index of task
    pub(crate) static SHIFT: Cell<usize>;                         // how much to shift the task's shard index by
    pub(crate) static SHARD_POLICY: Cell<ShardPolicyKind>;        // shard policy for buffer
    // TODO: Experiment to see how CachePadding works with this
    // pub(crate) static TASK_NODE: Box<UnsafeCell<TaskNode>>;  // The enqueuer/dequeuer task has a reference to its node
    pub(crate) static TASK_NODE: Arc<TaskNode>;  // The enqueuer/dequeuer task has a reference to its node

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
pub(crate) fn get_task_node() -> AtomicPtr<TaskNode> {
    TASK_NODE
        .try_with(|ptr| AtomicPtr::new(ptr.as_ref() as *const _ as *mut TaskNode))
        .unwrap_or_else(|_| {
            panic!(
                "TASK_NODE is not initialized. Use `.spawn_buffer_task()` with CFT shard policy."
            )
        })
}

// #[inline(always)]
// pub(crate) fn set_task_node(task_ptr: CachePadded<TaskNodePtr>) {
//     TASK_NODE
//         .try_with(|ptr| {
//             ptr.set(task_ptr);
//         })
//         .unwrap_or_else(|_| {
//             panic!(
//                 "TASK_NODE is not initialized. Use `.spawn_buffer_task()` with CFT shard policy."
//             )
//         })
// }

// #[inline(always)]
// pub(crate) fn set_task_node(task_ptr: CachePadded<TaskNodePtr>) {
//     TASK_NODE
//         .try_with(|ptr| {
//             ptr.set(task_ptr);
//         })
//         .unwrap_or_else(|_| {
//             panic!(
//                 "TASK_NODE is not initialized. Use `.spawn_buffer_task()` with CFT shard policy."
//             )
//         })
// }

// #[inline(always)]
// pub(crate) fn set_task_done() {
//     TASK_NODE
//         .try_with(|ptr| {
//             // let TaskNodePtr(task_ptr) = ptr.get();
//             unsafe {
//                 (*ptr.get().0).is_done.store(true, Ordering::Relaxed)
//                 // (*ptr.get().0).is_done.store(true, Ordering::Release)
//             }
//         })
//         .unwrap_or_else(|_| {
//             panic!(
//                 "TASK_NODE is not initialized. Use `.spawn_buffer_task()` with CFT shard policy."
//             )
//         })
// }
