use lf_shardedringbuf::LFShardedRingBuf;
use lf_shardedringbuf::spawn_with_shard_index;
use std::sync::Arc;
use tokio::sync::Barrier as AsyncBarrier;

#[tokio::test]
// SPMC test case
async fn test_counter() {
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_THREADS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));
    let mut deq_threads = Vec::with_capacity(MAX_THREADS.try_into().unwrap());
    let mut enq_threads = Vec::new();

    // Spawn MAX_THREADS dequerer threads
    for _ in 0..MAX_THREADS {
        let rb = Arc::clone(&rb);
        let handler = spawn_with_shard_index(None, async move {
            let rb = rb.clone();
            let mut counter: usize = 0;
            loop {
                let item: Option<usize> = rb.dequeue().await;
                match item {
                    Some(_) => counter += 1,
                    None => break,
                }
            }
            counter
        });
        deq_threads.push(handler);
    }

    // Just spawn a single enquerer thread
    {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_with_shard_index(None, async move {
            let rb = rb.clone();
            for _ in 0..2 * MAX_ITEMS {
                rb.enqueue(20).await;
            }
        });
        enq_threads.push(enq_handler);
    }

    for enq in enq_threads {
        enq.await.unwrap();
    }

    for _ in 0..MAX_THREADS {
        rb.poison_deq().await;
    }

    let mut items_taken: usize = 0;
    while let Some(curr_thread) = deq_threads.pop() {
        items_taken += curr_thread.await.unwrap();
    }

    assert_eq!(200, items_taken);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
// MPMC test case
async fn benchmark_lock_free_sharded_buffer() {
    const MAX_SHARDS: usize = 100;
    const MAX_THREADS: usize = 8;
    const CAPACITY: usize = 100000;

    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(CAPACITY, MAX_SHARDS));

    // barrier used to make sure all threads are operating at the same time
    let barrier = Arc::new(AsyncBarrier::new(MAX_THREADS * 2));

    let mut deq_threads = Vec::with_capacity(MAX_THREADS);
    let mut enq_threads = Vec::with_capacity(MAX_THREADS);

    // spawn deq threads
    for _ in 0..MAX_THREADS {
        let rb = Arc::clone(&rb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<usize> = spawn_with_shard_index(None, async move {
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
        });
        deq_threads.push(handler);
    }

    // spawn enq threads
    for _ in 0..MAX_THREADS {
        let rb = Arc::clone(&rb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<()> = spawn_with_shard_index(None, async move {
            barrier.wait().await;
            for _i in 0..CAPACITY {
                rb.enqueue(20).await;
            }
        });
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

    assert_eq!(CAPACITY * MAX_THREADS, items_taken);
}
