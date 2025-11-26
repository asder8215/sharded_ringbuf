use crate::cs_srb::{Acquire, InnerRingBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// This is a guard for all the shards in LFShardedRingBuf struct
/// Implemented to make certain functions cancel-safe
///
/// NOTE: These guards can't make it cancel safe :/
/// So all of these is just fluff
pub struct ShardLockGuard<'a, T> {
    pub(crate) role: Acquire,
    lock: &'a AtomicBool, // lifetime of the lock is necessary to drop the lock on task abortion
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

/// The beauty of this is now I can I just let my locks go out of
/// scope and it'll automatically drop it
impl<T> Drop for ShardLockGuard<'_, T> {
    #[inline(always)]
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
        self.notify.notify_one();
    }
}
