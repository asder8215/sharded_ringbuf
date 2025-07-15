use crate::{
    shard_policies::ShardPolicyKind, task_locals::{
        get_shard_ind, set_shard_ind, set_task_done, set_task_node, OPERATION_COUNT, SHARD_INDEX, SHARD_POLICY, SHIFT, TASK_NODE
    }, task_node::{TaskNode, TaskNodePtr}, LFShardedRingBuf, ShardPolicy, TaskRole
};
use crossbeam_utils::CachePadded;
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    ptr,
    sync::{Arc, atomic::Ordering},
};
// use futures_util::Stream;
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
                Cell::new(CachePadded::new(TaskNodePtr(ptr::null_mut()))),
                SHARD_POLICY.scope(
                    Cell::new(ShardPolicyKind::CFT),
                    SHARD_INDEX.scope(
                        Cell::new(None),
                        SHIFT.scope(Cell::new(0), async move {
                            let buffer = Arc::clone(&buffer);

                            // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                            let node = Box::new(TaskNode::new(role));
                            let task_node_ptr = CachePadded::new(TaskNodePtr(Box::into_raw(node)));
                            set_task_node(task_node_ptr);

                            // This performs task registration in a task yielding loop
                            loop {
                                let current = buffer.get_head(); // get head with Relaxed Memory ordering
                                // The cool thing about this below is that the next ptr for
                                // the task node ptr depends on that compare exchange for memory
                                // visibility, so this relaxed ordering is synchronized properly
                                // by the compare_exchange's AcqRel ordering.
                                unsafe {
                                    (*task_node_ptr.0).next.store(current.0, Ordering::Relaxed);
                                }

                                if buffer
                                    .head
                                    .compare_exchange_weak(
                                        current.0,
                                        task_node_ptr.0,
                                        Ordering::AcqRel,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                                {
                                    break;
                                } else {
                                    yield_now().await;
                                }
                            }

                            // do whatever the user wants us to do with the future
                            let val = fut.await;

                            // mark that the task is done once the future is over
                            set_task_done();

                            // return the future value
                            val
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
            runtime.spawn(OPERATION_COUNT.scope(Cell::new(0), TASK_NODE.scope(
                Cell::new(CachePadded::new(TaskNodePtr(ptr::null_mut()))),
                SHARD_POLICY.scope(
                    Cell::new(ShardPolicyKind::CFT),
                    SHARD_INDEX.scope(
                        Cell::new(None),
                        SHIFT.scope(Cell::new(0), async move {
                            let buffer = Arc::clone(&buffer);

                            // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                            let node = Box::new(TaskNode::new(role));
                            let task_node_ptr = CachePadded::new(TaskNodePtr(Box::into_raw(node)));
                            set_task_node(task_node_ptr);

                            // This performs task registration in a task yielding loop
                            loop {
                                let current = buffer.get_head(); // get head with Relaxed Memory ordering
                                // The cool thing about this below is that the next ptr for
                                // the task node ptr depends on that compare exchange for memory
                                // visibility, so this relaxed ordering is synchronized properly
                                // by the compare_exchange's AcqRel ordering.
                                unsafe {
                                    (*task_node_ptr.0).next.store(current.0, Ordering::Relaxed);
                                }

                                if buffer
                                    .head
                                    .compare_exchange_weak(
                                        current.0,
                                        task_node_ptr.0,
                                        Ordering::AcqRel,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                                {
                                    break;
                                } else {
                                    yield_now().await;
                                }
                            }

                            // do whatever the user wants us to do with the future
                            let val = fut.await;

                            // println!("Marking task done");

                            // mark that the task is done once the future is over
                            set_task_done();

                            // println!("Marking task done");
                            // return the future value
                            val
                        }),
                    ),
                ),
            )))
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
        let mut shard_task_map: HashMap<usize, HashSet<TaskNodePtr>> = HashMap::new();
        let mut pairs_map: HashMap<TaskNodePtr, TaskNodePtr> = HashMap::new();
        loop {
            let mut prev = TaskNodePtr(ptr::null_mut());
            let mut current = buffer_clone.get_head();

            // assigner can get out of this function once it complete cleaning up
            // the list
            if buffer.assigner_terminate.load(Ordering::Relaxed) && current.0.is_null() {
                break;
            }

            while !current.0.is_null() {
                unsafe {
                    let task_node = &*current.0;

                    let is_done = task_node.is_done.load(Ordering::Relaxed);
                    let is_paired = task_node.is_paired.load(Ordering::Relaxed);
                    let is_assigned = task_node.is_assigned.load(Ordering::Relaxed);
                    let role = task_node.role;

                    let shard_ind = &task_node.shard_ind.load(Ordering::Relaxed);
                    let is_shard_empty = buffer.is_shard_empty(task_node.shard_ind.load(Ordering::Relaxed));

                    let pairs = shard_task_map
                            .get_mut(shard_ind);

                    // in the case that the enqueuer task is done and the shard its at is
                    // not empty, that means that a dequeuer task has not completed its
                    if is_done && is_shard_empty && role == TaskRole::Enqueue {
                        // first check hashmap because we need to see a few things:
                        // if this is an enqueuer that was done first, and the dequeuer
                        // is not done, and the shard is empty, AND there are no other
                        // enquerer here at this shard_index that are also not done
                        // unpair that dequerer and let it find another enqueuer to match with
                        // This is a concern when you have an enqueuer task that puts up
                        // 10 items, but the pairing dequerer task pops off 20 items,
                        // and then there's another enqueuer task that puts up 10 items
                        // exists on another shard but isn't paired yet the pairing dequeuer task
                        // will never be done

                        // this is guaranteed to contain a hashset because something was paired at this point
                        // task_node and current should be the same thing
                        let pairs = pairs.unwrap();


                        // We remove done pairs from the Hashmap/Hashset together
                        // If done task notices a None in the map, that means its
                        // partner has already removed them in the map already
                        // Sidenote:
                        // Cloning the Option doesn't mean cloning the whole TaskNode internals
                        // It's actually cloning the memory location to the TaskNode
                        // Super fast!
                        if let Some(pair) = pairs_map.get(&current).cloned() {
                            if !(*pair.0).is_done.load(Ordering::Relaxed) {
                                // if I see another enqueuer task here who is not done yet, then I can
                                // offer to work on dequeuing that enqueuer items, so just unpair but not reassign
                                // otherwise, I need reassignment!
                                if pairs
                                    .iter()
                                    .find(|ptr| {
                                        (*ptr.0).role == TaskRole::Enqueue
                                            && (*ptr.0).is_done.load(Ordering::Relaxed)
                                                == false  && !(*ptr.0).is_paired.load(Ordering::Relaxed)
                                    })
                                    .is_some()
                                {
                                    (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                } else {
                                    (*pair.0).is_assigned.store(false, Ordering::Relaxed);
                                    (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                }
                            }

                            pairs_map.remove(&pair);
                            pairs_map.remove(&current);
                            pairs.remove(&pair);
                            pairs.remove(&current);
                        }

                        // perform cleanup for the completed task
                        let next = task_node.next.load(Ordering::Relaxed);
                        let free_status = if prev.0.is_null() {
                            // Because task registration pre-pends at head, must be very careful here
                            // we use CAS as result to see if we grabbed the head
                            // buffer.head.compare_exchange(current.0, next, Ordering::AcqRel, Ordering::Relaxed).is_ok()
                            buffer
                                .head
                                .compare_exchange_weak(
                                    current.0,
                                    next,
                                    Ordering::AcqRel,
                                    Ordering::Relaxed,
                                )
                                .is_ok()
                        } else {
                            // because task registration happens at the beginning, this is always safe
                            // to free
                            (*prev.0).next.store(next, Ordering::Relaxed);
                            true
                        };

                        // we can successfully free this item
                        if free_status {
                            drop(Box::from_raw(current.0));
                        }
                        current = TaskNodePtr(next);
                        continue;
                    } else if is_done && role == TaskRole::Dequeue {
                        // first check hashmap because we need to see a few things:
                        // if this is an enqueuer that was done first, and the dequeuer
                        // is not done, and the shard is empty, AND there are no other
                        // enquerer here at this shard_index that are also not done
                        // unpair that dequerer and let it find another enqueuer to match with
                        // This is a concern when you have an enqueuer task that puts up
                        // 10 items, but the pairing dequerer task pops off 20 items,
                        // and then there's another enqueuer task that puts up 10 items
                        // exists on another shard but isn't paired yet the pairing dequeuer task
                        // will never be done

                        // this is guaranteed to contain a hashset because something was paired at this point
                        // task_node and current should be the same thing
                        let pairs = pairs.unwrap();
                        
                        // We remove done pairs from the Hashmap/Hashset together
                        // If done task notices a None in the map, that means its
                        // partner has already removed them in the map already
                        // Sidenote:
                        // Cloning the Option doesn't mean cloning the whole TaskNode internals
                        // It's actually cloning the memory location to the TaskNode
                        // Super fast!
                        if let Some(pair) = pairs_map.get(&current).cloned() {
                            // an enqueuer who is not done will stick to the shard
                            // they were assigned to, but they will be denoted as unpaired
                            // this is so that future dequeuers will bundle up with this enqueuer
                            (*pair.0).is_paired.store(false, Ordering::Relaxed);
                            pairs_map.remove(&pair);
                            pairs_map.remove(&current);
                            pairs.remove(&pair);
                            pairs.remove(&current);
                        }

                        // perform cleanup for the completed task
                        let next = task_node.next.load(Ordering::Relaxed);
                        let free_status = if prev.0.is_null() {
                            // Because task registration pre-pends at head, must be very careful here
                            // we use CAS as result to see if we grabbed the head
                            buffer
                                .head
                                .compare_exchange_weak(
                                    current.0,
                                    next,
                                    Ordering::AcqRel,
                                    Ordering::Relaxed,
                                )
                                .is_ok()
                        } else {
                            // because task registration happens at the beginning, this is always safe
                            // to free
                            (*prev.0).next.store(next, Ordering::Relaxed);
                            true
                        };

                        // we can successfully free this item
                        if free_status {
                            drop(Box::from_raw(current.0));
                        }
                        current = TaskNodePtr(next);
                        continue;
                    }

                    // if it's paired skip forward
                    if is_paired {
                        prev = TaskNodePtr(current.0);
                        current = TaskNodePtr(task_node.next.load(Ordering::Relaxed));
                        continue;
                    }

                    // provide enqueuer with shard_index early!
                    if role == TaskRole::Enqueue && !is_assigned {
                        let shard_index = get_shard_ind().unwrap(); // guarantee to have a value inside

                        task_node.shard_ind.store(shard_index, Ordering::Relaxed);
                        task_node.is_assigned.store(true, Ordering::Relaxed);

                        set_shard_ind((shard_index + 1) % buffer.get_num_of_shards());
                    } else if role == TaskRole::Dequeue && !is_paired && is_assigned {
                        let pairs = shard_task_map
                            .get_mut(&task_node.shard_ind.load(Ordering::Relaxed))
                            .unwrap();
                        if is_shard_empty
                            && pairs
                                .iter()
                                .find(|ptr| {
                                    (*ptr.0).role == TaskRole::Enqueue
                                        && (*ptr.0).is_done.load(Ordering::Relaxed) == false &&
                                        !(*ptr.0).is_paired.load(Ordering::Relaxed)
                                })
                                .is_none()
                        {
                            task_node.is_assigned.store(false, Ordering::Relaxed);
                        } else {
                            prev = TaskNodePtr(current.0);
                            current = TaskNodePtr(task_node.next.load(Ordering::Relaxed));
                            continue;
                        }
                    }

                    // This is our second pointer, we use this to scan forward
                    // for a match to pair with
                    let mut scan_prev = current;
                    let mut scan = TaskNodePtr(task_node.next.load(Ordering::Relaxed));

                    while !scan.0.is_null() {
                        let scan_node = &*scan.0;

                        let scan_done = scan_node.is_done.load(Ordering::Relaxed);
                        let scan_paired = scan_node.is_paired.load(Ordering::Relaxed);
                        let scan_assigned = scan_node.is_assigned.load(Ordering::Relaxed);
                        let scan_role = scan_node.role;

                        let shard_ind = &scan_node.shard_ind.load(Ordering::Relaxed);
                        let is_shard_empty = buffer.is_shard_empty(*shard_ind);

                        let pairs = shard_task_map
                                .get_mut(shard_ind);

                        // scanner can also try to clean up as well and free
                        if scan_done && is_shard_empty && role == TaskRole::Enqueue {
                            let pairs = pairs
                                .unwrap();

                            // We remove done pairs from the Hashmap/Hashset together
                            // If done task notices a None in the map, that means its
                            // partner has already removed them in the map already
                            // Sidenote:
                            // Cloning the Option doesn't mean cloning the whole TaskNode internals
                            // It's actually cloning the memory location to the TaskNode
                            // Super fast!
                            if let Some(pair) = pairs_map.get(&scan).cloned() {
                                if !(*pair.0).is_done.load(Ordering::Relaxed) {
                                    // if my shard is not empty, I need to be unpaired but not reassigned yet!
                                    // if I see another enqueuer task here who is not done yet, then I can
                                    // offer to work on dequeuing that enqueuer items, so just unpair but not reassign
                                    // otherwise, I need reassignment!
                                    if !is_shard_empty {
                                        (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                    } else if pairs
                                        .iter()
                                        .find(|ptr| {
                                            (*ptr.0).role == TaskRole::Enqueue
                                                && (*ptr.0).is_done.load(Ordering::Relaxed)
                                                    == false
                                        })
                                        .is_some()
                                    {
                                        (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                    } else {
                                        (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                        (*pair.0)
                                            .is_assigned
                                            .store(false, Ordering::Relaxed);
                                    }
                                }
                                pairs_map.remove(&pair);
                                pairs_map.remove(&scan);
                                pairs.remove(&pair);
                                pairs.remove(&scan);
                            }

                            let next = scan_node.next.load(Ordering::Relaxed);
                            let free_status = if scan_prev.0.is_null() {
                                // Because task registration pre-pends at head, must be very careful here
                                // we use CAS as result to see if we grabbed the head
                                buffer
                                    .head
                                    .compare_exchange_weak(
                                        scan.0,
                                        next,
                                        Ordering::AcqRel,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                            } else {
                                // because task registration happens at the beginning, this is always safe
                                // to free
                                (*scan_prev.0).next.store(next, Ordering::Relaxed);
                                true
                            };

                            // we can successfully free this item
                            if free_status {
                                drop(Box::from_raw(scan.0));
                            }
                            scan = TaskNodePtr(next);
                            continue;
                        } else if scan_done && scan_role == TaskRole::Dequeue {
                            let pairs = pairs
                                .unwrap();

                            // We remove done pairs from the Hashmap/Hashset together
                            // If done task notices a None in the map, that means its
                            // partner has already removed them in the map already
                            // Sidenote:
                            // Cloning the Option doesn't mean cloning the whole TaskNode internals
                            // It's actually cloning the memory location to the TaskNode
                            // Super fast!
                            if let Some(pair) = pairs_map.get(&scan).cloned() {
                                match scan_role {
                                    TaskRole::Enqueue => {
                                        if !(*pair.0).is_done.load(Ordering::Relaxed) {
                                            // if my shard is not empty, I need to be unpaired but not reassigned yet!
                                            // if I see another enqueuer task here who is not done yet, then I can
                                            // offer to work on dequeuing that enqueuer items, so just unpair but not reassign
                                            // otherwise, I need reassignment!
                                            if !is_shard_empty {
                                                (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                            } else if pairs
                                                .iter()
                                                .find(|ptr| {
                                                    (*ptr.0).role == TaskRole::Enqueue
                                                        && (*ptr.0).is_done.load(Ordering::Relaxed)
                                                            == false
                                                })
                                                .is_some()
                                            {
                                                (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                            } else {
                                                (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                                (*pair.0)
                                                    .is_assigned
                                                    .store(false, Ordering::Relaxed);
                                            }
                                        }
                                        pairs_map.remove(&pair);
                                        pairs_map.remove(&scan);
                                        pairs.remove(&pair);
                                        pairs.remove(&scan);
                                    }
                                    TaskRole::Dequeue => {
                                        // an enqueuer who is not done will stick to the shard
                                        // they were assigned to, but they will be denoted as unpaired
                                        // this is so that future dequeuers will bundle up with this enqueuer
                                        (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                        pairs_map.remove(&pair);
                                        pairs_map.remove(&scan);
                                        pairs.remove(&pair);
                                        pairs.remove(&scan);
                                    }
                                }
                            }

                            let next = scan_node.next.load(Ordering::Relaxed);
                            let free_status = if scan_prev.0.is_null() {
                                // Because task registration pre-pends at head, must be very careful here
                                // we use CAS as result to see if we grabbed the head
                                buffer
                                    .head
                                    .compare_exchange_weak(
                                        scan.0,
                                        next,
                                        Ordering::AcqRel,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                            } else {
                                // because task registration happens at the beginning, this is always safe
                                // to free
                                (*scan_prev.0).next.store(next, Ordering::Relaxed);
                                true
                            };

                            // we can successfully free this item
                            if free_status {
                                drop(Box::from_raw(scan.0));
                            }
                            scan = TaskNodePtr(next);
                            continue;
                        }

                        // if it's paired skip forward, heck don't even let this
                        // be something that current can rebound to
                        if scan_paired {
                            scan_prev = TaskNodePtr(scan.0);
                            scan = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));
                            continue;
                        }

                        // provide enqueuer with shard_index early!
                        if scan_role == TaskRole::Enqueue && !scan_assigned {
                            let shard_index = get_shard_ind().unwrap(); // guarantee to have a value inside

                            scan_node.shard_ind.store(shard_index, Ordering::Relaxed);
                            scan_node.is_assigned.store(true, Ordering::Relaxed);

                            set_shard_ind((shard_index + 1) % buffer.get_num_of_shards());
                        } else if scan_role == TaskRole::Dequeue && !scan_paired && scan_assigned {
                            // set unassigned if there is nothing this dequeuer is doing and it is not done yet

                            let pairs = shard_task_map
                                .get_mut(&scan_node.shard_ind.load(Ordering::Relaxed))
                                .unwrap();
                            if is_shard_empty
                                && pairs
                                    .iter()
                                    .find(|ptr| {
                                        (*ptr.0).role == TaskRole::Enqueue
                                            && (*ptr.0).is_done.load(Ordering::Relaxed) == false 
                                            //&& !(*ptr.0).is_paired.load(Ordering::Relaxed)
                                    })
                                    .is_none()
                            {
                                scan_node.is_assigned.store(false, Ordering::Relaxed);
                            } else {
                                scan_prev = TaskNodePtr(scan.0);
                                scan = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));
                                continue;
                            }
                        }

                        // Sweet I found my pair because our roles don't match!
                        if scan_role != role {
                            let (enq, deq) = match role {
                                TaskRole::Enqueue => (current, scan),
                                TaskRole::Dequeue => (scan, current),
                            };
                            // if deq is assigned, you don't want to override it yet
                            // until it is completely sure that it won't dequeue anymore
                            if !(*deq.0).is_assigned.load(Ordering::Relaxed) {

                                let shard_ind = (*enq.0).shard_ind.load(Ordering::Relaxed);
                                (*deq.0).shard_ind.store(shard_ind, Ordering::Relaxed);

                                (*deq.0).is_assigned.store(true, Ordering::Relaxed);

                                // mark them as paired
                                (*enq.0).is_paired.store(true, Ordering::Relaxed);
                                (*deq.0).is_paired.store(true, Ordering::Relaxed);

                                // either append to the shard shard_task_map's hashset or create the hashset
                                // and append to it
                                shard_task_map
                                    .entry(shard_ind)
                                    .or_default() // default create new HashSet
                                    .extend([enq, deq]);

                                // backward and forward necessary for clean up and unpairing
                                pairs_map.insert(enq, deq);
                                pairs_map.insert(deq, enq);

                                // This refers to the one you just paired or matched up with!
                                prev = scan;
                                current = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));

                                break;
                            }
                        }
                        scan_prev = scan;
                        scan = TaskNodePtr(scan_node.next.load(Ordering::Relaxed));
                    }

                    // Couldn't find a pair to match enqueue/dequeue :(
                    if scan.0.is_null() {
                        break;
                    }
                }
            }
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
