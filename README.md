# lf-shardedringbuf
An async, lock-free, sharded, cache-aware SPSC/MPSC/MPMC ring buffer in Rust.

# Features
* It uses multiple smaller simple ring buffers (shards) each with capacity = requested capacity / # of shards. This value is ceiled up currently so all shard have the same capacity.
* It is lock-free; only uses atomic primitives and no mutexes or rwlocks
* False sharing is avoided through cache padding the shard atomic locks + shards.
* It uses tokio's task local variables as a shard index reference and to remember the shard acquistion policy strategy to take for tasks to effectively acquire a shard to enqueue/dequeue on.
* ~~Exponential backoff + random jitter (capped at 20 ms) used to yield CPU in functions that loops.~~
    * This backoff method was removed since it introduced a bit more delay. This is because what you want to yield here are not the threads but rather the asynchronous tasks; you want the threads to always be working and let tokio reassign these threads to a task that will actually perform work. Instead of sleeping the threads, if a thread performs a full sweep around the shards (a sweep defined in respect to the shard acquisition policies), tokio's `yield_now()` function will put the running task to the back of the scheduled list. Doing this in a way also enables that task to actually perform work as an enqueuer or dequeuer later on because presumably a successful dequeue or enqueue operation has occurred by then.
    * Also, unfortunately, `tokio::time::sleep()` function works on a millisecond granularity, so sleeping on a task could be a bit more costly.
* Different shard acquisition policies are provided: `Sweep`, `RandomAndSweep`, and `ShiftBy` (see `src/shard_policies.rs` for more info) 
* It can perform in an async multithreaded or async single threaded environment (optimal for multiple producer, multiple consumer situations)

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
    rb.poison().await;

    // Wait for dequeuers
    for deq in deq_threads {
        deq.await.unwrap();
    }
```
If enqueuer tasks need be in a loop, you can use the `async_stream` crate and hook up enqueuer tasks to a stream, where you can denote that a `None` value returned by the stream means that it is done enqueuing.

# Benchmark Results
I tried benchmarking this ring buffer (and comparing it with kanal async) with the following parameters:
* 4 tasks for enqueuing and 4 tasks for dequeuing (each adding/removing 250,000 usize)
* 8 worker threads
* Total capacity of the buffer is 1024 entries
* Varying shards value I experimented on starting (4, 8, 16, 32, 64, 128, 256) using a ShiftBy policies using random initial shard indices for enqueuer and dequeuer tasks and shift of 4.

The following are timing results using `cargo bench` with varying shards in the order mentioned above:

Without barrier synchronization:

```
kanal_async/1024        time:   [20.690 ms 21.061 ms 21.439 ms]
                        change: [+12.212% +14.716% +17.107%] (p = 0.00 < 0.05)

4shard_buffer/1024      time:   [13.938 ms 14.109 ms 14.283 ms]
                        change: [+23.241% +24.800% +26.346%] (p = 0.00 < 0.05)

8shard_buffer/1024      time:   [18.105 ms 18.299 ms 18.491 ms]
                        change: [+19.169% +22.492% +25.801%] (p = 0.00 < 0.05)

16shard_buffer/1024     time:   [17.490 ms 17.594 ms 17.696 ms]
                        change: [−9.0458% −8.4163% −7.8831%] (p = 0.00 < 0.05)

32shard_buffer/1024     time:   [20.207 ms 20.316 ms 20.436 ms]
                        change: [−14.603% −14.031% −13.416%] (p = 0.00 < 0.05)

64shard_buffer/1024     time:   [29.082 ms 29.536 ms 29.994 ms]
                        change: [−9.8435% −8.0829% −6.4858%] (p = 0.00 < 0.05)

128shard_buffer/1024    time:   [24.379 ms 24.948 ms 25.514 ms]
                        change: [−11.931% −9.6158% −7.2861%] (p = 0.00 < 0.05)

256shard_buffer/1024    time:   [20.393 ms 20.788 ms 21.185 ms]
                        change: [−5.8023% −3.2172% −0.3579%] (p = 0.03 < 0.05)
```

With barrier synchronization on all tasks:
```
kanal_async/1024        time:   [22.418 ms 22.815 ms 23.216 ms]
                        change: [+5.6374% +8.3242% +11.093%] (p = 0.00 < 0.05)

4shard_buffer/1024      time:   [13.572 ms 13.726 ms 13.881 ms]
                        change: [−4.2682% −2.7131% −1.1723%] (p = 0.00 < 0.05)

8shard_buffer/1024      time:   [18.018 ms 18.339 ms 18.687 ms]
                        change: [−1.8632% +0.2186% +2.3834%] (p = 0.84 > 0.05)

16shard_buffer/1024     time:   [17.159 ms 17.265 ms 17.371 ms]
                        change: [−2.7330% −1.8720% −0.9990%] (p = 0.00 < 0.05)

32shard_buffer/1024     time:   [19.535 ms 19.595 ms 19.654 ms]
                        change: [−4.1731% −3.5472% −2.9584%] (p = 0.00 < 0.05)

64shard_buffer/1024     time:   [28.766 ms 29.240 ms 29.726 ms]
                        change: [−3.2440% −1.0032% +1.2517%] (p = 0.38 > 0.05)

128shard_buffer/1024    time:   [23.947 ms 24.480 ms 25.017 ms]
                        change: [−4.9885% −1.8747% +1.1849%] (p = 0.24 > 0.05)

256shard_buffer/1024    time:   [20.025 ms 20.464 ms 20.912 ms]
                        change: [−4.4560% −1.5594% +1.2597%] (p = 0.29 > 0.05)
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

# Future Additions/Thoughts
* Enqueuing/Dequeuing items in batches to take advantage of Auto-Vectorization compiler optimizations
* Play around with shard acquiring policies, so there are fewer failing calls to `self.shard_jobs[current].occupied.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()`. For example, introduce a `SweepBy` and `SweepAndShiftBy` policies so that the task is yielded through less attempts of acquiring a shard.
* Currently, this buffer makes each shard have the same capacity to promote an evenly distributed load of enqueue and dequeue operation among the shards, but in the future, I may allow for uneven shards, a way to decrease or increase the number of shards this buffer uses, and a way to increase (though unsure about decreasing) the capacity of the buffer. 

# Contribution
All contributions (i.e. documentation, testing, providing feedback) are welcome! Just make sure to `cargo clippy` and `cargo fmt` before you create a pull request. And if it's a major change or concern, make a GitHub Issue first before creating a pull request.

# License
This project is licensed under the [MIT License](LICENSE).
