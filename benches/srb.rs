#[allow(unused)]
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sharded_ringbuf::srb::ShardedRingBuf;
use tokio::spawn;
use tokio::task::yield_now;

#[derive(Default, Debug, Clone, Copy)]
#[allow(dead_code)]
struct Message {
    item_one: u128,
    item_two: u128,
    item_three: u128,
    item_four: u128,
    // item_five: u128,
    // item_six: u128,
    // item_seven: u128,
    // item_eight: u128,
    // item_nine: u128,
    // item_ten: u128,
    // item_eleven: u128,
    // item_twelve: u128,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BigData {
    // buf: Box<[u8; 1 * 1024]>, // 1 KiB
    buf: Box<[u8; 4]>, // 8 bytes
}

static FUNC_TO_TEST: i32 = 3;
#[allow(dead_code)]
fn test_func(x: u128) -> u128 {
    match FUNC_TO_TEST {
        0 => mult_add_ops(x),
        1 => fib(x),
        2 => prime_sieve(x),
        3 => mul_stress(x as usize),
        _ => {
            todo!()
        }
    }
}

fn mult_add_ops(x: u128) -> u128 {
    let mut y = x;
    y = y.wrapping_mul(31);
    y = y.rotate_left(7);
    y = y.wrapping_add(1);
    y
}

fn fib(x: u128) -> u128 {
    let mut a = 0u128;
    let mut b = 1u128;
    for _ in 0..x {
        let tmp = a + b;
        a = b;
        b = tmp;
    }
    a
}

fn prime_sieve(x: u128) -> u128 {
    let mut is_prime = vec![true; x as usize];
    let mut count = 0;

    for i in 2..x {
        if is_prime[i as usize] {
            count += 1;
            let mut j = i * 2;
            while j < x {
                is_prime[j as usize] = false;
                j += i;
            }
        }
    }

    count
}

fn mul_stress(iter: usize) -> u128 {
    let mut acc = 1u128;
    for i in 1..=iter as u128 {
        acc = acc.wrapping_mul(i ^ 0xdeadbeefdeadbeef);
    }
    acc
}

#[allow(dead_code)]
async fn srb_bench(capacity: usize, shards: usize, task_count: usize) {
    // println!("Hi");
    let max_items: usize = capacity;

    let rb = ShardedRingBuf::new(max_items, shards);

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    for i in 0..task_count {
        // println!("Hi");
        let items = 0 as i64..10000000;
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                let mut counter = 0;
                // let full_enq = rb_clone
                //     .get_shard_capacity(rb_clone.get_num_of_shards() - 1)
                //     .unwrap();
                let full_enq = 2000;
                let mut enq_vec = Vec::with_capacity(full_enq);
                for item in items {
                    if counter != 0 && counter % full_enq == 0 {
                        rb_clone.enqueue_full_in_shard(enq_vec, i).await;
                        enq_vec = Vec::with_capacity(full_enq);
                        enq_vec.push(item);
                        counter += 1;
                    } else {
                        enq_vec.push(item);
                        counter += 1;
                    }
                }
                if !enq_vec.is_empty() {
                    rb_clone.enqueue_full_in_shard(enq_vec, i).await;
                }
            }
        });
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    match rb_clone.dequeue_full_in_shard(i).await {
                        Some(items) => {
                            for _j in items {
                                // println!("{j}");
                            }
                        }
                        None => break,
                    }
                }
            }
        });
        deq_tasks.push(handle);
    }

    // Wait for enqueuers
    for enq in enq_tasks {
        enq.await.unwrap();
    }

    rb.poison();

    // necessary for poison
    let notifier_task = spawn({
        let rb_clone = rb.clone();
        async move {
            loop {
                for i in 0..shards {
                    rb_clone.notify_dequeuer_in_shard(i % rb_clone.get_num_of_shards());
                }
                yield_now().await;
            }
        }
    });

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }

    notifier_task.abort();
}

