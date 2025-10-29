# Change for shardedringbuf:

## In v0.3.0:
* `ShardedRingBuf<T>` has a new instance method `new_with_enq_num` for the purpose of using the buffer knowing how many
enqueue tasks a user will spawn (useful for SPMC or if enqueuer tasks < num of shards) and distributing notifications to
enqueuers evenly by dequeuers
* Added Result return types on enqueue methods for `ShardedRingBuf<T>`
* Removed unnecessary code and async functions
* Updated documentation of `ShardedRingBuf<T>` methods
* Transferred over some basic tests from `ExpShardedRingBuf<T>` (deprecated) to `ShardedRingBuf<T>` (finally)

## In v0.2.0:
* `ShardedRingBuf<T>` has been refactored to only support the previous Pin policy code from previous iteration (see `exp_srb/` to see previous iteration)
* A new hybrid lock-free/async-waiting data structure added to this crate called `MLFShardedRingBuf<T>` (Most Lock Free Sharded Ring Buffer)
* Benchmarking results has been collected through Criterion and shown on README.md
* Lots of refactoring and documentation updates

## In v0.1.2:
* Forgot to update this, so you might want to check the commit history for this

## In v0.1.1:
* Changed the enqueue and dequeue indices to Cell usize rather than AtomicUsize in `InnerRingBuffer<T>` because everything is protected under a shard lock.

## In v0.1.0:
* A new policy has been made called `CFT` (or Completely Fair Tasks). See `src/task_local_spawn.rs` for more details.
* Spawn functions have been refactored to specifically call on enqueue and dequeue functions
of `ShardedRingBuf<T>` (as a result, the async enqueue and dequeue functions for the buffer is private)
* Updated doc comments on spawn function and Shard Policies and various other places.
* No new benchmarking details have been updated as of now due (though benchmarking code
is being modularized currently).

# Change Log for lf-shardedringbuf:

## In v4.1.0:
* Crate has been deprecated and moved to `sharded_ringbuf` due to high number of version changes and the crate name is a misnomer because it technically uses locks with the key feature of this buffer being that it has shards.
* There have been other changes here before it was marked deprecated like `CFT` (Completely Fair Tasks) being implemented, which is why the major version change has been updated here instead of patch version.

## In v3.1.0:
* Metadata in `LFShardedRingBuf<T>` is once again refactored such that everything uses atomic primitives (i.e. `Box<[UnsafeCell<MaybeUninit<T>>]>` -> `Box<[AtomicPtr<MaybeUninit<T>>]>`). It implement `Send` and `Sync` by default now!
* Some methods have been renamed (for better naming conventions and these names are finalized moving forward) such as:
    * `LFShardedRingBuf::concurrent_clear` -> `LFShardedRingBuf::async_clear`
    * `LFShardedRingBuf::get_enq_ind_for_shard` -> `LFShardedRingBuf::get_enq_ind_at_shard`
    * `LFShardedRingBuf::get_deq_ind_for_shard` -> `LFShardedRingBuf::get_deq_ind_at_shard`
    * `LFShardedRingBuf::get_item_in_shard` -> `LFShardedRingBuf::get_item_at_shard`
    * `LFShardedRingBuf::rb_items_at_shard` -> `LFShardedRingBuf::async_clone_items_at_shard`
    * `LFShardedRingBuf::rb_items` -> `LFShardedRingBuf::async_clone_items`
    * `LFShardedRingBuf::print_buffer` -> `LFShardedRingBuf::async_print`
    * Note these methods might be moved into traits in the future.
* Internally, a `ShardLockGuard` type is created and used to make locking the shard in many of these
async function ergonomic and in theory cancel safe (though this of course needs testing)
* All methods for this data structure have a `sync` and `async` version (though testing still needs to be performed on many of these methods for thread safety and cancel safetiness), except for the following:
    * `LFShardedRingBuf::enqueue` and `LFShardedRingBuf::dequeue` methods, which remains to be async. If there is interest, a sync version of these methods can be made that uses `thread_local` variables, but keep in mind that this buffer highly benefits from being an asynchronous data structure as a result of context switching tasks to threads
* New benchmarking in `README.md`

## In v2.1.0:
* Removed unnecessary metadata in `LFShardedRingBuf<T>` and changed `poisoned` field inside `LFShardedRingBuf<T>` as well as the `InnerRingBuffer<T>` `enqueue_ind`, `dequeue_ind`, and `job_count` fields from cell variants (i.e. `Cell<usize>`, `Cell<bool>`) to atomic variants (`AtomicUsize`, `AtomicBool`) for safe accesses among threads (and correctness)
* `LFShardedRingBuf<T>` now supports uneven shards (that is if request capacity is not divisible by requested number of shards) and balances uneven capacity among each shard nicely.
* Modified how `LFShardedRingBuf::clear` method works, which clears with Relaxed memory ordering. Added in an async clear method called `LFShardedRingBuf::concurrent_clear` should users want to clear the buffer in an asynchronous manner.
* Changed `LFShardedRingBuf::poison` and `LFShardedRingBuf::clear_poison` to not be async functions. Didn't make sense. Also, added `LFShardedRingBuf::is_poisoned` to test if the buffer
is poisoned.
* Added in `LFShardedRingBuf::get_num_of_shards`, `LFShardedRingBuf::get_shard_capacity`, `LFShardedRingBuf::get_total_capacity`, `LFShardedRingBuf::get_job_count_at_shard`, so users are exposed to reading what the `LFShardedRingBuf<T>` contains. Done so with Relaxed memory ordering.
* Added in `rt_spawn_buffer_task` method in case users want to provide a specific Tokio Runtime to spawn a task rather than work with `[tokio::main]` macro.
* Renamed `LFShardedRingBuf::next_enqueue_index_for_shard` and `LFShardedRingBuf::next_dequeue_index_for_shard` to `LFShardedRingBuf::get_enq_ind_for_shard` and `LFShardedRingBuf::get_deq_ind_for_shard` respectively for better naming conventions.
* README updated with more tips on how to use this buffer and new benchmarking is shown.

## In v1.1.0:
* Removed `LFShardedRingBuf::poison_deq` method. Use `LFShardedRingBuf::poison` method instead (see examples from `README.md`).
* Improved performance of `LFShardedRingBuf<T>` through using `MaybeUninit<T>` (instead of `Option<T>`) for each `InnerRingBuffer<T>` items
    * As a result, `Drop` trait has been implemented for `InnerRingBuffer<T>`
* Updated cargo benchmarking details in `README.md` and added in `benchmark_res/` folder to store old benchmark data.

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
