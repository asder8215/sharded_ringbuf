use crate::{
    LFShardedRingBuf, ShardPolicy, TaskRole,
    guards::TaskDoneGuard,
    shard_policies::ShardPolicyKind,
    task_locals::{
        SHARD_INDEX, SHARD_POLICY, SHIFT, TASK_NODE, get_shard_ind, set_shard_ind, set_task_node,
    },
    task_node::{TaskNode, TaskNodePtr},
};
use crossbeam_utils::CachePadded;
use fastrand::usize as frand;
use std::{
    cell::Cell,
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

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of using it with `LFShardedRingBuf<T>`.
///
/// This function or [rt_spawn_buffer_task] *must* be used in order to
/// enqueue or dequeue items onto `LFShardedRingBuf<T>` due to task local
/// variables. Using this function otherwise just spawns a regular Tokio task.
///
/// Here's an example of how you would use it with `LFShardedRingBuf<T>`:
/// ```rust
/// let CAPACITY: usize = 1024;
/// let SHARDS: usize = 4;
/// let ITEM_PER_TASK = 1000;
/// let MAX_TASK = 4;
///
/// let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(CAPACITY, SHARDS));
///
/// let mut deq_threads = Vec::with_capacity(MAX_TASKS);
/// let mut enq_threads = Vec::with_capacity(MAX_TASKS);
///
/// // spawn enq tasks with shift by policy
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let handler: tokio::task::JoinHandle<()> = spawn_buffer_task(
///         ShardPolicy::ShiftBy {
///             initial_index: None,
///             shift: MAX_TASKS,
///         },
///         async move {
///             for i in 0..ITEM_PER_TASK {
///                 rb.enqueue(i).await;
///             }
///         },
///     );
///     enq_threads.push(handler);
/// }
///
/// // spawn deq tasks with shift by policy
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let handler: tokio::task::JoinHandle<usize> = spawn_buffer_task(
///         ShardPolicy::ShiftBy {
///             initial_index: None,
///             shift: MAX_TASKS,
///         },
///         async move {
///             let mut counter: usize = 0;
///             for _i in 0..ITEM_PER_TASK {
///                 let item = rb.dequeue().await;
///                 match item {
///                     Some(_) => counter += 1,
///                     None => break,
///                 }
///             }
///             counter
///         },
///     );
///     deq_threads.push(handler);
/// }
///
/// // Wait for enqueuers
/// for enq in enq_threads {
///     enq.await.unwrap();
/// }
/// // Wait for dequeuers
/// for deq in deq_threads {
///     deq.await.unwrap();
/// }
/// ```
pub fn spawn_buffer_task<F, T>(policy: ShardPolicy, fut: F) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
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
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` and Tokio Runtime for the purpose
/// of using it with `LFShardedRingBuf<T>`.
///
/// This function or [spawn_buffer_task] *must* be used in order to enqueue or
/// dequeue items onto `LFShardedRingBuf<T>` due to task local variables.
/// Using this function otherwise just spawns a regular Tokio task.
///
/// Here's an example of how you might use it with `LFShardedRingBuf<T>`.
/// ```rust
/// let CAPACITY: usize = 1024;
/// let SHARDS: usize = 4;
/// let ITEM_PER_TASK = 1000;
/// let MAX_TASK = 4;
/// let MAX_THREADS = 4;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread()
///   .enable_all()
///   .worker_threads(MAX_THREADS)
///   .build()
///   .unwrap();
///
/// let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(CAPACITY, SHARDS));
///
/// let mut deq_threads = Vec::with_capacity(MAX_TASKS);
/// let mut enq_threads = Vec::with_capacity(MAX_TASKS);
///
/// // spawn enq tasks with shift by policy
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let handler: tokio::task::JoinHandle<()> = rt_spawn_buffer_task(
///         runtime,
///         ShardPolicy::ShiftBy {
///             initial_index: None,
///             shift: MAX_TASKS,
///         },
///         async move {
///             for i in 0..ITEM_PER_TASK {
///                 rb.enqueue(i).await;
///             }
///         },
///     );
///     enq_threads.push(handler);
/// }
///
/// // spawn deq tasks with shift by policy
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let handler: tokio::task::JoinHandle<usize> = rt_spawn_buffer_task(
///         runtime,
///         ShardPolicy::ShiftBy {
///             initial_index: None,
///             shift: MAX_TASKS,
///         },
///         async move {
///             let mut counter: usize = 0;
///             for _i in 0..ITEM_PER_TASK {
///                 let item = rb.dequeue().await;
///                 match item {
///                     Some(_) => counter += 1,
///                     None => break,
///                 }
///             }
///             counter
///         },
///     );
///     deq_threads.push(handler);
/// }
///
/// // Wait for enqueuers
/// for enq in enq_threads {
///     enq.await.unwrap();
/// }
/// // Wait for dequeuers
/// for deq in deq_threads {
///     deq.await.unwrap();
/// }
/// ```
pub fn rt_spawn_buffer_task<F, T>(runtime: &Runtime, policy: ShardPolicy, fut: F) -> JoinHandle<T>
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
    }
}

/// This is spawning with a new policy called CFT, aka Completely Fair Tasks. It uses a
/// "smart task" called the assigner via `spawn_assigner()` which handles all pinning of
/// enqueuer tasks to dequeuer tasks to a shard for you. Task registration to the list is
/// not done in a lock free manner unfortunately (it uses a spin lock under the hood).
/// It's meant to be a convenient policy, where if cancel-safety is not important, all
/// that the users would need to think about is the appropriate number of shards and capacity
/// per shard.
///
/// There are certain caveats with this policy:
/// * The user is responsible for providing the correct `TaskRole` (Enqueuer or Dequeuer)
///   and utilize the enqueue or dequeue function appropriately from the buffer
/// * The user is responsible for providing the correct, associated buffer to spawn the
///   buffer with.
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
///
/// This function may be refactored in the future to be a part of one of LFShardedRingBuf<T>'s functions
/// so that the first two caveats can be addressed nicely.
///
/// Here's an example of how to use this function:
///
/// ```rust
/// const MAX_ITEMS:  usize = 100000;
/// const MAX_SHARDS: usize = 10;
/// const MAX_TASKS:  usize = 5;
/// let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));
///
/// let mut deq_threads = Vec::new();
/// let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();
///
/// let assigner = spawn_assigner(rb.clone());
///
/// Spawn MAX_TASKS dequeuer tasks
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let handler =
///         spawn_with_cft::<_, usize, usize>(TaskRole::Dequeue, rb.clone(), async move {
///             let rb = rb.clone();
///             let mut counter: usize = 0;
///             loop {
///                 let item = rb.dequeue().await;
///                 match item {
///                     Some(_) => counter += 1,
///                     None => break,
///                 }
///             }
///             counter
///         });
///     deq_threads.push(handler);
/// }
///
/// Spawn MAX_TASKS enqueuer tasks
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let enq_handler =
///         spawn_with_cft::<_, usize, ()>(TaskRole::Enqueue, rb.clone(), async move {
///             let rb = rb.clone();
///             for _i in 0..2 * MAX_ITEMS {
///                 rb.enqueue(20).await;
///             }
///         });
///     enq_threads.push(enq_handler);
/// }
///
/// for enq in enq_threads {
///     enq.await.unwrap();
/// }
///
/// guarantees that the dequeuer finish remaining jobs in the buffer
/// before exiting
/// rb.poison();
///
/// let mut items_taken: usize = 0;
/// while let Some(curr_thread) = deq_threads.pop() {
///     items_taken += curr_thread.await.unwrap();
/// }
///
/// terminate_assigner(rb.clone());
///
/// let _ = assigner.await;
/// ```
pub fn spawn_with_cft<F, T, R>(
    role: TaskRole,
    buffer: Arc<LFShardedRingBuf<T>>,
    fut: F,
) -> JoinHandle<R>
where
    F: std::future::Future<Output = R> + Send + 'static,
    T: Send + 'static,
    R: Send + 'static,
{
    spawn(TASK_NODE.scope(
        Cell::new(CachePadded::new(TaskNodePtr(ptr::null_mut()))),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    let buffer = Arc::clone(&buffer);

                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let node = Box::new(TaskNode::new(role));
                    let task_node_ptr = CachePadded::new(TaskNodePtr(Box::into_raw(node)));
                    set_task_node(task_node_ptr);
                    let mut attempt: i32 = 0;

                    // This performs task registration in a thread sleeping loop
                    // This is inexpensive (just needs one successful compare swap)
                    // and helpful in the long run because
                    // you want your tasks to be registered for the assigner to
                    // to be able assign a shard to these tasks
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
                            let backoff_us = (1u64 << attempt.min(5)).min(100) as usize;
                            let jitter = frand(0..=backoff_us) as u64;
                            sleep(Duration::from_nanos(jitter));
                            attempt = attempt.saturating_add(1); // Avoid overflow
                        }
                    }

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // do whatever the user wants us to do with the future
                    fut.await
                }),
            ),
        ),
    ))
}

