#![cfg(unix)]

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;

fn git(s: &Session, root: &Path, args: &[&str]) {
    let out = s.program("git").arg("-C").arg(root).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn session() -> Session {
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-repository-publish".into()));
    s
}

#[test]
fn repository_facts_follow_cwd_and_forward_through_a_scratch_machine() {
    let s = session();
    s.json(&["server", "start"]);
    let hello = s.connect().call(1, "hello", json!({}));
    assert!(hello["result"]["features"].as_array().unwrap().contains(&json!("repository_facts")));
    let root = s.root.join("acme-shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&s, &root, &["init", "--quiet", "-b", "main"]);
    git(&s, &root, &["config", "user.name", "alice"]);
    git(&s, &root, &["config", "user.email", "alice@example.invalid"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "initial"]);
    git(&s, &root, &["branch", "upstream"]);
    git(&s, &root, &["branch", "--set-upstream-to=upstream"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "ahead"]);
    let mut conn = s.connect();
    let subscribed = conn.call(2, "events.subscribe", json!({}));
    assert!(subscribed["result"]["terminals"].is_array());
    let t = run(
        &s,
        &[
            "--cwd",
            root.join("src").to_str().unwrap(),
            "sh",
            "-c",
            &format!(
                "while [ ! -e '{}/move' ]; do sleep 0.02; done; cd '{}'; exec sleep 100",
                s.root.display(),
                s.root.display()
            ),
        ],
    );
    let mut repository = Value::Null;
    wait_until("ready repository facts", || {
        repository = get(&s, &t)["repository"].clone();
        repository["state"] == "ready" && repository["branch"] == "main"
    });
    assert_eq!(repository["auto_label"], "acme-shop");
    assert_eq!(repository["ahead_behind"], json!({"ahead":1,"behind":0}));
    assert_eq!(repository["checkout"]["is_linked_worktree"], false);
    let event = next_event(&mut conn, |m| {
        m["params"]["type"] == "repository" && m["params"]["data"]["repository"]["state"] == "ready"
    });
    assert_eq!(event["params"]["terminal"], t);
    assert_eq!(event["params"]["data"]["repository"], repository);

    let mut relay = session();
    relay.env.push(fake_ssh(&relay, &[("host-a", &s)]));
    relay.json(&["machine", "add", "host-a", "--session", "c3-repository-publish"]);
    let target = format!("host-a/{t}");
    wait_until("remote repository facts", || get(&relay, &target)["repository"] == repository);
    let listed = relay.connect().call(3, "terminal.list", json!({}))["result"].clone();
    let listed = listed["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["address"] == target)
        .expect("listed remote terminal");
    assert_eq!(listed["repository"], repository, "{listed}");
    touch(&s, "move");
    wait_until("plain home facts", || get(&s, &t)["repository"]["auto_label"] == "~");
    wait_until("remote clear patch", || {
        let facts = get(&relay, &target)["repository"].clone();
        facts["state"] == "ready" && facts["checkout"].is_null() && facts["branch"].is_null()
    });
}
