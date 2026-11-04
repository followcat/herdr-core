use super::*;
use serde_json::json;

#[tokio::test(start_paused = true)]
async fn keys_pressed_during_a_paste_follow_its_submit() {
    use crate::viewers::{AttachRequest, Display};
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let (core, root) = test_core();
    let (writer, frames) = mpsc::channel();
    let (bye, _) = mpsc::channel();
    let id = {
        let mut st = core.lock();
        let id =
            st.reg.create(crate::terminals::NewTerminal { argv: vec!["sh".into()], ..Default::default() }, 0).unwrap();
        st.reg.terminals.get_mut(&id).unwrap().holder = Some("fake".into());
        st.screens.insert(id, Screen::new(80, 24, 1 << 20));
        st.holders.push(Link { writer, pid: 0, instance: "fake".into(), current: true, bye, bye_rx: None });
        id
    };
    let person = Arc::new(crate::wire::Conn::default());
    let channel = core
        .attach(
            &person,
            id,
            AttachRequest {
                observe: false,
                resize_notifications: false,
                history: 0,
                size: (80, 24),
                display: Display::default(),
                resumed: false,
            },
        )
        .unwrap();
    // Ignore attach's resize frames. From here the fake holder sees only input.
    while frames.try_recv().is_ok() {}
    core.output(&mut core.lock(), id, b"\x1b]7501;state=working\x1b\\");
    {
        let sending = core.send(id, "abc", false, None);
        tokio::pin!(sending);
        poll_fn(|cx| {
            assert!(sending.as_mut().poll(cx).is_pending(), "the paste must pause before submit");
            Poll::Ready(())
        })
        .await;
        assert_eq!(frames.try_recv().unwrap(), proto::frame(proto::INPUT, id, b"abc"));
        assert!(frames.try_recv().is_err());
        core.viewer_input(&person, channel, b"Z", false);
        assert!(frames.try_recv().is_err(), "the person's keys must stay held");
        core.output(&mut core.lock(), id, b"\x1b]7501;state=done\x1b\\");
        tokio::time::advance(crate::input::PASTE_GAP).await;
        sending.await.unwrap();
        assert_eq!(frames.try_recv().unwrap(), proto::frame(proto::INPUT, id, b"\r"));
        assert_eq!(frames.try_recv().unwrap(), proto::frame(proto::INPUT, id, b"Z"));
    }
    // The injected Enter's acknowledgement arrives after the held draft was flushed.
    core.output(&mut core.lock(), id, b"\x1b]7501;state=working\x1b\\");
    {
        let next = core.type_in(id, Duration::ZERO, true, |_, _| Ok(((), vec![])));
        tokio::pin!(next);
        poll_fn(|cx| {
            assert!(next.as_mut().poll(cx).is_pending(), "the delayed working report must preserve the fresh draft");
            Poll::Ready(())
        })
        .await;
        assert!(core.lock().gates[&id].waiting_for_person);
    }
    drop(core);
    remove_root(&root);
}

#[tokio::test]
async fn server_exit_flushes_even_when_background_tasks_still_own_core() {
    let (mut core, root) = test_core();
    core.flush_saved().await.unwrap();
    let (release, blocked) = mpsc::channel();
    Arc::get_mut(&mut core).unwrap().registry_writer = RegistryWriter::start(move |_| {
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    })
    .unwrap();
    let background_owner = core.clone();
    core.save(&core.lock());
    let finish = crate::server::finish(&core);
    tokio::pin!(finish);
    let waited = tokio::time::timeout(Duration::from_millis(10), &mut finish).await.is_err();
    release.send(()).unwrap();
    if waited {
        finish.await.unwrap();
    }
    drop(background_owner);
    remove_root(&root);
    assert!(waited, "serve returned before its queued snapshot finished");
}

#[tokio::test]
async fn server_stop_succeeds_even_when_the_registry_flush_times_out() {
    let (mut core, root) = test_core();
    core.flush_saved().await.unwrap();
    let (release, blocked) = mpsc::channel();
    let mut first = true;
    Arc::get_mut(&mut core).unwrap().registry_writer = RegistryWriter::start(move |_| {
        if first {
            first = false;
            blocked.recv_timeout(Duration::from_secs(15)).unwrap();
        }
        Ok(())
    })
    .unwrap();
    let call = crate::api::Call { core: &core, conn: None, plain: None };
    let started = Instant::now();
    let stop = crate::api::dispatch(&call, "server.stop", json!({})).await;
    release.send(()).unwrap();
    core.flush_saved().await.unwrap();
    drop(core);
    remove_root(&root);
    assert!(started.elapsed() >= Duration::from_secs(10), "stuck disks must still time out");
    assert_eq!(stop.unwrap()["stopped"], true);
}

