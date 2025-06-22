use std::cell::Cell;
use tokio::task::{JoinHandle, spawn};
use tokio::task_local;
// use fastrand::usize as frand;

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

pub fn get_shard_ind() -> Option<usize> {
    SHARD_INDEX
        .try_with(|cell| {
            let cell_val = cell.get();
            // let current = match cell_val {
            //     Some(val) => (val + 1) % self.shards,  // look at the next shard
            //     None => frand(0..self.shards),  // init rand shard for thread to look at
            // };
            // cell.set(Some(current));
            cell_val
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        });
    return None;
}

pub fn set_shard_ind(val: usize) {
    SHARD_INDEX
        .try_with(|cell| {
            cell.set(Some(val));
        })
        .unwrap_or_else(|_| {
            panic!("SHARD_INDEX is not initialized. Use `.spawn_with_shard_index()`.")
        });
}
