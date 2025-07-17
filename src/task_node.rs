use std::{
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    usize,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TaskRole {
    Enqueue,
    Dequeue,
}

/// The assigner task uses this to traverse in a singular lock-free LinkedList manner to
/// pair up enqueuer and dequeuer task together by shard index. It is responsible
/// for cleaning up the LinkedList when the enqueuer/dequeuer says they are done.
///
/// The enqueuer and dequeuer task uses this to find out what shard index to perform its
/// operations at. It is responsible for adding itself to the head of the LinkedList in
/// LFShardedRingBuf and marking that it's done.
///
/// If the LFShardedRingBuf is dropped, the LFShardedRingBuf is responsible for cleaning up any
/// leftover allocated TaskNodes that possibly the assigner task has not had the chance to clean up
/// in the event that the assigner task is terminated early. As a result, there should be no memory
/// leaks when the program finishes.
///
/// Made all the field pub(crate) because too lazy to make getter and setter methods for these :p
/// Besides, it's private to the crate so no misuse possible by user (though I might organize this
/// later down the line)
#[derive(Debug)]
pub(crate) struct TaskNode {
    pub(crate) role: TaskRole,         // A static role of what the Task is
    pub(crate) shard_ind: AtomicUsize, // The shard index that a task will look at (assigner writes, enq/deq reads)

    pub(crate) rotate: AtomicBool,            
    pub(crate) my_pair: AtomicPtr<TaskNode>, // my enqueuer/dequeuer pair
    pub(crate) prev: AtomicPtr<TaskNode>,    // The prev TaskNode
    pub(crate) next: AtomicPtr<TaskNode>,    // The next TaskNode
}

impl TaskNode {
    pub(crate) fn new(role: TaskRole) -> Self {
        Self {
            role,
            // usize max acts as a sentinel shard index value
            shard_ind: AtomicUsize::new(usize::MAX),
            rotate: AtomicBool::new(false),
            my_pair: AtomicPtr::new(ptr::null_mut()),
            prev: AtomicPtr::new(ptr::null_mut()),
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

impl Drop for TaskNode {
    fn drop(&mut self) {
        // TaskNode can't implement Copy because it has Drop, but TaskNodePtr can keep
        // a reference to this memory location and copy that so we can keep loading
        // and seeing any future updates to prev and next

        // self.myself.store(ptr::null_mut(), Ordering::Release);

        let prev = TaskNodePtr(self.prev.load(Ordering::Acquire));
        let next = TaskNodePtr(self.next.load(Ordering::Acquire));

        // Make sure prev's next is set to our next!
        if !prev.0.is_null() {
            let mut prev_next = TaskNodePtr(unsafe { &*prev.0 }.next.load(Ordering::Acquire));
            loop {
                if prev_next.0 != self {
                    // Changed possibly or possibly unlinked;
                    break;
                }

                // if this fails it's possibly that our prev was dropped from the list
                // or the assigner might've moved us to the back of the list
                if unsafe { &*prev.0 }
                    .next
                    .compare_exchange_weak(prev_next.0, next.0, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }

                // update what we see for next
                prev_next = TaskNodePtr(unsafe { &*prev.0 }.next.load(Ordering::Acquire));
            }
        }

        // Make sure next's prev is set to our prev's!
        if !next.0.is_null() {
            let mut next_prev = TaskNodePtr(unsafe { &*next.0 }.prev.load(Ordering::Acquire));

            loop {
                if next_prev.0 != self {
                    // Change possibly or possibly unlinked;
                    break;
                }

                // if this fails it's possibly that our next was dropped from the list
                // or the assigner might've moved us to the back of the list
                if unsafe { &*next.0 }
                    .prev
                    .compare_exchange_weak(next_prev.0, prev.0, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }

                // update what we see for next
                next_prev = TaskNodePtr(unsafe { &*next.0 }.prev.load(Ordering::Acquire));
            }
        }
    }
}

impl PartialEq for TaskNode {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TaskNodePtr(pub *mut TaskNode);

unsafe impl Send for TaskNodePtr {}
unsafe impl Sync for TaskNodePtr {}

unsafe impl Send for TaskNode {}
unsafe impl Sync for TaskNode {}

impl PartialEq for TaskNodePtr {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for TaskNodePtr {}

impl std::hash::Hash for TaskNodePtr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.0 as usize).hash(state);
    }
}
