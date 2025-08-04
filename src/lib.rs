//! An async, bounded, lock-free (not formally however), cache-aware ring buffer that can be
//! used in a SPSC, MPSC, and MPMC environment (optimal for MPMC however).
//!
//! The key feature of this ring buffer is that it takes a sharded approach to enqueuing
//! and dequeuing items with the use of Tokio's task local variables and context switching
//! via Tokio's `yield_now()`. In essence, this is a scheduler-based approach to making an
//! asynchronous, non-blocking ring buffer (Tokio, in this case, serving as the scheduler).
//!
//! Why use sharding in this ring buffer?
//! In Tokio's multithreaded runtime, each worker thread is provided its own set of tasks in
//! their local queue. The motivation to using a sharded-based approach is that the worker
//! threads are able to enqueue/dequeue an item to one of the shards individually;
//! that is to say, multiple worker threads can actually perform work at a time enqueuing
//! or dequeuing an item to unique shards.
//!
//! What if multiple threads are contending at a shard?
//!
//! This is indeed possible because we can't control how tasks are distributed to each worker
//! thread via Tokio's runtime (+ Tokio's work stealing inhibits the maximum potential of this buffer).
//! This means that worker threads can be given tasks coming from multiple different shards.
//! In an ideal world, we would have individual threads operate on all tasks to a unique shard,
//! which would remove the need for atomic CAS locks on the shard and they can context switch
//! tasks minimally when their shard is either full or empty. This would allow for maximal cache
//! locality per core/thread while also enabling it to be non-blocking and truly be lock-free.
//!
//! Currently, in the event that a worker thread is unable to enqueue or dequeue an item to a shard
//! (due to a shard being occupied by another thread or the shard is full/empty), then the worker
//! thread simply context switches the task (putting it to the back of its queue) and tries working
//! on a different task that likely operates on a different shard. Doing this instead of using a slot
//! based approach tends to be faster because 1) context switching task is cheap (since kernel is not involved)
//! and 2) the synchronization performed in slots end up being costly when it is possible for multiple threads
//! to contend on a specific shard based on the task they are currently working on

mod guards;
mod lf_shardedringbuf;
mod shard_policies;
mod task_local_spawn;
mod task_locals;
mod task_node;

pub use lf_shardedringbuf::LFShardedRingBuf;
pub use shard_policies::ShardPolicy;
pub use task_local_spawn::{
    rt_spawn_buffer_task, rt_spawn_with_cft, spawn_assigner, spawn_bounded_dequeue_full_with_cft,
    spawn_bounded_dequeue_with_cft, spawn_bounded_dequeuer, spawn_bounded_dequeuer_full,
    spawn_bounded_enqueue_with_cft, spawn_bounded_enqueuer, spawn_buffer_task,
    spawn_dequeue_full_with_cft, spawn_dequeue_with_cft, spawn_dequeuer, spawn_dequeuer_full,
    spawn_enqueue_with_cft, spawn_enqueuer, spawn_unbounded_dequeue_full_with_cft,
    spawn_unbounded_dequeue_with_cft, spawn_unbounded_dequeuer, spawn_unbounded_dequeuer_full,
    spawn_unbounded_enqueue_with_cft, spawn_unbounded_enqueuer, spawn_with_cft, terminate_assigner,
};
pub use task_node::TaskRole;
