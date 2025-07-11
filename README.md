# lf-shardedringbuf
An async, lock-free, sharded, cache-aware SPSC/MPSC/MPMC ring buffer in Rust.

# Features
* It uses multiple smaller simple ring buffers (shards) each with capacity = requested capacity / # of shards. Note if the requested capacity is not divisible by the number of shards, then the last shard will have an uneven load compared to the rest.
* It is lock-free; only uses atomic primitives and no mutexes or rwlocks.
* False sharing is avoided through cache padding the shard atomic locks + shards.
* It uses tokio's task local variables as a shard index reference and to remember the shard acquistion policy strategy to take for tasks to effectively acquire a shard to enqueue/dequeue on.
* Different shard acquisition policies are provided: `Sweep`, `RandomAndSweep`, and `ShiftBy` (see `src/shard_policies.rs` for more info) 
* Task backoffs/yielding are done based on shard acquisition policies; this is to encourage threads to always be performing on work with different tasks (plus, `tokio::time::sleep()` works on a millisecond granularity, which is unoptimal for this buffer)
* It can perform in an async multithreaded or async single threaded environment (optimal for multiple producer task, multiple consumer task situation)

# Example Usage
The following are examples of how to use LFShardedRingBuf:

If enqueuer and dequeuer tasks are done with a limited number of enqueue/dequeue operations:
```rust
    let max_items = 1024;
    let shards = 8;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(max_items, shards));

    let mut deq_threads = Vec::with_capacity(MAX_TASKS);
    let mut enq_threads = Vec::with_capacity(MAX_TASKS);

    // Spawn enqueuer tasks with ShiftBy policy
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler: tokio::task::JoinHandle<()> = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
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

    // Spawn dequeuer tasks with ShiftBy policy
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler: tokio::task::JoinHandle<usize> = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
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
```

If dequeuer tasks are performing in a loop and enqueuer task is performing with limited operations:
```rust
    const MAX_ITEMS: usize = 100;
    const MAX_SHARDS: usize = 10;
    const MAX_TASKS: usize = 5;
    let rb: Arc<LFShardedRingBuf<usize>> = Arc::new(LFShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS));
    let mut deq_threads = Vec::with_capacity(MAX_TASKS.try_into().unwrap());
    let mut enq_threads = Vec::new();

    // Spawn MAX_TASKS dequeuer tasks
    for i in 0..MAX_TASKS {
        let rb = Arc::clone(&rb);
        let handler = spawn_buffer_task(
            ShardPolicy::ShiftBy {
                initial_index: Some(i),
                shift: MAX_TASKS,
            },
            async move {
                let rb = rb.clone();
                let mut counter: usize = 0;
                loop {
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

    // Spawn an enqueuer task with Sweep policy
    {
        let rb = Arc::clone(&rb);
        let enq_handler = spawn_buffer_task(
            ShardPolicy::Sweep {
                initial_index: None,
            },
            async move {
                let rb = rb.clone();
                for _i in 0..2 * MAX_ITEMS {
                    rb.enqueue(20).await;
                }
            },
        );
        enq_threads.push(enq_handler);
    }

    // Wait for enqueuer tasks to complete first
    for enq in enq_threads {
        enq.await.unwrap();
    }

    // Poison for dequeuer tasks to exit gracefully, completing any remaining jobs
    // on the buffer 
    rb.poison();

    // Wait for dequeuers
    for deq in deq_threads {
        deq.await.unwrap();
    }
```
If enqueuer tasks need be in a loop, you can use the `async_stream` crate and hook up enqueuer tasks to a stream, where you can denote that a `None` value returned by the stream means that it is done enqueuing.

# Benchmark Results
I tried benchmarking this ring buffer (and comparing it with kanal async) with the following parameters:
* 4 tasks for enqueuing and 4 tasks for dequeuing (each adding/removing 250,000 usize)
* 4 worker threads
* Total capacity of the buffer is 1024 entries
* Varying shards value I experimented on starting (4, 8, 16, 32, 64, 128, 256) using a ShiftBy policies using random initial indices for enqueuer and dequeuer tasks and shift of 4.

Note I am running this on:
Machine: AMD Ryzen 7 5800X 3.8 GHz 8-Core Processor
Rust:rustc rustc 1.87.0 (17067e9ac 2025-05-09)
OS: Windows 10
Date: July 9, 2025

The cargo benchmarking in GitHub Action may not reflect the full possibility of this buffer due to limited number of threads and small cache size. To check out previous cargo benchmarking results, you can look at `benchmark_res`.

The following are timing results using `cargo bench` with varying shards in the order mentioned above:

Without barrier synchronization:

