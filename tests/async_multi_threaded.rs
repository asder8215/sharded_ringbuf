use lf_shardedringbuf::LFShardedRingBuf;
use lf_shardedringbuf::ShardPolicy;
use lf_shardedringbuf::TaskRole;
use lf_shardedringbuf::spawn_assigner;
use lf_shardedringbuf::spawn_buffer_task;
use lf_shardedringbuf::spawn_with_cft;
use lf_shardedringbuf::terminate_assigner;
use std::sync::Arc;
use tokio::task::JoinHandle;

#[tokio::test(flavor = "multi_thread")]
async fn test_spsc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: u8 = 100;
        const CAP: usize = 100;
        const MAX_SHARDS: usize = 10;
        let rb  =
            Arc::new(LFShardedRingBuf::new(CAP, MAX_SHARDS));

        assert!(rb.is_empty());

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
                            Some(_) => {
                                counter += 1;
                                // println!("{:?}", item);
                            },
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
                    for i in 0..2 * MAX_ITEMS {
                        rb.enqueue(i).await;
                    }
                },
            );
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        assert!(rb.is_empty());

        assert_eq!(2 * CAP, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_spmc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

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
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mpsc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

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
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mpmc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

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
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_random_and_sweep() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

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
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_full_clear_empty() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

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

        assert!(rb.is_full());

        rb.clear();

        assert!(rb.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_shiftby_uneven_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100000;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();

        // let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let rb = Arc::clone(&rb);
            let handler = spawn_buffer_task(
                ShardPolicy::ShiftBy {
                    initial_index: None,
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
        for _ in 0..MAX_TASKS * 2 {
            let rb = Arc::clone(&rb);
            let enq_handler = spawn_buffer_task(
                ShardPolicy::ShiftBy {
                    initial_index: None,
                    shift: MAX_TASKS * 2,
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
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS * 2, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cft_policy() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100000;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let rb = Arc::clone(&rb);
            let handler =
                spawn_with_cft::<_, usize, usize>(TaskRole::Dequeue, rb.clone(), async move {
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
            let enq_handler =
                spawn_with_cft::<_, usize, ()>(TaskRole::Enqueue, rb.clone(), async move {
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
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        terminate_assigner(rb.clone());

        let _ = assigner.await;

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cft_uneven_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let rb = Arc::clone(&rb);
            let handler = spawn_with_cft(TaskRole::Dequeue, rb.clone(), async move {
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
        for _ in 0..MAX_TASKS * 2 {
            let rb = Arc::clone(&rb);
            let enq_handler = spawn_with_cft(TaskRole::Enqueue, rb.clone(), async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
            // println!("I'm done enqueueing!");
        }

        // guarantees that the dequeuer finish remaining jobs in the buffer
        // before exiting
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        terminate_assigner(rb.clone());

        let _ = assigner.await;

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS * 2, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cft_more_tasks_less_shards() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100000;
        const CAPACITY: usize = 1024;
        const MAX_SHARDS: usize = 5;
        const MAX_TASKS: usize = 10;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(CAPACITY, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let rb = Arc::clone(&rb);
            let handler = spawn_with_cft(TaskRole::Dequeue, rb.clone(), async move {
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
            let enq_handler = spawn_with_cft(TaskRole::Enqueue, rb.clone(), async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
            // println!("I'm done enqueueing!");
        }

        // guarantees that the dequeuer finish remaining jobs in the buffer
        // before exiting
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        terminate_assigner(rb.clone());

        let _ = assigner.await;

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cft_more_deq_than_enq() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100000;
        const CAPACITY: usize = 1024;
        const MAX_SHARDS: usize = 5;
        const MAX_TASKS: usize = 10;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(CAPACITY, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS * 2 {
            let rb = Arc::clone(&rb);
            let handler = spawn_with_cft(TaskRole::Dequeue, rb.clone(), async move {
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
            let enq_handler = spawn_with_cft(TaskRole::Enqueue, rb.clone(), async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
            // println!("I'm done enqueueing!");
        }

        // guarantees that the dequeuer finish remaining jobs in the buffer
        // before exiting
        rb.poison();

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        terminate_assigner(rb.clone());

        let _ = assigner.await;

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cft_more_deq_items_than_enq() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100000;
        const CAPACITY: usize = 1024;
        const MAX_SHARDS: usize = 5;
        let rb: Arc<LFShardedRingBuf<usize>> =
            Arc::new(LFShardedRingBuf::new(CAPACITY, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads: Vec<JoinHandle<()>> = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..2 {
            let rb = Arc::clone(&rb);
            let handler = spawn_with_cft(TaskRole::Dequeue, rb.clone(), async move {
                let rb = rb.clone();
                let mut counter: usize = 0;
                for _i in 0..(3 * MAX_ITEMS) / 2 {
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
        for i in 1..3 {
            let rb = Arc::clone(&rb);
            let enq_handler = spawn_with_cft(TaskRole::Enqueue, rb.clone(), async move {
                let rb = rb.clone();
                for i in 0..MAX_ITEMS * i {
                    rb.enqueue(i).await;
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        terminate_assigner(rb.clone());

        let _ = assigner.await;

        assert!(rb.is_empty());

        assert_eq!(3 * MAX_ITEMS, items_taken);
    }
}
