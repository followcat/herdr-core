use super::*;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq)]
enum Reply {
    Normal,
    Relink,
    Slow,
    Malformed,
    Negative,
    Zero,
    Shell,
    Opaque,
    Zombie,
}

/// Both production loops run on actual time. The agent is just a shell
/// script named devin, and the holder is a protocol peer under this root.
struct LiveHolder {
    core: Arc<Core>,
    root: PathBuf,
    socket: PathBuf,
    id: u32,
    agent: Option<std::process::Child>,
    pty: Arc<platform::Pty>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    peers: Arc<Mutex<Vec<UnixStream>>>,
    server: Option<std::thread::JoinHandle<()>>,
    requests: Arc<AtomicUsize>,
    detach: mpsc::Sender<()>,
    reply: Reply,
}

impl LiveHolder {
    async fn start(reply: Reply) -> Self {
        let root = PathBuf::from(format!("/var/tmp/hc-r5-live-{}", platform::random_hex()));
        let core = recovery::test_core_at(&root);
        let script = root.join("devin");
        let source = if reply == Reply::Opaque {
            "import ctypes, pathlib, sys\nsys.stdin.readline()\nassert ctypes.CDLL(None).prctl(4, 0, 0, 0, 0) == 0\npathlib.Path(__file__).with_name('opaque-ready').touch()\nsys.stdin.readline()\n"
        } else if reply == Reply::Zombie {
            "import os, pathlib, sys, time\nwhile not pathlib.Path(__file__).with_name('foreground-ready').exists(): time.sleep(.01)\nsys.stdin.readline()\nos._exit(0)\n"
        } else {
            "#!/bin/sh\nread line\n"
        };
        std::fs::write(&script, source).unwrap();
        let argv = if reply == Reply::Shell {
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "set -m; sh \"$1\"; read line".into(),
                "hc-test-shell".into(),
                script.display().to_string(),
            ]
        } else if reply == Reply::Zombie {
            vec![
                "python3".into(),
                "-c".into(),
                r#"import os, pathlib, signal, sys, time
root = pathlib.Path(sys.argv[1]).parent
signal.signal(signal.SIGTTOU, signal.SIG_IGN)
pid = os.fork()
if pid == 0:
    os.setpgid(0, 0)
    os.execvp('python3', ['python3', sys.argv[1]])
try: os.setpgid(pid, pid)
except PermissionError: pass
assert os.getpgid(pid) == pid
os.tcsetpgrp(0, pid)
(root / 'foreground-ready').touch()
while not (root / 'reap').exists(): time.sleep(.01)
os.waitpid(pid, 0)
(root / 'reaped').touch()
while True: time.sleep(1)
"#
                .into(),
                script.display().to_string(),
            ]
        } else {
            vec![if reply == Reply::Opaque { "python3" } else { "/bin/sh" }.into(), script.display().to_string()]
        };
        let mut env = std::collections::BTreeMap::new();
        env.insert("PATH".into(), std::env::var("PATH").unwrap());
        let (pty, agent) = platform::spawn_pty(&argv, &env, root.to_str().unwrap(), 80, 24).unwrap();
        let pty = Arc::new(pty);
        let started = Instant::now();
        let foreground = loop {
            let pgid = pty.foreground();
            if let Some(fg) = crate::foreground::about(pgid)
                && fg.agent == Some("devin")
            {
                break fg;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "fake agent never entered its script");
            std::thread::sleep(Duration::from_millis(1));
        };
        let (id, _) = screen_agent(&core, "devin");
        {
            let mut st = core.lock();
            st.reg.terminals.get_mut(&id).unwrap().foreground = Some(foreground);
            st.holders.clear();
        }
        std::fs::create_dir_all(core.paths.detect_dir()).unwrap();
        std::fs::write(core.paths.detect_dir().join("devin.toml"), TEST_MANIFEST).unwrap();
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
        let peers = Arc::new(Mutex::new(Vec::new()));
        let sockets = peers.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let (detach, actions) = mpsc::channel();
        let terminal = pty.clone();
        let server = std::thread::spawn(move || {
            let accept = || {
                let (mut stream, _) = listener.accept().ok()?;
                sockets.lock().unwrap().push(stream.try_clone().ok()?);
                proto::read(&mut stream).ok()??;
                Some(stream)
            };
            let Some(mut stream) = accept() else { return };
            if io::Write::write_all(&mut stream, &answer).is_err() {
                return;
            }
            if reply == Reply::Relink {
                if actions.recv_timeout(Duration::from_secs(10)).is_err() {
                    return;
                }
                let _ = io::Write::write_all(&mut stream, &GARBAGE);
                while let Ok(Some(_)) = proto::read(&mut stream) {}
                // Automatic reconnect fails. The unchanged program survives
                // several actual watcher/worker ticks before later adoption.
                let Some(mut failed) = accept() else { return };
                let _ = io::Write::write_all(&mut failed, &GARBAGE);
                drop(failed);
                let Some(next) = accept() else { return };
                stream = next;
                if io::Write::write_all(&mut stream, &answer).is_err() {
                    return;
                }
            }
            while let Ok(Some((kind, id, _))) = proto::read(&mut stream) {
                if kind == proto::GOODBYE {
                    let _ = io::Write::write_all(&mut stream, &proto::frame(proto::BYE, 0, &[]));
                    return;
                }
                if kind != proto::FOREGROUND {
                    continue;
                }
                seen.fetch_add(1, Ordering::SeqCst);
                if reply == Reply::Slow {
                    // Withhold every reply. A delayed reply could win a
                    // scheduling race after both it and the timeout are ready.
                    continue;
                }
                let response = if reply == Reply::Malformed {
                    proto::frame(proto::FOREGROUND_IS, id, b"not json")
                } else {
                    proto::json_frame(
                        proto::FOREGROUND_IS,
                        id,
                        &proto::Foreground {
                            pgid: match reply {
                                Reply::Negative => -1,
                                Reply::Zero => 0,
                                _ => terminal.foreground(),
                            },
                        },
                    )
                };
                if io::Write::write_all(&mut stream, &response).is_err() {
                    return;
                }
            }
        });
        core.take_over(platform::connect(&socket).unwrap(), false, &socket, None).unwrap();
        core.screen_status_pass(&test_detect_catalog()).await;
        assert_eq!(word(&core, id).as_deref(), Some("working"));
        let tasks =
            vec![tokio::spawn(core.clone().watch_foreground()), tokio::spawn(core.clone().watch_screen_status())];
        Self {
            core,
            root,
            socket,
            id,
            agent: Some(agent),
            pty,
            tasks,
            peers,
            server: Some(server),
            requests,
            detach,
            reply,
        }
    }

    fn unchanged(&self, baseline: (u64, u64, u64)) {
        let st = self.core.lock();
        let t = &st.reg.terminals[&self.id];
        assert_eq!(t.status.as_ref().map(|s| s.state.as_str()), Some("working"));
        assert_eq!((t.turn, t.news, st.events.seq), baseline, "no null event, turn or news");
        assert_eq!(t.foreground.as_ref().and_then(|f| f.agent), Some("devin"));
    }

    fn baseline(&self) -> (u64, u64, u64) {
        let st = self.core.lock();
        let t = &st.reg.terminals[&self.id];
        (t.turn, t.news, st.events.seq)
    }

    async fn wait_outcome(&self, baseline: (u64, u64, u64), counter: &AtomicU64, after: u64) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while counter.load(Ordering::SeqCst) <= after {
            self.unchanged(baseline);
            assert!(Instant::now() < deadline, "the expected worker/lookup outcome was never acknowledged");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.unchanged(baseline);
    }

    fn stop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        {
            let mut st = self.core.lock();
            st.stopping = true;
            st.freeze_screen_detection();
        }
        let _ = self.detach.send(());
        for peer in self.peers.lock().unwrap().iter() {
            let _ = peer.shutdown(std::net::Shutdown::Both);
        }
        if self.reply == Reply::Relink {
            // Wake accept too if an assertion failed before the later relink.
            if let Ok(peer) = platform::connect(&self.socket) {
                let _ = peer.shutdown(std::net::Shutdown::Both);
            }
        }
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
        if self.reply == Reply::Zombie && self.agent.is_some() {
            // Let the live wrapper reap its owned child before killing it,
            // including when an assertion unwinds before the child exits.
            if let Ok(mut writer) = self.pty.writer() {
                let _ = io::Write::write_all(&mut writer, b"\n");
            }
            let _ = std::fs::write(self.root.join("reap"), []);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.root.join("reaped").exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if let Some(mut agent) = self.agent.take() {
            self.pty.signal(agent.id(), platform::signal::KILL);
            let _ = agent.wait();
        }
    }

    async fn finish(mut self) {
        self.stop();
        self.core.save(&self.core.lock());
        self.core.flush_saved().await.unwrap();
    }
}

