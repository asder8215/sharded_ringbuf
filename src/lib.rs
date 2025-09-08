//! A Tokio async, sharded, bounded ring buffer that can be
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
//! on a different task that likely operates on a different shard.

mod exp_shardedringbuf;
mod guards;
mod mlf_shardedringbuf;
mod shard_policies;
mod shardedringbuf;
mod task_local_spawn;
mod task_locals;
mod task_node;

pub use exp_shardedringbuf::ExpShardedRingBuf;
pub use mlf_shardedringbuf::MLFShardedRingBuf;
pub use shard_policies::ShardPolicy;
pub use shardedringbuf::ShardedRingBuf;
pub use task_local_spawn::{
    cft_spawn_dequeuer, cft_spawn_dequeuer_bounded, cft_spawn_dequeuer_full,
    cft_spawn_dequeuer_full_bounded, cft_spawn_dequeuer_full_unbounded,
    cft_spawn_dequeuer_unbounded, cft_spawn_enqueuer, cft_spawn_enqueuer_with_iterator,
    cft_spawn_enqueuer_with_stream, mlf_spawn_dequeuer_unbounded, mlf_spawn_enqueuer_with_iterator,
    spawn_assigner, spawn_dequeuer, spawn_dequeuer_bounded, spawn_dequeuer_full,
    spawn_dequeuer_full_bounded, spawn_dequeuer_full_unbounded, spawn_dequeuer_unbounded,
    spawn_enqueuer, spawn_enqueuer_full_with_iterator, spawn_enqueuer_with_iterator,
    spawn_enqueuer_with_stream, terminate_assigner,
};
