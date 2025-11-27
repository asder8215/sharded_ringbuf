// use std::sync::Arc;
// use std::sync::atomic::{AtomicUsize, Ordering};
#[allow(unused)]
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sharded_ringbuf::cs_srb::{self, CSShardedRingBuf};
use tokio::spawn;
use tokio::task::yield_now;

#[allow(dead_code)]
async fn cssrb_bench(capacity: usize, shards: usize, task_count: usize) {
    let max_items: usize = capacity;
    let rb = CSShardedRingBuf::new_with_enq_num(max_items, shards, task_count);

    // let counter = Arc::new(AtomicUsize::new(0));
    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    for _i in 0..task_count {
        let items = 0 as i64..10_000_000;
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            async move {
                let mut counter = 0;
                let full_enq = 2000;
                let mut enq_vec = Vec::with_capacity(full_enq);
                for item in items {
                    if counter != 0 && counter % full_enq == 0 {
                        // let guard = rb_clone.enqueue_shard_guard().await;
                        let guard = rb_clone.acquire_shard_guard(cs_srb::Acquire::Enqueue, _i).await;
                        match guard {
                            None => break,
                            Some(guard) => rb_clone.enqueue_item(enq_vec, guard),
                        }
                        enq_vec = Vec::with_capacity(full_enq);
                        enq_vec.push(item);
                        counter += 1;
                    } else {
                        enq_vec.push(item);
                        counter += 1;
                    }
                }
                if !enq_vec.is_empty() {
                    // let guard = rb_clone.enqueue_shard_guard().await;
                    let guard = rb_clone.acquire_shard_guard(cs_srb::Acquire::Enqueue, _i).await;
                    match guard {
                        None => {}
                        Some(guard) => rb_clone.enqueue_item(enq_vec, guard),
                    }
                }
            }
        });
        enq_tasks.push(handle);
    }

    for i in 0..shards {
        let handle = tokio::spawn({
            let rb_clone = rb.clone();
            // let counter_clone = counter.clone();
            async move {
                loop {
                    let guard = rb_clone
                        .acquire_shard_guard(cs_srb::Acquire::Dequeue, i)
                        .await;
                    // match rb_clone.dequeue_full_in_shard(i).await {
                    match guard {
                        Some(guard) => {
                            for _j in rb_clone.dequeue_item(guard) {
                                // println!("{j}");
                                // counter_clone.fetch_add(1, Ordering::Relaxed);
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

    // println!("{}", counter.load(Ordering::Relaxed));
    notifier_task.abort();
}

fn benchmark_cssrb(c: &mut Criterion) {
    // let plot_config = PlotConfiguration::default().summary_scale(AxisScale::Linear);
    // const MAX_THREADS: [usize; 2] = [4, 8];

    let mut group = c.benchmark_group("ShardedRingBuf");
    const MAX_THREADS: [usize; 1] = [8];
    const CAPACITY: usize = 8;
    const SHARDS: [usize; 1] = [8];
    const TASKS: [usize; 1] = [8];

    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        for shard_num in SHARDS {
            for task_count in TASKS {
                let func_name = format!("ShardedRingBuf");

                group.bench_with_input(
                    BenchmarkId::new(func_name, CAPACITY),
                    &(CAPACITY),
                    |b, &cap| {
                        // Insert a call to `to_async` to convert the bencher to async mode.
                        // The timing loops are the same as with the normal bencher.
                        b.to_async(&runtime).iter(async || {
                            cssrb_bench(cap, shard_num, task_count).await;
                        });
                    },
                );
            }
        }
    }
}

criterion_group!(benches, benchmark_cssrb);
criterion_main!(benches);
