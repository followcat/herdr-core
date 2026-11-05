use super::*;

#[cfg(unix)]
mod fresh_foreground;

// --- recovery: adoption, the unconfirmed window and the freeze (fork) -----

/// Re-adopts `id` onto a fresh fake holder that claims to hold it — the way
/// `take_over` meets the next core or a reconnected holder. `previous` is
/// what the holder says happened ("handover" or "crash"); `tail` is replayed
/// into the rebuilt screen. The fake then keeps the link open until the core
/// leaves, exactly like `on_fake_holder`.
#[cfg(unix)]
fn adopt_onto(core: &Arc<Core>, root: &std::path::Path, id: u32, previous: &str, tail: &[u8]) {
    let held = proto::HeldTerminal { id, pid: 4321, cols: 80, rows: 24, exit: None };
    let welcome = proto::Welcome {
        version: proto::VERSION,
        instance: "fake".into(),
        pid: 0,
        build: crate::client::build_id(),
        previous: previous.into(),
        terminals: vec![held],
    };
    let mut answer = proto::json_frame(proto::WELCOME, 0, &welcome);
    answer.extend(proto::frame(proto::TAIL, id, tail));
    let socket = root.join(format!("run/adopt-{}.sock", platform::random_hex()));
    let listener = platform::listen(&socket).unwrap();
    std::thread::spawn(move || {
        let Some(Ok(mut stream)) = listener.incoming().next() else { return };
        if proto::read(&mut stream).is_err() {
            return;
        }
        let _ = io::Write::write_all(&mut stream, &answer);
        while let Ok(Some(_)) = proto::read(&mut stream) {}
    });
    core.take_over(platform::connect(&socket).unwrap(), false, &socket, None).unwrap();
}

/// The real restart path: a second `Core::start` on `root`, whose
/// `attach_all` meets a fake holder holding `id` — the registry and its
/// status come back from disk, the screen from `tail`, and every
/// `Foreground.agent` is `None` (the field never reaches disk). The fake
/// keeps the link open like `adopt_onto`'s.
#[cfg(unix)]
fn restart_on_fake_holder(
    root: &std::path::Path,
    id: u32,
    previous: &str,
    tail: &[u8],
    foreground: Option<i32>,
) -> Arc<Core> {
    let held = proto::HeldTerminal { id, pid: 4321, cols: 80, rows: 24, exit: None };
    let welcome = proto::Welcome {
        version: proto::VERSION,
        instance: "fake".into(),
        pid: 0,
        build: crate::client::build_id(),
        previous: previous.into(),
        terminals: vec![held],
    };
    let mut answer = proto::json_frame(proto::WELCOME, 0, &welcome);
    answer.extend(proto::frame(proto::TAIL, id, tail));
    let socket = root.join("run/core.holder.fake.sock");
    let listener = platform::listen(&socket).unwrap();
    std::thread::spawn(move || {
        let Some(Ok(mut stream)) = listener.incoming().next() else { return };
        if proto::read(&mut stream).is_err() {
            return;
        }
        let _ = io::Write::write_all(&mut stream, &answer);
        while let Ok(Some((kind, id, _))) = proto::read(&mut stream) {
            if kind == proto::FOREGROUND
                && let Some(pgid) = foreground
            {
                let _ = io::Write::write_all(
                    &mut stream,
                    &proto::json_frame(proto::FOREGROUND_IS, id, &proto::Foreground { pgid }),
                );
            } else if kind == proto::GOODBYE {
                let _ = io::Write::write_all(&mut stream, &proto::frame(proto::BYE, 0, &[]));
                break;
            }
        }
    });
    test_core_at(root)
}

/// The foreground the watcher reports, like `watch_foreground` does:
/// replace the stored record, then tell detection what it replaced.
#[cfg(unix)]
fn watch_sees(core: &Core, id: u32, now: crate::foreground::Foreground) {
    let mut st = core.lock();
    let was = st.reg.terminals.get_mut(&id).unwrap().foreground.replace(now);
    core.foreground_changed(&mut st, id, was.as_ref());
}

