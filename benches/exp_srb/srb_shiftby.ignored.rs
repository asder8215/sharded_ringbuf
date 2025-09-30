#![cfg(feature = "exp_srb")]
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
#[allow(deprecated)]
use sharded_ringbuf::exp_srb::{ExpShardedRingBuf, spawn_dequeuer_unbounded, spawn_enqueuer_with_iterator};
#[allow(deprecated)]
use sharded_ringbuf::exp_srb::{ShardPolicy, spawn_dequeuer_full_unbounded};
use std::sync::Arc;

fn test_add(x: usize) -> usize {
    let mut y = x;
    y = y.wrapping_mul(31);
    y = y.rotate_left(7);
    y = y.wrapping_add(1);
    y
}

#[allow(deprecated)]
async fn lfsrb_shiftby(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(ExpShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with shift by policy
    for i in 0..task_count {
        let handle = spawn_enqueuer_with_iterator(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: task_count,
            },
            0..=1000000,
        );
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle = spawn_dequeuer_unbounded(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: shards,
            },
            |x| {
                test_add(x);
            },
        );
        deq_tasks.push(handle);
    }

    // Wait for enqueuers
    for enq in enq_tasks {
        enq.await.unwrap();
    }

    rb.poison();

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }
}

#[allow(deprecated)]
async fn lfsrb_shiftby_deq_full(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(ExpShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with shift by policy
    for i in 0..task_count {
        let handle = spawn_enqueuer_with_iterator(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: task_count,
            },
            0..=1000000,
        );
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle = spawn_dequeuer_full_unbounded(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: shards,
            },
            |x| {
                test_add(x);
            },
        );
        deq_tasks.push(handle);
    }

    // Wait for enqueuers
    for enq in enq_tasks {
        enq.await.unwrap();
    }

    rb.poison();

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }
}

fn benchmark_shiftby(c: &mut Criterion) {
    const MAX_THREADS: [usize; 2] = [4, 8];
    const CAPACITY: usize = 1024;
    const SHARDS: [usize; 5] = [1, 2, 4, 8, 16];
    const TASKS: [usize; 5] = [1, 2, 4, 8, 16];
    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        for shard_num in SHARDS {
            for task_count in TASKS {
                let func_name = format!(
                    "ShiftBy: {thread_num} threads, {shard_num} shards, {task_count} enq tasks enqueuing 1 million items, {shard_num} looping deq task"
                );

                c.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY * shard_num),
                    &(CAPACITY * shard_num),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            lfsrb_shiftby(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }

    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        for shard_num in SHARDS {
            for task_count in TASKS {
                let func_name = format!(
                    "ShiftBy: {thread_num} threads, {shard_num} shards, {task_count} enq tasks enqueuing 1 million items, {shard_num} looping deq full task"
                );

                c.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY * shard_num),
                    &(CAPACITY * shard_num),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            lfsrb_shiftby_deq_full(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }
}

criterion_group!(benches, benchmark_shiftby);
criterion_main!(benches);
