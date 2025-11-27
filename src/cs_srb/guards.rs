use crate::cs_srb::{Acquire, InnerRingBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// This is a guard for all the shards in CSShardedRingBuf struct
/// to make it cancel safe.
pub struct ShardLockGuard<'a, T> {
    pub(crate) role: Acquire,
    lock: &'a AtomicBool,
    notify: &'a Notify,
    pub(crate) shard: &'a InnerRingBuffer<T>,
}

impl<'a, T> ShardLockGuard<'a, T> {
    #[inline(always)]
    pub(crate) fn acquire_shard_guard(
        role: Acquire,
        lock: &'a AtomicBool,
        notify: &'a Notify,
        shard: &'a InnerRingBuffer<T>,
    ) -> Self {
        ShardLockGuard {
            role,
            lock,
            notify,
            shard,
        }
    }
}

/// The guard can go out of scope and it'll automatically
/// unlock the shard + notify the opposing task type
impl<T> Drop for ShardLockGuard<'_, T> {
    #[inline(always)]
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
        self.notify.notify_one();
    }
}
