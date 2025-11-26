//! An async, sharded, bounded ring buffer that can be
//! used in a SPSC, MPSC, and MPMC environment (optimal for MPMC however).
//!
//! The key feature of this ring buffer is that it takes a sharded approach to enqueuing
//! and dequeuing items and context switches the task a thread is working on if it could
//! not acquire the shard. This enables the buffer to perform work in a non-blocking manner
//! and dynamically receive enqueue/dequeue tasks under an async runtime.
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
//! This is possible because we can't control how tasks are distributed to each worker
//! thread via Tokio's runtime (+ Tokio's work stealing inhibits the maximum potential of this buffer).
//! This means that worker threads can be given tasks coming from multiple different shards.
//! In an ideal world, we would have individual threads operate on all tasks to a unique shard,
//! which would remove the need for atomic CAS locks on the shard and they can context switch
//! tasks minimally when their shard is either full or empty. This would allow for maximal cache
//! locality per core/thread while also enabling it to be non-blocking and be lock-free.
//!
//! Currently, in the event that a worker thread is unable to enqueue or dequeue an item to a shard
//! (due to a shard being occupied by another thread or the shard is full/empty), then the worker
//! thread simply context switches the task (putting it to the back of its queue) and tries working
//! on a different task that likely operates on a different shard.

#[cfg(feature = "exp_srb")]
pub mod exp_srb;
/// Contains the MLFShardedRingBuf (Mostly Lock-Free Sharded Ring Buffer) struct
///
/// Useful for single item enqueue/dequeue tasks
///
/// Can only be used in an **async** environment/runtime
#[cfg(feature = "mlf_srb")]
pub mod mlf_srb;
/// Contains the ShardedRingBuf struct
///
/// Supports both single item and batching enqueue/dequeue tasks.
/// Optimal for batching items.
///
/// Can only be used in an **async** environment/runtime.
#[cfg(feature = "srb")]
pub mod srb;

// A 100% cancel safe version of ShardedRingBuf using guards
#[cfg(feature = "cs_srb")]
pub mod cs_srb;