fn test_core() -> (Arc<Core>, PathBuf) {
    let root = std::env::temp_dir().join(format!("hc-persistence-{}", platform::random_hex()));
    let paths = Paths {
        state: root.join("state"),
        config: root.join("config.toml"),
        socket: root.join("run/core.sock"),
        holder_socket: root.join("run/core.holder.sock"),
    };
    paths.create_dirs().unwrap();
    let core = Core::start(paths, CoreInfo { id: "test".into(), name: "test".into() }).unwrap();
    (core, root)
}

#[tokio::test]
async fn label_and_kill_reply_with_the_record_observed_before_flushing() {
    for method in ["terminal.kill", "terminal.label"] {
        let (mut core, root) = test_core();
        core.flush_saved().await.unwrap();
        let (release, blocked) = mpsc::channel();
        Arc::get_mut(&mut core).unwrap().registry_writer = RegistryWriter::start(move |_| {
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
        let id = {
            let mut st = core.lock();
            let id = st
                .reg
                .create(crate::terminals::NewTerminal { argv: vec!["sh".into()], ..Default::default() }, now_ms())
                .unwrap();
            st.reg.finish(id, Some(0), None, "exited", now_ms());
            core.save(&st);
            id
        };
        let reply = {
            let call = crate::api::Call { core: &core, conn: None, plain: None };
            let request =
                crate::api::dispatch(&call, method, json!({"terminal": crate::terminals::address(id), "labels": {}}));
            tokio::pin!(request);
            assert!(tokio::time::timeout(Duration::from_millis(10), &mut request).await.is_err());
            core.lock().reg.terminals.get_mut(&id).unwrap().turn = 1;
            // Label queued a second snapshot; kill reused the existing one.
            release.send(()).unwrap();
            let _ = release.send(());
            request.await.unwrap()
        };
        let captured = reply["turn"].clone();
        drop(core);
        remove_root(&root);
        assert_eq!(captured, 0, "{method} returned a turn observed after its barrier: {reply}");
    }
}

#[cfg(unix)]
#[test]
fn a_blocked_registry_write_does_not_hold_the_core_lock() {
    use std::os::unix::ffi::OsStrExt;
    let root = PathBuf::from(format!("/var/tmp/hc-save-lock-{}", platform::random_hex()));
    let paths = Paths {
        state: root.join("state"),
        config: root.join("config.toml"),
        socket: root.join("run/core.sock"),
        holder_socket: root.join("run/core.holder.sock"),
    };
    paths.create_dirs().unwrap();
    let core = Core::start(paths, CoreInfo { id: "test".into(), name: "test".into() }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !core.paths.state.join("terminals.json").exists() {
        assert!(Instant::now() < deadline, "startup registry was not written");
        std::thread::sleep(Duration::from_millis(1));
    }
    let tmp = core.paths.state.join("terminals.tmp");
    let name = std::ffi::CString::new(tmp.as_os_str().as_bytes()).unwrap();
    // Opening this temporary file for writing waits for a reader, like a stalled disk.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (saved, done) = mpsc::channel();
    let saving = core.clone();
    let worker = std::thread::spawn(move || {
        saving.save(&saving.lock());
        saved.send(()).unwrap();
    });
    let returned = done.recv_timeout(Duration::from_millis(250)).is_ok();
    let unlocked = core.state.try_lock().is_ok();
    // Always release the blocked write, even when checking the old synchronous implementation.
    let reader = std::fs::File::open(&tmp).unwrap();
    worker.join().unwrap();
    drop(core);
    drop(reader);
    remove_root(&root);
    assert!(returned && unlocked, "saving to a stalled disk must leave output and input free to take the core lock");
}

#[test]
fn host_shutdown_is_lost_without_overriding_explicit_actions() {
    for (killed, stopping, shutdown, expected) in [
        (false, false, false, "exited"),
        (false, false, true, "host_shutdown"),
        (false, true, false, "server_stop"),
        (false, true, true, "server_stop"),
        (true, false, false, "killed"),
        (true, false, true, "killed"),
        (true, true, false, "killed"),
        (true, true, true, "killed"),
    ] {
        assert_eq!(finish_reason(killed, stopping, shutdown), expected);
    }
}

#[cfg(unix)]
#[test]
fn a_new_holder_of_another_build_reports_both_identities() {
    let root = PathBuf::from(format!("/var/tmp/hc-holder-build-{}", platform::random_hex()));
    let paths = Paths {
        state: root.join("state"),
        config: root.join("config.toml"),
        socket: root.join("run/core.sock"),
        holder_socket: root.join("run/core.holder.sock"),
    };
    paths.create_dirs().unwrap();
    let core = Core::start(paths, CoreInfo { id: "test".into(), name: "test".into() }).unwrap();
    let (stream, mut holder) = UnixStream::pair().unwrap();
    let fake = std::thread::spawn(move || {
        assert_eq!(proto::read(&mut holder).unwrap().unwrap().0, proto::HELLO);
        let welcome = proto::Welcome {
            version: proto::VERSION,
            instance: "other".into(),
            pid: 0,
            build: "other-build".into(),
            previous: "none".into(),
            terminals: vec![],
        };
        io::Write::write_all(&mut holder, &proto::json_frame(proto::WELCOME, 0, &welcome)).unwrap();
        assert_eq!(proto::read(&mut holder).unwrap().unwrap().0, proto::DRAIN);
    });
    let err = core.take_over(stream, true, &root.join("run/core.holder.x.sock"), None).unwrap_err().to_string();
    assert!(err.contains("other-build"), "{err}");
    assert!(err.contains(&crate::client::build_id()), "{err}");
    assert!(core.lock().holders.is_empty());
    fake.join().unwrap();
    drop(core);
    remove_root(&root);
}

/// A frame no core can read: the reader of a link that gets it fails.
#[cfg(unix)]
const GARBAGE: [u8; 9] = [proto::OUTPUT, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];

/// A core with one live terminal on a fake holder. Each connection's hello is
/// answered as told ("welcome", "garbage", or both), and the fake reports
/// what it saw: connection number, then "hello", "no hello" or "closed".
#[cfg(unix)]
fn on_fake_holder(root: &std::path::Path, answers: &[&str]) -> (Arc<Core>, u32, mpsc::Receiver<String>) {
    let paths = Paths {
        state: root.join("state"),
        config: root.join("config.toml"),
        socket: root.join("run/core.sock"),
        holder_socket: root.join("run/core.holder.sock"),
    };
    paths.create_dirs().unwrap();
    let core = Core::start(paths, CoreInfo { id: "test".into(), name: "test".into() }).unwrap();
    let id = {
        let mut st = core.lock();
        let new = crate::terminals::NewTerminal { argv: vec!["sh".into()], ..Default::default() };
        let id = st.reg.create(new, now_ms()).unwrap();
        st.reg.terminals.get_mut(&id).unwrap().holder = Some("fake".into());
        id
    };
    let held = proto::HeldTerminal { id, pid: 0, cols: 80, rows: 24, exit: None };
    let welcome = proto::Welcome {
        version: proto::VERSION,
        instance: "fake".into(),
        pid: 0,
        build: crate::client::build_id(),
        previous: "crash".into(),
        terminals: vec![held],
    };
    let mut welcome = proto::json_frame(proto::WELCOME, 0, &welcome);
    welcome.extend(proto::frame(proto::TAIL, id, b""));
    let socket = root.join("run/core.holder.fake.sock");
    let listener = platform::listen(&socket).unwrap();
    let answers: Vec<Vec<u8>> = answers
        .iter()
        .map(|a| {
            let mut frames = if a.contains("welcome") { welcome.clone() } else { vec![] };
            if a.contains("garbage") {
                frames.extend(GARBAGE);
            }
            frames
        })
        .collect();
    let (heard, reports) = mpsc::channel();
    std::thread::spawn(move || {
        for (n, (stream, answer)) in listener.incoming().zip(answers).enumerate() {
            let (mut stream, heard) = (stream.unwrap(), heard.clone());
            std::thread::spawn(move || {
                let Ok(Some(_)) = proto::read(&mut stream) else {
                    return heard.send(format!("{n} no hello")).unwrap();
                };
                heard.send(format!("{n} hello")).unwrap();
                std::thread::sleep(Duration::from_millis(50)); // time for the test to act, well within RELINK_WAIT
                let _ = io::Write::write_all(&mut stream, &answer);
                while let Ok(Some(_)) = proto::read(&mut stream) {} // the core's frames, until it closes
                let _ = heard.send(format!("{n} closed"));
            });
        }
    });
    core.take_over(platform::connect(&socket).unwrap(), false, &socket, None).unwrap();
    (core, id, reports)
}

#[cfg(unix)]
fn reported(reports: &mpsc::Receiver<String>, n: usize) -> Vec<String> {
    let mut seen: Vec<String> = (0..n).map_while(|_| reports.recv_timeout(Duration::from_secs(5)).ok()).collect();
    seen.sort();
    seen
}

/// Live and not ended by losing its holder: left for whoever adopts it next.
#[cfg(unix)]
fn still_live(core: &Core, id: u32) -> bool {
    core.lock().reg.terminals.get(&id).is_some_and(|t| t.is_live() && t.reason.is_none())
}

#[cfg(unix)]
#[test]
fn a_link_whose_reading_fails_reconnects_but_not_twice_in_a_row() {
    let root = PathBuf::from(format!("/var/tmp/hc-holder-relink-{}", platform::random_hex()));
    let (core, id, reports) = on_fake_holder(&root, &["welcome garbage", "welcome garbage", ""]);
    assert_eq!(
        reported(&reports, 5),
        ["0 closed", "0 hello", "1 closed", "1 hello", "2 no hello"],
        "each failed link closes; the first failure reconnects, the second soon after does not"
    );
    assert!(core.lock().holders.is_empty());
    assert!(still_live(&core, id));
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[test]
fn a_reconnect_the_holder_never_answers_gives_up_and_frees_holder_start() {
    let root = PathBuf::from(format!("/var/tmp/hc-holder-relink-{}", platform::random_hex()));
    let (core, id, reports) = on_fake_holder(&root, &["welcome garbage", ""]);
    assert_eq!(reported(&reports, 4), ["0 closed", "0 hello", "1 closed", "1 hello"]);
    let start = Instant::now();
    while core.holder_start.try_lock().is_err() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(core.holder_start.try_lock().is_ok(), "runs and stops can start holders again");
    assert!(still_live(&core, id));
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[test]
fn a_reconnect_whose_handshake_is_done_is_not_cut_however_slow_the_rest() {
    let root = PathBuf::from(format!("/var/tmp/hc-holder-relink-{}", platform::random_hex()));
    let (core, id, reports) = on_fake_holder(&root, &["welcome garbage", "welcome"]);
    while reports.recv_timeout(Duration::from_secs(5)).unwrap() != "1 hello" {}
    // Adopting waits for this lock: slow, but after the welcome came in time.
    let st = core.lock();
    std::thread::sleep(RELINK_WAIT * 3);
    drop(st);
    let start = Instant::now();
    while core.lock().holders.is_empty() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(RELINK_WAIT * 2);
    assert_eq!(core.lock().holders.len(), 1, "the new link stays");
    assert!(reports.try_iter().all(|r| r != "1 closed"), "and is not cut");
    assert!(still_live(&core, id));
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[test]
fn a_reconnect_that_fails_leaves_the_holders_terminals_live() {
    let root = PathBuf::from(format!("/var/tmp/hc-holder-relink-{}", platform::random_hex()));
    let (core, id, reports) = on_fake_holder(&root, &["welcome garbage", "garbage"]);
    assert_eq!(reported(&reports, 4), ["0 closed", "0 hello", "1 closed", "1 hello"]);
    assert!(core.lock().holders.is_empty());
    assert!(still_live(&core, id), "its holder still answers: nothing ends");
    drop(core);
    remove_root(&root);
}

#[cfg(unix)]
#[tokio::test]
async fn waiting_for_holder_start_does_not_block_the_core_executor() {
    let root = PathBuf::from(format!("/var/tmp/hc-holder-start-{}", platform::random_hex()));
    let paths = Paths {
        state: root.join("state"),
        config: root.join("config.toml"),
        socket: root.join("run/core.sock"),
        holder_socket: root.join("run/core.holder.sock"),
    };
    paths.create_dirs().unwrap();
    let core = Core::start(paths, CoreInfo { id: "test".into(), name: "test".into() }).unwrap();
    let (ready, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let c = core.clone();
    let blocker = std::thread::spawn(move || {
        let _starting = c.holder_start.lock().unwrap();
        ready.send(()).unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(1));
        // Prevent any real holder from starting, including if the regression stalls the executor.
        c.lock().stopping = true;
    });
    entered.recv().unwrap();
    let c = core.clone();
    let start = tokio::spawn(async move { c.ensure_holder().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let answered_while_starting = !core.lock().stopping;
    let _ = release.send(());
    blocker.join().unwrap();
    assert!(start.await.unwrap().is_err());
    drop(core);
    remove_root(&root);
    assert!(answered_while_starting, "holder startup blocked the core executor");
}

/// A live terminal on a fake holder, old enough that `send`'s settle passes.
fn terminal_on_fake_holder(core: &Core) -> (u32, mpsc::Receiver<Vec<u8>>) {
    let (writer, frames) = mpsc::channel();
    let (bye, _) = mpsc::channel();
    let mut st = core.lock();
    let id = st.reg.create(crate::terminals::NewTerminal { argv: vec!["sh".into()], ..Default::default() }, 0).unwrap();
    {
        let t = st.reg.terminals.get_mut(&id).unwrap();
        t.holder = Some("fake".into());
        t.pid = Some(4321);
        t.created_ms = 0;
    }
    st.screens.insert(id, Screen::new(80, 24, 1 << 20));
    st.holders.push(Link { writer, pid: 0, instance: "fake".into(), current: true, bye, bye_rx: None });
    (id, frames)
}

#[tokio::test(start_paused = true)]
async fn an_inferred_root_is_observed_and_waited_on_but_never_esc() {
    let (core, root) = test_core();
    let (id, frames) = terminal_on_fake_holder(&core);
    core.inferred_status(&mut core.lock(), id, Some(("working", None)));
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert_eq!(t.status.as_ref().unwrap().state, "working");
        assert_eq!(t.status.as_ref().unwrap().source.as_deref(), Some("screen"));
        assert!(!t.reported, "inference never claims a report");
    }
    // send observes the inferred turn like a reported one.
    let c = core.clone();
    let sending = tokio::spawn(async move { c.send(id, "go on", false, None).await });
    tokio::time::advance(Duration::from_millis(500)).await;
    let sent = sending.await.unwrap().unwrap();
    assert_eq!(sent.started, Some(true), "an inferred status is observed like a report");

    // a wait follows the inferred word's changes.
    let c = core.clone();
    let waiting = tokio::spawn(async move {
        c.wait(id, crate::wait::WaitSpec { output: None, timeout: Duration::from_secs(30), after_turn: None }, None)
            .await
    });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    core.inferred_status(&mut core.lock(), id, Some(("idle", None)));
    let result = waiting.await.unwrap();
    assert_eq!(result.reason, "status");
    assert_eq!(result.status.as_deref(), Some("idle"));
    assert_eq!(result.turn, 1, "working → idle ended one turn");

    // interrupt still picks by `reported`: unreported means ctrl+c, not esc.
    let c = core.clone();
    let interrupting = tokio::spawn(async move { c.interrupt(id, Duration::ZERO).await });
    for _ in 0..3 {
        for tx in core.lock().foreground.remove(&id).unwrap_or_default() {
            let _ = tx.send(-1);
        }
        tokio::task::yield_now().await;
    }
    let (outcome, key) = interrupting.await.unwrap().unwrap();
    assert_eq!(key, "ctrl+c", "an inferred status must not switch interrupt to esc");
    assert!(matches!(outcome, crate::input::Interrupted::Unknown));
    let typed = std::iter::from_fn(|| frames.try_recv().ok()).collect::<Vec<_>>();
    assert!(typed.contains(&proto::frame(proto::INPUT, id, b"\x03")), "the holder got ctrl+c");

    // A native report then owns the word: same observations, but esc now.
    core.output(&mut core.lock(), id, b"\x1b]7501;state=working\x1b\\");
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.reported && t.status.as_ref().is_some_and(|s| s.source.is_none()));
        assert_eq!(t.turn, 1, "the native root replaced the inferred one: entering working ends no turn");
    }
    core.inferred_status(&mut core.lock(), id, Some(("blocked", None)));
    assert_eq!(core.lock().reg.terminals[&id].status.as_ref().unwrap().state, "working", "reported keeps the word");
    let c = core.clone();
    let interrupting = tokio::spawn(async move { c.interrupt(id, Duration::ZERO).await });
    let (outcome, key) = interrupting.await.unwrap().unwrap();
    assert_eq!(key, "esc");
    assert!(matches!(outcome, crate::input::Interrupted::StillWorking));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn without_status_a_program_stays_invisible_to_these_paths() {
    let (core, root) = test_core();
    let (id, _frames) = terminal_on_fake_holder(&core);
    let c = core.clone();
    let sent = tokio::spawn(async move { c.send_first_task(id, "first task").await });
    tokio::time::advance(Duration::from_millis(500)).await;
    let sent = sent.await.unwrap().unwrap();
    assert_eq!(sent.started, None, "no status: nobody can tell whether it started");
    assert!(core.lock().reg.terminals[&id].first_task_turn.is_some());

    // And a wait treats it as working until it exits.
    let c = core.clone();
    let waiting = tokio::spawn(async move {
        c.wait(id, crate::wait::WaitSpec { output: None, timeout: Duration::from_millis(200), after_turn: None }, None)
            .await
    });
    tokio::time::advance(Duration::from_millis(300)).await;
    assert_eq!(waiting.await.unwrap().reason, "timeout");

    // First inferred evidence finishes the sent first task like a report would.
    core.inferred_status(&mut core.lock(), id, Some(("blocked", Some("permission"))));
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.first_task_turn.is_none(), "an inferred blocker finishes the first task");
        assert_eq!(t.news, 1);
    }
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn leaving_an_inferred_blocked_releases_the_persons_draft() {
    let (core, root) = test_core();
    let (id, _frames) = terminal_on_fake_holder(&core);
    core.inferred_status(&mut core.lock(), id, Some(("blocked", Some("permission"))));
    core.lock().gates.entry(id).or_default().viewer(b"draft text", true);
    let c = core.clone();
    let typing = tokio::spawn(async move { c.type_in(id, Duration::ZERO, true, |_, _| Ok(((), vec![]))).await });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert!(core.lock().gates[&id].waiting_for_person, "the draft holds agent input");
    core.inferred_status(&mut core.lock(), id, Some(("idle", None)));
    tokio::time::advance(Duration::from_millis(50)).await;
    typing.await.unwrap().unwrap();
    drop(core);
    remove_root(&root);
}

#[test]
fn a_viewers_paint_replays_native_status_never_inferred() {
    let (core, root) = test_core();
    let (id, _frames) = terminal_on_fake_holder(&core);
    core.inferred_status(&mut core.lock(), id, Some(("working", None)));
    {
        let st = core.lock();
        let paint = crate::viewers::first_paint(&st, id, 0);
        assert!(!paint.windows(4).any(|w| w == b"7501"), "inference is never replayed as OSC");
    }
    core.output(&mut core.lock(), id, b"\x1b]7501;state=done:id=sub\x1b\\");
    {
        let st = core.lock();
        let paint = crate::viewers::first_paint(&st, id, 0);
        assert!(paint.windows(4).any(|w| w == b"7501"), "native records still reach a new viewer");
    }
    drop(core);
    remove_root(&root);
}

// --- screen_status worker (fork) ----------------------------------------

/// A manifest that maps marker strings to verdicts, loaded as a `devin`
/// override so `identify` and the catalog meet on the same canonical id.
const TEST_MANIFEST: &str = r#"
id = "devin"

[[rules]]
id = "marks_viewer"
state = "unknown"
skip_state_update = true
priority = 300
region = "whole_recent"
contains = ["TRANSCRIPT VIEW"]

[[rules]]
id = "marks_blocked"
state = "blocked"
kind = "permission"
priority = 200
region = "whole_recent"
contains = ["NEED YES"]

[[rules]]
id = "marks_working"
state = "working"
priority = 100
region = "whole_recent"
contains = ["WORKING NOW"]

[[rules]]
id = "marks_idle"
state = "idle"
priority = 50
region = "whole_recent"
contains = ["READY>"]
"#;

fn test_detect_catalog() -> crate::detect::Catalog {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hc-detect-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("devin.toml"), TEST_MANIFEST).unwrap();
    let catalog = crate::detect::Catalog::load(Some(&dir), None);
    let _ = std::fs::remove_dir_all(&dir);
    catalog
}

fn agent(name: &str) -> crate::foreground::Foreground {
    let mut fg = crate::foreground::Foreground {
        pid: 100,
        name: name.into(),
        argv: vec![name.into()],
        cwd: "/work".into(),
        agent: None,
    };
    fg.agent = crate::detect::identify(&fg); // what `about()` saw at observation
    fg
}

/// A live terminal on a fake holder whose foreground identifies as `agent`.
fn screen_agent(core: &Core, name: &str) -> (u32, mpsc::Receiver<Vec<u8>>) {
    let (id, frames) = terminal_on_fake_holder(core);
    core.lock().reg.terminals.get_mut(&id).unwrap().foreground = Some(agent(name));
    (id, frames)
}

fn word(core: &Core, id: u32) -> Option<String> {
    core.lock().reg.terminals[&id].status.as_ref().map(|s| s.state.clone())
}

#[tokio::test(start_paused = true)]
async fn a_screen_blocker_publishes_at_once_working_too() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        let status = st.reg.terminals[&id].status.as_ref().unwrap();
        assert_eq!(status.state, "blocked");
        assert_eq!(status.kind.as_deref(), Some("permission"));
        assert_eq!(status.source.as_deref(), Some("screen"));
        assert!(!st.reg.terminals[&id].reported);
    }
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HWORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_screen_confirms_idle_past_its_deadline() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"READY>\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None, "idle waits for the settle deadline");
    tokio::time::advance(Duration::from_millis(1600)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("idle"), "unchanged screen past the deadline confirms idle");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_redraw_between_snapshot_and_commit_drops_the_verdict() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"READY>\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HNEED YES\r\n"); // the screen moved under us
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id), None, "a stale verdict is rejected at commit");
    core.screen_status_pass(&catalog).await; // next pass sees the new screen
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_native_report_between_snapshot_and_commit_wins() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    core.output(&mut core.lock(), id, b"\x1b]7501;state=working\x1b\\"); // the program spoke first
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    let st = core.lock();
    let status = st.reg.terminals[&id].status.as_ref().unwrap();
    assert_eq!(status.state, "working");
    assert!(status.source.is_none(), "the native report owns the root, not the raced verdict");
    drop(st);
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn synchronized_output_and_stitching_freeze_detection() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    // A pending idle first.
    core.output(&mut core.lock(), id, b"READY>\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None);
    // Synchronized output cancels the pending idle: mid-batch rows are not evidence.
    tokio::time::advance(Duration::from_millis(200)).await;
    core.output(&mut core.lock(), id, b"\x1b[?2026h");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None);
    // Sync off again: the pending settle restarts, it did not survive at T.
    tokio::time::advance(Duration::from_millis(100)).await;
    core.output(&mut core.lock(), id, b"\x1b[?2026l");
    core.screen_status_pass(&catalog).await;
    tokio::time::advance(Duration::from_millis(1400)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None, "the sync-canceled settle restarted instead of confirming early");
    tokio::time::advance(Duration::from_millis(200)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("idle"));

    // Stitching freezes the pass entirely: no snapshot while the live view is read.
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HNEED YES\r\n");
    core.lock().stitching.insert(id);
    assert!(core.screen_status_collect(&catalog).is_empty(), "no snapshot while stitching");
    // Completion invalidates the cached pass exactly like stitch.rs does.
    {
        let mut st = core.lock();
        st.stitching.remove(&id);
        st.invalidate_screen(id);
    }
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_resize_stales_the_token_and_drops_the_pending_settle() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"READY>\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    // What apply_size does to the screen and the record.
    {
        let mut st = core.lock();
        st.screens.get_mut(&id).unwrap().resize(100, 40);
        let t = st.reg.terminals.get_mut(&id).unwrap();
        t.cols = 100;
        t.rows = 40;
    }
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id), None, "a resized screen makes the verdict stale");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_resize_there_and_back_still_kills_the_verdict() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    // Narrower and straight back with no output: dimensions and output_seq
    // end identical, yet the invalidation generation moved — the verdict
    // the old screen produced must still die at commit.
    for (cols, rows) in [(100, 40), (80, 24)] {
        let mut st = core.lock();
        st.screens.get_mut(&id).unwrap().resize(cols, rows);
        let t = st.reg.terminals.get_mut(&id).unwrap();
        t.cols = cols;
        t.rows = rows;
        st.invalidate_screen(id);
    }
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id), None, "an invalidated verdict dies even when the token fields match again");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_foreground_there_and_back_still_kills_the_verdict() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    // devin → bash → the same devin: the foreground token ends identical,
    // yet both changes bumped the generation — the verdict still dies.
    {
        let mut st = core.lock();
        let devin = st.reg.terminals[&id].foreground.clone();
        let was = st.reg.terminals.get_mut(&id).unwrap().foreground.replace(agent("bash"));
        core.foreground_changed(&mut st, id, was.as_ref());
        let was = st.reg.terminals.get_mut(&id).unwrap().foreground.replace(devin.unwrap());
        core.foreground_changed(&mut st, id, was.as_ref());
    }
    for (snap, detection) in crate::screen_status::evaluate(snaps) {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id), None, "a foreground excursion kills the in-flight verdict");
    // And the next pass still detects on the current screen.
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn unknown_never_times_out_into_idle_and_cancels_the_pending_one() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"plain untracked text\r\n");
    core.screen_status_pass(&catalog).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None, "unknown stays unknown forever");

    // Idle pending, then unknown evidence: the pending dies with it.
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HREADY>\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None);
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[Hsomething unrecognized now\r\n");
    core.screen_status_pass(&catalog).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None, "the canceled pending must not confirm later");

    // A skip rule (a transcript view) behaves the same: pending dies, verdict stands.
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HWORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HREADY>\r\n");
    core.screen_status_pass(&catalog).await; // starts a fresh pending idle
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HTRANSCRIPT VIEW\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"), "a skip leaves the published verdict alone");
    tokio::time::advance(Duration::from_secs(60)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"), "skip canceled the pending too");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn foreground_loss_and_replacement_withdraw_never_done() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));

    // The agent is replaced by a plain shell: the verdict belonged to devin.
    {
        let mut st = core.lock();
        let t = st.reg.terminals.get_mut(&id).unwrap();
        let was = t.foreground.replace(agent("bash"));
        core.foreground_changed(&mut st, id, was.as_ref());
    }
    assert_eq!(word(&core, id), None, "a new foreground owns the screen now");
    // The watch entry stays — its invalidation generation must outlive the
    // change so a verdict snapshotted before it still dies at commit.
    assert!(core.lock().screen_status.contains_key(&id));
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None, "a shell is never inferred");

    // The foreground leaving entirely withdraws too — never means "done".
    core.lock().reg.terminals.get_mut(&id).unwrap().foreground = Some(agent("devin"));
    core.output(&mut core.lock(), id, b"\x1b[2J\x1b[HNEED YES\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    {
        let mut st = core.lock();
        let t = st.reg.terminals.get_mut(&id).unwrap();
        let was = t.foreground.take();
        core.foreground_changed(&mut st, id, was.as_ref());
    }
    assert_eq!(word(&core, id), None, "foreground loss is not completion");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_cwd_move_is_not_a_new_program() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"READY>\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id), None); // pending
    {
        let mut st = core.lock();
        let t = st.reg.terminals.get_mut(&id).unwrap();
        let mut moved = t.foreground.clone().unwrap();
        moved.cwd = "/elsewhere".into();
        let was = t.foreground.replace(moved);
        core.foreground_changed(&mut st, id, was.as_ref());
    }
    tokio::time::advance(Duration::from_millis(1600)).await;
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("idle"), "the pending settle survived a cwd move");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn matching_runs_off_the_runtime_lock() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    let snaps = core.screen_status_collect(&catalog);
    assert_eq!(snaps.len(), 1);
    // Hold the runtime lock across the whole evaluation: if matching ever
    // needed the core state, this join would deadlock.
    let guard = core.lock();
    let verdicts = std::thread::spawn(move || crate::screen_status::evaluate(snaps)).join().unwrap();
    drop(guard);
    for (snap, detection) in verdicts {
        core.screen_status_commit(&catalog, snap, detection);
    }
    assert_eq!(word(&core, id).as_deref(), Some("blocked"));
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn the_worker_reads_the_agent_stored_at_observation() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = terminal_on_fake_holder(&core);
    // `about()` identified devin once; these process facts no longer identify
    // (an uninstalled shim, a name nothing knows). The stored field stands.
    let mut fg = agent("totally-unknown-tool");
    assert_eq!(fg.agent, None, "nothing in this record identifies as devin");
    fg.agent = Some("devin");
    core.lock().reg.terminals.get_mut(&id).unwrap().foreground = Some(fg);
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"), "detection reads the stored agent");
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn every_terminal_gets_its_turn_even_past_the_pass_cap() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let ids: Vec<u32> = (0..40)
        .map(|_| {
            let (id, _frames) = screen_agent(&core, "devin");
            core.output(&mut core.lock(), id, b"READY>\r\n");
            id
        })
        .collect();
    core.screen_status_pass(&catalog).await; // pass 1: the first 32 snapshots
    // All 40 keep producing output: a lowest-first scan would re-pick the
    // same 32 forever and starve the rest, pending idle confirms included.
    for id in &ids {
        core.output(&mut core.lock(), *id, b"READY>\r\n");
    }
    core.screen_status_pass(&catalog).await; // pass 2: the remaining 8, then wraps
    tokio::time::advance(Duration::from_millis(1600)).await;
    core.screen_status_pass(&catalog).await; // pass 3: unchanged screens — every due idle confirms
    for id in &ids {
        assert_eq!(word(&core, *id).as_deref(), Some("idle"), "terminal {id} was never reached");
    }
    drop(core);
    remove_root(&root);
}

