use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq)]
enum Reply {
    Relink,
    Slow,
    Malformed,
    Negative,
    Zero,
    Shell,
    Opaque,
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
        let root = PathBuf::from(format!("/var/tmp/hc-r4-live-{}", platform::random_hex()));
        let core = recovery::test_core_at(&root);
        let script = root.join("devin");
        let source = if reply == Reply::Opaque {
            "import ctypes, pathlib, sys\nsys.stdin.readline()\nassert ctypes.CDLL(None).prctl(4, 0, 0, 0, 0) == 0\npathlib.Path(__file__).with_name('opaque-ready').touch()\nsys.stdin.readline()\n"
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
                    std::thread::sleep(Duration::from_millis(1200));
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

    async fn wait_unknown(&self, baseline: (u64, u64, u64), after: u64) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.core.foreground_unknown.load(Ordering::SeqCst) <= after {
            self.unchanged(baseline);
            assert!(Instant::now() < deadline, "the watcher never acknowledged an unknown outcome");
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
    let detached = Instant::now();
    while detached.elapsed() < Duration::from_millis(1100) {
        tokio::time::sleep(Duration::from_millis(20)).await;
        live.unchanged(baseline);
    }
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
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1850) {
        tokio::time::sleep(Duration::from_millis(20)).await;
        live.unchanged(baseline);
    }
    assert!(live.requests.load(Ordering::SeqCst) > 0);
    assert!(live.core.lock().link(live.id).is_some(), "the holder stayed linked throughout the timeout");
    live.finish().await;
}

#[tokio::test]
async fn a_malformed_foreground_reply_is_unknown_and_preserves_the_root() {
    let live = LiveHolder::start(Reply::Malformed).await;
    let baseline = live.baseline();
    let deadline = Instant::now() + Duration::from_secs(5);
    while live.requests.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    live.unchanged(baseline);
    live.finish().await;
}

#[tokio::test]
async fn nonpositive_foreground_replies_are_unknown_while_the_agent_lives() {
    for reply in [Reply::Negative, Reply::Zero] {
        let live = LiveHolder::start(reply).await;
        let baseline = live.baseline();
        let unknown = live.core.foreground_unknown.load(Ordering::SeqCst);
        live.wait_unknown(baseline, unknown).await;
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
    let unknown = live.core.foreground_unknown.load(Ordering::SeqCst);
    live.wait_unknown(baseline, unknown).await;
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