#[allow(unused)]
async fn srb_with_msg_vec(
    msg_vecs: Vec<Vec<BigData>>,
    capacity: usize,
    shards: usize,
    task_count: usize,
) {
    let max_items: usize = capacity;

    let rb = ShardedRingBuf::new(max_items, shards);

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // enqueuing batches of items per task
    for msg_vec in msg_vecs {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                rb_clone.enqueue_full(msg_vec).await;
            }
        });
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    match rb_clone.dequeue_full_in_shard(i).await {
                        Some(items) => for _i in items {},
                        None => break,
                    }
                }
            }
        });
        deq_tasks.push(handle);
    }

    // Wait for enqueuers to complete
    for enq in enq_tasks {
        enq.await.unwrap();
    }

    rb.poison();

    // necessary for poison
    let notifier_task = spawn({
        let rb_clone = rb.clone();
        async move {
            loop {
                for i in 0..shards {
                    rb_clone.notify_dequeuer_in_shard(i % rb_clone.get_num_of_shards());
                }
                yield_now().await;
            }
        }
    });

    // Wait for dequeuers to complete
    for deq in deq_tasks {
        deq.await.unwrap();
    }

    notifier_task.abort();
}

fn benchmark_srb(c: &mut Criterion) {
    // let plot_config = PlotConfiguration::default().summary_scale(AxisScale::Linear);
    // const MAX_THREADS: [usize; 2] = [4, 8];

    let mut group = c.benchmark_group("ShardedRingBuf");
    const MAX_THREADS: [usize; 1] = [2];
    // const CAPACITY: usize = 1024;
    const CAPACITY: usize = 32768;
    // const CAPACITY: usize = 200000;
    // const SHARDS: [usize; 5] = [1, 2, 4, 8, 16];
    // const TASKS: [usize; 5] = [1, 2, 4, 8, 16];
    const SHARDS: [usize; 1] = [1];
    const TASKS: [usize; 1] = [2];

    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        for shard_num in SHARDS {
            for task_count in TASKS {
                let func_name = format!(
                    "ShardedRingBuf"
                );

                group.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY),
                    &(CAPACITY),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            srb_bench(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }

    // const MSG_SIZES: [usize; 8] = [1, 2, 4, 8, 16, 32, 64, 128];
    // const MSG_SIZES: [usize; 1] = [10000];

    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for shard_num in SHARDS {
    //         for task_count in TASKS {
    //             for msg_size in MSG_SIZES {
    //                 let msg_count: usize = msg_size;
    //                 // let msg = BigData { buf: Box::new([0; 1 * 1024]) };
    //                 let msg = BigData {
    //                     buf: Box::new([0; 4]),
    //                 };
    //                 let mut msg_vecs = Vec::with_capacity(TASKS[0]);
    //                 for _ in 0..TASKS[0] {
    //                     msg_vecs.push(Vec::with_capacity(msg_count));
    //                     let msg_vecs_len = msg_vecs.len();
    //                     for _ in 0..msg_count {
    //                         msg_vecs[msg_vecs_len - 1].push(msg.clone());
    //                     }
    //                 }
    //                 // let func_name = format!(
    //                 //     "{} threads, {} shards, {} enq tasks each w/ {msg_count} 8 byte items",
    //                 //     thread_num, shard_num, task_count
    //                 // );
    //                 let func_name = format!("Batching {msg_count} 8-byte items");
    //                 group.bench_with_input(
    //                     BenchmarkId::new(func_name, msg_size),
    //                     &(CAPACITY),
    //                     |b, &cap| {
    //                         // Insert a call to `to_async` to convert the bencher to async mode.
    //                         // The timing loops are the same as with the normal bencher.
    //                         // let msg_vec_clone = msg_vecs.clone();
    //                         b.to_async(&runtime).iter_custom(|iters| {
    //                             let msg_vecs = msg_vecs.clone();
    //                             async move {
    //                                 let mut total = Duration::ZERO;

    //                                 for _i in 0..iters {
    //                                     let msg_vecs = msg_vecs.clone();
    //                                     let start = Instant::now();
    //                                     srb_with_msg_vec(msg_vecs, cap, shard_num, task_count)
    //                                         .await;
    //                                     let end = Instant::now();
    //                                     total += end - start;
    //                                 }
    //                                 total
    //                             }
    //                         });
    //                     },
    //                 );
    //             }
    //         }
    //     }
    // }
}

criterion_group!(benches, benchmark_srb);
criterion_main!(benches);
