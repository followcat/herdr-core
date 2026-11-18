#![cfg(unix)] // Windows: named pipes name no peer process, so there is no client to record

mod common;

use common::*;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.env.push(("HC_QUIET_MS".into(), "300".into()));
    s
}

fn journal(s: &Session) -> std::path::PathBuf {
    s.root.join("state/herdr-core/test/ended.jsonl")
}

/// The journal's lines, oldest first, as JSON.
fn entries(s: &Session) -> Vec<Value> {
    std::fs::read_to_string(journal(s))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

fn line_for(s: &Session, event: &str, address: &str) -> Value {
    entries(s)
        .into_iter()
        .rev()
        .find(|l| l["event"] == event && l["address"] == address)
        .unwrap_or_else(|| panic!("no {event} line for {address}: {:?}", entries(s)))
}

/// `hc` as if typed inside terminal `t` (its HC_TERMINAL_ID).
fn hc_in(s: &Session, t: &str, args: &[&str]) -> std::process::Output {
    s.cmd().env("HC_TERMINAL_ID", t).args(args).output().unwrap()
}

/// Reading the journal is local: it must not start a core.
#[test]
fn hc_ended_never_starts_a_core() {
    let s = session();
    let out = s.hc(&["ended"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!s.connect_ok(), "no core was started");
    let json = s.json(&["ended"]);
    assert_eq!(json, json!([]), "an empty journal is an empty list");
}

#[test]
fn a_kill_records_the_request_the_caller_and_the_client_process() {
    let s = session();
    let t = run(&s, &["sleep", "100"]);
    let out = hc_in(&s, &t, &["kill", &t]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let by = get(&s, &t)["ended_by"].clone();
    assert_eq!(by["by"], "terminal.kill");
    assert_eq!(by["caller"], t.as_str(), "the caller hc was told it had");
    assert!(by["pid"].as_i64().is_some_and(|pid| pid > 0), "{by}");
    assert_eq!(by["comm"], "herdr-core", "the client process's own name");
    assert!(by.get("terminal").is_none(), "it ran outside a terminal: {by}");
    assert_eq!(get(&s, &t)["reason"], "killed");

    let line = line_for(&s, "ended", &t);
    assert_eq!(line["reason"], "killed");
    assert_eq!(line["program"], "sleep", "argv[0]'s basename");
    assert_eq!(line["ended_by"], by);

    // `hc ls --all` keeps it in its trimmed JSON too.
    let list = s.json(&["ls", "--all"]);
    let row = list["terminals"].as_array().unwrap().iter().find(|row| row["address"] == t.as_str()).unwrap();
    assert_eq!(row["ended_by"], by, "{list}");
}

/// A client that sends a bare request (no `caller`) still names its process.
#[test]
fn a_bare_client_is_recorded_by_its_process() {
    let s = session();
    let t = run(&s, &["sleep", "100"]);
    let mut conn = s.connect();
    let reply = conn.call(1, "terminal.kill", json!({"terminal": t}));
    let by = reply["result"]["ended_by"].clone();
    assert_eq!(by["by"], "terminal.kill");
    assert!(by.get("caller").is_none(), "the request carried none: {by}");
    assert_eq!(by["pid"], std::process::id() as i64, "this test binary sent it");
    assert!(by["comm"].as_str().is_some_and(|c| !c.is_empty()), "{by}");
    assert!(by.get("terminal").is_none());
}
/// The client runs inside one of this core's terminals: the walk up its
/// parents names that terminal, both as the caller and as where it ran.
#[test]
fn a_client_running_in_a_terminal_names_it() {
    let s = session();
    let victim = run(&s, &["sleep", "100"]);
    let body = format!("sleep 0.4; {} kill {victim}; sleep 100", common::BIN);
    let killer = run(&s, &["sh", "-c", &body]);
    wait_until("the victim ends", || get(&s, &victim)["state"] == "finished");

    let by = get(&s, &victim)["ended_by"].clone();
    assert_eq!(by["caller"], killer.as_str(), "its HC_TERMINAL_ID");
    assert_eq!(by["terminal"], killer.as_str(), "found by walking the client's parents");
    let pid = by["pid"].as_i64().unwrap();
    assert!(pid > 0 && Some(pid) != get(&s, &killer)["pid"].as_i64(), "the hc process, not its terminal's: {by}");
}

/// A helper that asked to die with its parent names the parent, and the
/// parent's own answer.
#[test]
fn a_child_that_dies_with_its_parent_names_the_parent() {
    let s = session();
    let parent = run(&s, &["sleep", "100"]);
    let kid = s.json(&["run", "--bg", "--parent", &parent, "sleep", "100"])["address"].as_str().unwrap().to_string();
    s.json(&["kill", &parent]);
    wait_until("the child ends with its parent", || get(&s, &kid)["state"] == "finished");

    let by = get(&s, &kid)["ended_by"].clone();
    assert_eq!(by["by"], "die_with_parent");
    assert_eq!(by["parent"], parent.as_str());
    assert_eq!(by["parent_by"]["by"], "terminal.kill", "{by}");
    assert_eq!(get(&s, &kid)["reason"], "killed");
}

#[test]
fn removing_a_terminal_leaves_a_removed_line_after_the_record_is_gone() {
    let s = session();
    let t = run(&s, &["sleep", "100"]);
    let out = hc_in(&s, &t, &["kill", &t]);
    assert!(out.status.success());
    let rm = hc_in(&s, &t, &["rm", &t]);
    assert!(rm.status.success(), "{}", String::from_utf8_lossy(&rm.stderr));
    let (_, err) = s.err(&["get", &t]);
    assert_eq!(err["error"]["kind"], "not_found", "the record is gone");

    let removed = line_for(&s, "removed", &t);
    assert_eq!(removed["reason"], "killed");
    assert_eq!(removed["ended_by"]["by"], "terminal.rm");
    assert_eq!(removed["ended_by"]["caller"], t.as_str());
    assert!(entries(&s).iter().any(|l| l["event"] == "ended" && l["address"] == t.as_str()), "the end stays");
}

#[test]
fn a_stop_says_the_core_did_it() {
    let s = session();
    let t = run(&s, &["sleep", "100"]);
    s.json(&["server", "stop"]);
    let line = line_for(&s, "ended", &t);
    assert_eq!(line["reason"], "server_stop");
    assert_eq!(line["ended_by"]["by"], "server_stop");
}

#[test]
fn the_journal_is_bounded_rotates_once_and_keeps_a_previous_file() {
    let mut s = session();
    s.env.push(("HC_ENDED_MAX_BYTES".into(), "400".into()));
    let mut running = Vec::new();
    for _ in 0..3 {
        running.push(run(&s, &["sleep", "100"]));
    }
    for t in &running {
        s.json(&["kill", t]);
    }
    let current = journal(&s);
    let previous = std::path::PathBuf::from(format!("{}.1", current.display()));
    assert!(previous.exists(), "the file that filled up was kept once");
    let size = std::fs::metadata(&current).unwrap().len();
    assert!(size <= 400, "the live file stays under the bound: {size}");
    assert!(!std::fs::read_to_string(&previous).unwrap().trim().is_empty());
    assert!(!std::fs::read_to_string(&current).unwrap().trim().is_empty());
}

/// argv can carry secrets (proxy credentials have been seen there) and so can
/// an environment: only argv[0]'s basename may reach the journal.
#[test]
fn argv_and_the_environment_never_reach_the_journal() {
    let mut s = session();
    s.env.push(("HC_TEST_SECRET".into(), "hunter2-in-the-environment".into()));
    let t =
        s.json(&["run", "--bg", "sh", "-c", "sleep 100", "hunter2-in-argv"])["address"].as_str().unwrap().to_string();
    s.json(&["kill", &t]);

    let text = std::fs::read_to_string(journal(&s)).unwrap();
    assert!(!text.contains("hunter2"), "the journal holds a secret: {text}");
    assert!(!text.contains("sleep"), "argv beyond argv[0] must not be there: {text}");
    assert_eq!(line_for(&s, "ended", &t)["program"], "sh");
}

#[test]
fn a_journal_that_cannot_be_written_never_fails_the_kill() {
    let s = session();
    s.json(&["server", "start"]);
    // A directory where the journal file goes: every append fails.
    std::fs::create_dir_all(journal(&s)).unwrap();
    let t = run(&s, &["sleep", "100"]);
    let out = s.hc(&["kill", &t]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(get(&s, &t)["state"], "finished");
    assert_eq!(get(&s, &t)["ended_by"]["by"], "terminal.kill", "the record still says who");
}

#[test]
fn hc_ended_prints_the_journal_and_since_keeps_only_recent_lines() {
    let s = session();
    let t = run(&s, &["sleep", "100"]);
    let out = hc_in(&s, &t, &["kill", &t]);
    assert!(out.status.success());
    s.json(&["rm", &t]);
    // An older line, as if it had been written a long time ago.
    let first = std::fs::read_to_string(journal(&s)).unwrap();
    let mut old: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
    old["event"] = "ended".into();
    old["address"] = "t1z".into();
    old["name"] = "old".into();
    old["time_ms"] = 0.into();
    let mut file = std::fs::OpenOptions::new().append(true).open(journal(&s)).unwrap();
    std::io::Write::write_all(&mut file, format!("{old}\n").as_bytes()).unwrap();

    let all = s.json(&["ended"]);
    let rows = all.as_array().unwrap();
    assert_eq!(rows.len(), 3, "{all}");
    assert_eq!(rows[0]["event"], "ended");
    assert_eq!(rows[1]["event"], "removed");
    assert_eq!(rows[2]["address"], "t1z");

    let recent = s.json(&["ended", "--since", "1h"]);
    let rows = recent.as_array().unwrap();
    assert_eq!(rows.len(), 2, "the old line is out: {recent}");
    assert!(rows.iter().all(|row| row["address"] == t.as_str()));
    assert_eq!(s.json(&["ended", "--since", "all"]).as_array().unwrap().len(), 3);

    let (code, err) = s.err(&["ended", "--since", "soon"]);
    assert_eq!(err["error"]["kind"], "usage", "{err}");
    assert_eq!(code, 2, "{err}");
}
