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
    record_word(s, t, "", state)
}

fn record_word(s: &Session, t: &str, id: &str, state: &str) -> Value {
    let mut got = Value::Null;
    wait_until(state, || {
        got = get(s, t);
        let status = if id.is_empty() { &got["status"] } else { &got["records"][id] };
        status["state"] == state
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
    let body = "r state=blocked:app=agent-a; w a; r state=idle:app=agent-a; w b; r state=working:app=agent-a; w c; r state=blocked:app=agent-a; w d; r state=idle:app=agent-a; w e; r state=done:app=agent-a; w alias_done; r state=idle:app=agent-a; w alias_idle; r state=idle:app=agent-b; w f";
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
    for (gate, state) in [("e", "done"), ("alias_done", "idle")] {
        touch(&s, gate);
        let alias = word(&s, &t, state);
        assert_eq!(alias["activity"], done["activity"], "raw alias retains complete chronology");
        let event = next_event(&mut events, |m| {
            m["params"]["type"] == "status" && m["params"]["data"]["status"]["state"] == state
        });
        assert_eq!(event["params"]["data"]["status"]["state"], state);
        assert_eq!(event["params"]["data"]["activity"], done["activity"]);
    }
    touch(&s, "alias_idle");
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

#[test]
fn named_readiness_aliases_publish_raw_status_with_unchanged_activity() {
    let s = session();
    s.json(&["server", "start"]);
    let mut events = s.connect();
    events.call(1, "events.subscribe", json!({}));
    let body = "r id=build/test:state=working:app=fixture; w a; r id=build/test:state=done:app=fixture; w b; r id=build/test:state=idle:app=fixture; w c; r id=build/test:state=done:app=fixture; w d";
    let t = run(&s, &["--cwd", s.root.to_str().unwrap(), "sh", "-c", &script(&s, body)]);
    record_word(&s, &t, "build/test", "working");
    touch(&s, "a");
    let done = record_word(&s, &t, "build/test", "done");
    assert!(done["activity"]["records"]["build/test"]["completion_ordinal"].as_u64().is_some());
    for (gate, state) in [("b", "idle"), ("c", "done")] {
        touch(&s, gate);
        let alias = record_word(&s, &t, "build/test", state);
        assert_eq!(alias["activity"], done["activity"]);
        let event = next_event(&mut events, |m| {
            m["params"]["type"] == "status" && m["params"]["data"]["records"]["build/test"]["state"] == state
        });
        assert_eq!(event["params"]["data"]["activity"], done["activity"]);
    }
    touch(&s, "d");
    wait_until("finished", || get(&s, &t)["state"] == "finished");
    assert!(get(&s, &t)["activity"]["records"]["build/test"]["completion_ordinal"].is_null());
}
