use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

#[allow(dead_code)]
fn test_add(x: usize) -> usize {
    let mut y = x;
    y = y.wrapping_mul(31);
    y = y.rotate_left(7);
    y = y.wrapping_add(1);
    y
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BigData {
    // buf: Box<[u8; 1 * 1024]>, // 1 KiB
    buf: Box<[u8; 8]>, // 1 KiB
}

#[allow(dead_code, unused_variables)]
#[allow(dead_code)]
async fn crossbeam(
    msg_vecs: Vec<Vec<BigData>>,
    c: usize,
    task_count: usize,
    msg_count: usize,
) {
    // let (s, r) = bounded_async(c);
    // let queue = crossbeam::queue::ArrayQueue::new(c);
    // let mut handles = Vec::new();

    // for _ in 0..1 {
    //     let rx = r.clone();
    //     handles.push(task::spawn(async move {
    //         for _ in 0..task_count {
    //             for _ in 0..msg_count {
    //                 let _x = rx.recv().await.unwrap();
    //                 // test_func(x.item_one as u128);
    //             }
    //         }
    //     }));
    // }

    // // for _ in 0..task_count {
    // for msg_vec in msg_vecs {
    //     let tx = s.clone();
    //     // let msg = Message::default();
    //     handles.push(task::spawn(async move {
    //         for msg in msg_vec {
    //             tx.send(msg).await.unwrap();
    //         }
    //     }));
    // }

    // for handle in handles {
    //     handle.await.unwrap();
    // }
}

use crossbeam::channel::*;
use std::thread;

fn sync_crossbeam() {
    let buf_size = 32_768;
    let producer_msg_no = 10_000_000;
    let (s, r) = bounded(buf_size);
    let s2 = s.clone();

    // let start_time = Instant::now();
    // Producer 1
    let t1 = thread::spawn(move || {
        for _ in 0..producer_msg_no {
            s.send(1).unwrap();
        }
    });

    // Producer 2
    let t2 = thread::spawn(move || {
        for _ in 0..producer_msg_no {
            s2.send(1).unwrap();
        }
    });

    //Consumer
    // let mut sum = 0;
    for msg in r {
        let _tmp = msg;
        // sum += tmp;
    }

    let _ = t1.join();
    let _ = t2.join();

    // let d = Instant::now().duration_since(start_time);
    // let delta = d.as_millis();
    // println!("Sum: {}, processed  time: {}", sum, delta);
}

#[allow(dead_code, unused_variables)]
fn benchmark_crossbeam(c: &mut Criterion) {
    // const MAX_THREADS: [usize; 2] = [4, 8];
    // const CAPACITY: usize = 1024;
    // const TASKS: [usize; 5] = [1, 2, 4, 8, 16];
    // for thread_num in MAX_THREADS {
    //     let runtime = tokio::runtime::Builder::new_multi_thread()
    //         .enable_all()
    //         .worker_threads(thread_num)
    //         .build()
    //         .unwrap();

    //     todo!();
    // }
                c.bench_with_input(
                        BenchmarkId::new("Crossbeam", 0),
                        &(0),
                        |b, &cap| {
                            // Insert a call to `to_async` to convert the bencher to async mode.
                            // The timing loops are the same as with the normal bencher.
                            // let msg_vec_clone = msg_vecs.clone();
                            b.iter(|| sync_crossbeam())
                        },
                    );
}

criterion_group!(benches, benchmark_crossbeam);
criterion_main!(benches);
