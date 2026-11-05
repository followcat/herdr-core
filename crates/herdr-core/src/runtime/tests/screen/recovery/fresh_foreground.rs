use super::*;

/// Saved facts claim this pid is devin; a live holder reports that pid,
/// and the real observation reads the process that actually owns it now.
async fn restored_positive_screen_with_stale_pid(pid: i32, worker_first: bool) {
    let root = PathBuf::from(format!("/var/tmp/hc-r3-stale-{}", platform::random_hex()));
    let core = test_core_at(&root);
    let catalog = test_detect_catalog();
    let (id, _) = screen_agent(&core, "devin");
    core.lock().reg.terminals.get_mut(&id).unwrap().foreground.as_mut().unwrap().pid = pid;
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    core.save(&core.lock());
    core.flush_saved().await.unwrap();
    drop(core);
    let core = restart_on_fake_holder(&root, id, "crash", b"WORKING NOW\r\n", Some(pid));
    let (turn, news) = {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        (t.turn, t.news)
    };
    if worker_first {
        core.screen_status_pass(&catalog).await;
        assert!(
            core.lock().screen_status[&id].unconfirmed_until.is_some(),
            "positive stale evidence cannot confirm before observation"
        );
    }
    // One actual watcher observation, ordered explicitly against the pass.
    // The background-loop counterpart is covered by the real-time relink test.
    let watcher = tokio::spawn(core.clone().watch_foreground());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while core.lock().reg.terminals[&id].foreground.as_ref().is_some_and(|f| f.agent.is_some()) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.status.is_none());
        assert_eq!((t.turn, t.news), (turn, news), "withdrawal of stale evidence ends no turn");
    }
    watcher.abort();
    let _ = watcher.await;
    core.handover().await.unwrap();
    drop(core);
    remove_root(&root);
}

#[tokio::test]
async fn a_dead_saved_pid_cannot_confirm_a_positive_restored_screen() {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id() as i32;
    child.wait().unwrap();
    assert!(crate::foreground::about(pid).is_none());
    for worker_first in [false, true] {
        restored_positive_screen_with_stale_pid(pid, worker_first).await;
    }
}

#[tokio::test]
async fn a_reused_saved_pid_cannot_confirm_a_positive_restored_screen() {
    // The test executable is alive at this exact pid, but it is not devin.
    let pid = std::process::id() as i32;
    assert!(crate::foreground::about(pid).unwrap().agent.is_none());
    for worker_first in [false, true] {
        restored_positive_screen_with_stale_pid(pid, worker_first).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_positive_restored_screen_without_a_fresh_observation_expires_turn_free() {
    let root = PathBuf::from(format!("/var/tmp/hc-r3-stale-{}", platform::random_hex()));
    let (core, id) = restarted_root(&root, b"WORKING NOW\r\n").await;
    let (turn, news, seq) = {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        (t.turn, t.news, st.events.seq)
    };
    core.screen_status_pass(&test_detect_catalog()).await;
    assert!(core.lock().screen_status[&id].unconfirmed_until.is_some());
    tokio::time::advance(crate::screen_status::RECONFIRM).await;
    core.screen_status_pass(&test_detect_catalog()).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.status.is_none());
        assert_eq!((t.turn, t.news, st.events.seq), (turn, news, seq + 1));
    }
    drop(core);
    remove_root(&root);
}
