use std::{thread::sleep, time::{Duration, Instant}};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kanal::bounded_async;
use tokio::task;

#[derive(Default, Debug, Clone, Copy)]
struct Message {
    // item_one: u128,
    item_one: usize,
    // item_two: u128,
    // item_three: u128,
    // item_four: u128,
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

async fn kanal_async(c: usize, task_count: usize) {
    let (s, r) = bounded_async(c);
    let mut handles = Vec::new();

    for _ in 0..1 {
        let rx = r.clone();
        handles.push(task::spawn(async move {
            for _ in 0..task_count {
                for _ in 0..1 {
                    let x: Message = rx.recv().await.unwrap();
                    test_func(x.item_one as u128);
                }
            }
        }));
    }

    for _ in 0..task_count {
        let tx = s.clone();
        let msg = Message::default();
        handles.push(task::spawn(async move {
            for i in 0..1 {
                tx.send(msg).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

async fn kanal_async_with_msg_vec(msg_vecs: Vec<Vec<BigData>>, c: usize, task_count: usize, msg_count: usize) {
    let (s, r) = bounded_async(c);
    let mut handles = Vec::new();

    for _ in 0..1 {
        let rx = r.clone();
        handles.push(task::spawn(async move {
            for _ in 0..task_count {
                for _ in 0..msg_count {
                    let x = rx.recv().await.unwrap();
                    // test_func(x.item_one as u128);
                }
            }
        }));
    }

    // for _ in 0..task_count {
    for msg_vec in msg_vecs {
        let tx = s.clone();
        // let msg = Message::default();
        handles.push(task::spawn(async move {
            for msg in msg_vec {
                tx.send(msg).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}


fn benchmark_kanal_async(c: &mut Criterion) {
    const MAX_THREADS: [usize; 1] = [8];
    const CAPACITY: usize = 1;
    const TASKS: [usize; 1] = [100000];
    const MSG_COUNT: usize = 5;
    let msg = BigData { buf: Box::new([0; 8]) };
    let mut msg_vecs = Vec::with_capacity(TASKS[0]);
    for _ in 0..TASKS[0] {
        msg_vecs.push(Vec::with_capacity(MSG_COUNT));
        let msg_vecs_len = msg_vecs.len();
        for _ in 0..MSG_COUNT {
            msg_vecs[msg_vecs_len - 1].push(msg.clone());
        }
    }



    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     for task_count in TASKS {
    //         let func_name = format!(
    //             "Kanal Async: {} threads, {} enq tasks enqueuing 1 million items, 1 looping deq task",
    //             thread_num, task_count
    //         );

    //         c.bench_with_input(
    //             BenchmarkId::new(func_name, CAPACITY),
    //             &(CAPACITY),
    //             |b, &cap| {
    //                 // Insert a call to `to_async` to convert the bencher to async mode.
    //                 // The timing loops are the same as with the normal bencher.
    //                 b.to_async(&runtime).iter(async || {
    //                     kanal_async(cap, task_count).await;
    //                 });
    //             },
    //         );
    //     }
    // }


    
    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        for task_count in TASKS {
            let func_name = format!(
                "Kanal Async: {} threads, {} enq tasks enqueuing 1 million items, 1 looping deq task",
                thread_num, task_count
            );

            c.bench_with_input(
                BenchmarkId::new(func_name, CAPACITY),
                &(CAPACITY),
                |b, &cap| {
                    // Insert a call to `to_async` to convert the bencher to async mode.
                    // The timing loops are the same as with the normal bencher.
                     b.to_async(&runtime).iter_custom(|iters| {
                        let msg_vecs = msg_vecs.clone();
                        async move {
                            let mut total = Duration::ZERO;
                            for _ in 0..iters {
                                let msg_vecs = msg_vecs.clone();
                                let start = Instant::now();
                                kanal_async_with_msg_vec(msg_vecs, cap, task_count, MSG_COUNT).await;
                                let end = Instant::now();
                                total += end - start;
                            }
                            total
                        }
                    });
                },
            );
        }
    }
}

criterion_group!(benches, benchmark_kanal_async);
criterion_main!(benches);
