use crate::{
    MLFShardedRingBuf, ShardPolicy, ShardedRingBuf,
    guards::TaskDoneGuard,
    shard_policies::ShardPolicyKind,
    task_locals::{
        SHARD_INDEX, SHARD_POLICY, SHIFT, TASK_NODE, get_shard_ind, set_shard_ind, set_task_node,
    },
    task_node::{TaskNode, TaskNodePtr, TaskRole},
};
use futures_util::{Stream, StreamExt};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    ptr,
    sync::{Arc, atomic::Ordering},
};
use tokio::task::{JoinHandle, spawn, yield_now};

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of enqueuing a single item onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
pub fn spawn_enqueuer<T>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    item: T,
) -> JoinHandle<usize>
where
    T: Send + 'static,
{
    let enq_fut = async move {
        let mut counter = 0;
        buffer.enqueue(item).await;
        counter += 1;

        match policy {
            ShardPolicy::Sweep { initial_index } => {}
            ShardPolicy::RandomAndSweep => {}
            ShardPolicy::ShiftBy {
                initial_index,
                shift,
            } => todo!(),
            ShardPolicy::Pin { initial_index } => {
                // println!("I completed work as a Enqueuer and need to notify Deq");
                buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()]
                    .notify_one();
                // buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()].notify_waiters();
            }
        }

        counter
    };

    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), enq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), enq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of enqueuing items through an iterator onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
pub fn spawn_enqueuer_with_iterator<T, I>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    items: I,
) -> JoinHandle<usize>
where
    I: IntoIterator<Item = T> + Send + 'static,
    T: Send + 'static,
    <I as IntoIterator>::IntoIter: Send,
{
    let enq_fut = async move {
        let mut counter = 0;
        for item in items {
            buffer.enqueue(item).await;
            counter += 1;
        }

        match policy {
            ShardPolicy::Sweep { initial_index } => {}
            ShardPolicy::RandomAndSweep => {}
            ShardPolicy::ShiftBy {
                initial_index,
                shift,
            } => {}
            ShardPolicy::Pin { initial_index } => {
                // println!("I completed work as a Enqueuer and need to notify Deq");
                buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()]
                    .notify_one();
                // buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()].notify_waiters();
            }
        };

        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), enq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), enq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of enqueuing items through an iterator onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
pub fn mlf_spawn_enqueuer_with_iterator<T, I>(
    buffer: Arc<MLFShardedRingBuf<T>>,
    policy: ShardPolicy,
    items: I,
) -> JoinHandle<usize>
where
    I: IntoIterator<Item = T> + Send + 'static,
    T: Send + 'static,
    <I as IntoIterator>::IntoIter: Send,
{
    let enq_fut = async move {
        let mut counter = 0;
        for item in items {
            buffer.enqueue(item).await;
            counter += 1;
        }

        match policy {
            ShardPolicy::Sweep { initial_index } => {}
            ShardPolicy::RandomAndSweep => {}
            ShardPolicy::ShiftBy {
                initial_index,
                shift,
            } => {}
            ShardPolicy::Pin { initial_index } => {
                // println!("I completed work as a Enqueuer and need to notify Deq");
                // buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()].notify_one();
                // buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()].notify_waiters();
            }
        };

        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), enq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), enq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of enqueuing items through a stream onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
pub fn spawn_enqueuer_with_stream<T, S>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    stream: S,
) -> JoinHandle<usize>
where
    S: Stream<Item = T> + Send + 'static,
    T: Send + 'static,
{
    let enq_fut = async move {
        tokio::pin!(stream);
        let mut counter = 0;
        while let Some(item) = stream.next().await {
            buffer.enqueue(item).await;
            counter += 1;
        }
        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), enq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), enq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing a single item from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
pub fn spawn_dequeuer<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        let deq_item = buffer.dequeue().await;
        if let Some(item) = deq_item {
            f(item);
            counter += 1;
        }

        match policy {
            ShardPolicy::Sweep { initial_index } => {}
            ShardPolicy::RandomAndSweep => {}
            ShardPolicy::ShiftBy {
                initial_index,
                shift,
            } => todo!(),
            ShardPolicy::Pin { initial_index } => {
                // println!("I completed work as a Enqueuer and need to notify Deq");
                buffer.job_space_shard_notifs[initial_index % buffer.get_num_of_shards()]
                    .notify_one();
            }
        }

        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing `count` number of items from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
