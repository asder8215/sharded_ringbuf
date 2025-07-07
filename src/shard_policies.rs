#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub enum ShardPolicy {
    Sweep {
        initial_index: Option<usize>,
    },
    RandomAndSweep,
    ShiftBy {
        initial_index: Option<usize>,
        shift: usize,
    },
}
