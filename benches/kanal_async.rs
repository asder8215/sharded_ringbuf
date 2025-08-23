use std::{thread::sleep, time::Duration};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kanal::bounded_async;
use tokio::task;

fn test_add(x: usize) -> usize {
    let mut y = x;
    y = y.wrapping_mul(31);
    y = y.rotate_left(7);
    y = y.wrapping_add(1);
    sleep(Duration::from_nanos(10));
    y
}

async fn kanal_async(c: usize, task_count: usize) {
    let (s, r) = bounded_async(c);
    let mut handles = Vec::new();

    for _ in 0..1 {
        let rx = r.clone();
        handles.push(task::spawn(async move {
            for _ in 0..task_count {
                for _ in 0..10 {
                    let x = rx.recv().await.unwrap();
                    test_add(x);
                }
            }
        }));
    }

    for _ in 0..task_count {
        let tx = s.clone();
        handles.push(task::spawn(async move {
            for i in 0..1 {
                tx.send(i).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

fn benchmark_kanal_async(c: &mut Criterion) {
    const MAX_THREADS: [usize; 1] = [16];
    const CAPACITY: usize = 1024;
    const TASKS: [usize; 1] = [1000];
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
                    b.to_async(&runtime).iter(async || {
                        kanal_async(cap, task_count).await;
                    });
                },
            );
        }
    }
}

criterion_group!(benches, benchmark_kanal_async);
criterion_main!(benches);
