use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use lf_shardedringbuf::{LFShardedRingBuf, spawn_with_shard_index};
use std::{
    i8, sync::{Arc, Barrier, Mutex}, thread, time::{Duration, Instant}
};
use tokio::sync::Barrier as AsyncBarrier;
use lf_shardedringbuf::ShardPolicy;

// comparing the benchmarking to
// https://github.com/fereidani/rust-channel-benchmarks/tree/main?tab=readme-ov-file
const MAX_SHARDS: [usize; 5] = [8, 16, 32, 64, 128];
const MAX_TASKS: usize = 4;
// const MAX_THREADS: usize = 16;
const MAX_THREADS: usize = 8;
// const CAPACITY: usize = (1 << 20) / (MAX_THREADS/2) * 2;
const CAPACITY: usize = 1000000;
// const ITEM_PER_TASK: usize = (1 << 20) / (MAX_THREADS/2);
const ITEM_PER_TASK: usize = 250000;

async fn benchmark_lock_free_sharded_buffer(capacity: usize, shards: usize) {
    let max_items: usize = capacity;

    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, shards));

    // barrier used to make sure all tasks are operating at the same time
    // so that threads are all assigned a task at once
    let barrier = Arc::new(AsyncBarrier::new(MAX_TASKS * 2));

    let mut deq_threads = Vec::with_capacity(MAX_TASKS);
    let mut enq_threads = Vec::with_capacity(MAX_TASKS);

    // spawn deq tasks
    // for _ in 0..MAX_TASKS {
    //     let rb = Arc::clone(&rb);
    //     let barrier = Arc::clone(&barrier);
    //     let handler: tokio::task::JoinHandle<usize> = spawn_with_shard_index(None, async move {
    //         barrier.wait().await;
    //         let mut counter: usize = 0;
    //         for _i in 0..ITEM_PER_TASK {
    //             let item = rb.dequeue().await;
    //             match item {
    //                 Some(_) => counter += 1,
    //                 None => break,
    //             }
    //         }
    //         counter
    //     });
    //     deq_threads.push(handler);
    // }

    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<usize> = spawn_with_shard_index(Some(i), ShardPolicy::ShiftBy, MAX_TASKS, async move {
            barrier.wait().await;
            let mut counter: usize = 0;
            for _i in 0..ITEM_PER_TASK {
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

    // spawn enq tasks
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let barrier = Arc::clone(&barrier);
        let handler: tokio::task::JoinHandle<()> = spawn_with_shard_index(Some(i), ShardPolicy::ShiftBy, MAX_TASKS,async move {
            barrier.wait().await;
            for i in 0..ITEM_PER_TASK {
                rb.enqueue(i).await;
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
        .enable_all()
        .worker_threads(MAX_THREADS)
        .build()
        .unwrap();

    c.bench_with_input(
        BenchmarkId::new("8shard_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    let start = Instant::now();
                    benchmark_lock_free_sharded_buffer(s, MAX_SHARDS[0]).await;
                    let end = Instant::now();
                    total += end - start;
                }

                total
            });
        },
    );

    c.bench_with_input(
        BenchmarkId::new("16shard_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    let start = Instant::now();
                    benchmark_lock_free_sharded_buffer(s, MAX_SHARDS[1]).await;
                    let end = Instant::now();
                    total += end - start;
                }

                total
            });
        },
    );

    c.bench_with_input(
        BenchmarkId::new("32shard_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    let start = Instant::now();
                    benchmark_lock_free_sharded_buffer(s, MAX_SHARDS[2]).await;
                    let end = Instant::now();
                    total += end - start;
                }

                total
            });
        },
    );

    c.bench_with_input(
        BenchmarkId::new("64shard_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    let start = Instant::now();
                    benchmark_lock_free_sharded_buffer(s, MAX_SHARDS[3]).await;
                    let end = Instant::now();
                    total += end - start;
                }

                total
            });
        },
    );

    c.bench_with_input(
        BenchmarkId::new("128shard_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    let start = Instant::now();
                    benchmark_lock_free_sharded_buffer(s, MAX_SHARDS[4]).await;
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
