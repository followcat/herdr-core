#![cfg(unix)]
mod common;
use common::framed::Framed;
use common::*;
use serde::Deserialize;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-compat".into()));
    s
}

#[derive(Deserialize)]
struct LegacyTerminal {
    address: String,
    state: String,
    turn: u64,
    reported: bool,
}

#[test]
fn raw_generation_one_client_and_missing_optional_fields_keep_existing_behavior() {
    let s = session();
    let t = run(
        &s,
        &[
            "--cwd",
            s.root.to_str().unwrap(),
            "sh",
            "-c",
            &format!("stty raw -echo; printf ready; cat > '{}'", s.root.join("input").display()),
        ],
    );
    wait_until("logger", || text(&s, &t).contains("ready"));
    let mut conn = s.connect();
    let hello = conn.call(1, "hello", json!({"generation":1}));
    assert_eq!(hello["result"]["generation"], 1);
    let features = hello["result"]["features"].as_array().unwrap();
    for f in ["os_hostname", "repository_facts", "quiet_human_release", "record_activity"] {
        assert!(features.contains(&json!(f)));
    }
    for f in ["state_labels", "activity_fences", "terminal_ack", "appearance"] {
        assert!(!features.contains(&json!(f)));
    }
    assert!(!hello["result"]["methods"].as_array().unwrap().contains(&json!("terminal.ack")));
    let record = conn.call(2, "terminal.get", json!({"terminal":t}))["result"].clone();
    let old: LegacyTerminal = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(old.address, t);
    assert_eq!(old.state, "live");
    assert_eq!(old.turn, 0);
    assert!(!old.reported);
    let mut without = record;
    for field in ["activity", "repository"] {
        without.as_object_mut().unwrap().remove(field);
    }
    let modern: herdr_core::terminals::Terminal = serde_json::from_value(without).unwrap();
    assert!(modern.activity.is_none() && modern.repository.is_none());
    let identity: herdr_core::api::CoreInfo =
        serde_json::from_value(json!({"id":"host-a","name":"alice-dev"})).unwrap();
    assert!(identity.hostname.is_none());
    let mut framed = Framed::open(&s);
    let reply = framed.call("terminal.attach", json!({"terminal":t,"cols":80,"rows":24}));
    assert_ne!(reply["result"]["quiet_human_release"], true);
    let ch = reply["result"]["channel"].as_u64().unwrap() as u32;
    framed.send(ch, herdr_core::wire::DATA, b"legacy");
    wait_until("DATA delivery", || std::fs::read(s.root.join("input")).unwrap_or_default() == b"legacy");
    assert!(framed.stash.iter().all(|(_, kind, _)| *kind != 7 && *kind != 8));
}

#[test]
fn origin_facts_survive_nested_calls_and_direct_mirror_status_patches() {
    let owner = session();
    let root = owner.root.join("acme-shop");
    std::fs::create_dir(&root).unwrap();
    assert!(owner.program("git").args(["init", "--quiet", "-b", "main"]).arg(&root).status().unwrap().success());
    let t = run(
        &owner,
        &[
            "--cwd",
            root.to_str().unwrap(),
            "sh",
            "-c",
            &script(
                &owner,
                "r state=working; r state=working:id=build/test; w a; r state=blocked; w b; r state=idle; r state=done:id=build/test; w c",
            ),
        ],
    );
    wait_until("origin ready", || get(&owner, &t)["repository"]["state"] == "ready");
    let mut relay = session();
    relay.env.push(fake_ssh(&relay, &[("host-a", &owner)]));
    relay.json(&["machine", "add", "host-a", "--session", "c3-compat"]);
    let mut client = session();
    client.env.push(fake_ssh(&client, &[("host-b", &relay)]));
    client.json(&["machine", "add", "host-b", "--session", "c3-compat"]);
    let remote = format!("host-b/host-a/{t}");
    let origin = get(&owner, &t);
    let nested = get(&client, &remote);
    for f in ["repository", "activity"] {
        assert_eq!(nested[f], origin[f], "origin {f} is opaque through relays");
    }
    let mut events = relay.connect();
    events.call(1, "events.subscribe", json!({"terminal":format!("host-a/{t}")}));
    touch(&owner, "a");
    wait_until("blocked", || get(&owner, &t)["status"]["state"] == "blocked");
    touch(&owner, "b");
    wait_until("named completion", || get(&owner, &t)["records"]["build/test"]["state"] == "done");
    let done = get(&owner, &t);
    let event = next_event(&mut events, |m| {
        m["params"]["type"] == "status" && m["params"]["data"]["records"]["build/test"]["state"] == "done"
    });
    assert_eq!(event["params"]["data"]["activity"], done["activity"]);
    assert_eq!(get(&client, &remote)["activity"], done["activity"]);
    assert_eq!(done["turn"], 1, "blocked -> idle does not recount the existing turn");
    owner.json(&["label", &t, "checkpoint=yes"]);
    let core = owner.json(&["server", "status"])["pid"].as_i64().unwrap();
    unsafe { libc::kill(core as i32, libc::SIGKILL) };
    wait_until("scratch core gone", || !pid_alive(core));
    let restored = get(&owner, &t);
    assert_ne!(restored["activity"]["epoch"], done["activity"]["epoch"]);
    assert_eq!(restored["activity"]["records"], done["activity"]["records"], "replayed completion is not recounted");
}
