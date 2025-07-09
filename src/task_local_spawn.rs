use crate::{
    ShardPolicy,
    task_locals::{SHARD_INDEX, SHARD_POLICY, SHIFT},
};
use std::cell::Cell;
use tokio::task::{JoinHandle, spawn};

/// Spawns a Tokio task for the purpose of using it with LFShardedRingBuf
///
/// This function *must* be used in order to enqueue or dequeue items onto
/// LFShardedRingBuf
pub fn spawn_buffer_task<F, T>(policy: ShardPolicy, fut: F) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(policy),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(Cell::new(policy), SHARD_INDEX.scope(Cell::new(None), fut)),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(policy),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
    }
}
