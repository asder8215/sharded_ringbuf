use std::cell::Cell;
use tokio::task::{JoinHandle, spawn};
use tokio::task_local;

task_local! {
    static SHARD_INDEX: Cell<Option<usize>>; // initial shard index of task
    static SHIFT: Cell<usize>; // how much to shift the task's shard index by
    static SHARD_POLICY: Cell<ShardPolicy>; // shard policy for buffer
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// These are the Shard Acquistion Policies supported by LFShardedRingBuf
/// 
/// Sweep: The task starts off at provided index (or random *initial index* if None) 
/// and performs a full sweep around the buffer to acquire shards to 
/// enqueue/dequeue items (shift by 1)
/// 
/// RandomAndSweep: The task starts off at a random index always and performs a full
/// sweep around the buffer to acquire shards to enqueue/dequeue items
/// 
/// ShiftBy: The task starts off at provided index (or random *initial index* if None)
/// and performs a sweep around the buffer using a provided shift value. To prevent
/// tasks from being stuck in a shift by sweep, every two attempts of failing to acquire
/// a shard results in task's shard_id being incremented by one before applying shift
/// by.
pub enum ShardPolicy {
    Sweep,
    RandomAndSweep,
    ShiftBy,
}

pub fn spawn_with_shard_index<F, T>(
    initial_index: Option<usize>,
    policy: ShardPolicy,
    shift: usize,
    fut: F,
) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match policy {
        ShardPolicy::ShiftBy => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(policy),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
        _ => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(Cell::new(policy), SHARD_INDEX.scope(Cell::new(None), fut)),
        )),
    }
}

#[inline(always)]
pub fn get_shard_ind() -> Option<usize> {
    SHARD_INDEX.try_with(|cell| cell.get()).unwrap_or_else(|_| {
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
        .try_with(|cell| cell.get())
        .unwrap_or_else(|_| {
            panic!("SHARD_POLICY is not initialized. Use `.spawn_with_shard_index()`.")
        })
}

#[inline(always)]
pub fn get_shift() -> usize {
    SHIFT
        .try_with(|cell| cell.get())
        .unwrap_or_else(|_| panic!("SHIFT is not initialized. Use `.spawn_with_shard_index()`."))
}