/// `id` running `devin` with a confirmed inferred `working` root, saved and
/// restored through a real restart: live, adopted, root standing unconfirmed
/// and its foreground re-identified (`agent` never reaches disk; `start`
/// rehydrates it before serving).
#[cfg(unix)]
async fn restarted_root(root: &std::path::Path, tail: &[u8]) -> (Arc<Core>, u32) {
    let core = test_core_at(root);
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    core.save(&core.lock());
    // `flush_saved` cannot wait the real writer thread under paused time;
    // poll the file instead — the write itself is prompt.
    let file = core.paths.state.join("terminals.json");
    let started = std::time::Instant::now();
    while !std::fs::read(&file).is_ok_and(|b| {
        serde_json::from_slice::<serde_json::Value>(&b)
            .is_ok_and(|v| v["terminals"][id.to_string()]["status"]["state"] == "working")
    }) {
        assert!(started.elapsed() < Duration::from_secs(5), "the registry was not saved");
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(core);
    let core = restart_on_fake_holder(root, id, "crash", tail, None);
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.is_live(), "adopted on restart");
        assert_eq!(t.foreground.as_ref().unwrap().agent, Some("devin"), "the leader was re-identified at load");
        assert_eq!(t.status.as_ref().map(|s| s.state.as_str()), Some("working"), "the root stands restored");
        assert!(st.screen_status[&id].unconfirmed_until.is_some(), "and waits for fresh proof");
    }
    (core, id)
}

