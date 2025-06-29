use std::cell::Cell;
use tokio::task::{JoinHandle, spawn};
use tokio::task_local;

task_local! {
    static SHARD_INDEX: Cell<Option<usize>>;
    static SHIFT: Cell<usize>;
    static SHARD_POLICY: Cell<ShardPolicy>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShardPolicy {
    Sweep,
    RandomAndSweep,
    ShiftBy
}

pub fn spawn_with_shard_index<F, T>(initial_index: Option<usize>, policy: ShardPolicy, shift: usize, fut: F) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match policy {
        ShardPolicy::ShiftBy => spawn(SHIFT.scope(Cell::new(shift), SHARD_POLICY.scope(Cell::new(policy), SHARD_INDEX.scope(Cell::new(initial_index), fut)))),
        _ => spawn(SHIFT.scope(Cell::new(1), SHARD_POLICY.scope(Cell::new(policy), SHARD_INDEX.scope(Cell::new(None), fut))))
    }
}

#[inline(always)]
pub fn get_shard_ind() -> Option<usize> {
    SHARD_INDEX
        .try_with(|cell| {
            cell.get()
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        })
}

#[inline(always)]
pub fn set_shard_ind(val: usize) {
    SHARD_INDEX
        .try_with(|cell| {
            cell.set(Some(val));
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        });
}

#[inline(always)]
pub fn get_shard_policy() -> ShardPolicy {
    SHARD_POLICY
        .try_with(|cell| {
            cell.get()
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        })
}

#[inline(always)]
pub fn get_shift() -> usize {
    SHIFT
        .try_with(|cell| {
            cell.get()
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        })
}
