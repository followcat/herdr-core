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
        crate::machines::save(&core.paths.state, &st.machines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
