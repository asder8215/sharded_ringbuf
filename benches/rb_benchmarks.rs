use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kanal::bounded_async;
use lf_shardedringbuf::{LFShardedRingBuf, spawn_buffer_task};
use lf_shardedringbuf::{ShardPolicy, TaskRole, spawn_assigner, terminate_assigner};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Barrier as AsyncBarrier;
use tokio::task;

// comparing the benchmarking to
// https://github.com/fereidani/rust-channel-benchmarks/tree/main?tab=readme-ov-file
// const MAX_SHARDS: [usize; 7] = [4, 8, 16, 32, 64, 128, 256];
const MAX_TASKS: usize = 1000;
const MAX_SHARDS: usize = 1000; 
// const MAX_TASKS: usize = 10;
// const MAX_THREADS: usize = MAX_TASKS;
const MAX_THREADS: usize = 16;
// const CAPACITY: usize = 1024;
const CAPACITY: usize = 1000000;
const ITEM_PER_TASK: usize = 250000;

async fn benchmark_kanal_async(c: usize) {
    let (s, r) = bounded_async::<usize>(c);
    let s = Arc::new(s);
    let r = Arc::new(r);
    let mut handles = Vec::new();

    for _ in 0..MAX_TASKS {
        let rx = r.clone();
        handles.push(task::spawn(async move {
            for _ in 0..ITEM_PER_TASK {
                rx.recv().await.unwrap();
            }
        }));
    }

    for _ in 0..MAX_TASKS {
        let tx = s.clone();
        handles.push(task::spawn(async move {
            for i in 0..ITEM_PER_TASK {
                tx.send(i).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

async fn benchmark_kanal_async_barrier(c: usize) {
    let (s, r) = bounded_async::<usize>(c);
    let s = Arc::new(s);
    let r = Arc::new(r);

    let barrier = Arc::new(AsyncBarrier::new(MAX_TASKS * 2));

    let mut handles = Vec::new();

    for _ in 0..MAX_TASKS {
        let rx = r.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(task::spawn(async move {
            barrier.wait().await;
            for _ in 0..ITEM_PER_TASK {
                rx.recv().await.unwrap();
            }
        }));
    }

    for _ in 0..MAX_TASKS {
        let tx = s.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(task::spawn(async move {
            barrier.wait().await;
            for i in 0..ITEM_PER_TASK {
                tx.send(i).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

async fn benchmark_lfsrb(capacity: usize, shards: usize) {
    let max_items: usize = capacity;

    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, shards));

    let mut deq_threads = Vec::with_capacity(MAX_TASKS);
    let mut enq_threads = Vec::with_capacity(MAX_TASKS);

    let assigner = spawn_assigner(rb.clone());

    // spawn enq tasks with shift by policy
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(
            ShardPolicy::CFT {
                role: TaskRole::Enqueue,
                buffer: rb.clone(),
            },
            async move {
                let rb = rb.clone();
                for i in 0..ITEM_PER_TASK {
                    rb.enqueue(i).await;
                }
            },
        );
        enq_threads.push(enq_handler);
    }

    // spawn deq tasks with shift by policy
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(
            ShardPolicy::CFT {
                role: TaskRole::Dequeue,
                buffer: rb.clone(),
            },
            async move {
                let mut counter: usize = 0;
                for _i in 0..ITEM_PER_TASK {
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

    // Wait for enqueuers
    for enq in enq_threads {
        enq.await.unwrap();
        // println!("I'm done as enqueuer");
    }

    // Wait for dequeuers
    for deq in deq_threads {
        deq.await.unwrap();
        // println!("I'm done as dequeuer");
    }

    terminate_assigner(rb.clone());
    // println!("Assigner has been signaled termination");

    let _ = assigner.await;
    // println!("Assigner has been terminated");

}

async fn benchmark_lfsrb_shiftby(capacity: usize, shards: usize) {
    let max_items: usize = capacity;

    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, shards));

    let mut deq_threads = Vec::with_capacity(MAX_TASKS);
    let mut enq_threads = Vec::with_capacity(MAX_TASKS);

    // spawn enq tasks with shift by policy
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler: tokio::task::JoinHandle<()> = spawn_buffer_task::<_, (), usize>(
            ShardPolicy::ShiftBy {
                initial_index: None,
                shift: MAX_TASKS,
            },
            async move {
                for i in 0..ITEM_PER_TASK {
                    rb.enqueue(i).await;
                }
            },
        );
        enq_threads.push(handler);
    }

    // spawn deq tasks with shift by policy
    for _ in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler: tokio::task::JoinHandle<usize> = spawn_buffer_task::<_, usize, usize>(
            ShardPolicy::ShiftBy {
                initial_index: None,
                shift: MAX_TASKS,
            },
            async move {
                let mut counter: usize = 0;
                for _i in 0..ITEM_PER_TASK {
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

    // Wait for enqueuers
    for enq in enq_threads {
        enq.await.unwrap();
    }

    // Wait for dequeuers
    for deq in deq_threads {
        deq.await.unwrap();
    }
}

// async fn benchmark_lfsrb(capacity: usize, shards: usize) {
//     let max_items: usize = capacity;

//     let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, shards));

//     let mut deq_threads = Vec::with_capacity(MAX_TASKS);
//     let mut enq_threads = Vec::with_capacity(MAX_TASKS);

//     // spawn enq tasks with shift by policy
//     for _ in 0..MAX_TASKS {
//         let rb = Arc::clone(&rb);
//         let handler: tokio::task::JoinHandle<()> = spawn_buffer_task::<_, (), usize>(
//             ShardPolicy::ShiftBy {
//                 initial_index: None,
//                 shift: MAX_TASKS,
//             },
//             async move {
//                 for i in 0..ITEM_PER_TASK {
//                     rb.enqueue(i).await;
//                 }
//             },
//         );
//         enq_threads.push(handler);
//     }

//     // spawn deq tasks with shift by policy
//     for _ in 0..MAX_TASKS {
//         let rb = Arc::clone(&rb);
//         let handler: tokio::task::JoinHandle<usize> = spawn_buffer_task(
//             ShardPolicy::ShiftBy {
//                 initial_index: None,
//                 shift: MAX_TASKS,
//             },
//             async move {
//                 let mut counter: usize = 0;
//                 for _i in 0..ITEM_PER_TASK {
//                     let item = rb.dequeue().await;
//                     match item {
//                         Some(_) => counter += 1,
//                         None => break,
//                     }
//                 }
//                 counter
//             },
//         );
//         deq_threads.push(handler);
//     }

//     // Wait for enqueuers
//     for enq in enq_threads {
//         enq.await.unwrap();
//     }

//     // Wait for dequeuers
//     for deq in deq_threads {
//         deq.await.unwrap();
//     }
// }

// async fn benchmark_lfsrb_barrier(capacity: usize, shards: usize) {
//     let max_items: usize = capacity;

//     let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, shards));

//     // barrier used to make sure all tasks are operating at the same time
//     // so that threads are all assigned a task at once
//     let barrier = Arc::new(AsyncBarrier::new(MAX_TASKS * 2));

//     let mut deq_threads = Vec::with_capacity(MAX_TASKS);
//     let mut enq_threads = Vec::with_capacity(MAX_TASKS);

//     // spawn enq tasks with shift by policy
//     for _ in 0..MAX_TASKS {
//         let rb = Arc::clone(&rb);
//         let barrier = Arc::clone(&barrier);
//         let handler: tokio::task::JoinHandle<()> = spawn_buffer_task(
//             ShardPolicy::ShiftBy {
//                 initial_index: None,
//                 shift: MAX_TASKS,
//             },
//             async move {
//                 barrier.wait().await;
//                 for i in 0..ITEM_PER_TASK {
//                     rb.enqueue(i).await;
//                 }
//             },
//         );
//         enq_threads.push(handler);
//     }

//     // spawn deq tasks with shift by policy
//     for _ in 0..MAX_TASKS {
//         let rb = Arc::clone(&rb);
//         let barrier = Arc::clone(&barrier);
//         let handler: tokio::task::JoinHandle<usize> = spawn_buffer_task(
//             ShardPolicy::ShiftBy {
//                 initial_index: None,
//                 shift: MAX_TASKS,
//             },
//             async move {
//                 barrier.wait().await;
//                 let mut counter: usize = 0;
//                 for _i in 0..ITEM_PER_TASK {
//                     let item = rb.dequeue().await;
//                     match item {
//                         Some(_) => counter += 1,
//                         None => break,
//                     }
//                 }
//                 counter
//             },
//         );
//         deq_threads.push(handler);
//     }

//     // Wait for enqueuers
//     for enq in enq_threads {
//         enq.await.unwrap();
//     }

//     // Wait for dequeuers
//     for deq in deq_threads {
//         deq.await.unwrap();
//     }
// }

fn benchmark_multithreaded(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(MAX_THREADS)
        .build()
        .unwrap();

    // c.bench_with_input(
    //     BenchmarkId::new("kanal_async", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_kanal_async(s).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    c.bench_with_input(
        BenchmarkId::new("1000shard_buffer", CAPACITY),
        &CAPACITY,
        |b, &s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _i in 0..iters {
                    // println!("Completed an iter!");
                    let start = Instant::now();
                    benchmark_lfsrb(s, MAX_SHARDS).await;
                    let end = Instant::now();
                    total += end - start;
                }

                total
            });
        },
    );

    // c.bench_with_input(
    //     BenchmarkId::new("4shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[0]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("8shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[1]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("16shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[2]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("32shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[3]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("64shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[4]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("SB4shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_shiftby(s, MAX_SHARDS[0]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("SB8shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_shiftby(s, MAX_SHARDS[1]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("SB16shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_shiftby(s, MAX_SHARDS[2]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("SB32shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_shiftby(s, MAX_SHARDS[3]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("SB64shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_shiftby(s, MAX_SHARDS[4]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("128shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[5]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("256shard_buffer", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb(s, MAX_SHARDS[6]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("kanal_async_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_kanal_async_barrier(s).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("4shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[0]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("8shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[1]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("16shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[2]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("32shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[3]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("64shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[4]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("128shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[5]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );

    // c.bench_with_input(
    //     BenchmarkId::new("256shard_buffer_barrier", CAPACITY),
    //     &CAPACITY,
    //     |b, &s| {
    //         // Insert a call to `to_async` to convert the bencher to async mode.
    //         // The timing loops are the same as with the normal bencher.
    //         b.to_async(&runtime).iter_custom(|iters| async move {
    //             let mut total = Duration::ZERO;
    //             for _i in 0..iters {
    //                 let start = Instant::now();
    //                 benchmark_lfsrb_barrier(s, MAX_SHARDS[6]).await;
    //                 let end = Instant::now();
    //                 total += end - start;
    //             }

    //             total
    //         });
    //     },
    // );
}

// fn benchmark_single_threaded(c: &mut Criterion) {
//     let runtime = tokio::runtime::Builder::new_multi_thread()
//         .enable_all()
//         .worker_threads(1)
//         .build()
//         .unwrap();

//     c.bench_with_input(
//         BenchmarkId::new("kanal_async", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_kanal_async(s).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("1shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, 1).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("4shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[0]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("8shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[1]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("16shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[2]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("32shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[3]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("64shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[4]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("128shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[5]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("256shard_buffer", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb(s, MAX_SHARDS[6]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("kanal_async_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_kanal_async_barrier(s).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("1shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, 1).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("4shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[0]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("8shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[1]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("16shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[2]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("32shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[3]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("64shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[4]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("128shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[5]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );

//     c.bench_with_input(
//         BenchmarkId::new("256shard_buffer_barrier", CAPACITY),
//         &CAPACITY,
//         |b, &s| {
//             // Insert a call to `to_async` to convert the bencher to async mode.
//             // The timing loops are the same as with the normal bencher.
//             b.to_async(&runtime).iter_custom(|iters| async move {
//                 let mut total = Duration::ZERO;
//                 for _i in 0..iters {
//                     let start = Instant::now();
//                     benchmark_lfsrb_barrier(s, MAX_SHARDS[6]).await;
//                     let end = Instant::now();
//                     total += end - start;
//                 }

//                 total
//             });
//         },
//     );
// }

// criterion_group!(benches, benchmark_multithreaded, benchmark_single_threaded);
criterion_group!(benches, benchmark_multithreaded);
criterion_main!(benches);
