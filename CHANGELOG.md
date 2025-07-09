# Change Log for lf-shardedringbuf:

## In v1.1.0:
* Removed `LFShardedRingBuf::poison_deq` method. Use `LFShardedRingBuf::poison` method instead (see examples from `README.md`).
* Improved performance of `LFShardedRingBuf<T>` through using `MaybeUninit<T>` (instead of `Option<T>`) for each `InnerRingBuffer<T>` items
    * As a result, `Drop` trait has been implemented for `InnerRingBuffer<T>`
* Updated cargo benchmarking details in `README.md` and added in `benchmark_res` folder to store old benchmark data.

## In v0.1.12:
* Added Apache licensing in repository
* Added `CHANGELOG.md` (will update this frequently from now on)
* Added `CONTRIBUTING.md`

## In v0.1.11:
* Removed unnecessary dependencies from `Cargo.toml`
* Repository code is modularized for organization and readability 
* LFShardedRingBuf fields have been modified to use only AtomicBool for shard locking and the job_count is kept inside InnerRingBuffer. This should promote cache locality on accessing and updating job_count when enqueuing/dequeuing.
* When a shard is acquired within `try_acquire_shard()`, if a dequeuer thread noticed that the shard is empty or an enqueuer thread noticed that the shard is full afterward, it will release that shard with a Relaxed memory ordering. Previously, this was done in a Release memory ordering, which was not necessary as this release was not modifying the respective InnerRingBuffer. Doing this possibly saved a couple of cycles.
* Testing for SPSC/SPMC/MPSC/MPMC is implemented in tests directory. `is_empty()`, `is_full()`, `clear()`, and `poison()` have also been tested. There are other functions that must be tested here and stress/fuzz testing should be done here, but it should be safe to use this structure in an async environment to enqueue and dequeue items.
* Cargo benchmarking added for single threaded and multithreaded cases

## In v0.1.0:
* Implemented LFShardedRingBuf<T>, which features:
    * It uses multiple smaller simple ring buffers (shards) each with capacity = requested capacity / # of shards. This value is ceiled up currently so all shard have the same capacity.
    * It is lock-free; only uses atomic primitives and no mutexes or rwlocks
    * False sharing is avoided through cache padding the shard atomic locks + shards.
    * It uses tokio's task local variables as a shard index reference and to remember the shard acquistion policy strategy to take for tasks to effectively acquire a shard to enqueue/dequeue on.
    * ~~Exponential backoff + random jitter (capped at 20 ms) used to yield CPU in functions that loops.~~
        * This backoff method was removed since it introduced a bit more delay. This is because what you want to yield here are not the threads but rather the asynchronous tasks; you want the threads to always be working and let tokio reassign these threads to a task that will actually perform work. Instead of sleeping the threads, if a thread performs a full sweep around the shards (a sweep defined in respect to the shard acquisition policies), tokio's `yield_now()` function will put the running task to the back of the scheduled list. Doing this in a way also enables that task to actually perform work as an enqueuer or dequeuer later on because presumably a successful dequeue or enqueue operation has occurred by then.
        * Also, unfortunately, `tokio::time::sleep()` function works on a millisecond granularity, so sleeping on a task could be a bit more costly.
    * Different shard acquisition policies are provided: `Sweep`, `RandomAndSweep`, and `ShiftBy` (see `src/shard_policies.rs` for more info) 
    * It can perform in an async multithreaded or async single threaded environment (optimal for multiple producer, multiple consumer situations)
* Added MIT Licensing
