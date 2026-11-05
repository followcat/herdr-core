#![cfg(unix)]

mod common;

use common::framed::Framed;
use common::*;
use herdr_core::wire;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-quiet-relay".into()));
    s
}

#[test]
fn two_relays_preserve_quiet_release_bytes_and_size_ownership() {
    let owner = session();
    let log = owner.root.join("input");
    let t = run(
        &owner,
        &[
            "--cwd",
            owner.root.to_str().unwrap(),
            "sh",
            "-c",
            &format!("stty raw -echo; printf ready; cat > '{}'", log.display()),
        ],
    );
    wait_until("logger ready", || log.exists() && text(&owner, &t).contains("ready"));
    let mut relay = session();
    relay.env.push(fake_ssh(&relay, &[("host-a", &owner)]));
    relay.json(&["machine", "add", "host-a", "--session", "c3-quiet-relay"]);
    let mut client = session();
    client.env.push(fake_ssh(&client, &[("host-b", &relay)]));
    client.json(&["machine", "add", "host-b", "--session", "c3-quiet-relay"]);
    let target = format!("host-b/host-a/{t}");
    let mut idle = Framed::open(&client);
    let reply = idle.call("terminal.attach", json!({"terminal":target,"cols":80,"rows":24,"quiet_human_release":true}));
    assert_eq!(reply["result"]["quiet_human_release"], true, "{reply}");
    let channel = reply["result"]["channel"].as_u64().unwrap() as u32;
    let mut leader = Framed::open(&owner);
    leader.call("terminal.attach", json!({"terminal":t,"cols":120,"rows":40}));
    let release = b"\x1b[97;1:3u";
    for byte in release {
        idle.send(channel, wire::QUIET_HUMAN_RELEASE, &[*byte]);
    }
    wait_until("relayed release delivered", || std::fs::read(&log).unwrap() == release);
    assert_eq!(get(&owner, &t)["cols"], 120);
    assert_eq!(get(&owner, &t)["rows"], 40);
    idle.send(channel, wire::DATA, release);
    wait_until("legacy release delivered", || std::fs::read(&log).unwrap() == release.repeat(2));
    assert_eq!(get(&owner, &t)["cols"], 80);
}
