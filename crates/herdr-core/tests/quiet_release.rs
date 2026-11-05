#![cfg(unix)]

mod common;

use common::framed::Framed;
use common::*;
use herdr_core::wire;
use serde_json::json;

#[test]
fn negotiated_human_releases_deliver_exact_bytes_without_reclaiming_size() {
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-quiet-release".into()));
    let log = s.root.join("input");
    let t = run(
        &s,
        &[
            "--cwd",
            s.root.to_str().unwrap(),
            "sh",
            "-c",
            &format!("stty raw -echo; printf ready; cat > '{}'", log.display()),
        ],
    );
    wait_until("byte logger ready", || log.exists() && text(&s, &t).contains("ready"));
    let mut idle = Framed::open(&s);
    let hello = idle.call("hello", json!({"framed":true}));
    assert!(hello["result"]["features"].as_array().unwrap().contains(&json!("quiet_human_release")));
    let reply = idle.call("terminal.attach", json!({"terminal":t,"cols":80,"rows":24,"quiet_human_release":true}));
    assert_eq!(reply["result"]["quiet_human_release"], true);
    let idle_ch = reply["result"]["channel"].as_u64().unwrap() as u32;
    let mut leader = Framed::open(&s);
    let reply = leader.call("terminal.attach", json!({"terminal":t,"cols":120,"rows":40}));
    let leader_ch = reply["result"]["channel"].as_u64().unwrap() as u32;
    let release = b"\x1b[97;1:3u";
    for byte in release {
        idle.send(idle_ch, wire::QUIET_HUMAN_RELEASE, &[*byte]);
    }
    idle.call("terminal.get", json!({"terminal":t}));
    wait_until("release delivered", || std::fs::read(&log).unwrap() == release);
    assert_eq!(get(&s, &t)["cols"], 120);
    assert_eq!(get(&s, &t)["rows"], 40);

    for observe in [false, true] {
        let mut other = Framed::open(&s);
        let reply = other.call(
            "terminal.attach",
            json!({"terminal":t,"cols":60,"rows":20,
            "observe":observe,"quiet_human_release":observe}),
        );
        assert_ne!(reply["result"]["quiet_human_release"], true);
        let channel = reply["result"]["channel"].as_u64().unwrap() as u32;
        leader.send(leader_ch, wire::FOCUS, &[1]);
        leader.call("terminal.get", json!({"terminal":t}));
        other.send(channel, wire::QUIET_HUMAN_RELEASE, b"ignored");
        other.call("terminal.get", json!({"terminal":t}));
        assert_eq!(std::fs::read(&log).unwrap(), release);
        assert_eq!(get(&s, &t)["cols"], 120);
    }
    // Legacy DATA retains its human activity/ownership behavior, including releases.
    idle.send(idle_ch, wire::DATA, release);
    idle.call("terminal.get", json!({"terminal":t}));
    wait_until("legacy release delivered", || std::fs::read(&log).unwrap() == release.repeat(2));
    assert_eq!(get(&s, &t)["cols"], 80);
}
