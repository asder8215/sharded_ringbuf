use std::{marker::PhantomData, mem::ManuallyDrop, pin::Pin, sync::atomic::{AtomicBool, Ordering}, task::{Context, Poll}, thread::sleep, time::Duration};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use tokio::task::{id, yield_now};
// use std::future::

use crate::{task_locals::set_task_done, task_node::TaskNodePtr, LFShardedRingBuf};

/// This is a guard for all the shards in LFShardedRingBuf struct
/// Implemented to make certain functions cancel-safe
pub(crate) struct ShardLockGuard<'a> {
    lock: &'a AtomicBool, // lifetime of the lock is necessary to drop the lock on task abortion
}

impl<'a> ShardLockGuard<'a> {
    #[inline(always)]
    fn try_acquire_lock(lock: &'a AtomicBool) -> bool {
        lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    pub(crate) async fn acquire(lock: &'a AtomicBool) -> Self {
        while !Self::try_acquire_lock(lock) {
            yield_now().await;
        }
        ShardLockGuard { lock }
    }
}

/// The beauty of this is now I can I just let my locks go out of
/// scope and it'll automatically drop it
impl Drop for ShardLockGuard<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}

// pub(crate) struct TaskNodeGuard<T> {
//     task_node_ptr: TaskNodePtr, // lifetime of this task node is necessary in case enqueuer/dequeuer task get cancelled
//     phantom: PhantomData<T>     // Because I need to register the task to the buffer, so I need the T type
// }

// I realized Drop is synchronous; I can't perform task clean up in a RAII style
// way
// impl<'a, T> TaskNodeGuard<T> {
//     #[inline(always)]
//     fn try_insert(buffer: &LFShardedRingBuf<T>, current: &TaskNodePtr, task_node_ptr: &TaskNodePtr) -> bool {
//         buffer
//                 .head
//                 .compare_exchange_weak(
//                     current.0,
//                     task_node_ptr.0,
//                     Ordering::AcqRel,
//                     Ordering::Relaxed,
//                 )
//                 .is_ok()
//     }

//     #[inline]
//     pub(crate) fn register_task(buffer: &LFShardedRingBuf<T>, task_node_ptr: &CachePadded<TaskNodePtr>) -> Self {
//         // This performs task registration in a task yielding loop
//         let mut attempt:i32 = 0;
//         loop {
//             let current = buffer.get_head(); // get head with Relaxed Memory ordering
//             // The cool thing about this below is that the next ptr for
//             // the task node ptr depends on that compare exchange for memory
//             // visibility, so this relaxed ordering is synchronized properly
//             // by the compare_exchange's AcqRel ordering.
//             // println!("Current is {:?}", current.0);
//             // println!("I am being called by {:?}", id());
//             unsafe {
//                 // (*task_node_ptr.0).next.store(current.0, Ordering::Relaxed);
//                 (*task_node_ptr.0).next.store(current.0, Ordering::Release);
//             }

//             if Self::try_insert(buffer, &current, task_node_ptr)
//             {
//                 break;
//             } else {
//                 // println!("I'm yielding...");
//                 // yield_now().await;
//                 let backoff_us = (1u64 << attempt.min(5)).min(20) as usize;
//                 let jitter = frand(0..=backoff_us) as u64;
//                 sleep(Duration::from_micros(jitter));
//                 attempt = attempt.saturating_add(1); // Avoid overflow
//             }
//         }
//         Self {task_node_ptr: **task_node_ptr, phantom: PhantomData::default()}
//     }
// }

// impl<T> Drop for TaskNodeGuard<T> {
//     #[inline(always)]
//     // When this guard is dropped, just mark the task node is done and let the
//     // assigner clean this task up
//     fn drop(&mut self) {
//         // println!("I dropped this {:?} and I'm a {:?}", id(), unsafe { (*self.task_node_ptr.0).role });
//         unsafe { (*self.task_node_ptr.0).is_done.store(true, Ordering::Release) };
//         // println!("I dropped this {:?} and I'm a {:?}", id(), unsafe { (*self.task_node_ptr.0).role });
//         // set_task_done();
//     }
// }

// pub(crate) struct TaskFutureGuard<'a, Fut> {
//     pub inner: Fut,
//     pub task_node_ptr: &'a AtomicBool,
//     // pub _phantom: PhantomData<T>
// }

// impl<Fut> Future for TaskFutureGuard<'_,Fut> 
// where
//     Fut: Future + Unpin + std::marker::Unpin,
// {
//     type Output = Fut::Output;

//     fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         // Safety: We are pinned, so this is okay

//         let this = self.get_mut();

//         let pinned = unsafe { Pin::new_unchecked(&mut this.inner) };
//         // pinned.poll(cx)
//         pinned.poll(cx)
//     }
// }

// impl<Fut> Drop for TaskFutureGuard<'_, Fut> {
//     fn drop(&mut self) {
//         // This runs if the task is cancelled (i.e., the future is dropped)
//         unsafe {
//             (*self.task_node_ptr.0).is_done.store(true, Ordering::Relaxed);
//         }
//     }
// }
// This is a guard for CFT policy using assigner task to make it cancel safe
// It's whole purpose is to set the done flag of the TaskNodePtr if
// the task got cancelled or finished normally
// It effectively does the same thing as ShardLockGuard, but I
// // just wanted to have a specific name for it
pub(crate) struct TaskDoneGuard<'a, > {
    lock: &'a AtomicBool, // lifetime of the lock is necessary to drop the lock on task abortion
}

impl<'a> TaskDoneGuard<'a> {
    #[inline(always)]
    fn try_acquire_done(lock: &'a AtomicBool) -> bool {
        lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    pub(crate) async fn acquire(lock: &'a AtomicBool) -> Self {
        while !Self::try_acquire_done(lock) {
            yield_now().await;
        }
        TaskDoneGuard { lock }
    }
}

/// The beauty of this is now I can I just let my locks go out of
/// scope and it'll automatically drop it
impl Drop for TaskDoneGuard<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}