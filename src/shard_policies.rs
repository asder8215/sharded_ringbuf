use std::sync::Arc;

use crate::{LFShardedRingBuf, TaskRole};

#[derive(Debug, Clone)]
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
/// tasks from being stuck in a shift by sweep, every full attempt of failing to acquire
/// a shard results in task's shard_id being incremented by one before applying shift
/// by.
///
/// CFT: CFT stands for Completely Fair Tasks (homage to Completely Fair Scheduler ;)).
/// This shard policy requires spawning the assigner task because the assigner task
/// decides the location of what shard an enqueuer and a dequeuer task works at. It tries
/// to pair the two tasks together on a shard (though it lets the enqueuer task go with a shard
/// first), so contention/starvation on a shard is at its most minimum. This is quite literally
/// the best policy for an arbitrary number of tasks, with a lot of shards being used (ideally
/// matching enqueuer task count).
pub enum ShardPolicy<T> {
    Sweep {
        initial_index: Option<usize>,
    },
    RandomAndSweep,
    ShiftBy {
        initial_index: Option<usize>,
        shift: usize,
    },
    CFT {
        role: TaskRole,
        buffer: Arc<LFShardedRingBuf<T>>,
    },
}

/// This is a private wrapper enum to abstract away from typed ShardPolicy
/// Since TaskPair depends on the type of the buffer
#[derive(Debug, Clone, Copy)]
pub(crate) enum ShardPolicyKind {
    Sweep,
    RandomAndSweep,
    ShiftBy,
    CFT,
}
