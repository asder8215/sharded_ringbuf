use lf_shardedringbuf::LFShardedRingBuf;
use lf_shardedringbuf::ShardPolicy;
use lf_shardedringbuf::spawn_with_shard_index;
use std::sync::Arc;
use tokio::sync::Barrier as AsyncBarrier;

#[tokio::test(flavor = "current_thread")]
// Single threaded test case
async fn test_counter() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));
    let mut deq_threads = Vec::with_capacity(MAX_TASKS.try_into().unwrap());
    let mut enq_threads = Vec::new();

    // Spawn MAX_TASKS dequerer *tasks*
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler = spawn_with_shard_index(
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
                // println!("I finished dequeuing!");
                counter
            },
        );
        deq_threads.push(handler);
    }

    // Just spawn a single enquerer task
    {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_with_shard_index(
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

    // guarantees that the dequerer finish remaining jobs in the buffer
    // before exiting
    rb.poison().await;

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(2 * MAX_ITEMS, items_taken);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
// MPMC test case
async fn benchmark_lock_free_sharded_buffer() {
    const MAX_SHARDS: usize = 100;
    const MAX_TASKS: usize = 8;
    const CAPACITY: usize = 100000;

    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(CAPACITY, MAX_SHARDS));

    // barrier used to make sure all threads are operating at the same time
    let barrier = Arc::new(AsyncBarrier::new(MAX_TASKS * 2));

    let mut deq_threads = Vec::with_capacity(MAX_TASKS);
    let mut enq_threads = Vec::with_capacity(MAX_TASKS);

    // spawn deq tasks with shift by policy
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<usize> = spawn_with_shard_index(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                barrier.wait().await;
                let mut counter: usize = 0;
                for _i in 0..CAPACITY {
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

    // spawn enq tasks with shift by policy
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<()> = spawn_with_shard_index(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                barrier.wait().await;
                for i in 0..CAPACITY {
                    rb.enqueue(i).await;
                }
            },
        );
        enq_threads.push(handler);
    }

    // Wait for enqueuers
    for enq in enq_threads {
        enq.await.unwrap();
    }

    let mut items_taken: usize = 0;
    // Wait for dequeuers
    for deq in deq_threads {
        items_taken += deq.await.unwrap();
    }

    assert_eq!(CAPACITY * MAX_TASKS, items_taken);
}
