#![cfg(feature = "srb")]
use sharded_ringbuf::srb::ShardedRingBuf;
use tokio::spawn;
use tokio::task::yield_now;

#[tokio::test(flavor = "multi_thread")]
async fn test_spsc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const MAX_SHARDS: usize = 1;
        let rb = ShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS);

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn 1 dequeuer task
        for i in 0..MAX_SHARDS {
            let deq_handler = tokio::spawn({
                let rb_clone = rb.clone();
                let mut counter = 0;
                async move {
                    loop {
                        match rb_clone.dequeue_full_in_shard(i).await {
                            Some(items) => {
                                for _ in items {
                                    counter += 1;
                                }
                            }
                            None => break,
                        }
                    }
                    counter
                }
            });
            deq_threads.push(deq_handler);
        }

        // Spawn 1 enqueuer task
        {
            let enq_handler = spawn({
                let rb_clone = rb.clone();
                async move {
                    let mut counter = 0;
                    let full_enq = rb_clone.get_shard_capacity(0).unwrap();
                    let mut enq_vec = Vec::with_capacity(full_enq);
                    for item in 0..2 * MAX_ITEMS {
                        if counter != 0 && counter % full_enq == 0 {
                            let _ = rb_clone.enqueue_full(enq_vec).await;
                            enq_vec = Vec::with_capacity(full_enq);
                            enq_vec.push(item);
                            counter += 1;
                        } else {
                            enq_vec.push(item);
                            counter += 1;
                        }
                    }
                    if !enq_vec.is_empty() {
                        let _ = rb_clone.enqueue_full(enq_vec).await;
                    }
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        rb.poison();

        // necessary for poison
        let notifier_task = spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    for i in 0..MAX_SHARDS {
                        rb_clone.notify_dequeuer_in_shard(i % rb_clone.get_num_of_shards());
                    }
                    yield_now().await;
                }
            }
        });

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        notifier_task.abort();

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_spmc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const MAX_SHARDS: usize = 5;
        let rb = ShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS);

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn MAX_SHARD dequeuer task
        for i in 0..MAX_SHARDS {
            let deq_handler = tokio::spawn({
                let rb_clone = rb.clone();
                let mut counter = 0;
                async move {
                    loop {
                        match rb_clone.dequeue_full_in_shard(i).await {
                            Some(items) => {
                                for _ in items {
                                    counter += 1;
                                }
                            }
                            None => break,
                        }
                    }
                    counter
                }
            });
            deq_threads.push(deq_handler);
        }

        // Spawn 1 enqueuer task
        {
            let enq_handler = spawn({
                let rb_clone = rb.clone();
                async move {
                    let mut counter = 0;
                    let full_enq = rb_clone.get_shard_capacity(0).unwrap() / MAX_SHARDS;
                    let mut enq_vec = Vec::with_capacity(full_enq);
                    for item in 0..2 * MAX_ITEMS {
                        if counter != 0 && counter % full_enq == 0 {
                            let _ = rb_clone.enqueue_full(enq_vec).await;
                            enq_vec = Vec::with_capacity(full_enq);
                            enq_vec.push(item);
                            counter += 1;
                        } else {
                            enq_vec.push(item);
                            counter += 1;
                        }
                    }
                    if !enq_vec.is_empty() {
                        let _ = rb_clone.enqueue_full(enq_vec).await;
                    }
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        rb.poison();

        // necessary for poison
        let notifier_task = spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    for i in 0..MAX_SHARDS {
                        rb_clone.notify_dequeuer_in_shard(i % rb_clone.get_num_of_shards());
                    }
                    yield_now().await;
                }
            }
        });

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        notifier_task.abort();

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mpsc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const MAX_SHARDS: usize = 1;
        const MAX_ENQ: usize = 5;
        let rb = ShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS);

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn 1 dequeuer task
        for i in 0..MAX_SHARDS {
            let deq_handler = tokio::spawn({
                let rb_clone = rb.clone();
                let mut counter = 0;
                async move {
                    loop {
                        match rb_clone.dequeue_full_in_shard(i).await {
                            Some(items) => {
                                for _ in items {
                                    counter += 1;
                                }
                            }
                            None => break,
                        }
                    }
                    counter
                }
            });
            deq_threads.push(deq_handler);
        }

        // Spawn MAX_ENQ enqueuer task
        for _ in 0..MAX_ENQ {
            let enq_handler = spawn({
                let rb_clone = rb.clone();
                async move {
                    let mut counter = 0;
                    let full_enq = rb_clone.get_shard_capacity(0).unwrap();
                    let mut enq_vec = Vec::with_capacity(full_enq);
                    for item in 0..2 * MAX_ITEMS {
                        if counter != 0 && counter % full_enq == 0 {
                            let _ = rb_clone.enqueue_full(enq_vec).await;
                            enq_vec = Vec::with_capacity(full_enq);
                            enq_vec.push(item);
                            counter += 1;
                        } else {
                            enq_vec.push(item);
                            counter += 1;
                        }
                    }
                    if !enq_vec.is_empty() {
                        let _ = rb_clone.enqueue_full(enq_vec).await;
                    }
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        rb.poison();

        // necessary for poison
        let notifier_task = spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    for i in 0..MAX_SHARDS {
                        rb_clone.notify_dequeuer_in_shard(i % rb_clone.get_num_of_shards());
                    }
                    yield_now().await;
                }
            }
        });

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        notifier_task.abort();

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_ENQ, items_taken);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mpmc_tasks() {
    for _ in 0..100 {
        const MAX_ITEMS: usize = 1000;
        const MAX_SHARDS: usize = 5;
        const MAX_ENQ: usize = 5;
        let rb = ShardedRingBuf::new(MAX_ITEMS, MAX_SHARDS);

        // Init rb check
        assert!(rb.is_empty());
        for i in 0..MAX_SHARDS {
            if let Some(val) = rb.get_deq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
            if let Some(val) = rb.get_enq_ind_at_shard(i) {
                assert_eq!(val, 0)
            }
        }

        let mut deq_threads = Vec::new();
        let mut enq_threads = Vec::new();

        // Spawn MAX_SHARD dequeuer task
        for i in 0..MAX_SHARDS {
            let deq_handler = tokio::spawn({
                let rb_clone = rb.clone();
                let mut counter = 0;
                async move {
                    loop {
                        match rb_clone.dequeue_full_in_shard(i).await {
                            Some(items) => {
                                for _ in items {
                                    counter += 1;
                                }
                            }
                            None => break,
                        }
                    }
                    counter
                }
            });
            deq_threads.push(deq_handler);
        }

        // Spawn MAX_ENQ enqueuer task
        for _ in 0..MAX_ENQ {
            let enq_handler = spawn({
                let rb_clone = rb.clone();
                async move {
                    let mut counter = 0;
                    let full_enq = rb_clone.get_shard_capacity(0).unwrap() / MAX_SHARDS;
                    let mut enq_vec = Vec::with_capacity(full_enq);
                    for item in 0..2 * MAX_ITEMS {
                        if counter != 0 && counter % full_enq == 0 {
                            let _ = rb_clone.enqueue_full(enq_vec).await;
                            enq_vec = Vec::with_capacity(full_enq);
                            enq_vec.push(item);
                            counter += 1;
                        } else {
                            enq_vec.push(item);
                            counter += 1;
                        }
                    }
                    if !enq_vec.is_empty() {
                        let _ = rb_clone.enqueue_full(enq_vec).await;
                    }
                }
            });
            enq_threads.push(enq_handler);
        }

        for enq in enq_threads {
            enq.await.unwrap();
        }

        rb.poison();

        // necessary for poison
        let notifier_task = spawn({
            let rb_clone = rb.clone();
            async move {
                loop {
                    for i in 0..MAX_SHARDS {
                        rb_clone.notify_dequeuer_in_shard(i % rb_clone.get_num_of_shards());
                    }
                    yield_now().await;
                }
            }
        });

        let mut items_taken: usize = 0;
        while let Some(curr_thread) = deq_threads.pop() {
            items_taken += curr_thread.await.unwrap();
        }

        notifier_task.abort();

        assert!(rb.is_empty());

        assert_eq!(2 * MAX_ITEMS * MAX_ENQ, items_taken);
    }
}
