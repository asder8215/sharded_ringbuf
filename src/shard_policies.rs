#[derive(Debug, Clone)]
/// These are the Shard Acquistion Policies supported by LFShardedRingBuf
/// All of these policies are stable, cancel-safe, and do not cause any
/// memory issues with use in the LFShardedRingBuf. To see about the
/// CFT policy, see `spawn_with_ctf()` function.
pub enum ShardPolicy {
    /// Sweep: The task starts off at provided index (or random *initial index* if None)
    /// and performs a full sweep around the buffer to acquire shards to
    /// enqueue/dequeue items (shift by 1)
    Sweep {
        initial_index: Option<usize>,
    },
    /// RandomAndSweep: The task starts off at a random index always and performs a full
    /// sweep around the buffer to acquire shards to enqueue/dequeue items
    RandomAndSweep,
    /// ShiftBy: The task starts off at provided index (or random *initial index* if None)
    /// and performs a sweep around the buffer using a provided shift value. To prevent
    /// tasks from being stuck in a shift by sweep, every full attempt of failing to acquire
    /// a shard results in task's shard_id being incremented by one before applying shift
    /// by.
    ShiftBy {
        initial_index: Option<usize>,
        shift: usize,
    },
    Pin {
        initial_index: usize,
    },
}

/// This is a private wrapper enum to abstract away from typed ShardPolicy
/// Because CFT requires use of buffer.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ShardPolicyKind {
    Sweep,
    RandomAndSweep,
    ShiftBy,
    Cft,
    Pin,
}