/// This is spawning with a user provided runtime with a new policy called CFT,
/// aka Completely Fair Tasks. It uses a "smart task" called the assigner via
/// `spawn_assigner()` which handles all pinning of enqueuer tasks to dequeuer tasks
/// to a shard for you. Task registration to the list is not done in a lock free manner
/// unfortunately (it uses a spin lock under the hood). It's meant to be a convenient policy,
/// where if cancel-safety is not important, all that the users would need to think about
/// is the appropriate number of shards and capacity per shard.
///
/// There are certain caveats with this policy:
/// * The user is responsible for providing the correct `TaskRole` (Enqueuer or Dequeuer)
///   and utilize the enqueue or dequeue function appropriately from the buffer
/// * The user is responsible for providing the correct, associated buffer to spawn the
///   buffer with.
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
///
/// This function may be refactored in the future to be a part of one of LFShardedRingBuf<T>'s functions
/// so that the first two caveats can be addressed nicely.
///
/// Here's an example of how to use this function:
/// ```rust
/// const MAX_ITEMS:   usize = 100000;
/// const MAX_SHARDS:  usize = 10;
/// const MAX_TASKS:   usize = 5;
/// const MAX_THREADS: usize = 5;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread()
///   .enable_all()
///   .worker_threads(MAX_THREADS)
///   .build()
///   .unwrap();
///
/// let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));
///
/// let mut deq_threads = Vec::new();
/// let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();
///
/// let assigner = spawn_assigner(rb.clone());
///
/// Spawn MAX_TASKS dequeuer tasks
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let handler =
///         rt_spawn_with_cft::<_, usize, usize>(runtime, TaskRole::Dequeue, rb.clone(), async move {
///             let rb = rb.clone();
///             let mut counter: usize = 0;
///             loop {
///                 let item = rb.dequeue().await;
///                 match item {
///                     Some(_) => counter += 1,
///                     None => break,
///                 }
///             }
///             counter
///         });
///     deq_threads.push(handler);
/// }
///
/// Spawn MAX_TASKS enqueuer tasks
/// for _ in 0..MAX_TASKS {
///     let rb = Arc::clone(&rb);
///     let enq_handler =
///         rt_spawn_with_cft::<_, usize, ()>(runtime, TaskRole::Enqueue, rb.clone(), async move {
///             let rb = rb.clone();
///             for _i in 0..2 * MAX_ITEMS {
///                 rb.enqueue(20).await;
///             }
///         });
///     enq_threads.push(enq_handler);
/// }
///
/// for enq in enq_threads {
///     enq.await.unwrap();
/// }
///
/// guarantees that the dequeuer finish remaining jobs in the buffer
/// before exiting
/// rb.poison();
///
/// let mut items_taken: usize = 0;
/// while let Some(curr_thread) = deq_threads.pop() {
///     items_taken += curr_thread.await.unwrap();
/// }
///
/// terminate_assigner(rb.clone());
///
/// let _ = assigner.await;
/// ```
pub fn rt_spawn_with_cft<F, T, R>(
    rt: Runtime,
    role: TaskRole,
    buffer: Arc<LFShardedRingBuf<T>>,
    fut: F,
) -> JoinHandle<R>
where
    F: std::future::Future<Output = R> + Send + 'static,
    T: Send + 'static,
    R: Send + 'static,
{
    rt.spawn(TASK_NODE.scope(
        Cell::new(CachePadded::new(TaskNodePtr(ptr::null_mut()))),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    let buffer = Arc::clone(&buffer);

                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let node = Box::new(TaskNode::new(role));
                    let task_node_ptr = CachePadded::new(TaskNodePtr(Box::into_raw(node)));
                    set_task_node(task_node_ptr);
                    let mut attempt: i32 = 0;

                    // This performs task registration in a thread sleeping loop
                    // This is inexpensive (just needs one successful compare swap)
                    // and helpful in the long run because
                    // you want your tasks to be registered for the assigner to
                    // to be able assign a shard to these tasks
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
                            let backoff_us = (1u64 << attempt.min(5)).min(100) as usize;
                            let jitter = frand(0..=backoff_us) as u64;
                            sleep(Duration::from_nanos(jitter));
                            attempt = attempt.saturating_add(1); // Avoid overflow
                        }
                    }

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // do whatever the user wants us to do with the future
                    fut.await
                }),
            ),
        ),
    ))
}

