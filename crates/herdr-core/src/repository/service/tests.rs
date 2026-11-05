use super::*;

#[test]
fn demand_coalesces_and_rate_concurrency_queue_and_cache_costs_are_bounded() {
    let mut scheduler = Scheduler::default();
    let now = Instant::now();
    for id in 1..=2000 {
        scheduler.observe(id, "/home/user/acme-shop", None, None, "host-a", now);
    }
    assert_eq!(scheduler.queue.len(), 1, "one directory job, not one per terminal");
    assert!(scheduler.next(now).is_some());
    assert!(scheduler.next(now + Duration::from_millis(499)).is_none());
    for id in 2001..=3200 {
        scheduler.observe(id, &format!("/home/user/plain-{id}"), None, None, "host-a", now);
    }
    assert!(scheduler.queue.len() <= 128);
    assert!(scheduler.directories.len() <= 1024);
    assert!(scheduler.next(now + Duration::from_millis(500)).is_some());
    assert!(scheduler.next(now + Duration::from_secs(2)).is_none(), "only two jobs can be outstanding");
}

#[test]
fn stale_generation_cannot_publish_and_overdue_facts_are_explicit() {
    let mut scheduler = Scheduler::default();
    let now = Instant::now();
    let cwd = "/home/user/plain";
    scheduler.observe(1, cwd, None, None, "host-a", now);
    let Some(Job::Discover { generation, .. }) = scheduler.next(now) else { panic!("discovery") };
    scheduler.invalidate(1);
    scheduler.observe(1, cwd, None, None, "host-a", now + Duration::from_secs(1));
    scheduler.complete(
        Completed::Discover {
            cwd: cwd.into(),
            generation,
            result: Ok(Discovery { auto_label: "obsolete".into(), repository: None }),
        },
        now,
    );
    assert!(scheduler.directories[cwd].result.is_none());
    scheduler.observe(1, cwd, None, None, "host-a", now + Duration::from_secs(1));
    let Some(Job::Discover { generation, .. }) = scheduler.next(now + Duration::from_secs(1)) else { panic!("retry") };
    scheduler.complete(
        Completed::Discover {
            cwd: cwd.into(),
            generation,
            result: Ok(Discovery { auto_label: "plain".into(), repository: None }),
        },
        now,
    );
    let facts = scheduler.observe(1, cwd, None, None, "host-a", now + Duration::from_secs(46));
    assert_eq!(facts.state, FactState::Stale);
    assert_eq!(facts.auto_label.as_deref(), Some("plain"));
    let moved = scheduler.observe(1, "/home/user/elsewhere", None, Some(&facts), "host-a", now);
    assert_eq!(moved.state, FactState::Pending);
    assert!(moved.auto_label.is_none(), "old-cwd facts cannot survive a move");
}

#[test]
fn dropping_the_pool_does_not_join_a_worker_that_never_returns() {
    let (started, observed) = std::sync::mpsc::channel();
    let pool = Pool::new(move |_| {
        started.send(()).unwrap();
        loop {
            std::thread::park();
        }
    })
    .unwrap();
    pool.submit(Job::Discover { cwd: "/home/user/plain".into(), generation: 1 }).ok().unwrap();
    observed.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    drop(pool);
    assert!(start.elapsed() < Duration::from_millis(100), "shutdown must not wait for repository IO");
}
