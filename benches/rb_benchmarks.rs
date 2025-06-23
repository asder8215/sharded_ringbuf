use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use lf_shardedringbuf::{LFShardedRingBuf, spawn_with_shard_index};
use std::{
    sync::{Arc, Barrier, Mutex},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::Barrier as AsyncBarrier;

const MAX_SHARDS: usize = 100;
const MAX_THREADS: usize = 8;
const CAPACITY: usize = 100000;

async fn benchmark_lock_free_sharded_buffer(capacity: usize) {
    let max_items: usize = capacity;

    let smtrb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, MAX_SHARDS));

    // barrier used to make sure all threads are operating at the same time
    let barrier = Arc::new(AsyncBarrier::new(MAX_THREADS * 2));

    let mut deq_threads = Vec::with_capacity(MAX_THREADS);
    let mut enq_threads = Vec::with_capacity(MAX_THREADS);

    // spawn deq threads
    for _ in 0..MAX_THREADS {
        let smtrb = Arc::clone(&smtrb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<usize> = spawn_with_shard_index(None, async move {
            barrier.wait().await;
            let mut counter: usize = 0;
            for _i in 0..max_items {
                let item = smtrb.dequeue().await;
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
        let smtrb = Arc::clone(&smtrb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<()> = spawn_with_shard_index(None, async move {
            barrier.wait().await;
            for _i in 0..max_items {
                smtrb.enqueue(20).await;
            }
        });
        enq_threads.push(handler);
    }

    // Wait for enqueuers
    for enq in enq_threads {
        enq.await.unwrap();
    }

    // Wait for dequeuers
    for deq in deq_threads {
        deq.await.unwrap();
    }
}

fn rb_benchmark(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        // .max_blocking_threads(MAX_THREADS * 2)
        .enable_all()
        .worker_threads(MAX_THREADS * 2)
        .build()
        .unwrap();

    c.bench_with_input(
        BenchmarkId::new("lock_free_sharded_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    let start = Instant::now();
                    benchmark_lock_free_sharded_buffer(s).await;
                    let end = Instant::now();
                    total += end - start;
                }

                total
            });
        },
    );
}

criterion_group!(benches, rb_benchmark);
criterion_main!(benches);
