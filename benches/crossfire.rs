use criterion::{Criterion, criterion_group, criterion_main};

#[allow(dead_code, unused_variables)]
fn test_add(x: usize) -> usize {
    let mut y = x;
    y = y.wrapping_mul(31);
    y = y.rotate_left(7);
    y = y.wrapping_add(1);
    y
}

#[allow(dead_code, unused_variables)]
async fn crossfire(capacity: usize, task_count: usize) {
    todo!()
}

#[allow(dead_code, unused_variables)]
fn benchmark_crossfire(c: &mut Criterion) {
    const MAX_THREADS: [usize; 2] = [4, 8];
    const CAPACITY: usize = 1024;
    const TASKS: [usize; 5] = [1, 2, 4, 8, 16];
    for thread_num in MAX_THREADS {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(thread_num)
            .build()
            .unwrap();

        todo!();
    }
}

criterion_group!(benches, benchmark_crossfire);
criterion_main!(benches);
