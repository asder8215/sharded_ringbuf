use lf_shardedringbuf::LFShardedRingBuf;
use lf_shardedringbuf::ShardPolicy;
use lf_shardedringbuf::spawn_buffer_task;
use std::sync::Arc;

#[tokio::test(flavor = "current_thread")]
async fn test_spsc_tasks() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

    assert_eq!(rb.is_empty(), true);

    let mut deq_threads = Vec::new();
    let mut enq_threads = Vec::new();

    // Spawn 1 dequeuer task
    {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(
            ShardPolicy::Sweep {
                initial_index: None,
            },
            async move {
                let rb = rb.clone();
                let mut counter: usize = 0;
                loop {
                    let item = rb.dequeue().await;
                    match item {
                        Some(_) => counter += 1,
                        None => break,
                    }
                }
                counter
            },
        );
        deq_threads.push(handler);
    }

    // Spawn 1 enqueuer task
    {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(
            ShardPolicy::Sweep {
                initial_index: None,
            },
            async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            },
        );
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    rb.poison().await;

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(rb.is_empty(), true);

    assert_eq!(2 * MAX_ITEMS, items_taken);
}

#[tokio::test(flavor = "current_thread")]
async fn test_spmc_tasks() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

    assert_eq!(rb.is_empty(), true);

    let mut deq_threads = Vec::new();
    let mut enq_threads = Vec::new();

    // Spawn MAX_TASKS dequeuer tasks
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                let rb = rb.clone();
                let mut counter: usize = 0;
                loop {
                    let item = rb.dequeue().await;
                    match item {
                        Some(_) => counter += 1,
                        None => break,
                    }
                }
                counter
            },
        );
        deq_threads.push(handler);
    }

    // Spawn 1 enqueuer task
    {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(
            ShardPolicy::Sweep {
                initial_index: None,
            },
            async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            },
        );
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    // guarantees that the dequeuer finish remaining jobs in the buffer
    // before exiting
    rb.poison().await;

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(rb.is_empty(), true);

    assert_eq!(2 * MAX_ITEMS, items_taken);
}

#[tokio::test(flavor = "current_thread")]
async fn test_mpsc_tasks() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

    assert_eq!(rb.is_empty(), true);

    let mut deq_threads = Vec::new();
    let mut enq_threads = Vec::new();

    // Spawn 1 dequeuer task
    {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(
            ShardPolicy::Sweep {
                initial_index: None,
            },
            async move {
                let rb = rb.clone();
                let mut counter: usize = 0;
                loop {
                    let item = rb.dequeue().await;
                    match item {
                        Some(_) => counter += 1,
                        None => break,
                    }
                }
                counter
            },
        );
        deq_threads.push(handler);
    }

    // Spawn multiple enqueuer tasks
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            },
        );
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    // guarantees that the dequeuer finish remaining jobs in the buffer
    // before exiting
    rb.poison().await;

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(rb.is_empty(), true);

    assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
}

#[tokio::test(flavor = "current_thread")]
async fn test_mpmc_tasks() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

    assert_eq!(rb.is_empty(), true);

    let mut deq_threads = Vec::new();
    let mut enq_threads = Vec::new();

    // Spawn MAX_TASKS dequeuer tasks
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                let rb = rb.clone();
                let mut counter: usize = 0;
                loop {
                    let item = rb.dequeue().await;
                    match item {
                        Some(_) => counter += 1,
                        None => break,
                    }
                }
                counter
            },
        );
        deq_threads.push(handler);
    }

    // Spawn MAX_TASKS enqueuer tasks
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            },
        );
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    // guarantees that the dequeuer finish remaining jobs in the buffer
    // before exiting
    rb.poison().await;

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(rb.is_empty(), true);

    assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
}

#[tokio::test(flavor = "current_thread")]
async fn test_random_and_sweep() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

    assert_eq!(rb.is_empty(), true);

    let mut deq_threads = Vec::new();
    let mut enq_threads = Vec::new();

    // Spawn MAX_TASKS dequeuer tasks
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(ShardPolicy::RandomAndSweep, async move {
            let rb = rb.clone();
            let mut counter: usize = 0;
            loop {
                let item = rb.dequeue().await;
                match item {
                    Some(_) => counter += 1,
                    None => break,
                }
            }
            counter
        });
        deq_threads.push(handler);
    }

    // Spawn MAX_TASKS enqueuer tasks
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(ShardPolicy::RandomAndSweep, async move {
            let rb = rb.clone();
            for _i in 0..2 * MAX_ITEMS {
                rb.enqueue(20).await;
            }
        });
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    // guarantees that the dequeuer finish remaining jobs in the buffer
    // before exiting
    rb.poison().await;

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(rb.is_empty(), true);

    assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
}

#[tokio::test(flavor = "current_thread")]
async fn test_full_clear_empty() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

    assert_eq!(rb.is_empty(), true);

    let mut enq_threads = Vec::new();

    // Spawn MAX_TASKS enqueuer tasks
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(ShardPolicy::RandomAndSweep, async move {
            let rb = rb.clone();
            for _ in 0..(MAX_ITEMS / MAX_TASKS) {
                rb.enqueue(20).await;
            }
        });
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    assert_eq!(rb.is_full(), true);

    rb.clear();

    assert_eq!(rb.is_empty(), true);
}
