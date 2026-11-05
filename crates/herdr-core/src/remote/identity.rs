//! Refresh observed peer identity without changing its saved display alias.

use super::*;

pub(super) fn refresh(core: &Core, id: &str, hello: &Value) {
    let mut st = core.lock();
    let Some(saved) = st.machines.iter_mut().find(|m| m.id == id) else { return };
    let hostname = hello["core"]["hostname"].as_str().filter(|s| !s.is_empty()).map(String::from);
    let version = hello["version"].as_str().unwrap_or(&saved.version).to_string();
    if saved.hostname != hostname || saved.version != version {
        saved.hostname = hostname;
        saved.version = version;
        core.save_machines(&st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_hostname_reconnect_with_blocked_machine_storage_keeps_pty_processing_and_api_calls_live() {
        use std::os::unix::ffi::OsStrExt;
        let root = std::path::PathBuf::from(format!("/var/tmp/hc-c3-hostname-fifo-{}", crate::platform::random_hex()));
        let paths = crate::paths::Paths {
            socket: root.join("core.sock"),
            holder_socket: root.join("holder.sock"),
            state: root.clone(),
            config: root.join("config.toml"),
        };
        paths.create_dirs().unwrap();
        let core =
            Core::start(paths, crate::api::CoreInfo { id: "here".into(), name: "alice-dev".into(), hostname: None })
                .unwrap();
        let id = {
            let mut st = core.lock();
            st.machines.push(
                serde_json::from_value(json!({"name":"away","target":"host-a","session":null,
                "id":"there","host":"alice-dev","hostname":"host-a","version":"v1"}))
                .unwrap(),
            );
            let id = st
                .reg
                .create(
                    crate::terminals::NewTerminal {
                        argv: vec!["fixture".into()],
                        cwd: "/home/user".into(),
                        cols: 80,
                        rows: 24,
                        ..Default::default()
                    },
                    0,
                )
                .unwrap();
            st.screens.insert(id, ghostty_vt::Terminal::new(80, 24, 1 << 20));
            id
        };
        let fifo = root.join("machines.tmp");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (refreshed, received) = std::sync::mpsc::channel();
        let reconnecting = core.clone();
        let refresh_thread = std::thread::spawn(move || {
            refresh(&reconnecting, "there", &json!({"core":{"hostname":"host-b"},"version":"v2"}));
            refreshed.send(()).unwrap();
        });
        let refresh_returned = received.recv_timeout(Duration::from_millis(250)).is_ok();
        let (responsive, observed) = std::sync::mpsc::channel();
        let serving = core.clone();
        let api_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let result = runtime.block_on(async {
                serving.output(&mut serving.lock(), id, b"alive\r\n\x1b]7501;state=working\x1b\\");
                let call = crate::api::Call { core: &serving, conn: None, plain: None };
                let record = crate::api::dispatch(&call, "terminal.get", json!({"terminal":"t1"})).await.unwrap();
                let status = crate::api::dispatch(&call, "server.status", json!({})).await.unwrap();
                record["status"]["state"] == "working"
                    && status["core"]["name"] == "alice-dev"
                    && serving.screen_text(&serving.lock(), id).unwrap().contains("alive")
            });
            responsive.send(result).unwrap();
        });
        let live = observed.recv_timeout(Duration::from_secs(1)).ok();
        // Always release the FIFO before asserting, including on the old code.
        let reader = std::fs::File::open(&fifo).unwrap();
        refresh_thread.join().unwrap();
        api_thread.join().unwrap();
        drop(core);
        drop(reader);
        std::fs::remove_dir_all(root).unwrap();
        assert!(refresh_returned && live == Some(true), "hostname persistence blocked PTY processing/API calls");
    }

    #[test]
    fn reconnect_replaces_or_clears_observed_hostname_without_renaming() {
        let root = std::path::PathBuf::from(format!("/var/tmp/c3-hostname-{}", crate::platform::random_hex()));
        let paths = crate::paths::Paths {
            socket: root.join("core.sock"),
            holder_socket: root.join("holder.sock"),
            state: root.clone(),
            config: root.join("config.toml"),
        };
        paths.create_dirs().unwrap();
        let core =
            Core::start(paths, crate::api::CoreInfo { id: "here".into(), name: "alice-dev".into(), hostname: None })
                .unwrap();
        core.lock().machines.push(
            serde_json::from_value(json!({
                "name":"away", "target":"host-a", "session":null, "id":"there", "host":"alice-dev", "version":"v1"
            }))
            .unwrap(),
        );
        for hostname in [json!("host-a"), json!("host-b"), Value::Null] {
            refresh(&core, "there", &json!({"core":{"hostname":hostname},"version":"v2"}));
            let st = core.lock();
            assert_eq!(
                serde_json::to_value(&st.machines[0]).unwrap().get("hostname"),
                if hostname.is_null() { None } else { Some(&hostname) }
            );
            assert_eq!(st.machines[0].host, "alice-dev");
            assert_eq!(st.machines[0].name, "away");
            assert_eq!(st.machines[0].version, "v2");
        }
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }
}
