#![cfg(unix)]

mod common;

use common::*;
use serde_json::{Value, json};

#[test]
fn os_hostname_is_independent_of_the_saved_core_name() {
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-hostname".into()));
    let status = s.json(&["server", "status"]);
    let pid = status["pid"].as_i64().unwrap();
    s.json(&["server", "stop"]);
    wait_until("scratch core stops", || !pid_alive(pid));
    let path = s.root.join("state/herdr-core/c3-hostname/core.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["name"] = "alice-dev".into();
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let status = s.json(&["server", "status"]);
    assert_eq!(status["core"]["name"], "alice-dev");
    let observed = herdr_core::platform::observed_hostname().unwrap();
    assert_eq!(status["core"]["hostname"], observed);
    // The fixture's custom session needs its own socket rather than common::socket's default.
    let stream = std::os::unix::net::UnixStream::connect(s.root.join("run/herdr-core/c3-hostname.sock")).unwrap();
    use std::io::{BufRead, BufReader, Write};
    let mut writer = stream.try_clone().unwrap();
    writeln!(writer, "{}", json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{}})).unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    let hello: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(hello["result"]["core"], status["core"]);
    assert!(hello["result"]["features"].as_array().unwrap().contains(&json!("os_hostname")));
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(saved.get("hostname").is_none(), "observations must not become permanent identity");
    s.json(&["server", "stop"]);
}

#[test]
fn old_identity_json_has_no_observed_hostname() {
    let old = json!({"id":"host-a-core","name":"alice-dev"});
    let core: herdr_core::api::CoreInfo = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(core.hostname, None);
    assert_eq!(serde_json::to_value(core).unwrap(), old);
}
