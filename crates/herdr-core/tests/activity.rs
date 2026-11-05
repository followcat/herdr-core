#![cfg(unix)]

mod common;
use common::*;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-activity".into()));
    s
}

fn word(s: &Session, t: &str, state: &str) -> Value {
    let mut got = Value::Null;
    wait_until(state, || {
        got = get(s, t);
        got["status"]["state"] == state
    });
    got
}

#[test]
fn activity_is_atomic_with_status_and_scopes_clear_done_without_reinterpreting_labels() {
    let s = session();
    s.json(&["server", "start"]);
    assert!(
        s.connect().call(1, "hello", json!({}))["result"]["features"]
            .as_array()
            .unwrap()
            .contains(&json!("record_activity"))
    );
    let mut events = s.connect();
    events.call(2, "events.subscribe", json!({}));
    let body = "r state=blocked:app=agent-a; w a; r state=idle:app=agent-a; w b; r state=working:app=agent-a; w c; r state=blocked:app=agent-a; w d; r state=idle:app=agent-a; w e; r state=idle:app=agent-b; w f";
    let t = run(&s, &["--cwd", s.root.to_str().unwrap(), "sh", "-c", &script(&s, body)]);
    word(&s, &t, "blocked");
    touch(&s, "a");
    let ready = word(&s, &t, "idle");
    assert!(ready["activity"]["records"][""]["completion_ordinal"].is_null());
    touch(&s, "b");
    word(&s, &t, "working");
    touch(&s, "c");
    word(&s, &t, "blocked");
    touch(&s, "d");
    let done = word(&s, &t, "idle");
    assert!(done["activity"]["records"][""]["completion_ordinal"].as_u64().is_some());
    let event = next_event(&mut events, |m| {
        m["params"]["type"] == "status"
            && m["params"]["data"]["activity"]["records"][""]["completion_ordinal"].as_u64().is_some()
    });
    assert_eq!(event["params"]["data"]["activity"], done["activity"]);
    s.json(&["label", &t, "hc.activity_session=ordinary"]);
    assert_eq!(get(&s, &t)["activity"], done["activity"], "labels remain uninterpreted");
    touch(&s, "e");
    wait_until("new app", || get(&s, &t)["status"]["app"] == "agent-b");
    let replaced = get(&s, &t);
    assert!(replaced["activity"]["scope"].as_u64() > done["activity"]["scope"].as_u64());
    assert!(replaced["activity"]["records"][""]["completion_ordinal"].is_null());
    let event = next_event(&mut events, |m| m["params"]["data"]["status"]["app"] == "agent-b");
    assert_eq!(event["params"]["data"]["activity"], replaced["activity"]);
    touch(&s, "f");
    wait_until("finished", || get(&s, &t)["state"] == "finished");
    assert!(get(&s, &t)["activity"]["records"][""]["completion_ordinal"].is_null());
}

#[test]
fn a_genuinely_working_record_completes_after_restart_with_a_new_epoch() {
    let s = session();
    let t = run(
        &s,
        &["--cwd", s.root.to_str().unwrap(), "sh", "-c", &script(&s, "r state=working; w a; r state=idle; w b")],
    );
    let working = word(&s, &t, "working");
    let epoch = working["activity"]["epoch"].clone();
    s.json(&["server", "restart"]);
    let restored = word(&s, &t, "working");
    assert_ne!(restored["activity"]["epoch"], epoch);
    assert_eq!(restored["activity"]["records"], working["activity"]["records"]);
    touch(&s, "a");
    let done = word(&s, &t, "idle");
    assert!(done["activity"]["records"][""]["completion_ordinal"].as_u64().is_some());
    assert!(done["activity"]["revision"].as_u64() > working["activity"]["revision"].as_u64());
}