pub fn spawn_dequeuer_bounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    count: usize,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        for _ in 0..count {
            let deq_item = buffer.dequeue().await;
            match deq_item {
                Some(item) => {
                    f(item);
                    counter += 1;
                }
                None => break,
            }
        }
        // println!("done");
        let _ = match policy {
            ShardPolicy::Sweep { initial_index } => {}
            ShardPolicy::RandomAndSweep => {}
            ShardPolicy::ShiftBy {
                initial_index,
                shift,
            } => {}
            ShardPolicy::Pin { initial_index } => {
                // println!("I completed work as a Enqueuer and need to notify Deq");
                // buffer.job_space_shard_notifs[initial_index % buffer.get_num_of_shards()].notify_one();
            }
        };
        // println!("I'm here");
        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing items an unbounded number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
pub fn spawn_dequeuer_unbounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        loop {
            let deq_item = buffer.dequeue().await;
            match deq_item {
                Some(item) => {
                    f(item);
                    counter += 1;
                }
                None => break,
            }
        }
        // println!("I'm done");
        // buffer.deq_fin_taken.set(false);
        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing items an unbounded number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
pub fn mlf_spawn_dequeuer_unbounded<T, F>(
    buffer: Arc<MLFShardedRingBuf<T>>,
    policy: ShardPolicy,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        loop {
            let deq_item = buffer.dequeue().await;
            match deq_item {
                Some(item) => {
                    f(item);
                    counter += 1;
                }
                None => break,
            }
        }
        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully a single time from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
pub fn spawn_dequeuer_full<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        let deq_items = buffer.dequeue_full().await;
        if let Some(items) = deq_items {
            for item in items {
                f(item);
                counter += 1;
            }
        }
        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully `count` number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
pub fn spawn_dequeuer_full_bounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    count: usize,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        for _ in 0..count {
            let deq_items = buffer.dequeue_full().await;
            match deq_items {
                Some(items) => {
                    for item in items {
                        f(item);
                        counter += 1;
                    }
                }
                None => break,
            }
        }
        // println!("I'm done");
        // buffer.deq_fin_taken.set(false);
        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully `count` number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
pub fn spawn_enqueuer_full_with_iterator<T, I>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    items: I,
) -> JoinHandle<usize>
where
    I: IntoIterator<Item = T> + Send + 'static,
    T: Send + 'static,
    <I as IntoIterator>::IntoIter: Send,
{
    let enq_fut = async move {
        let mut counter = 0;
        // loop {
        let full_enq = buffer
            .get_shard_capacity(buffer.get_num_of_shards() - 1)
            .unwrap();
        let mut enq_vec = Vec::with_capacity(full_enq);
        for item in items {
            if counter != 0 && counter % full_enq == 0 {
                buffer.enqueue_full(enq_vec).await;
                enq_vec = Vec::with_capacity(full_enq);
                enq_vec.push(item);
                counter += 1;
            } else {
                enq_vec.push(item);
                counter += 1;
            }
        }

        if enq_vec.len() != 0 {
            buffer.enqueue_full(enq_vec).await;
        }

        match policy {
            ShardPolicy::Sweep { initial_index } => {}
            ShardPolicy::RandomAndSweep => {}
            ShardPolicy::ShiftBy {
                initial_index,
                shift,
            } => todo!(),
            ShardPolicy::Pin { initial_index } => {
                // println!("I completed work as a Enqueuer and need to notify Deq");
                buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()]
                    .notify_one();
                // buffer.job_post_shard_notifs[initial_index % buffer.get_num_of_shards()].notify_waiters();
            }
        }

        counter
    };
    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), enq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), enq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), enq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with a provided `ShardPolicy` using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully an unbounded number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
