use super::*;

/// A real reader failure with a reconnect held at the handshake. Paused
/// Tokio time lets several worker ticks pass inside the real handshake limit.
fn detaching_holder(returns: bool) -> (Arc<Core>, PathBuf, u32, mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (core, root) = test_core();
    let (id, _) = screen_agent(&core, "devin");
    core.lock().holders.clear();
    let welcome = proto::Welcome {
        version: proto::VERSION,
        instance: "fake".into(),
        pid: 0,
        build: crate::client::build_id(),
        previous: "crash".into(),
        terminals: vec![proto::HeldTerminal { id, pid: 4321, cols: 80, rows: 24, exit: None }],
    };
    let mut answer = proto::json_frame(proto::WELCOME, 0, &welcome);
    answer.extend(proto::frame(proto::TAIL, id, b"WORKING NOW\r\n"));
    let socket = root.join("run/core.holder.fake.sock");
    let listener = platform::listen(&socket).unwrap();
    let (act, actions) = mpsc::channel();
    let (ready, waiting) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        proto::read(&mut first).unwrap();
        io::Write::write_all(&mut first, &answer).unwrap();
        actions.recv_timeout(Duration::from_secs(5)).unwrap();
        // Keep the old socket draining until the core closes it.
        io::Write::write_all(&mut first, &GARBAGE).unwrap();
        while let Ok(Some(_)) = proto::read(&mut first) {}
        if !returns {
            // The second handshake fails: programs stay live, gated, until
            // an observed exit or the existing lost/finish path accounts them.
            let (mut second, _) = listener.accept().unwrap();
            proto::read(&mut second).unwrap();
            io::Write::write_all(&mut second, &GARBAGE).unwrap();
            ready.send(()).unwrap();
            return;
        }
        let (mut second, _) = listener.accept().unwrap();
        proto::read(&mut second).unwrap();
        ready.send(()).unwrap();
        actions.recv_timeout(Duration::from_secs(5)).unwrap();
        io::Write::write_all(&mut second, &answer).unwrap();
        while let Ok(Some(_)) = proto::read(&mut second) {}
    });
    core.take_over(platform::connect(&socket).unwrap(), false, &socket, None).unwrap();
    (core, root, id, act, waiting)
}

fn wait_for(core: &Core, predicate: impl Fn(&State) -> bool) {
    let start = Instant::now();
    while !predicate(&core.lock()) {
        assert!(start.elapsed() < Duration::from_secs(5), "holder reader did not reach the boundary");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[tokio::test(start_paused = true)]
async fn a_holder_detach_relink_gates_ticks_then_reconfirms_silently() {
    let (core, root, id, act, waiting) = detaching_holder(true);
    let catalog = test_detect_catalog();
    core.screen_status_pass(&catalog).await;
    // A pending idle must not mature while the holder is absent.
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HREADY>\r\n");
    core.screen_status_pass(&catalog).await;
    let (turn, news, seq) = {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert_eq!(t.status.as_ref().map(|s| s.state.as_str()), Some("working"));
        (t.turn, t.news, st.events.seq)
    };
    act.send(()).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..4 {
        tokio::time::advance(Duration::from_millis(500)).await;
        assert!(core.screen_status_collect(&catalog).is_empty());
        core.inferred_status(&mut core.lock(), id, None);
        assert_eq!(word(&core, id).as_deref(), Some("working"));
    }
    // Expiry is gated too, even if a restored deadline falls in the gap.
    core.lock().screen_status.get_mut(&id).unwrap().unconfirmed_until = Some(tokio::time::Instant::now());
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    act.send(()).unwrap();
    wait_for(&core, |st| st.link(id).is_some());
    assert!(core.lock().screen_status[&id].unconfirmed_until.is_some());
    // Model the first actual observation after this adoption.
    {
        let mut st = core.lock();
        let was = st.reg.terminals[&id].foreground.clone();
        core.foreground_changed(&mut st, id, was.as_ref());
    }
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert_eq!(t.status.as_ref().map(|s| s.state.as_str()), Some("working"));
        assert!(st.screen_status[&id].unconfirmed_until.is_none());
        assert_eq!((t.turn, t.news, st.events.seq), (turn, news, seq));
    }
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_pass_in_flight_across_holder_detach_is_rejected() {
    let (core, root, id, act, waiting) = detaching_holder(true);
    let catalog = test_detect_catalog();
    let verdicts = crate::screen_status::evaluate(core.screen_status_collect(&catalog));
    let late = crate::screen_status::evaluate(core.screen_status_collect(&catalog));
    let seq = core.lock().output_seq[&id];
    assert_eq!(verdicts.len(), 1);
    act.send(()).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    for (snap, verdict) in verdicts {
        core.screen_status_commit(&catalog, snap, verdict);
    }
    assert_eq!(word(&core, id), None);
    act.send(()).unwrap();
    wait_for(&core, |st| st.link(id).is_some());
    // Isolate generation invalidation: all the token's other facts match
    // again, including output_seq, as if the old link returned unchanged.
    core.lock().output_seq.insert(id, seq);
    for (snap, verdict) in late {
        core.screen_status_commit(&catalog, snap, verdict);
    }
    assert_eq!(word(&core, id), None, "detaching invalidated the old generation");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_holder_that_never_returns_finishes_without_an_inferred_turn() {
    let (core, root, id, act, waiting) = detaching_holder(false);
    let catalog = test_detect_catalog();
    core.screen_status_pass(&catalog).await;
    let (turn, news, seq) = {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        (t.turn, t.news, st.events.seq)
    };
    act.send(()).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_for(&core, |st| st.disconnected.contains_key("fake"));
    for _ in 0..4 {
        tokio::time::advance(Duration::from_millis(500)).await;
        core.screen_status_pass(&catalog).await;
    }
    {
        let mut st = core.lock();
        assert_eq!(st.events.seq, seq, "no inferred null or turn before finish");
        core.record_finish(&mut st, id, Exit { code: None, signal: None }, "holder_lost");
        let t = &st.reg.terminals[&id];
        assert_eq!(t.reason.as_deref(), Some("holder_lost"));
        assert_eq!(t.turn, turn + 1, "only normal finish accounting ends the working turn");
        assert!(t.news > news);
        assert!(t.status.is_none());
    }
    core.screen_status_pass(&catalog).await;
    assert_eq!(core.lock().reg.terminals[&id].turn, turn + 1);
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn native_reports_stay_live_during_holder_detach() {
    let (core, root, id, act, waiting) = detaching_holder(false);
    let catalog = test_detect_catalog();
    core.screen_status_pass(&catalog).await;
    act.send(()).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_for(&core, |st| st.disconnected.contains_key("fake"));
    core.output(&mut core.lock(), id, b"\x1b]7501;state=blocked\x1b\\");
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    assert!(core.lock().reg.terminals[&id].reported);
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn holderless_detection_waits_without_a_detach_record() {
    let (core, root) = test_core();
    let (id, _) = screen_agent(&core, "devin");
    let catalog = test_detect_catalog();
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    let seq = core.lock().events.seq;
    core.lock().holders.clear();
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    assert_eq!(core.lock().events.seq, seq);
    drop(core);
    remove_root(&root);
}
