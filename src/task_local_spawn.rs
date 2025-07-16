use crate::{
    LFShardedRingBuf, ShardPolicy, TaskRole,
    shard_policies::ShardPolicyKind,
    task_locals::{
        SHARD_INDEX, SHARD_POLICY, SHIFT, TASK_NODE, get_shard_ind, get_task_node, set_shard_ind,
    },
    task_node::{TaskNode, TaskNodePtr},
};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::{Cell, UnsafeCell},
    collections::{HashMap, HashSet},
    ptr,
    sync::{Arc, atomic::Ordering},
    thread::sleep,
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    task::{JoinHandle, spawn, yield_now},
};

/// Spawns a Tokio task using the current Tokio runtime context for the purpose
/// of using it with LFShardedRingBuf
///
/// This function or [rt_spawn_buffer_task] *must* be used in order to
/// enqueue or dequeue items onto LFShardedRingBuf
pub fn spawn_buffer_task<F, T, R>(policy: ShardPolicy<R>, fut: F) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
    R: Send + 'static,
{
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
        // will need to refactor this in the future so users don't have the responsibility
        // of giving me the right buffer and role
        ShardPolicy::CFT { role, buffer } => {
            spawn(TASK_NODE.scope(
                // Box::new(UnsafeCell::new(TaskNode::new(role))),
                Arc::new(TaskNode::new(role)),
                SHARD_POLICY.scope(
                    Cell::new(ShardPolicyKind::CFT),
                    SHARD_INDEX.scope(
                        Cell::new(None),
                        SHIFT.scope(Cell::new(0), async move {
                            let buffer = Arc::clone(&buffer);

                            // Getting the task node pointer
                            // This is wrapped around a TaskNodePtr because *mut TaskNode does not
                            // implement Send and Sync, which the compiler complains across the
                            // yield_now.await loop
                            let task_node = TaskNodePtr(get_task_node().load(Ordering::Relaxed));
                            unsafe { (&*task_node.0).myself.store(task_node.0, Ordering::Relaxed) };
                            let null = ptr::null_mut();

                            // we're inserting at head!
                            if buffer
                                .head
                                .compare_exchange(
                                    null,
                                    task_node.0,
                                    Ordering::AcqRel,
                                    Ordering::Relaxed,
                                )
                                .is_ok()
                            {
                                // since we guaranteed getting the head, we can put the have our
                                // be the tail as well in a Relaxed Memory Ordering fashion
                                buffer.tail.store(task_node.0, Ordering::Release);
                            }
                            // we're insert at tail!
                            else {
                                loop {
                                    // Relaxed ordering here because we'll eventually get the correct tail
                                    // (Protected by the compare exchange weak AcqRel ordering)
                                    let tail = TaskNodePtr(buffer.tail.load(Ordering::Acquire));

                                    // safety check for the first insertion node to head to have
                                    // time to set itself to tail as well
                                    if tail.0.is_null() {
                                        yield_now().await;
                                        continue;
                                    }

                                    unsafe {
                                        // setting the prev of this node to this current task (no need to
                                        // do anything with next node because it's already null)
                                        (*task_node.0).prev.store(tail.0, Ordering::Relaxed);
                                    }

                                    // Now we're swapping the tail with my task!
                                    // This is done in compare and exchange weak because it's safe to do this in a
                                    // loop (and possibly efficient!)
                                    if buffer
                                        .tail
                                        .compare_exchange_weak(
                                            tail.0,
                                            task_node.0,
                                            Ordering::AcqRel,
                                            Ordering::Relaxed,
                                        )
                                        .is_ok()
                                    {
                                        // this is our old tail from before; we must make sure that this tail's
                                        // next is correctly connected to our current task.
                                        // this needs Release ordering because we want to ensure that other threads-tasks
                                        // see this
                                        unsafe {
                                            (*tail.0).next.store(task_node.0, Ordering::Release);
                                        }
                                        break;
                                    }

                                    // if we couldn't register the task, let's yield this task for now and try again later
                                    yield_now().await;
                                }
                            }
                            // do whatever the user wants us to do with the future
                            fut.await
                        }),
                    ),
                ),
            ))
        }
    }
}

