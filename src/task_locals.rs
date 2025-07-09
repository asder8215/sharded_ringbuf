use crate::ShardPolicy;
use std::cell::Cell;
use tokio::task_local;

task_local! {
    pub(crate) static SHARD_INDEX: Cell<Option<usize>>; // initial shard index of task
    pub(crate) static SHIFT: Cell<usize>; // how much to shift the task's shard index by
    pub(crate) static SHARD_POLICY: Cell<ShardPolicy>; // shard policy for buffer
}

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
pub(crate) fn get_shard_policy() -> ShardPolicy {
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