```
kanal_async/1024        time:   [17.980 ms 18.192 ms 18.404 ms]

4shard_buffer/1024      time:   [11.935 ms 11.970 ms 12.014 ms]

8shard_buffer/1024      time:   [13.449 ms 13.479 ms 13.517 ms]

16shard_buffer/1024     time:   [17.725 ms 17.771 ms 17.824 ms]

32shard_buffer/1024     time:   [22.640 ms 22.768 ms 22.914 ms]

64shard_buffer/1024     time:   [27.089 ms 27.396 ms 27.719 ms]

128shard_buffer/1024    time:   [25.890 ms 26.242 ms 26.611 ms]

256shard_buffer/1024    time:   [21.637 ms 22.008 ms 22.387 ms]
```

With barrier synchronization on all tasks:
```
kanal_async/1024        time:   [18.556 ms 18.757 ms 18.960 ms]

4shard_buffer/1024      time:   [11.811 ms 11.832 ms 11.858 ms]

8shard_buffer/1024      time:   [13.589 ms 13.641 ms 13.702 ms]

16shard_buffer/1024     time:   [18.343 ms 18.572 ms 18.819 ms]

32shard_buffer/1024     time:   [23.209 ms 23.405 ms 23.616 ms]

64shard_buffer/1024     time:   [28.632 ms 29.100 ms 29.583 ms]

128shard_buffer/1024    time:   [26.119 ms 26.464 ms 26.816 ms]

256shard_buffer/1024    time:   [20.831 ms 21.199 ms 21.569 ms]
```

# Some Considerations When Using This Buffer

Assume we have X enqueuer tasks, Y dequeuer tasks.
* How many threads should you use? 
    * In theory, this should be min(X, max # of threads) threads because dequeuers are bounded by the enqueuers in terms of the items they can pop off, and your machine is limited by the number of cores it has. Moreover, using more threads than the number of tasks that are spawned in results in some threads being unused in simultaneous operations because those threads are not assigned a task.
* What should the capacity of my buffer be?
    * This is still being experimented on, but it's true that using more capacity (up to a certain diminishing point) leads to faster results. This is likely as a result of less collisions for an enqueuer task acquiring a shard and finding out it is full or dequeuer task acquiring a shard and finding out it is empty.
* How many shards should I use? 
    * min(max(X, Y), max # of threads) shards. Ideally, you want to limit how many enqueuer and dequeuer tasks are fighting for a specific shard, but also keep in mind that if you don't have more threads than the number of shards, you may cause an enqueuer/dequeuer task to take a bit more time to acquire a shard that is non-full or non-empty respectively.
* What shift acquisition policy should I go for? 
    * In a SPSC task case:
        * Use Sweep policy. You can use 1 shard and a single thread because Tokio's scheduling on task assignments on threads will assist with that thread taking turns on performing enqueue/dequeue operations.
    * In a MPSC task case:
        * If you know how many enqueuer tasks you are spawning, use a ShiftBy policy with a shift of X for enqueuers assigning the initial shard index for each of these task manually from 0 - (X - 1). Use Sweep policy on the dequeuer task starting off with index 0 or random initial index (pass in "None").
        * If you don't know how many tasks you are spawning, then use a ShiftBy policy with a random initial shard index (pass in "None") and base the shift value by the number of shards you are using. 
    * In an MPMC task case:
        * If you know how many enqueuer/dequeuer tasks you are spawning, use a ShiftBy policy with a shift of X for enqueuers assigning the initial shard index for each of these task manually from 0 - (X - 1) and a shift of Y for dequeuers assigning the initial shard index for each of these task manually from 0 - (Y - 1). 
        * If you don't know how many tasks you are spawning, then use a ShiftBy policy with a random initial shard index (pass in "None") and base the shift value by the number of shards you are using for enqueuer/dequeuer tasks.

Now let's assume the following (which is more likely in a real world Tokio project): 1000+ enqueuer tasks, 1000+ dequeuer tasks
* Use as many threads as you can
* Use as many shards as you can with a higher capacity per shard. You want to minimize contention between what shards a task uses as much as possible.
* It's probably best to go for a ShiftBy acquisition policy (until a better acquisition policy is made) that matches the number of shards used.

# Future Additions/Thoughts
* Enqueuing/Dequeuing items in batches to take advantage of Auto-Vectorization compiler optimizations
* Play around with shard acquiring policies, so there are fewer failing calls to `self.shard_jobs[current].occupied.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()`. For example, introduce a `SweepBy` and `SweepAndShiftBy` policies so that the task is yielded through less attempts of acquiring a shard.

# Contribution
All contributions (i.e. documentation, testing, providing feedback) are welcome! Check out [CONTRIBUTING.md](CONTRIBUTING.md) on how to start.

# License
This project is licensed under the [MIT License](LICENSE-MIT) and [Apache License](LICENSE-APACHE).
