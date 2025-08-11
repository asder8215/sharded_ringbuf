[![GitHub Actions Workflow Status](https://img.shields.io/github/actions/workflow/status/asder8215/lf-shardedringbuf/rust.yml?style=for-the-badge&logo=github)](https://github.com/asder8215/lf-shardedringbuf)
[![Crates.io](https://img.shields.io/crates/v/lf_shardedringbuf.svg?style=for-the-badge&logo=docsdotrs)](https://crates.io/crates/lf-shardedringbuf)
[![Docs](https://img.shields.io/docsrs/lf-shardedringbuf?style=for-the-badge&logo=rust)](https://docs.rs/lf-shardedringbuf/latest/lf_shardedringbuf/)

# sharded_ringbuf
A Tokio async, sharded SPSC/MPSC/MPMC ring buffer in Rust.

# What's Different About This Buffer?
What this buffer does in particular is can allow for the usage of multiple simpler inner ring buffers (denoted as shards) each with capacity = requested capacity / requested # of shards (uneven shards are supported if capacity is not divisible by # of shards). It uses Tokio's task local variables as a shard index reference and to remember the Shard Acquisition policy strategy (i.e. Sweep, ShiftBy, RandomAndSweep, etc.) that the task is using to effectively acquire a shard to enqueue/dequeue on and as a way to inform when a task should context switch.

The essential goal of this buffer is to have a really effective MPMC queue underneath a Tokio async runtime, so that multiple threads can perform work independently in shards in a nonblocking manner. It can also be used in a SPSC/MPSC manner, but mostly with MPMC in mind.

# Example Usage
The following are examples of how to use ShardedRingBuf:

If enqueuer and dequeuer tasks are done with a limited number of enqueue/dequeue operations:
```rust
    let max_items: usize = 1024;
    let shards = 8;
    let task_count = 100;

    let rb = Arc::new(ShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with shift by policy
    for i in 0..task_count {
        let handle = spawn_enqueuer_with_iterator(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: task_count,
            },
            0..max_items,
        );
        enq_tasks.push(handle);
    }

    // spawn deq tasks with shift by policy
    for i in 0..task_count {
        let handle = spawn_dequeuer_bounded(
            rb.clone(),
            max_items,
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: shards,
            },
            |_| {
            // can put a function here optionally
               // if some work needs to be performed
               // on dequeued item
            },
        );
        deq_tasks.push(handle);
    }

    // Wait for enqueuers
    for enq in enq_tasks {
        enq.await.unwrap();
    }

    // Wait for dequeuers
    for deq in deq_tasks {
        deq.await.unwrap();
    }
```

If dequeuer_full tasks are performing in a loop and enqueuer task is performing with limited operations:
```rust
    let max_items: usize = 1024;
    let shards = 8;
    
    let rb = Arc::new(ShardedRingBuf::new(max_items, shards));

    let mut deq_tasks = Vec::with_capacity(shards);
    let mut enq_tasks = Vec::with_capacity(task_count);

    // spawn enq tasks with shift by policy
    for i in 0..task_count {
        let handle = spawn_enqueuer_with_iterator(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: task_count,
            },
            0..max_items,
        );
        enq_tasks.push(handle);
    }

    // spawn deq_full tasks with shift by policy
    for i in 0..shards {
        let handle = spawn_dequeuer_full_unbounded(
            rb.clone(),
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: shards,
            },
            |_| {
               // can put a function here optionally
               // if some work needs to be performed
               // on dequeued item 
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
```

You can also take a look at the `tests/` or `benches/` directory to see examples on how to use this structure. 

# Benchmark Results
Benchmark results and plots still needs to be collected thoroughly to see how this structure operates across different number of shards, threads, enqueuer/dequeuer tasks, and capacity.

# Future Additions/Thoughts
* Find a way to utilize Tokio's Notify or build a `Waker` to minimize the number of context switching performed within this structure.
* Maybe implement different 'smart tasks' (like CFT) to better support dynamic spawning and pairing of enqueuer/dequeuer tasks in an efficient manner.

# Contribution
All contributions (i.e. documentation, testing, providing feedback) are welcome! Check out [CONTRIBUTING.md](CONTRIBUTING.md) on how to start.

# License
This project is licensed under the [MIT License](LICENSE-MIT) and [Apache License](LICENSE-APACHE).
