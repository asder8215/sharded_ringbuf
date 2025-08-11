use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use lf_shardedringbuf::{LFShardedRingBuf, spawn_bounded_enqueuer, spawn_unbounded_dequeuer};
use lf_shardedringbuf::{ShardPolicy, spawn_unbounded_dequeuer_full};
use std::sync::Arc;

fn test_add(x: usize) -> usize {
    let mut y = x;
    y = y.wrapping_mul(31);
    y = y.rotate_left(7);
    y = y.wrapping_add(1);
    y
}

async fn lfsrb_sweep(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(LFShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(1);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with shift by policy
    for _ in 0..task_count {
        let handle = spawn_bounded_enqueuer(
            rb.clone(),
            ShardPolicy::Sweep {
                initial_index: None,
            },
            (0..=1000000).collect::<Vec<_>>().into_iter(),
        );
        enq_tasks.push(handle);
    }

    for _ in 0..1 {
        let handle = spawn_unbounded_dequeuer(
            rb.clone(),
            ShardPolicy::Sweep {
                initial_index: None,
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

async fn lfsrb_sweep_deq_full(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(LFShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(1);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with shift by policy
    for _ in 0..task_count {
        let handle = spawn_bounded_enqueuer(
            rb.clone(),
            ShardPolicy::Sweep {
                initial_index: None,
            },
            (0..=1000000).collect::<Vec<_>>().into_iter(),
        );
        enq_tasks.push(handle);
    }

    for _ in 0..1 {
        let handle = spawn_unbounded_dequeuer_full(
            rb.clone(),
            ShardPolicy::Sweep {
                initial_index: None,
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
                    "Sweep: {} threads, {} shards, {} enq tasks enqueuing 1 million items, 1 looping deq task",
                    thread_num, shard_num, task_count
                );

                c.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY * shard_num),
                    &(CAPACITY * shard_num),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            lfsrb_sweep(cap, shard_num, task_count).await;
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
                    "Sweep: {} threads, {} shards, {} enq tasks enqueuing 1 million items, 1 looping deq full task",
                    thread_num, shard_num, task_count
                );

                c.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY * shard_num),
                    &(CAPACITY * shard_num),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            lfsrb_sweep_deq_full(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }
}

criterion_group!(benches, benchmark_shiftby);
criterion_main!(benches);