#[cfg(unix)]
fn test_core_at(root: &std::path::Path) -> Arc<Core> {
    let paths = Paths {
        state: root.join("state"),
        config: root.join("config.toml"),
        socket: root.join("run/core.sock"),
        holder_socket: root.join("run/core.holder.sock"),
    };
    paths.create_dirs().unwrap();
    Core::start(paths, CoreInfo { id: "test".into(), name: "test".into() }).unwrap()
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_restored_root_confirms_silently_after_a_restart_in_either_order() {
    for worker_first in [false, true] {
        let root = PathBuf::from(format!("/var/tmp/hc-restart-fg-{}", platform::random_hex()));
        let (core, id) = restarted_root(&root, b"WORKING NOW\r\n").await;
        let catalog = test_detect_catalog();
        let (turn, news, seq) = {
            let st = core.lock();
            let t = &st.reg.terminals[&id];
            (t.turn, t.news, st.events.seq)
        };
        if worker_first {
            core.screen_status_pass(&catalog).await;
        }
        watch_sees(&core, id, agent("devin"));
        core.screen_status_pass(&catalog).await;
        {
            let st = core.lock();
            let t = &st.reg.terminals[&id];
            assert_eq!(t.status.as_ref().map(|s| s.state.as_str()), Some("working"));
            assert!(st.screen_status[&id].unconfirmed_until.is_none(), "fresh evidence confirmed it");
            assert_eq!(
                (t.turn, t.news, st.events.seq),
                (turn, news, seq),
                "silent confirmation, worker first: {worker_first}"
            );
        }
        drop(core);
        remove_root(&root);
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_restored_root_expires_on_unknown_after_a_restart_in_either_order() {
    for worker_first in [false, true] {
        let root = PathBuf::from(format!("/var/tmp/hc-restart-fg-{}", platform::random_hex()));
        let (core, id) = restarted_root(&root, b"plain untracked text\r\n").await;
        let catalog = test_detect_catalog();
        let (turn, news, seq) = {
            let st = core.lock();
            let t = &st.reg.terminals[&id];
            (t.turn, t.news, st.events.seq)
        };
        if worker_first {
            core.screen_status_pass(&catalog).await;
        }
        watch_sees(&core, id, agent("devin"));
        core.screen_status_pass(&catalog).await;
        tokio::time::advance(crate::screen_status::RECONFIRM - Duration::from_millis(1)).await;
        core.screen_status_pass(&catalog).await;
        {
            let st = core.lock();
            assert_eq!(st.reg.terminals[&id].status.as_ref().map(|s| s.state.as_str()), Some("working"));
            assert!(st.screen_status[&id].unconfirmed_until.is_some());
            assert_eq!(st.events.seq, seq, "no early null, worker first: {worker_first}");
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        core.screen_status_pass(&catalog).await;
        {
            let st = core.lock();
            let t = &st.reg.terminals[&id];
            assert!(t.status.is_none());
            assert_eq!((t.turn, t.news), (turn, news), "expiry ends no turn");
            assert_eq!(st.events.seq, seq + 1, "one null event");
        }
        drop(core);
        remove_root(&root);
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_changed_foreground_after_a_restart_withdraws_in_either_order() {
    for worker_first in [false, true] {
        for change in ["pid", "agent"] {
            let root = PathBuf::from(format!("/var/tmp/hc-restart-fg-{}", platform::random_hex()));
            let (core, id) = restarted_root(&root, b"plain untracked text\r\n").await;
            let (turn, news, seq) = {
                let st = core.lock();
                let t = &st.reg.terminals[&id];
                (t.turn, t.news, st.events.seq)
            };
            if worker_first {
                core.screen_status_pass(&test_detect_catalog()).await;
            }
            let mut other = agent(if change == "agent" { "claude" } else { "devin" });
            if change == "pid" {
                other.pid = 200;
            }
            watch_sees(&core, id, other);
            {
                let st = core.lock();
                let t = &st.reg.terminals[&id];
                assert!(t.status.is_none(), "{change}, worker first: {worker_first}");
                assert_eq!((t.turn, t.news), (turn, news), "unconfirmed withdrawal ends no turn");
                assert_eq!(st.events.seq, seq + 1, "one null event");
            }
            drop(core);
            remove_root(&root);
        }
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_verdict_in_flight_across_adoption_dies() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    let seq = core.lock().output_seq.get(&id).copied().unwrap_or(0);
    // The screen is replaced: both staleness axes move under the same lock.
    adopt_onto(&core, &root, id, "crash", b"NEED YES\r\n");
    assert!(core.lock().output_seq.get(&id).copied().unwrap_or(0) > seq, "adoption bumps output_seq");
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id), None, "a verdict from before adoption is rejected");
    // The pass that follows works on the rebuilt screen.
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn native_authority_survives_handover() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"\x1b]7501;state=working\x1b\\");
    // What a handover saves: the exact screen for the next core.
    let snap = core.lock().screens[&id].snapshot().unwrap();
    write_atomic(&core.snapshot_path(id), &snap).unwrap();
    adopt_onto(&core, &root, id, "handover", b"");
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.reported, "native authority survives a handover");
        assert!(t.status.as_ref().is_some_and(|s| s.source.is_none()));
    }
    // Inference stays off for the program's lifetime: a marker screen means nothing.
    core.screen_status_pass(&catalog).await;
    assert!(core.lock().reg.terminals[&id].status.as_ref().is_some_and(|s| s.source.is_none()));
    drop(core);
    remove_root(&root);
}

// Real time: the registry writer is a real thread `flush_saved` waits on.
#[cfg(unix)]
#[tokio::test]
async fn native_authority_survives_a_crash_without_7501_in_the_tail() {
    for bytes in [
        b"\x1b]7501;state=working\x1b\\".as_slice(),        // root
        b"\x1b]7501;state=working:id=sub\x1b\\".as_slice(), // child-only
        b"\x1b]7501;state=clear\x1b\\".as_slice(),          // clear
    ] {
        let (core, root) = test_core();
        let catalog = test_detect_catalog();
        let (id, _frames) = screen_agent(&core, "devin");
        core.output(&mut core.lock(), id, bytes);
        // The durable checkpoint a crash keeps: authority must reach disk even
        // when the root word never moved (child-only, clear, same-word takeovers).
        core.flush_saved().await.unwrap();
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(core.paths.state.join("terminals.json")).unwrap()).unwrap();
        assert_eq!(
            on_disk["terminals"][id.to_string()]["reported"],
            true,
            "{:?}: reported must be persisted, not merely queued",
            String::from_utf8_lossy(bytes)
        );
        // The tail the next core sees no longer carries the report.
        adopt_onto(&core, &root, id, "crash", b"NEED YES\r\n");
        assert!(core.lock().reg.terminals[&id].reported, "authority survived a crash");
        // And inference never re-arms: the marker screen stays a marker.
        core.screen_status_pass(&catalog).await;
        let st = core.lock();
        let status = st.reg.terminals[&id].status.as_ref();
        assert!(status.is_none_or(|s| s.source.is_none()), "a restored authority ends inference: {status:?}");
        drop(st);
        drop(core);
        remove_root(&root);
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_restored_root_is_confirmed_by_a_fresh_verdict_silently() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    let (turn, seq) = {
        let st = core.lock();
        (st.reg.terminals[&id].turn, st.events.seq)
    };
    adopt_onto(&core, &root, id, "crash", b"WORKING NOW\r\n");
    {
        let st = core.lock();
        assert!(st.screen_status[&id].unconfirmed_until.is_some(), "the restored root waits for proof");
        assert!(st.reg.terminals[&id].status_available(), "it still counts for waits meanwhile");
    }
    watch_sees(&core, id, agent("devin"));
    // The screen proves it again with the same word: the marker clears, but
    // nothing publishes — no turn, no event.
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        assert!(st.screen_status[&id].unconfirmed_until.is_none(), "the fresh verdict confirmed it");
        assert_eq!(st.reg.terminals[&id].turn, turn, "a silent confirm ends no turn");
        assert_eq!(st.events.seq, seq, "and emits nothing");
    }
    // After confirmation a foreground-loss withdrawal counts the turn as ever.
    {
        let mut st = core.lock();
        let was = st.reg.terminals.get_mut(&id).unwrap().foreground.replace(agent("bash"));
        core.foreground_changed(&mut st, id, was.as_ref());
    }
    assert_eq!(word(&core, id), None);
    assert_eq!(core.lock().reg.terminals[&id].turn, turn + 1, "a confirmed withdrawal still counts");
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn unknown_evidence_leaves_a_restored_root_unconfirmed_until_expiry() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    let (turn, news, since) = {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        (t.turn, t.news, t.status_ms)
    };
    adopt_onto(&core, &root, id, "crash", b"plain untracked text\r\n");
    // Unknown evidence neither re-proves the root nor extends its deadline.
    for _ in 0..3 {
        core.screen_status_pass(&catalog).await;
        tokio::time::advance(Duration::from_millis(500)).await;
    }
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(st.screen_status[&id].unconfirmed_until.is_some());
        assert_eq!(t.status.as_ref().map(|s| s.state.as_str()), Some("working"), "it stands unconfirmed meanwhile");
    }
    tokio::time::advance(crate::screen_status::RECONFIRM).await;
    let seq = core.lock().events.seq;
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.status.is_none(), "past the deadline it is withdrawn as unknown");
        assert_eq!(t.turn, turn, "an unconfirmed withdrawal ends no turn");
        assert_eq!(t.news, news, "and reports no completion");
        assert!(t.status_ms > since, "the withdrawal still stamps");
        // wherdr's "unread done" (idle/done plus an unread turn) stays false.
        let unread_done =
            t.turn != turn && matches!(t.status.as_ref().map(|s| s.state.as_str()), Some("idle" | "done"));
        assert!(!unread_done);
        assert!(st.screen_status[&id].unconfirmed_until.is_none());
    }
    assert_eq!(core.lock().events.seq, seq + 1, "one null status event, no turn event");
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn an_unconfirmed_restored_root_is_still_waitable() {
    // Choice: a restored root is the best evidence there is — waits and
    // `status_available` keep it, exactly like a confirmed inferred root.
    // (Withdrawing it at restore would hide a visibly working program for
    // up to 30 s.)
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    adopt_onto(&core, &root, id, "crash", b"WORKING NOW\r\n");
    assert!(core.lock().screen_status[&id].unconfirmed_until.is_some());
    // Unconfirmed but available: the wait does not answer early, it follows.
    let c = core.clone();
    let waiting = tokio::spawn(async move {
        c.wait(id, crate::wait::WaitSpec { output: None, timeout: Duration::from_secs(30), after_turn: None }, None)
            .await
    });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    watch_sees(&core, id, agent("devin"));
    // A positive verdict re-proves it quietly: still working, wait holds.
    core.output(&mut core.lock(), id, b"output the old screen would not have\r\n");
    core.screen_status_pass(&catalog).await;
    assert!(!waiting.is_finished(), "the same word confirmed leaves the wait alone");
    // A real change ends it like any other status event.
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HNEED YES\r\n");
    core.screen_status_pass(&catalog).await;
    let result = waiting.await.unwrap();
    assert_eq!(result.reason, "status");
    assert_eq!(result.status.as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_restored_root_is_withdrawn_when_its_agent_left_the_catalog() {
    let (core, root) = test_core();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&test_detect_catalog()).await;
    adopt_onto(&core, &root, id, "crash", b"WORKING NOW\r\n");
    assert!(core.lock().screen_status[&id].unconfirmed_until.is_some());
    watch_sees(&core, id, agent("devin"));
    // Configured down to claude only: nothing here can re-prove devin's root.
    let catalog = crate::detect::Catalog::load(
        None,
        Some(&crate::detect::Limits { enabled: None, agents: Some(vec!["claude".into()]) }),
    );
    core.screen_status_collect(&catalog);
    assert_eq!(word(&core, id), None, "an agent outside the catalog loses the restored root at once");
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_restored_root_is_withdrawn_when_detection_is_off() {
    let (core, root) = test_core();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&test_detect_catalog()).await;
    adopt_onto(&core, &root, id, "crash", b"WORKING NOW\r\n");
    assert!(core.lock().screen_status[&id].unconfirmed_until.is_some());
    // `[detect] enabled = false`: the worker reconciles restored roots before
    // its empty-catalog early return, so the root drops now, not at 30 s.
    std::fs::write(&core.paths.config, "[detect]\nenabled = false\n").unwrap();
    core.clone().watch_screen_status().await;
    assert_eq!(word(&core, id), None, "detection off: withdrawn before the worker exits");
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn the_reconfirm_deadline_fires_while_stitching_or_synced() {
    for mode in ["stitch", "sync"] {
        let (core, root) = test_core();
        let catalog = test_detect_catalog();
        let (id, _frames) = screen_agent(&core, "devin");
        core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
        core.screen_status_pass(&catalog).await;
        adopt_onto(&core, &root, id, "crash", b"WORKING NOW\r\n");
        if mode == "stitch" {
            core.lock().stitching.insert(id);
        } else {
            core.output(&mut core.lock(), id, b"\x1b[?2026h");
        }
        tokio::time::advance(crate::screen_status::RECONFIRM + Duration::from_millis(500)).await;
        core.screen_status_pass(&catalog).await;
        assert_eq!(word(&core, id), None, "{mode}: the deadline fires without a snapshot");
        drop(core);
        remove_root(&root);
    }
}

#[tokio::test(start_paused = true)]
async fn a_kept_replacement_re_arms_detection() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    // It spoke natively once, and a recovery marker is still open.
    {
        let mut st = core.lock();
        st.screen_status.entry(id).or_default().unconfirmed_until =
            Some(tokio::time::Instant::now() + crate::screen_status::RECONFIRM);
        let t = st.reg.terminals.get_mut(&id).unwrap();
        t.keep = true;
        t.reported = true;
        (t.cols, t.rows) = (80, 24); // the fake never told its real size
    }
    // Lost with its holder: kept, it waits for a replacement shell — which the
    // fake "current" link lets recover_kept queue without a real spawn.
    {
        let mut st = core.lock();
        core.record_finish(&mut st, id, Exit { code: None, signal: None }, "holder_lost");
    }
    core.clone().recover_kept(true).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(!t.reported, "the replacement must re-earn native authority too");
        assert!(st.screen_status[&id].unconfirmed_until.is_none(), "the old deadline is gone");
    }
    assert!(frames.try_iter().any(|f| f.first() == Some(&proto::SPAWN)), "a spawn went to the holder");
    // The new shell is identified like any program: detection works again.
    core.lock().reg.terminals.get_mut(&id).unwrap().foreground = Some(agent("devin"));
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"), "a replacement re-earns inference");
    // The old deadline is gone for good: far past it, the new verdict stands.
    tokio::time::advance(crate::screen_status::RECONFIRM + Duration::from_secs(1)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_pass_in_flight_across_stop_finish_and_removal_dies() {
    // A restart request, a finish and a remove each reject a verdict already
    // taken — frozen commits, finished terminals and forgotten ones alike.
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    core.lock().freeze_screen_detection();
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id), None, "a frozen core takes no verdict");
    assert!(core.screen_status_collect(&catalog).is_empty(), "and collects nothing more");
    drop(core);
    remove_root(&root);

    for remove in [false, true] {
        let (core, root) = test_core();
        let catalog = test_detect_catalog();
        let (id, _frames) = screen_agent(&core, "devin");
        core.output(&mut core.lock(), id, b"NEED YES\r\n");
        let snaps = core.screen_status_collect(&catalog);
        assert_eq!(snaps.len(), 1);
        {
            let mut st = core.lock();
            core.record_finish(&mut st, id, Exit { code: Some(0), signal: None }, "exited");
            if remove {
                core.remove(&mut st, id);
            }
        }
        for (snap, detection) in crate::screen_status::evaluate(snaps) {
            core.screen_status_commit(&catalog, snap, detection);
        }
        let status = core.lock().reg.terminals.get(&id).and_then(|t| t.status.as_ref()).map(|s| s.state.clone());
        assert_eq!(status, None, "remove={remove}: a late commit never lands");
        drop(core);
        remove_root(&root);
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn restart_and_handover_freeze_detection() {
    // `server.restart` sets the flag and the freeze under the same lock.
    let (core, root) = test_core();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let call = crate::api::Call { core: &core, conn: None, plain: None };
    crate::api::dispatch(&call, "server.restart", json!({})).await.unwrap();
    assert!(core.restart.load(std::sync::atomic::Ordering::SeqCst));
    {
        let mut st = core.lock();
        assert!(st.screen_frozen, "a restart request freezes detection");
        core.inferred_status(&mut st, id, Some(("blocked", None)));
        assert!(st.reg.terminals[&id].status.is_none(), "frozen commits publish nothing");
        core.inferred_status(&mut st, id, None); // withdrawals gate the same way
    }
    assert!(core.screen_status_collect(&test_detect_catalog()).is_empty(), "and collection stops");
    // Native reports still apply under the freeze — finish accounting too.
    core.output(&mut core.lock(), id, b"\x1b]7501;state=idle\x1b\\");
    assert_eq!(word(&core, id).as_deref(), Some("idle"), "native reports stay live while frozen");
    drop(core);
    remove_root(&root);

    // The handover freeze goes down before the goodbye awaits.
    let (core, root) = test_core();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&test_detect_catalog()).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    core.handover().await.unwrap();
    assert!(core.lock().screen_frozen, "handover froze before saving screens");
    drop(core);
    remove_root(&root);
}
