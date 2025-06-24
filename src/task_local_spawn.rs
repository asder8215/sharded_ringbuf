use std::cell::Cell;
use tokio::task::{JoinHandle, spawn};
use tokio::task_local;

task_local! {
    static SHARD_INDEX: Cell<Option<usize>>;
}

pub fn spawn_with_shard_index<F, T>(initial_index: Option<usize>, fut: F) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    spawn(SHARD_INDEX.scope(Cell::new(initial_index), fut))
}

#[inline(always)]
pub fn get_shard_ind() -> Option<usize> {
    SHARD_INDEX
        .try_with(|cell| {
            cell.get()
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        });
    None
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