/// Spawns a Tokio task with a provided Tokio Runtime for the purpose
/// of using it with LFShardedRingBuf
///
/// This function or [spawn_buffer_task] *must* be used in order to enqueue or
/// dequeue items onto LFShardedRingBuf
pub fn rt_spawn_buffer_task<F, T>(
    runtime: &Runtime,
    policy: ShardPolicy<T>,
    fut: F,
) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match policy {
        ShardPolicy::Sweep { initial_index } => runtime.spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => runtime.spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => runtime.spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), fut),
            ),
        )),
        ShardPolicy::CFT { role, buffer } => {
            runtime.spawn(TASK_NODE.scope(
                Arc::new(TaskNode::new(role)),
                SHARD_POLICY.scope(
                    Cell::new(ShardPolicyKind::CFT),
                    SHARD_INDEX.scope(
                        Cell::new(None),
                        SHIFT.scope(Cell::new(0), async move {
                            // let buffer = Arc::clone(&buffer);

                            // // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                            // let node = Box::new(TaskNode::new(role));
                            // let task_node_ptr = CachePadded::new(TaskNodePtr(Box::into_raw(node)));
                            // set_task_node(task_node_ptr);

                            // // This performs task registration in a task yielding loop
                            // loop {
                            //     let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
                            //     // The cool thing about this below is that the next ptr for
                            //     // the task node ptr depends on that compare exchange for memory
                            //     // visibility, so this relaxed ordering is synchronized properly
                            //     // by the compare_exchange's AcqRel ordering.
                            //     unsafe {
                            //         (*task_node_ptr.0).next.store(current.0, Ordering::Relaxed);
                            //     }

                            //     if buffer
                            //         .head
                            //         .compare_exchange(
                            //             current.0,
                            //             task_node_ptr.0,
                            //             Ordering::AcqRel,
                            //             Ordering::Relaxed,
                            //         )
                            //         .is_ok()
                            //     {
                            //         break;
                            //     } else {
                            //         yield_now().await;
                            //     }
                            // }

                            // let guard = TaskNodeGuard::register_task(&buffer, &task_node_ptr).await;

                            // do whatever the user wants us to do with the future
                            let val = fut.await;
                            // fut.await

                            // drop(guard);

                            // println!("Marking task done");

                            // mark that the task is done once the future is over
                            // set_task_done();

                            // println!("Marking task done");
                            // return the future value
                            val
                        }),
                    ),
                ),
            ))
        }
    }
}

/// This is your helpful assigner task that handles assigning enqueuer tasks to dequeuer
/// and vice versa in an optimal way.
///
/// To use CTF Shard Policy, you must spawn the assigner task first.
///
/// To terminate the assigner, use [terminate_assigner()] and then you can await
/// on the assigner. The assigner also handles memory clean up of allocated enqueuer and
/// dequeuer task node pointers to the buffer.
///
/// It is the responsibility of the user to provide the correct buffer that tasks are
/// spawning into and that the assigner task should work with.