/// This is your helpful assigner task that handles assigning enqueuer tasks to dequeuer
/// and vice versa in an optimal way.
///
/// To use CTF Shard Policy, you must spawn the assigner task first.
///
/// To terminate the assigner, use `terminate_assigner()`` and then you can await
/// on the assigner. The assigner also handles memory clean up of allocated enqueuer and
/// dequeuer task node pointers to the buffer. Do NOT call abort() on the assigner.
///
/// It is the responsibility of the user to provide the correct buffer that tasks are
/// spawning into and that the assigner task should work with.
///
/// Moreover, there are certain caveats to using this assigner task:
/// * It can't handle more infinite looping enqueuers than infinite loop dequeuers should
///   the number of shards used is greater than the number of infinite looping dequeuers.
///
/// This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
/// re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
/// an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
/// use only {# of infinite looping dequeuers} shards.
pub fn spawn_assigner<T: 'static>(buffer: Arc<LFShardedRingBuf<T>>) -> JoinHandle<()> {
    spawn(SHARD_INDEX.scope(Cell::new(Some(0)), async move {
        let buffer_clone = Arc::clone(&buffer);
        let mut shard_task_map: HashMap<usize, HashSet<TaskNodePtr>> = HashMap::new();
        let mut pairs_map: HashMap<TaskNodePtr, TaskNodePtr> = HashMap::new();
        loop {
            let mut prev = TaskNodePtr(ptr::null_mut());
            // let mut current = buffer_clone.get_head();
            let mut current = buffer_clone.get_head_relaxed();

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
                    let is_shard_empty =
                        buffer.is_shard_empty(task_node.shard_ind.load(Ordering::Relaxed));

                    let pairs = shard_task_map.get_mut(shard_ind);

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

                        // We remove done pairs from the Hashmap/Hashset together
                        // If done task notices a None in the map, that means its
                        // partner has already removed them in the map already
                        // Sidenote:
                        // Cloning the Option doesn't mean cloning the whole TaskNode internals
                        // It's actually cloning the memory location to the TaskNode
                        // Super fast!
                        if let Some(pairs) = pairs {
                            if let Some(pair) = pairs_map.get(&current).cloned() {
                                if !(*pair.0).is_done.load(Ordering::Relaxed) {
                                    // if I see another enqueuer task here who is not done yet, then I can
                                    // offer to work on dequeuing that enqueuer items, so just unpair but not reassign
                                    // otherwise, I need reassignment!
                                    if pairs.iter().any(|ptr| {
                                        (*ptr.0).role == TaskRole::Enqueue
                                            && !(*ptr.0).is_done.load(Ordering::Relaxed)
                                            && !(*ptr.0).is_paired.load(Ordering::Relaxed)
                                    }) {
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
                            (*prev.0).next.store(next, Ordering::Release);
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

                        // We remove done pairs from the Hashmap/Hashset together
                        // If done task notices a None in the map, that means its
                        // partner has already removed them in the map already
                        // Sidenote:
                        // Cloning the Option doesn't mean cloning the whole TaskNode internals
                        // It's actually cloning the memory location to the TaskNode
                        // Super fast!
                        if let Some(pairs) = pairs {
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
                            && !pairs.iter().any(|ptr| {
                                (*ptr.0).role == TaskRole::Enqueue
                                    && !(*ptr.0).is_done.load(Ordering::Relaxed)
                                    && !(*ptr.0).is_paired.load(Ordering::Relaxed)
                            })
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

                        let pairs = shard_task_map.get_mut(shard_ind);

                        // scanner can also try to clean up as well and free
                        if scan_done && is_shard_empty && role == TaskRole::Enqueue {
                            if let Some(pairs) = pairs {
                                if let Some(pair) = pairs_map.get(&scan).cloned() {
                                    if !(*pair.0).is_done.load(Ordering::Relaxed) {
                                        // if my shard is not empty, I need to be unpaired but not reassigned yet!
                                        // if I see another enqueuer task here who is not done yet, then I can
                                        // offer to work on dequeuing that enqueuer items, so just unpair but not reassign
                                        // otherwise, I need reassignment!
                                        if !is_shard_empty
                                            && pairs.iter().any(|ptr| {
                                                (*ptr.0).role == TaskRole::Enqueue
                                                    && !(*ptr.0).is_done.load(Ordering::Relaxed)
                                            })
                                        {
                                            (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                        } else {
                                            (*pair.0).is_paired.store(false, Ordering::Relaxed);
                                            (*pair.0).is_assigned.store(false, Ordering::Relaxed);
                                        }
                                    }
                                    pairs_map.remove(&pair);
                                    pairs_map.remove(&scan);
                                    pairs.remove(&pair);
                                    pairs.remove(&scan);
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
                                // (*scan_prev.0).next.store(next, Ordering::Release);
                                true
                            };

                            // we can successfully free this item
                            if free_status {
                                drop(Box::from_raw(scan.0));
                            }
                            scan = TaskNodePtr(next);
                            continue;
                        } else if scan_done && scan_role == TaskRole::Dequeue {
                            // We remove done pairs from the Hashmap/Hashset together
                            // If done task notices a None in the map, that means its
                            // partner has already removed them in the map already
                            // Sidenote:
                            // Cloning the Option doesn't mean cloning the whole TaskNode internals
                            // It's actually cloning the memory location to the TaskNode
                            // Super fast!
                            if let Some(pairs) = pairs {
                                if let Some(pair) = pairs_map.get(&scan).cloned() {
                                    match scan_role {
                                        TaskRole::Enqueue => {
                                            if !(*pair.0).is_done.load(Ordering::Relaxed) {
                                                // if my shard is not empty, I need to be unpaired but not reassigned yet!
                                                // if I see another enqueuer task here who is not done yet, then I can
                                                // offer to work on dequeuing that enqueuer items, so just unpair but not reassign
                                                // otherwise, I need reassignment!
                                                if !is_shard_empty
                                                    && pairs.iter().any(|ptr| {
                                                        (*ptr.0).role == TaskRole::Enqueue
                                                            && !(*ptr.0)
                                                                .is_done
                                                                .load(Ordering::Relaxed)
                                                    })
                                                {
                                                    (*pair.0)
                                                        .is_paired
                                                        .store(false, Ordering::Relaxed);
                                                } else {
                                                    (*pair.0)
                                                        .is_paired
                                                        .store(false, Ordering::Relaxed);
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
                                && !pairs.iter().any(|ptr| {
                                    (*ptr.0).role == TaskRole::Enqueue
                                        && !(*ptr.0).is_done.load(Ordering::Relaxed)
                                        && !(*ptr.0).is_paired.load(Ordering::Relaxed)
                                })
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

/// This is the termination signal to the assigner. Use this instead of cancelling
/// the assigner for it to terminate gracefully.
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