#[tokio::test(start_paused = true)]
async fn a_poisoned_state_lock_still_detects_and_commits() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"NEED YES\r\n");
    // A holder path panics while holding the state mutex: it is poisoned now.
    // `Core::lock()` recovers from that; a bare try_lock would skip forever.
    let c = core.clone();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = c.state.lock().unwrap();
        panic!("a holder bug");
    }));
    assert!(panicked.is_err());
    assert!(core.state.is_poisoned());
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("blocked"), "detection survives a poisoned lock");
    drop(core);
    remove_root(&root);
}

#[test]
fn nothing_under_the_state_lock_identifies() {
    // `identify` reads the filesystem (`canonicalize` for shims); everything
    // under `Core.state` — worker, commit, `foreground_changed` — reads the
    // `Foreground.agent` stored at observation instead.
    let src = include_str!("../screen_status.rs");
    assert!(!src.contains("identify"), "screen_status must not call identify under Core.state");
}

/// Holder threads can keep the core, and so its registry writer, alive a
/// moment after the test drops it: a last snapshot may still land.
fn remove_root(root: impl AsRef<std::path::Path>) {
    let start = Instant::now();
    loop {
        match std::fs::remove_dir_all(root.as_ref()) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("removing {}: {e}", root.as_ref().display()),
        }
    }
}