pub fn spawn_assigner<T: 'static>(buffer: Arc<LFShardedRingBuf<T>>) -> JoinHandle<()> {
    spawn(SHARD_INDEX.scope(Cell::new(Some(0)), async move {
        let buffer_clone = Arc::clone(&buffer);
        loop {
            // fetch head to start traversal!
            // let mut current = buffer.get_head_relaxed();
            let mut current = buffer.get_head();

            // if user sends a termination signal to the assigner, then it's
            // time to stop working
            if buffer.assigner_terminate.load(Ordering::Relaxed) {
                return;
            }

            // let rebound_curr = None;
            // let rebound_scan = None;
            // println!("{:?}", buffer.get_job_count_total());
            // as long as the current node I'm looking at is not null, continue traversing!
            // println!("I'm starting right now!");
            while !current.0.is_null() {
                let task_node = unsafe { &*current.0 };
                let myself = task_node.myself.load(Ordering::Acquire);
                let my_role = task_node.role;
                // let my_pair = task_node.my_pair.load(Ordering::Acquire);
                let my_pair = task_node.my_pair.load(Ordering::Relaxed);
                let my_shard = task_node.shard_ind.load(Ordering::Relaxed);

                // this signals us that the enqueuer/dequeuer task dropped itself
                // skip forward, do NOT store or do anything with the items inside here
                // it's invalid memory
                println!("The next node I saw was {:?}", task_node);
                if myself.is_null() {
                    let prev = task_node.prev.load(Ordering::Acquire);
                    let next = task_node.next.load(Ordering::Acquire);

                    if buffer.head.load(Ordering::Acquire) == current.0 {
                        let _ = buffer.head.compare_exchange(current.0, next, Ordering::AcqRel, Ordering::Relaxed);
                    }

                    // If we're the tail, try moving tail to prev
                    if buffer.tail.load(Ordering::Acquire) == current.0 {
                        let _ = buffer.tail.compare_exchange(current.0, prev, Ordering::AcqRel, Ordering::Relaxed);
                    }

                    // Unlink us from prev -> next
                    if !prev.is_null() {
                        let _ = unsafe { &*prev }.next.compare_exchange(current.0, next, Ordering::AcqRel, Ordering::Relaxed);
                    }

                    // Unlink us from next -> prev
                    if !next.is_null() {
                        let _ = unsafe { &*next }.prev.compare_exchange(current.0, prev, Ordering::AcqRel, Ordering::Relaxed);
                    }

                    // task_node.my_pair

                        // Fix the prev's next pointer to skip this node
                    // if !prev.is_null() {
                    //     let prev_node = unsafe { &*prev };
                    //     loop {
                    //         let prev_next = prev_node.next.load(Ordering::Acquire);
                    //         if prev_next != current.0 {
                    //             break; // someone else updated it
                    //         }

                    //         if prev_node.next
                    //             .compare_exchange_weak(prev_next, next, Ordering::AcqRel, Ordering::Relaxed)
                    //             .is_ok()
                    //         {
                    //             break;
                    //         }
                    //     }
                    // } else {
                    //     // We're the head; update buffer.head to our next
                    //     loop {
                    //         let head = buffer.head.load(Ordering::Acquire);
                    //         if head != current.0 {
                    //             break;
                    //         }

                    //         if buffer
                    //             .head
                    //             .compare_exchange_weak(head, next, Ordering::AcqRel, Ordering::Relaxed)
                    //             .is_ok()
                    //         {
                    //             break;
                    //         }
                    //     }
                    // }

                    // // Fix the next's prev pointer to skip this node
                    // if !next.is_null() {
                    //     let next_node = unsafe { &*next };
                    //     loop {
                    //         let next_prev = next_node.prev.load(Ordering::Acquire);
                    //         if next_prev != current.0 {
                    //             break;
                    //         }

                    //         if next_node.prev
                    //             .compare_exchange_weak(next_prev, prev, Ordering::AcqRel, Ordering::Relaxed)
                    //             .is_ok()
                    //         {
                    //             break;
                    //         }
                    //     }
                    // } else {
                    //     // We're the tail; update buffer.tail to our prev
                    //     loop {
                    //         let tail = buffer.tail.load(Ordering::Acquire);
                    //         if tail != current.0 {
                    //             break;
                    //         }

                    //         if buffer
                    //             .tail
                    //             .compare_exchange_weak(tail, prev, Ordering::AcqRel, Ordering::Relaxed)
                    //             .is_ok()
                    //         {
                    //             break;
                    //         }
                    //     }
                    // }



                    // current = TaskNodePtr(task_node.next.load(Ordering::Acquire));
                    current = TaskNodePtr(next);
                    // println!("The next node I saw was {:?}", unsafe {&*task_node.next.load(Ordering::Acquire)});
                    // break;
                    continue;
                }

                // if I am paired already, then I should skip to the next node
                if !my_pair.is_null() {
                    // current = TaskNodePtr(task_node.next.load(Ordering::Relaxed));
                    current = TaskNodePtr(task_node.next.load(Ordering::Acquire));
                    continue;
                }

                // If the dequeuers didn't process the shards inside yet, do not pair them up with someone else yet
                // it's guaranteed that enqueuers will finish first though
                // if my_role == TaskRole::Dequeue && my_shard <= buffer.get_num_of_shards() && !buffer.is_shard_empty(my_shard) {
                if my_role == TaskRole::Dequeue
                    && my_shard != usize::MAX
                    && !buffer.is_shard_empty(my_shard)
                {
                    // current = TaskNodePtr(task_node.next.load(Ordering::Relaxed));
                    current = TaskNodePtr(task_node.next.load(Ordering::Acquire));

                    continue;
                }
                // otherwise I scan forward to find the next node to pair this up with.
                else {
                    // println!("I'm scanning!");
                    // let mut scan = TaskNodePtr(task_node.next.load(Ordering::Relaxed));
                    let mut scan = TaskNodePtr(task_node.next.load(Ordering::Acquire));

                    // if 
                    

                    if !scan.0.is_null() {
                        println!("The scan node I started with is {:?}", unsafe {&*scan.0});
                    }
                    while !scan.0.is_null() {
                        let scan_node = unsafe { &*scan.0 };
                        let scan_myself = scan_node.myself.load(Ordering::Acquire);
                        let scan_role = scan_node.role;
                        let scan_pair = scan_node.my_pair.load(Ordering::Acquire);
                        // let scan_shard = scan_node.shard_ind.load(Ordering::Relaxed);
                        let scan_shard = scan_node.shard_ind.load(Ordering::Acquire);

                        if scan_myself.is_null() {
                            // let prev_self = scan_node.prev
                            let prev = scan_node.prev.load(Ordering::Acquire);
                            let next = scan_node.next.load(Ordering::Acquire);
                            
                            // let prev_ptr = unsafe { &*prev }.myself.load(Ordering::Acquire);
                            // let next_ptr = unsafe { &*next }.myself.load(Ordering::Acquire);
                            if buffer.head.load(Ordering::Acquire) == scan.0 {
                                let _ = buffer.head.compare_exchange(scan.0, next, Ordering::AcqRel, Ordering::Relaxed);
                            }

                            // If we're the tail, try moving tail to prev
                            if buffer.tail.load(Ordering::Acquire) == scan.0 {
                                let _ = buffer.tail.compare_exchange(scan.0, prev, Ordering::AcqRel, Ordering::Relaxed);
                            }

                            // Unlink us from prev -> next
                            if !prev.is_null() {
                                let _ = unsafe { &*prev }.next.compare_exchange(scan.0, next, Ordering::AcqRel, Ordering::Relaxed);
                            }

                            // Unlink us from next -> prev
                            if !next.is_null() {
                                let _ = unsafe { &*next }.prev.compare_exchange(scan.0, prev, Ordering::AcqRel, Ordering::Relaxed);
                            }


                            // if prev_ptr.is_null()
                            // if !prev.is_null() {
                            //     let prev_node = unsafe { &*prev };
                            //     loop {
                            //         let prev_next = prev_node.next.load(Ordering::Acquire);
                            //         if prev_next != scan.0 {
                            //             break; // someone else updated it
                            //         }

                            //         if prev_node.next
                            //             .compare_exchange_weak(prev_next, next, Ordering::AcqRel, Ordering::Relaxed)
                            //             .is_ok()
                            //         {
                            //             break;
                            //         }
                            //     }
                            // } else {
                            //     // We're the head; update buffer.head to our next
                            //     loop {
                            //         let head = buffer.head.load(Ordering::Acquire);
                            //         if head != scan.0 {
                            //             break;
                            //         }

                            //         if buffer
                            //             .head
                            //             .compare_exchange_weak(head, next, Ordering::AcqRel, Ordering::Relaxed)
                            //             .is_ok()
                            //         {
                            //             break;
                            //         }
                            //     }
                            // }

                            // // Fix the next's prev pointer to skip this node
                            // if !next.is_null() {
                            //     let next_node = unsafe { &*next };
                            //     loop {
                            //         let next_prev = next_node.prev.load(Ordering::Acquire);
                            //         if next_prev != scan.0 {
                            //             break;
                            //         }

                            //         if next_node.prev
                            //             .compare_exchange_weak(next_prev, prev, Ordering::AcqRel, Ordering::Relaxed)
                            //             .is_ok()
                            //         {
                            //             break;
                            //         }
                            //     }
                            // } else {
                            //     // We're the tail; update buffer.tail to our prev
                            //     loop {
                            //         let tail = buffer.tail.load(Ordering::Acquire);
                            //         if tail != scan.0 {
                            //             break;
                            //         }

                            //         if buffer
                            //             .tail
                            //             .compare_exchange_weak(tail, prev, Ordering::AcqRel, Ordering::Relaxed)
                            //             .is_ok()
                            //         {
                            //             break;
                            //         }
                            //     }
                            // }


                            // Move forward
                            // current = Tasknext;
                            // continue;

                            // current = TaskNodePtr(task_node.next.load(Ordering::Acquire));
                            // println!("The next node I saw was {:?}", unsafe {&*task_node.next.load(Ordering::Acquire)});
                            // break;
                            // continue;
                            // scan = TaskNodePtr(scan_node.next.load(Ordering::Acquire));
                            scan = TaskNodePtr(next);
                            // scan = TaskNodePtr(ptr::null_mut());
                            // break;
                            continue;
                        }

                        // just like curr, just continue forward
                        if !scan_pair.is_null() {
                            // scan = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));
                            scan = TaskNodePtr(scan_node.next.load(Ordering::Acquire));
                            continue;
                        }

                        // println!("The issue came here");
                        // If the dequeuers didn't process the shards inside yet, do not pair them up with someone else yet
                        // it's guaranteed that enqueuers will finish first though
                        // if scan_role == TaskRole::Dequeue && scan_shard <= buffer.get_num_of_shards() && !buffer.is_shard_empty(scan_shard) {
                        if scan_role == TaskRole::Dequeue
                            && scan_shard != usize::MAX
                            && !buffer.is_shard_empty(scan_shard)
                        {
                            // scan = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));
                            scan = TaskNodePtr(scan_node.next.load(Ordering::Acquire));
                            continue;
                        }

                        // I found a scan node that is opposite to my role, let's see
                        // if we can pair each other up!
                        if my_role != scan_role {
                            // println!("I'm pairing with {:?}", scan_node);
                            // but if we did find a pair... pair em!
                            // also this is safe to unwrap because we guaranteed
                            // initializing this to 0
                            let shard_ind = get_shard_ind().unwrap();
                            // println!("Shard index {}", shard_ind);

                            // ordering of assigning shard_ind doesn't matter here
                            scan_node.shard_ind.store(shard_ind, Ordering::Release);
                            task_node.shard_ind.store(shard_ind, Ordering::Release);

                            // update the assigner shard ind value to assign the next enq/deq pair
                            // to the next shard (minimizes contention of enq/deq task pair to shard)
                            set_shard_ind((shard_ind + 1) % buffer.get_num_of_shards());

                            // we wanna signal the enq first that it's paired and then
                            // signal to deq that it's paired
                            match my_role {
                                TaskRole::Dequeue => {
                                    // this is enq
                                    scan_node.my_pair.store(current.0, Ordering::Release);
                                    // this is deq
                                    task_node.my_pair.store(scan.0, Ordering::Release);
                                }
                                TaskRole::Enqueue => {
                                    // this is enq
                                    task_node.my_pair.store(scan.0, Ordering::Release);
                                    // this is deq
                                    scan_node.my_pair.store(current.0, Ordering::Release);
                                }
                            };
                            // current = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));

                            current = TaskNodePtr(scan_node.next.load(Ordering::Acquire));
                            break;
                        }
                        // scan the net node
                        // scan = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));
                        scan = TaskNodePtr(scan_node.next.load(Ordering::Acquire));
                    }

                    // if scan got to this point, it couldn't find
                    // any pairs to make :(
                    if scan.0.is_null() {
                        break;
                    }
                }
            }
            // yield the assigner task and try running through this list another
            // time
            yield_now().await;
        }
    }))
}

#[inline(always)]
pub fn terminate_assigner<T>(buffer: Arc<LFShardedRingBuf<T>>)
where
    T: Send + 'static,
{
    buffer.assigner_terminate.store(true, Ordering::Relaxed);
}

// These methods below are for CTF Policy for less flexibility and responsibility
// on the user to spawn an enqueuer and dequeuer task
// pub fn spawn_enqueuer<S, T>(rb: Arc<LFShardedRingBuf<T>>, bounded: bool, items: usize, Stream: S)
// where
//     S: Stream<Item=T> + Send + 'static,
//     T: Send + 'static
// {
//     todo!();
//     // spawn()
// }

// pub fn spawn_dequeuer<F, T>(rb: Arc<LFShardedRingBuf<T>>, bounded: bool, items: usize, func: impl Fn()) {
//     todo!();
// }

// And then down below create two different rt version of it
