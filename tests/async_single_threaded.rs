use sharded_ringbuf::ExpShardedRingBuf;
use sharded_ringbuf::ShardPolicy;
use sharded_ringbuf::cft_spawn_dequeuer_bounded;
use sharded_ringbuf::cft_spawn_dequeuer_unbounded;
use sharded_ringbuf::cft_spawn_enqueuer_with_iterator;
use sharded_ringbuf::spawn_assigner;
use sharded_ringbuf::spawn_dequeuer_unbounded;
use sharded_ringbuf::spawn_enqueuer_with_iterator;
use sharded_ringbuf::terminate_assigner;
use std::sync::Arc;

#[tokio::test(flavor = "current_thread")]
async fn test_spsc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn 1 dequeuer task
        {
            let handler = spawn_dequeuer_unbounded(
                rb.clone(),
                ShardPolicy::Sweep {
                    initial_index: None,
                },
                |_| {},
            );
            deq_threads.push(handler);
        }

        // Spawn 1 enqueuer task
        {
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::Sweep {
                    initial_index: None,
                },
                0..2 * MAX_ITEMS,
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

        assert_eq!(2 * MAX_ITEMS, items_taken);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_spmc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn MAX_TASKS dequeuer tasks
        for i in 0..MAX_TASKS {
            let handler = spawn_dequeuer_unbounded(
                rb.clone(),
                ShardPolicy::ShiftBy {
                    initial_index: Some(i),
                    shift: MAX_TASKS,
                },
                |_| {},
            );
            deq_threads.push(handler);
        }

        // Spawn 1 enqueuer task
        {
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::Sweep {
                    initial_index: None,
                },
                0..2 * MAX_ITEMS,
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

#[tokio::test(flavor = "current_thread")]
async fn test_mpsc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn 1 dequeuer task
        {
            let handler = spawn_dequeuer_unbounded(
                rb.clone(),
                ShardPolicy::Sweep {
                    initial_index: None,
                },
                |_| {},
            );
            deq_threads.push(handler);
        }

        // Spawn multiple enqueuer tasks
        for i in 0..MAX_TASKS {
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::ShiftBy {
                    initial_index: Some(i),
                    shift: MAX_TASKS,
                },
                0..2 * MAX_ITEMS,
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

#[tokio::test(flavor = "current_thread")]
async fn test_mpmc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn MAX_TASKS dequeuer tasks
        for i in 0..MAX_TASKS {
            let handler = spawn_dequeuer_unbounded(
                rb.clone(),
                ShardPolicy::ShiftBy {
                    initial_index: Some(i),
                    shift: MAX_TASKS,
                },
                |_| {},
            );
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for i in 0..MAX_TASKS {
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::ShiftBy {
                    initial_index: Some(i),
                    shift: MAX_TASKS,
                },
                0..2 * MAX_ITEMS,
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

#[tokio::test(flavor = "current_thread")]
async fn test_random_and_sweep() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let handler = spawn_dequeuer_unbounded(rb.clone(), ShardPolicy::RandomAndSweep, |_| {});
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS {
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::RandomAndSweep,
                0..2 * MAX_ITEMS,
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

#[tokio::test(flavor = "current_thread")]
async fn test_full_clear_empty() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut enq_threads = Vec::new();

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS {
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::RandomAndSweep,
                0..(MAX_ITEMS / MAX_TASKS),
            );
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        assert!(rb.is_full());

        // No dequeuing has been done, so should be at 0 still
        // With a full buffer it means that all shard are back at
        // index 0
        // Init rb check
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        rb.clear();

        assert!(rb.is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_shiftby_uneven_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 100;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let handler = spawn_dequeuer_unbounded(
                rb.clone(),
                ShardPolicy::ShiftBy {
                    initial_index: None,
                    shift: MAX_TASKS,
                },
                |_| {},
            );
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS * 2 {
            let rb = Arc::clone(&rb);
            let enq_handler = spawn_enqueuer_with_iterator(
                rb.clone(),
                ShardPolicy::ShiftBy {
                    initial_index: None,
                    shift: MAX_TASKS * 2,
                },
                0..2 * MAX_ITEMS,
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

#[tokio::test(flavor = "current_thread")]
async fn test_cft_policy() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let handler = cft_spawn_dequeuer_unbounded(rb.clone(), |_| {});
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS {
            let enq_handler = cft_spawn_enqueuer_with_iterator(rb.clone(), 0..2 * MAX_ITEMS);
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

#[tokio::test(flavor = "current_thread")]
async fn test_cft_uneven_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const MAX_SHARDS: usize = 10;
        const MAX_TASKS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let handler = cft_spawn_dequeuer_unbounded(rb.clone(), |_| {});
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS * 2 {
            let enq_handler = cft_spawn_enqueuer_with_iterator(rb.clone(), 0..2 * MAX_ITEMS);
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

        assert_eq!(2 * MAX_ITEMS * MAX_TASKS * 2, items_taken);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_cft_more_tasks_less_shards() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const CAPACITY: usize = 512;
        const MAX_SHARDS: usize = 5;
        const MAX_TASKS: usize = 10;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(CAPACITY, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS {
            let handler = cft_spawn_dequeuer_unbounded(rb.clone(), |_| {});
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS {
            let rb = Arc::clone(&rb);
            let enq_handler = cft_spawn_enqueuer_with_iterator(rb.clone(), 0..2 * MAX_ITEMS);
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

#[tokio::test(flavor = "current_thread")]
async fn test_cft_more_deq_than_enq() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const CAPACITY: usize = 512;
        const MAX_SHARDS: usize = 5;
        const MAX_TASKS: usize = 10;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(CAPACITY, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..MAX_TASKS * 2 {
            let handler = cft_spawn_dequeuer_unbounded(rb.clone(), |_| {});
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for _ in 0..MAX_TASKS {
            let enq_handler = cft_spawn_enqueuer_with_iterator(rb.clone(), 0..2 * MAX_ITEMS);
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

#[tokio::test(flavor = "current_thread")]
async fn test_cft_more_deq_items_than_enq() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const CAPACITY: usize = 512;
        const MAX_SHARDS: usize = 5;
        let rb: Arc<ExpShardedRingBuf<usize>> =
            Arc::new(ExpShardedRingBuf::new(CAPACITY, MAX_SHARDS));

        assert!(rb.is_empty());

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        let assigner = spawn_assigner(rb.clone());

        // Spawn MAX_TASKS dequeuer tasks
        for _ in 0..2 {
            let handler = cft_spawn_dequeuer_bounded(rb.clone(), (3 * MAX_ITEMS) / 2, |_| {});
            deq_threads.push(handler);
        }

        // Spawn MAX_TASKS enqueuer tasks
        for i in 1..3 {
            let enq_handler = cft_spawn_enqueuer_with_iterator(rb.clone(), 0..MAX_ITEMS * i);
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
