use super::*;

#[cfg(unix)]
mod expiry;
#[cfg(target_os = "linux")]
mod incarnation;
#[cfg(unix)]
mod live_foreground;
mod recovery;
#[cfg(unix)]
mod relink;

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
        started: None,
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

/// The whole fork path on a real PTY: a script literally named `devin`
/// (interpreter script-arg identification), real output bytes, real
/// foreground reads, and the worker — driven pass by pass so the test can
/// acknowledge each observed status before the script prints the next.
#[cfg(unix)]
#[tokio::test]
async fn a_script_named_devin_drives_inference_end_to_end() {
    use std::io::{Read, Write};
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    // Built-ins only: clear, print one marker, wait for our ack line.
    let script = root.join("devin");
    std::fs::write(
        &script,
        "say() { printf '\\033[2J\\033[H%s\\n' \"$1\"; read _a; }\n\
         say 'WORKING NOW'\n\
         say 'NEED YES'\n\
         say 'WORKING NOW'\n\
         say 'READY>'\n\
         read _last\n",
    )
    .unwrap();
    let argv = vec!["/bin/sh".into(), script.display().to_string()];
    let (pty, mut child) = platform::spawn_pty(&argv, &Default::default(), root.to_str().unwrap(), 80, 24).unwrap();
    let (id, _frames) = terminal_on_fake_holder(&core);
    {
        let mut st = core.lock();
        let t = st.reg.terminals.get_mut(&id).unwrap();
        t.argv = argv;
        t.pid = Some(child.id());
        t.program_started_ms = Some(now_ms());
    }
    // Output reaches the screen the way holder OUTPUT frames deliver it.
    let mut reader = pty.reader().unwrap();
    let feeder = std::thread::spawn({
        let core = core.clone();
        move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                core.output(&mut core.lock(), id, &buf[..n]);
            }
        }
    });
    let mut writer = pty.writer().unwrap();
    let (tx, mut events) = tokio::sync::mpsc::channel::<String>(32);
    {
        let mut st = core.lock();
        core.subscribe(
            &mut st,
            crate::events::Filter { terminal: Some(crate::terminals::address(id)), ..Default::default() },
            crate::events::Sink::Plain { tx, behind: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) },
        );
    }
    let words = ["working", "blocked", "working", "idle"];
    let mut waiting = None;
    for (stage, want) in words.iter().enumerate() {
        // Poll until the worker's verdict is `want`: each step updates the
        // foreground like watch_foreground (our PTY, not a holder query),
        // then runs one pass. Real time: the idle settle really takes 1.5 s.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while word(&core, id).as_deref() != Some(want) {
            let pgid = pty.foreground();
            let now = (pgid > 0).then(|| crate::foreground::about(pgid)).flatten();
            {
                let mut st = core.lock();
                if st.reg.terminals.get(&id).is_some_and(|t| t.foreground != now) {
                    let was = std::mem::replace(&mut st.reg.terminals.get_mut(&id).unwrap().foreground, now);
                    core.foreground_changed(&mut st, id, was.as_ref());
                }
            }
            core.screen_status_pass(&catalog).await;
            assert!(std::time::Instant::now() < deadline, "stage {stage}: never saw {want}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if stage == 2 {
            // An ordinary wait also returns on blocked: start it only once
            // the second working is published, so idle is what ends it.
            let c = core.clone();
            waiting = Some(tokio::spawn(async move {
                c.wait(
                    id,
                    crate::wait::WaitSpec { output: None, timeout: Duration::from_secs(15), after_turn: None },
                    None,
                )
                .await
            }));
        }
        writer.write_all(b"\n").unwrap(); // acknowledged: print the next state
    }
    let result = waiting.expect("the idle wait").await.unwrap();
    assert_eq!(result.reason, "status");
    assert_eq!(result.status.as_deref(), Some("idle"));
    let (statuses, turn) = {
        let mut statuses = Vec::new();
        while let Ok(line) = events.try_recv() {
            let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
            if msg["params"]["type"] == "status" {
                statuses.push(msg["params"]["data"]["status"]["state"].as_str().map(String::from));
            }
        }
        (statuses, core.lock().reg.terminals[&id].turn)
    };
    assert_eq!(
        statuses,
        vec![Some("working".into()), Some("blocked".into()), Some("working".into()), Some("idle".into())]
    );
    assert_eq!(turn, 2, "working → blocked → working → idle: two turns, as native counts");
    writer.write_all(b"\n").unwrap(); // let the script exit
    let _ = platform::wait(&mut child);
    drop(writer);
    drop(pty);
    drop(core);
    feeder.join().unwrap();
    remove_root(&root);
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
async fn removal_during_a_stitch_never_recreates_its_watch() {
    let (core, root) = test_core();
    let (id, _frames) = screen_agent(&core, "devin");
    core.output(&mut core.lock(), id, b"READY>\r\n");
    core.lock().stitching.insert(id);
    // While the read is off scrolling, the terminal ends and is forgotten.
    {
        let mut st = core.lock();
        core.record_finish(&mut st, id, Exit { code: Some(0), signal: None }, "exited");
        core.remove(&mut st, id);
        assert!(!st.screen_status.contains_key(&id));
    }
    // What stitch.rs does when its scroll ends, whatever happened meanwhile.
    {
        let mut st = core.lock();
        st.stitching.remove(&id);
        st.invalidate_screen(id);
        assert!(!st.screen_status.contains_key(&id), "invalidating a removed terminal must not recreate its watch");
    }
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
    let src = include_str!("../../screen_status.rs");
    assert!(!src.contains("identify"), "screen_status must not call identify under Core.state");
}