impl Drop for LiveHolder {
    fn drop(&mut self) {
        self.stop();
        remove_root(&self.root);
    }
}

#[tokio::test]
async fn real_watchers_preserve_and_silently_reconfirm_across_relink() {
    let live = LiveHolder::start(Reply::Relink).await;
    let baseline = live.baseline();
    live.detach.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !live.core.lock().disconnected.contains_key("fake") {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let unknown = live.core.observations.foreground_unknown.load(Ordering::SeqCst);
    let passes = live.core.observations.screen_status_passes.load(Ordering::SeqCst);
    live.wait_outcome(baseline, &live.core.observations.foreground_unknown, unknown + 1).await;
    live.wait_outcome(baseline, &live.core.observations.screen_status_passes, passes + 1).await;
    live.core.take_over(platform::connect(&live.socket).unwrap(), false, &live.socket, None).unwrap();
    assert!(live.core.lock().screen_status[&live.id].unconfirmed_until.is_some());
    while live.core.lock().screen_status[&live.id].unconfirmed_until.is_some() {
        assert!(Instant::now() < deadline, "real watcher/worker never reconfirmed");
        tokio::time::sleep(Duration::from_millis(20)).await;
        live.unchanged(baseline);
    }
    assert!(live.requests.load(Ordering::SeqCst) > 0, "a real post-relink foreground request was answered");
    live.unchanged(baseline);
    live.finish().await;
}

#[tokio::test]
async fn a_slow_linked_holder_is_unknown_and_preserves_a_confirmed_root() {
    let live = LiveHolder::start(Reply::Slow).await;
    let baseline = live.baseline();
    let unknown = live.core.observations.foreground_unknown.load(Ordering::SeqCst);
    let timeouts = live.core.observations.foreground_timeouts.load(Ordering::SeqCst);
    live.wait_outcome(baseline, &live.core.observations.foreground_timeouts, timeouts).await;
    live.wait_outcome(baseline, &live.core.observations.foreground_unknown, unknown).await;
    assert!(live.requests.load(Ordering::SeqCst) > 0);
    assert!(live.core.lock().link(live.id).is_some(), "the holder stayed linked throughout the timeout");
    live.finish().await;
}

#[tokio::test]
async fn a_malformed_foreground_reply_is_unknown_and_preserves_the_root() {
    let live = LiveHolder::start(Reply::Malformed).await;
    let baseline = live.baseline();
    let unknown = live.core.observations.foreground_unknown.load(Ordering::SeqCst);
    let malformed = live.core.observations.foreground_bad_replies.load(Ordering::SeqCst);
    live.wait_outcome(baseline, &live.core.observations.foreground_bad_replies, malformed).await;
    live.wait_outcome(baseline, &live.core.observations.foreground_unknown, unknown).await;
    live.finish().await;
}

#[tokio::test]
async fn nonpositive_foreground_replies_are_unknown_while_the_agent_lives() {
    for reply in [Reply::Negative, Reply::Zero] {
        let live = LiveHolder::start(reply).await;
        let baseline = live.baseline();
        let unknown = live.core.observations.foreground_unknown.load(Ordering::SeqCst);
        let negative = live.core.observations.foreground_nonpositive.load(Ordering::SeqCst);
        live.wait_outcome(baseline, &live.core.observations.foreground_nonpositive, negative).await;
        live.wait_outcome(baseline, &live.core.observations.foreground_unknown, unknown).await;
        assert!(live.core.lock().link(live.id).is_some());
        assert!(!live.core.lock().screen_status[&live.id].foreground_observed);
        live.finish().await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_unreadable_live_foreground_is_unknown_without_a_turn_or_marker() {
    use std::io::Write;
    let live = LiveHolder::start(Reply::Opaque).await;
    let baseline = live.baseline();
    live.pty.writer().unwrap().write_all(b"\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !live.root.join("opaque-ready").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = live.pty.foreground();
    assert!(PathBuf::from(format!("/proc/{pid}")).exists());
    assert_eq!(std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    live.core.lock().screen_status.get_mut(&live.id).unwrap().foreground_observed = false;
    let unknown = live.core.observations.foreground_unknown.load(Ordering::SeqCst);
    let unreadable = live.core.observations.foreground_read_errors.load(Ordering::SeqCst);
    live.wait_outcome(baseline, &live.core.observations.foreground_read_errors, unreadable).await;
    live.wait_outcome(baseline, &live.core.observations.foreground_unknown, unknown).await;
    assert!(!live.core.lock().screen_status[&live.id].foreground_observed);
    live.finish().await;
}

#[tokio::test]
async fn an_agent_returning_to_its_shell_still_withdraws_the_confirmed_root() {
    use std::io::Write;
    let live = LiveHolder::start(Reply::Shell).await;
    let baseline = live.baseline();
    let shell = live.agent.as_ref().unwrap().id() as i32;
    assert_ne!(live.pty.foreground(), shell, "the agent owns a separate foreground group");
    live.pty.writer().unwrap().write_all(b"\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while word(&live.core, live.id).is_some() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    {
        let st = live.core.lock();
        let t = &st.reg.terminals[&live.id];
        assert_eq!(t.turn, baseline.0 + 1);
        assert!(t.news > baseline.1);
        let fg = t.foreground.as_ref().unwrap();
        assert_eq!(fg.pid, shell);
        assert!(fg.agent.is_none());
    }
    live.finish().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_restored_start_stamp_rehydrates_without_a_status_change() {
    let live = LiveHolder::start(Reply::Normal).await;
    // This fixture's watcher tasks have not run yet. Restore only the
    // serializable foreground facts, exactly as registry loading does.
    for task in &live.tasks {
        task.abort();
    }
    let baseline = live.baseline();
    let stamp = {
        let mut st = live.core.lock();
        let fg = st.reg.terminals[&live.id].foreground.as_ref().unwrap();
        let stamp = fg.started.unwrap();
        let mut restored: crate::foreground::Foreground =
            serde_json::from_value(serde_json::to_value(fg).unwrap()).unwrap();
        assert!(restored.started.is_none());
        restored.agent = crate::detect::identify(&restored);
        st.reg.terminals.get_mut(&live.id).unwrap().foreground = Some(restored);
        st.invalidate_screen(live.id);
        st.screen_status.get_mut(&live.id).unwrap().foreground_observed = false;
        st.screen_status.get_mut(&live.id).unwrap().unconfirmed_until =
            Some(tokio::time::Instant::now() + crate::screen_status::RECONFIRM);
        stamp
    };
    // Drive the actual watcher with the holder's real positive PTY reply.
    let c = live.core.clone();
    let id = live.id;
    let watcher = tokio::spawn(c.clone().watch_foreground());
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.lock().reg.terminals[&id].foreground.as_ref().unwrap().started.is_none() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    c.screen_status_pass(&test_detect_catalog()).await;
    assert_eq!(c.lock().reg.terminals[&id].foreground.as_ref().unwrap().started, Some(stamp));
    assert!(c.lock().screen_status[&id].unconfirmed_until.is_none());
    live.unchanged(baseline);
    watcher.abort();
    let _ = watcher.await;
    live.finish().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_unreaped_foreground_leader_withdraws_while_its_wrapper_lives() {
    use std::io::Write;
    let mut live = LiveHolder::start(Reply::Zombie).await;
    let baseline = live.baseline();
    let child = live.pty.foreground();
    assert_ne!(child, live.agent.as_ref().unwrap().id() as i32);
    live.pty.writer().unwrap().write_all(b"\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let stat = format!("/proc/{child}/stat");
    while !std::fs::read_to_string(&stat).unwrap().rsplit_once(") ").unwrap().1.starts_with("Z ") {
        assert!(Instant::now() < deadline, "the wrapper never left its child unreaped");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(live.pty.foreground(), child, "the positive pgid still names the zombie");
    assert!(std::fs::read(format!("/proc/{child}/cmdline")).unwrap().is_empty());
    while word(&live.core, live.id).is_some() {
        assert!(Instant::now() < deadline, "the real watcher never withdrew the zombie's confirmed root");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    {
        let st = live.core.lock();
        let t = &st.reg.terminals[&live.id];
        assert!(t.is_live(), "this is foreground loss, not a whole-program finish");
        assert!(t.foreground.is_none());
        assert_eq!(t.turn, baseline.0 + 1);
        assert!(t.news > baseline.1);
    }
    assert!(live.agent.as_mut().unwrap().try_wait().unwrap().is_none(), "the wrapper is still alive");
    assert!(std::fs::read_to_string(&stat).unwrap().rsplit_once(") ").unwrap().1.starts_with("Z "));
    live.stop();
    assert!(live.root.join("reaped").exists(), "the fixture must reap its child during cleanup");
    live.finish().await;
}