pub fn spawn_dequeuer_full_unbounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    policy: ShardPolicy,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    let deq_fut = async move {
        let mut counter = 0;
        loop {
            let deq_items = buffer.dequeue_full().await;
            match deq_items {
                Some(items) => {
                    for item in items {
                        f(item);
                        counter += 1;
                    }
                }
                None => break,
            }
        }
        counter
    };

    match policy {
        ShardPolicy::Sweep { initial_index } => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Sweep),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::RandomAndSweep => spawn(SHIFT.scope(
            Cell::new(1),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::RandomAndSweep),
                SHARD_INDEX.scope(Cell::new(None), deq_fut),
            ),
        )),
        ShardPolicy::ShiftBy {
            initial_index,
            shift,
        } => spawn(SHIFT.scope(
            Cell::new(shift),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::ShiftBy),
                SHARD_INDEX.scope(Cell::new(initial_index), deq_fut),
            ),
        )),
        ShardPolicy::Pin { initial_index } => spawn(SHIFT.scope(
            Cell::new(0),
            SHARD_POLICY.scope(
                Cell::new(ShardPolicyKind::Pin),
                SHARD_INDEX.scope(Cell::new(Some(initial_index)), deq_fut),
            ),
        )),
    }
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of enqueuing a single item onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_enqueuer<T>(buffer: Arc<ShardedRingBuf<T>>, item: T) -> JoinHandle<usize>
where
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Enqueue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // enq future
                    let mut counter = 0;
                    buffer.enqueue(item).await;
                    counter += 1;
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of enqueuing items through an iterator onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_enqueuer_with_iterator<T, I>(
    buffer: Arc<ShardedRingBuf<T>>,
    items: I,
) -> JoinHandle<usize>
where
    I: IntoIterator<Item = T> + Send + 'static,
    T: Send + 'static,
    <I as IntoIterator>::IntoIter: Send,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Enqueue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // enq future
                    let mut counter = 0;
                    for item in items {
                        buffer.enqueue(item).await;
                        counter += 1;
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of enqueuing items through a stream onto a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// enqueue operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_enqueuer_with_stream<T, S>(
    buffer: Arc<ShardedRingBuf<T>>,
    stream: S,
) -> JoinHandle<usize>
where
    S: Stream<Item = T> + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Enqueue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // enq future
                    tokio::pin!(stream);
                    let mut counter = 0;
                    while let Some(item) = stream.next().await {
                        buffer.enqueue(item).await;
                        counter += 1;
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of dequeuing a single item from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_dequeuer<T, F>(buffer: Arc<ShardedRingBuf<T>>, f: F) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Dequeue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // deq future
                    let mut counter = 0;
                    let deq_item = buffer.dequeue().await;

                    if let Some(item) = deq_item {
                        f(item);
                        counter += 1;
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of dequeuing `count` number of items from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_dequeuer_bounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    count: usize,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Dequeue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // deq future
                    let mut counter = 0;
                    for _ in 0..count {
                        let deq_item = buffer.dequeue().await;
                        match deq_item {
                            Some(item) => {
                                f(item);
                                counter += 1;
                            }
                            None => break,
                        }
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of dequeuing items an unbounded number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_dequeuer_unbounded<T, F>(buffer: Arc<ShardedRingBuf<T>>, f: F) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Dequeue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // deq future
                    let mut counter = 0;
                    loop {
                        let deq_item = buffer.dequeue().await;
                        match deq_item {
                            Some(item) => {
                                f(item);
                                counter += 1;
                            }
                            None => break,
                        }
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully a single time from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_dequeuer_full<T, F>(buffer: Arc<ShardedRingBuf<T>>, f: F) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Dequeue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // deq future
                    let mut counter = 0;
                    let deq_items = buffer.dequeue_full().await;

                    if let Some(items) = deq_items {
                        for item in items {
                            f(item);
                            counter += 1;
                        }
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully `count` number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_dequeuer_full_bounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    count: usize,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Dequeue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // deq future
                    let mut counter = 0;
                    for _ in 0..count {
                        let deq_items = buffer.dequeue_full().await;
                        match deq_items {
                            Some(items) => {
                                for item in items {
                                    f(item);
                                    counter += 1;
                                }
                            }
                            None => break,
                        }
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// Spawns a Tokio task with the CFT (Completely Fair Tasks) policy using the current Tokio runtime
/// context for the purpose of dequeuing items from a shard fully an unbounded number of times from a `ShardedRingBuf<T>`.
///
/// On return, it returns a JoinHandle that, when completed, returns the number of successful
/// dequeue full operations that occurred.
///
/// `spawn_assigner()` must be called first before using this function
///
/// Important caveat with CFT policy:
/// * This policy can't handle more infinite looping enqueuers than infinite loop dequeuers *should*
///   the number of shards used is greater than the number of infinite looping dequeuers.
///     * This is because the assigner pins the dequeuer to an enqueuer to a shard together. It will only
///       re-pin dequeuer once the dequeuer completely empties the shard and it is not paired with
///       an enqueuer. If you really want to use more infinite looping enqueuers than dequeuers, then
///       use only {# of infinite looping dequeuers} shards.
pub fn cft_spawn_dequeuer_full_unbounded<T, F>(
    buffer: Arc<ShardedRingBuf<T>>,
    f: F,
) -> JoinHandle<usize>
where
    F: Fn(T) + Send + 'static,
    T: Send + 'static,
{
    assert!(
        buffer.assigner_spawned.load(Ordering::Relaxed),
        "CFT task cannot be spawned without assigner task being spawned"
    );
    spawn(TASK_NODE.scope(
        Cell::new(TaskNodePtr(ptr::null_mut())),
        SHARD_POLICY.scope(
            Cell::new(ShardPolicyKind::Cft),
            SHARD_INDEX.scope(
                Cell::new(None),
                SHIFT.scope(Cell::new(0), async move {
                    // Allocate for pointer and set it to the TASK_NODE for internal use in the future
                    let mut node = Box::new(TaskNode::new(TaskRole::Dequeue));
                    let mut task_node_ptr = TaskNodePtr(node.as_mut() as *mut TaskNode);
                    set_task_node(task_node_ptr);

                    // This performs task registration in a yield now loop
                    // if the task wasn't able to be registered into the list
                    // it can do it later down the line when there are less
                    // task fighting to insert at head and the assigner is more
                    // likely to work on areas besides the head.
                    // CANCEL SAFETY: yield_now() cancelling here means
                    // that the node was not inserted into the linked list
                    // and the Box Drop method is applied deallocating the node for me
                    loop {
                        let current = buffer.get_head_relaxed(); // get head with Relaxed Memory ordering
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

                    // We allocate memory for the node because we want the
                    // assigner to have full control over deleting this node
                    // when it is done!
                    task_node_ptr = TaskNodePtr(Box::into_raw(node));

                    // guard is used to mark the done flag as done if future aborts or gets completed
                    let _guard = TaskDoneGuard::get_done(unsafe { &(&*task_node_ptr.0).is_done });

                    // deq future
                    let mut counter = 0;
                    loop {
                        let deq_items = buffer.dequeue_full().await;
                        match deq_items {
                            Some(items) => {
                                for item in items {
                                    f(item);
                                    counter += 1;
                                }
                            }
                            None => break,
                        }
                    }
                    counter
                }),
            ),
        ),
    ))
}

/// This is your helpful assigner task that handles assigning enqueuer tasks to dequeuer
/// and vice versa in an optimal way.
///
/// To use CFT (Completely Fair Tasks) Policy, you must spawn the assigner task first.
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
pub fn spawn_assigner<T: 'static>(buffer: Arc<ShardedRingBuf<T>>) -> JoinHandle<()> {
    assert!(
        !buffer.assigner_spawned.load(Ordering::Relaxed),
        "Assigner is spawned already"
    );
    buffer.assigner_spawned.store(true, Ordering::Relaxed);
    spawn(SHARD_INDEX.scope(Cell::new(Some(0)), async move {
        let mut shard_task_map: HashMap<usize, HashSet<TaskNodePtr>> = HashMap::new();
        let mut pairs_map: HashMap<TaskNodePtr, TaskNodePtr> = HashMap::new();
        loop {
            let mut prev = TaskNodePtr(ptr::null_mut());

            // head can be claimed in a relaxed manner (doesn't matter if we start
            // off with a stale head because we'll eventually handle those nodes
            // at another point)
            let mut current = buffer.get_head_relaxed();

            // assigner can get out of this function once it complete cleaning up
            // the list and resets the termination signal to false
            if buffer.assigner_terminate.load(Ordering::Relaxed) {
                while !current.0.is_null() {
                    let task_node = unsafe { &*current.0 };
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
                        unsafe { &*prev.0 }.next.store(next, Ordering::Release);
                        true
                    };

                    // we can successfully free this item
                    if free_status {
                        drop(unsafe { Box::from_raw(current.0) });
                    }
                    current = TaskNodePtr(next)
                }

                buffer.assigner_terminate.store(true, Ordering::Relaxed);
                buffer.assigner_spawned.store(false, Ordering::Relaxed);
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
pub fn terminate_assigner<T>(buffer: Arc<ShardedRingBuf<T>>)
where
    T: Send + 'static,
{
    buffer.assigner_terminate.store(true, Ordering::Relaxed);
}
