use criterion::async_executor::AsyncExecutor;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sharded_ringbuf::{
    ExpShardedRingBuf, ShardedRingBuf, spawn_dequeuer_unbounded, spawn_enqueuer_with_iterator,
};
use sharded_ringbuf::{
    MLFShardedRingBuf, ShardPolicy, mlf_spawn_dequeuer_unbounded, mlf_spawn_enqueuer_with_iterator,
    spawn_dequeuer_full_unbounded, spawn_enqueuer_full_with_iterator,
};
use smol::future;
use std::future::Future;
use std::sync::Arc;
use tokio::spawn;
use tokio::task::yield_now;

#[derive(Default, Debug, Clone, Copy)]
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

struct BigData {
    // buf: Box<[u8; 1 * 1024]>, // 1 KiB
    buf: Box<[u8; 8]>, // 1 KiB
}

static FUNC_TO_TEST: i32 = 2;
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
    // thread::sleep(Duration::from_nanos(5));
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

async fn lfsrb_pin(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    // let rb = Arc::new(ExpShardedRingBuf::new(max_items, shards));
    let rb = ShardedRingBuf::new(max_items, shards);

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with pin policy
    // for i in 0..task_count {
    //     let handle = spawn_enqueuer_with_iterator(
    //         rb.clone(),
    //         ShardPolicy::Pin { initial_index: i },
    //         2..=5,
    //     );
    //     enq_tasks.push(handle);
    // }
    // for i in 0..task_count {
    //     let handle = spawn_enqueuer_with_iterator(
    //         rb.clone(),
    //         ShardPolicy::Pin { initial_index: i },
    //         0..250000,
    //     );
    //     enq_tasks.push(handle);
    // }

    // for i in 0..task_count {
    //     let handle = spawn_enqueuer_full_with_iterator(
    //         rb.clone(),
    //         ShardPolicy::Pin { initial_index: i },
    //         0..250000,
    //     );
    //     enq_tasks.push(handle);
    // }

    for i in 0..task_count {
        // let mut items = Vec::with_capacity(250000);
        // for j in 0..250000 {
        //     items.push(j);
        // }
        let items = 0..250000;
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                // for j in 0..rb_clone.get_shard_capacity(i % rb_clone.get_num_of_shards()).unwrap();
                //     rb.clone().enqueue_full(items).await
                // }
                let mut counter = 0;
                let full_enq = rb_clone
                    .get_shard_capacity(rb_clone.get_num_of_shards() - 1)
                    .unwrap();
                let mut enq_vec = Vec::with_capacity(full_enq);
                for item in items {
                    if counter != 0 && counter % full_enq == 0 {
                        // println!("Len of buffer is {}", enq_vec.len());
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
    // for i in 0..task_count {
    //     let handle =
    //         spawn_dequeuer_unbounded(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
    //             // test_func(x as u128);
    //         });
    //     deq_tasks.push(handle);
    // }

    // for i in 0..task_count {
    //     let handle =
    //         spawn_dequeuer_full_unbounded(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
    //             // test_func(x as u128);
    //         });
    //     deq_tasks.push(handle);
    // }
    // println!("Am I here?");
    for i in 0..shards {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    match rb_clone.dequeue_full_in_shard(i).await {
                        Some(items) => {
                            for i in items {
                                // test_func(i);
                                // println!("I is {}", i);
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

    // println!("Did it reach here?");
    // necessary for poison
    let notifier_task = spawn({
        let rb_clone = rb.clone();
        async move {
            // let rb = rb.clone();
            loop {
                for i in 0..shards {
                    // sleep(Duration::from_nanos(50));
                    rb_clone.notify_pin_shard(i % rb_clone.get_num_of_shards());
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

async fn mlfsrb_pin(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(MLFShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // let notifier_task = spawn({
    //     let rb_clone = rb.clone();
    //     async move {
    //         // let rb = rb.clone();
    //         loop {
    //             for i in 0..shards {
    //                 // sleep(Duration::from_nanos(50));
    //                 rb_clone.notify_pin_shard(i % rb_clone.get_num_of_shards());
    //             }
    //             yield_now().await;
    //         }
    //     }
    // });
    // spawn enq tasks with pin policy
    // for i in 0..task_count {
    //     let handle = spawn_enqueuer_with_iterator(
    //         rb.clone(),
    //         ShardPolicy::Pin { initial_index: i },
    //         2..=5,
    //     );
    //     enq_tasks.push(handle);
    // }
    // for i in 0..task_count {
    //     let handle = mlf_spawn_enqueuer_with_iterator(rb.clone(), i, 0..1);
    //     enq_tasks.push(handle);
    // }

    // for i in 0..shards {
    //     let handle = mlf_spawn_dequeuer_unbounded(rb.clone(), i, |x| {
    //         // test_func(x as u128);
    //         // println!("{:?}", x);
    //     });
    //     deq_tasks.push(handle);
    // }

    for i in 0..task_count {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                for j in 0..250000 {
                    rb_clone.enqueue(j).await;
                }
            }
        });
        enq_tasks.push(handle);
    }

    for i in 0..task_count {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    // let item = rb_clone.dequeue().await;
                    let item = rb_clone.dequeue_in_shard(i).await;
                    match item {
                        None => break,
                        Some(val) => {}
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

    for i in 0..task_count {
        {
            let rb = rb.clone();
            spawn(async move {
                rb.poison_at_shard(i % rb.get_num_of_shards()).await;
            })
        };
    }

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }

    // notifier_task.abort();
}

async fn lfsrb_pin_deq_full(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;

    let rb = Arc::new(ExpShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);
    // let mut notifier_task = Vec::new();

    let rb_clone = rb.clone();
    let notifier_task = spawn({
        let rb_clone = rb.clone();
        async move {
            // let rb = rb.clone();
            loop {
                for i in 0..shards {
                    // sleep(Duration::from_nanos(50));
                    rb_clone.notify_pin_shard(i % rb_clone.get_num_of_shards());
                }
                yield_now().await;
            }
        }
    });
    // let notifier_thread = thread::spawn(move || {
    //     // let message = "Hello from a standard thread!";
    //     // println!("{}", message);
    //     // // Can't use await here, use a regular send
    //     // tx.blocking_send(message).expect("Failed to send message");
    //     // let rb_clone = rb.clone();
    //     loop {
    //         if rb_clone.assigner_terminate.load(std::sync::atomic::Ordering::Relaxed) {
    //             break;
    //         }
    //         for i in 0..shards {
    //             rb_clone.notify_pin_shard(i % rb_clone.get_num_of_shards());
    //         }
    //         sleep(Duration::from_nanos(500));
    //     }
    // });

    // spawn enq tasks with pin policy
    for i in 0..task_count {
        // let msg = Message::default();
        // let msg = BigData { buf: Box::new([0; 1 * 1024]) };
        // let msg = BigData { buf: Box::new([0; 8]) };
        // let mut msg_vec = Vec::new();
        // for _ in 0..1 {
        //     msg_vec.push(msg.clone());
        // }
        let handle = spawn_enqueuer_with_iterator(
            // let handle = spawn_enqueuer(
            rb.clone(),
            ShardPolicy::Pin { initial_index: i },
            2..102,
            // Box::new(i),
        );
        enq_tasks.push(handle);
    }

    // for i in 0..task_count {
    //     let msg = Message::default();
    //     let mut msg_vec = Vec::new();
    //     for _ in 0..100 {
    //         msg_vec.push(msg);
    //     }
    //     let handle = spawn_enqueuer_full_with_iterator(
    //     // let handle = spawn_enqueuer(
    //         rb.clone(),
    //         ShardPolicy::Pin { initial_index: i },
    //         msg_vec,
    //         // Box::new(i),
    //     );
    //     enq_tasks.push(handle);
    // }

    for i in 0..shards {
        let handle =
            spawn_dequeuer_full_unbounded(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
                // let _ = test_func(x.item_one);
                // println!("X is {} with res is {}", x, res);
            });
        deq_tasks.push(handle);
    }

    // for i in 0..16 {
    //     let handle =
    //         spawn_dequeuer(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
    //             test_add(*x);
    //         });
    //     deq_tasks.push(handle);
    // }

    // Wait for enqueuers
    for enq in enq_tasks {
        enq.await.unwrap();
    }

    rb.poison();

    // for i in 0..shards {
    //     rb.notify_pin_shard(i % rb.get_num_of_shards());
    // }

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }

    notifier_task.abort();
    // terminate_assigner(rb.clone());
    // notifier_thread.join();
}

async fn lfsrb_pin_with_msg_vec(
    msg_vecs: Vec<Vec<BigData>>,
    capacity: usize,
    shards: usize,
    task_count: usize,
) {
    let max_items: usize = capacity;

    let rb = Arc::new(ExpShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);
    // let mut notifier_task = Vec::new();

    let rb_clone = rb.clone();
    let notifier_task = spawn({
        let rb_clone = rb.clone();
        async move {
            // let rb = rb.clone();
            loop {
                // println!("doing stuff");
                for i in 0..rb_clone.get_num_of_shards() {
                    // println!("doing stuff");
                    // sleep(Duration::from_nanos(50));
                    rb_clone.notify_pin_shard(i % rb_clone.get_num_of_shards());
                }
                yield_now().await;
            }
        }
    });
    // let notifier_thread = thread::spawn(move || {
    //     // let message = "Hello from a standard thread!";
    //     // println!("{}", message);
    //     // // Can't use await here, use a regular send
    //     // tx.blocking_send(message).expect("Failed to send message");
    //     // let rb_clone = rb.clone();
    //     loop {
    //         if rb_clone.assigner_terminate.load(std::sync::atomic::Ordering::Relaxed) {
    //             break;
    //         }
    //         for i in 0..shards {
    //             rb_clone.notify_pin_shard(i % rb_clone.get_num_of_shards());
    //         }
    //         sleep(Duration::from_nanos(500));
    //     }
    // });

    // spawn enq tasks with pin policy
    // for i in 0..task_count {
    let mut counter = 0;
    // for msg_vec in msg_vecs{
    //     // let msg = Message::default();
    //     // let msg = BigData { buf: Box::new([0; 128 * 1024]) };
    //     // let mut msg_vec = Vec::new();
    //     // for _ in 0..1 {
    //     //     msg_vec.push(msg.clone());
    //     // }
    //     let handle = spawn_enqueuer_with_iterator(
    //     // let handle = spawn_enqueuer(
    //         rb.clone(),
    //         ShardPolicy::Pin { initial_index: shard },
    //         msg_vec,
    //         // Box::new(i),
    //     );
    //     shard = shard.wrapping_add(1);
    //     enq_tasks.push(handle);
    // }

    for msg_vec in msg_vecs {
        // let msg = Message::default();
        // let msg = BigData { buf: Box::new([0; 128 * 1024]) };
        // let mut msg_vec = Vec::new();
        // for _ in 0..1 {
        //     msg_vec.push(msg.clone());
        // }
        let handle = spawn_enqueuer_full_with_iterator(
            // let handle = spawn_enqueuer(
            rb.clone(),
            ShardPolicy::Pin {
                initial_index: counter,
            },
            msg_vec,
            // Box::new(i),
        );
        counter = counter.wrapping_add(1);
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle =
            spawn_dequeuer_full_unbounded(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
                // let _ = test_func(x.item_one);
                // println!("X is {} with res is {}", x, res);
            });
        deq_tasks.push(handle);
    }

    // for i in 0..16 {
    //     let handle =
    //         spawn_dequeuer(rb.clone(), ShardPolicy::Pin { initial_index: i }, |x| {
    //             test_add(*x);
    //         });
    //     deq_tasks.push(handle);
    // }

    // Wait for enqueuers
    for enq in enq_tasks {
        // println!("here");
        enq.await.unwrap();
    }

    rb.poison();

    // for i in 0..shards {
    //     rb.notify_pin_shard(i % rb.get_num_of_shards());
    // }

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }

    notifier_task.abort();
    // terminate_assigner(rb.clone());
    // notifier_thread.join();
}

async fn mlfsrb_pin_with_msg_vec(
    msg_vecs: Vec<Vec<BigData>>,
    capacity: usize,
    shards: usize,
    task_count: usize,
) {
    let max_items: usize = capacity;

    let rb = Arc::new(MLFShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with pin policy
    let mut counter = 0;

    for msg_vec in msg_vecs {
        // let msg = Message::default();
        // let msg = BigData { buf: Box::new([0; 128 * 1024]) };
        // let mut msg_vec = Vec::new();
        // for _ in 0..1 {
        //     msg_vec.push(msg.clone());
        // }
        let handle = mlf_spawn_enqueuer_with_iterator(
            // let handle = spawn_enqueuer(
            rb.clone(),
            // ShardPolicy::Pin {
            //     initial_index: counter,
            // },
            counter,
            msg_vec,
            // Box::new(i),
        );
        counter = counter.wrapping_add(1);
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle = mlf_spawn_dequeuer_unbounded(rb.clone(), i, |x| {
            // let _ = test_func(x.item_one);
            // println!("X is {} with res is {}", x, res);
        });

        deq_tasks.push(handle);
    }

    // Wait for enqueuers
    for enq in enq_tasks {
        // println!("here");
        enq.await.unwrap();
    }

    for i in 0..shards {
        {
            let rb = rb.clone();
            spawn(async move {
                rb.poison_at_shard(i % rb.get_num_of_shards()).await;
            })
        };
    }

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }
}

fn benchmark_pin(c: &mut Criterion) {
    // const MAX_THREADS: [usize; 2] = [4, 8];
    const MAX_THREADS: [usize; 1] = [8];
    const CAPACITY: usize = 1024;
    // const CAPACITY: usize = 200000;
    // const SHARDS: [usize; 5] = [1, 2, 4, 8, 16];
    // const TASKS: [usize; 5] = [1, 2, 4, 8, 16];
    const SHARDS: [usize; 1] = [8];
    const TASKS: [usize; 1] = [100];

    // const MSG_COUNT: usize = 250000;
    // // let msg = BigData { buf: Box::new([0; 1 * 1024]) };
    // let msg = BigData {
    //     buf: Box::new([0; 8]),
    // };
    // let mut msg_vecs = Vec::with_capacity(TASKS[0]);
    // for _ in 0..TASKS[0] {
    //     msg_vecs.push(Vec::with_capacity(MSG_COUNT));
    //     let msg_vecs_len = msg_vecs.len();
    //     for _ in 0..MSG_COUNT {
    //         msg_vecs[msg_vecs_len - 1].push(msg.clone());
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
                    "Pin: {} threads, {} shards, {} enq tasks enqueuing 1 million items, {} looping deq task",
                    thread_num, shard_num, task_count, shard_num
                );

                c.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY),
                    &(CAPACITY),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            lfsrb_pin(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }

    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for shard_num in SHARDS {
    //         for task_count in TASKS {
    //             let func_name = format!(
    //                 "Pin: {} threads, {} shards, {} enq tasks enqueuing 1 million items, {} looping deq full task",
    //                 thread_num, shard_num, task_count, shard_num
    //             );

    //             c.bench_with_input(
    //                 BenchmarkId::new(func_name, CAPACITY),
    //                 &(CAPACITY),
    //                 |b, &cap| {
    //                     // Insert a call to `to_async` to convert the bencher to async mode.
    //                     // The timing loops are the same as with the normal bencher.
    //                     b.to_async(&runtime).iter(async || {
    //                         lfsrb_pin_deq_full(cap, shard_num, task_count).await;
    //                     });
    //                 },
    //             );
    //         }
    //     }
    // }

    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for shard_num in SHARDS {
    //         for task_count in TASKS {
    //             let func_name = format!(
    //                 "Pin: {} threads, {} shards, {} enq tasks enqueuing 1 million items, {} looping deq full task",
    //                 thread_num, shard_num, task_count, shard_num
    //             );
    //             c.bench_with_input(
    //                 BenchmarkId::new(func_name, CAPACITY),
    //                 &(CAPACITY),
    //                 |b, &cap| {
    //                     // Insert a call to `to_async` to convert the bencher to async mode.
    //                     // The timing loops are the same as with the normal bencher.
    //                     // let msg_vec_clone = msg_vecs.clone();
    //                     b.to_async(&runtime).iter_custom(|iters| {
    //                         let msg_vecs = msg_vecs.clone();
    //                         async move {
    //                         let mut total = Duration::ZERO;

    //                         for _i in 0..iters {
    //                             let msg_vecs = msg_vecs.clone();
    //                             let start = Instant::now();
    //                             lfsrb_pin_with_msg_vec(msg_vecs, cap, shard_num, task_count).await;
    //                             let end = Instant::now();
    //                             total += end - start;
    //                         }
    //                         total
    //                     }});
    //                 },
    //             );
    //         }
    //     }
    // }

    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for shard_num in SHARDS {
    //         for task_count in TASKS {
    //             let func_name = format!(
    //                 "Pin: {thread_num} threads, {shard_num} shards, {task_count} enq tasks enqueuing 1 million items, {shard_num} looping deq task"
    //             );

    //             c.bench_with_input(
    //                 BenchmarkId::new(func_name, CAPACITY),
    //                 &(CAPACITY),
    //                 |b, &cap| {
    //                     // Insert a call to `to_async` to convert the bencher to async mode.
    //                     // The timing loops are the same as with the normal bencher.
    //                     b.to_async(&runtime).iter(async || {
    //                         mlfsrb_pin(cap, shard_num, task_count).await;
    //                     });
    //                 },
    //             );
    //         }
    //     }
    // }

    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for shard_num in SHARDS {
    //         for task_count in TASKS {
    //             let func_name = format!(
    //                 "Pin: {} threads, {} shards, {} enq tasks enqueuing 1 million items, {} looping deq full task",
    //                 thread_num, shard_num, task_count, shard_num
    //             );
    //             c.bench_with_input(
    //                 BenchmarkId::new(func_name, CAPACITY),
    //                 &(CAPACITY),
    //                 |b, &cap| {
    //                     // Insert a call to `to_async` to convert the bencher to async mode.
    //                     // The timing loops are the same as with the normal bencher.
    //                     // let msg_vec_clone = msg_vecs.clone();
    //                     b.to_async(&runtime).iter_custom(|iters| {
    //                         let msg_vecs = msg_vecs.clone();
    //                         async move {
    //                             let mut total = Duration::ZERO;

    //                             for _i in 0..iters {
    //                                 let msg_vecs = msg_vecs.clone();
    //                                 let start = Instant::now();
    //                                 mlfsrb_pin_with_msg_vec(msg_vecs, cap, shard_num, task_count)
    //                                     .await;
    //                                 let end = Instant::now();
    //                                 total += end - start;
    //                             }
    //                             total
    //                         }
    //                     });
    //                 },
    //             );
    //         }
    //     }
    // }
}

criterion_group!(benches, benchmark_pin);
criterion_main!(benches);
