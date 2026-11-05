use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq)]
enum Reply {
    Relink,
    Slow,
    Malformed,
    Absent,
}

/// Both production loops run on actual time. The agent is just a shell
/// script named devin, and the holder is a protocol peer under this root.
struct LiveHolder {
    core: Arc<Core>,
    root: PathBuf,
    socket: PathBuf,
    id: u32,
    agent: Option<std::process::Child>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    peers: Arc<Mutex<Vec<UnixStream>>>,
    server: Option<std::thread::JoinHandle<()>>,
    requests: Arc<AtomicUsize>,
    detach: mpsc::Sender<()>,
    reply: Reply,
}

impl LiveHolder {
    async fn start(reply: Reply) -> Self {
        let root = PathBuf::from(format!("/var/tmp/hc-r3-live-{}", platform::random_hex()));
        let core = recovery::test_core_at(&root);
        let script = root.join("devin");
        std::fs::write(&script, "#!/bin/sh\nread line\n").unwrap();
        let agent = std::process::Command::new("sh")
            .arg(&script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pgid = agent.id() as i32;
        let started = Instant::now();
        let foreground = loop {
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
                        &proto::Foreground { pgid: if reply == Reply::Absent { -1 } else { pgid } },
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
        Self { core, root, socket, id, agent: Some(agent), tasks, peers, server: Some(server), requests, detach, reply }
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
            let _ = agent.kill();
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
async fn an_observed_negative_foreground_reply_still_withdraws() {
    let live = LiveHolder::start(Reply::Absent).await;
    let baseline = live.baseline();
    let deadline = Instant::now() + Duration::from_secs(5);
    while word(&live.core, live.id).is_some() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    {
        let st = live.core.lock();
        let t = &st.reg.terminals[&live.id];
        assert_eq!(t.turn, baseline.0 + 1, "an observed loss ends the confirmed turn");
        assert!(t.news > baseline.1);
        assert!(t.foreground.is_none());
    }
    live.finish().await;
}
