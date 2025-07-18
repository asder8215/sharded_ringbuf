// use std::{
//     sync::atomic::{AtomicBool, Ordering},
// };
// use tokio::task::{yield_now};

// /// This is a guard for all the shards in LFShardedRingBuf struct
// /// Implemented to make certain functions cancel-safe
// /// 
// /// NOTE: These guards can't make it cancel safe :/
// /// So all of these is just fluff
// pub(crate) struct ShardLockGuard<'a> {
//     lock: &'a AtomicBool, // lifetime of the lock is necessary to drop the lock on task abortion
// }

// impl<'a> ShardLockGuard<'a> {
//     #[inline(always)]
//     fn try_acquire_lock(lock: &'a AtomicBool) -> bool {
//         lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
//             .is_ok()
//     }

//     #[inline]
//     pub(crate) async fn acquire(lock: &'a AtomicBool) -> Self {
//         while !Self::try_acquire_lock(lock) {
//             yield_now().await;
//         }
//         ShardLockGuard { lock }
//     }
// }

// /// The beauty of this is now I can I just let my locks go out of
// /// scope and it'll automatically drop it
// impl Drop for ShardLockGuard<'_> {
//     #[inline(always)]
//     fn drop(&mut self) {
//         self.lock.store(false, Ordering::Release);
//     }
// }
// // This is a guard for CFT policy using assigner task to make it cancel safe
// // It's whole purpose is to set the done flag of the TaskNodePtr if
// // the task got cancelled or finished normally
// // It effectively does the same thing as ShardLockGuard, but I
// // just wanted to have a specific name for it
// pub(crate) struct TaskDoneGuard<'a> {
//     lock: &'a AtomicBool, // lifetime of the lock is necessary to drop the lock on task abortion
// }

// impl<'a> TaskDoneGuard<'a> {
//     #[inline(always)]
//     fn try_acquire_done(lock: &'a AtomicBool) -> bool {
//         lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
//             .is_ok()
//     }

//     #[inline]
//     pub(crate) async fn acquire(lock: &'a AtomicBool) -> Self {
//         while !Self::try_acquire_done(lock) {
//             yield_now().await;
//         }
//         TaskDoneGuard { lock }
//     }
// }

// /// The beauty of this is now I can I just let my locks go out of
// /// scope and it'll automatically drop it
// impl Drop for TaskDoneGuard<'_> {
//     #[inline(always)]
//     fn drop(&mut self) {
//         self.lock.store(false, Ordering::Release);
//     }
// }
