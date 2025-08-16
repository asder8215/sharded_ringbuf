use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sharded_ringbuf::{ShardPolicy, spawn_dequeuer_full_unbounded};
use sharded_ringbuf::{ShardedRingBuf, spawn_dequeuer_unbounded, spawn_enqueuer_with_iterator};
use std::sync::Arc;

// fn test_add(x: usize) -> usize {
//     let mut y = x;
//     y = y.wrapping_mul(31);
//     y = y.rotate_left(7);
//     y = y.wrapping_add(1);
//     y
// }

async fn lfsrb_pin(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(ShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with pin policy
    for i in 0..task_count {
        let handle = spawn_enqueuer_with_iterator(
            rb.clone(),
            ShardPolicy::Pin { initial_index: i },
            0..=1000000,
        );
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle =
            spawn_dequeuer_unbounded(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
                // test_add(x);
            });
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

async fn lfsrb_pin_deq_full(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(ShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with pin policy
    for i in 0..task_count {
        let handle = spawn_enqueuer_with_iterator(
            rb.clone(),
            ShardPolicy::Pin { initial_index: i },
            // 0..=1000000,
            0..=250000,
        );
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle =
            spawn_dequeuer_full_unbounded(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
                // test_add(x);
            });
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

fn benchmark_pin(c: &mut Criterion) {
    // const MAX_THREADS: [usize; 2] = [4, 8];
    // const CAPACITY: usize = 1024;
    // const SHARDS: [usize; 5] = [1, 2, 4, 8, 16];
    // const TASKS: [usize; 5] = [1, 2, 4, 8, 16];

    const MAX_THREADS: [usize; 1] = [8];
    const CAPACITY: usize = 1024;
    const SHARDS: [usize; 1] = [8];
    const TASKS: [usize; 1] = [1000];
    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for shard_num in SHARDS {
    //         for task_count in TASKS {
    //             let func_name = format!(
    //                 "Pin: {} threads, {} shards, {} enq tasks enqueuing 1 million items, {} looping deq task",
    //                 thread_num, shard_num, task_count, shard_num
    //             );

    //             c.bench_with_input(
    //                 BenchmarkId::new(func_name, CAPACITY * shard_num),
    //                 &(CAPACITY * shard_num),
    //                 |b, &cap| {
    //                     // Insert a call to `to_async` to convert the bencher to async mode.
    //                     // The timing loops are the same as with the normal bencher.
    //                     b.to_async(&runtime).iter(async || {
    //                         lfsrb_pin(cap, shard_num, task_count).await;
    //                     });
    //                 },
    //             );
    //         }
    //     }
    // }

    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        for shard_num in SHARDS {
            for task_count in TASKS {
                let func_name = format!(
                    "Pin: {} threads, {} shards, {} enq tasks enqueuing 1 million items, {} looping deq full task",
                    thread_num, shard_num, task_count, shard_num
                );

                // c.bench_with_input(
                //     BenchmarkId::new(func_name, CAPACITY * shard_num),
                //     &(CAPACITY * shard_num),
                //     |b, &cap| {
                //         // Insert a call to `to_async` to convert the bencher to async mode.
                //         // The timing loops are the same as with the normal bencher.
                //         b.to_async(&runtime).iter(async || {
                //             lfsrb_pin_deq_full(cap, shard_num, task_count).await;
                //         });
                //     },
                // );

                c.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY),
                    &(CAPACITY),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            lfsrb_pin_deq_full(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }
}

criterion_group!(benches, benchmark_pin);
criterion_main!(benches);
