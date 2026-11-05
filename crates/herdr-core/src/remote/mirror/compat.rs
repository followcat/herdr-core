use super::*;

fn fixture() -> (Arc<Core>, Machine, std::path::PathBuf) {
    let root = std::path::PathBuf::from(format!(
        "/var/tmp/hc-c3-compat-{}-{}",
        std::process::id(),
        crate::platform::random_hex()
    ));
    let paths = crate::paths::Paths {
        socket: root.join("core.sock"),
        holder_socket: root.join("holder.sock"),
        state: root.clone(),
        config: root.join("config.toml"),
    };
    paths.create_dirs().unwrap();
    let core =
        Core::start(paths, crate::api::CoreInfo { id: "host-b".into(), name: "alice-dev".into(), hostname: None })
            .unwrap();
    let machine = Machine {
        name: "host-a".into(),
        target: "host-a".into(),
        via: false,
        temporary: false,
        session: Some("c3-compat".into()),
        id: "origin".into(),
        host: "host-a".into(),
        hostname: None,
        version: "fixture".into(),
    };
    core.lock().machines.push(machine.clone());
    (core, machine, root)
}

#[tokio::test]
async fn framed_old_and_mixed_peers_strip_optional_facts_without_touching_opaque_payloads() {
    for features in
        [vec![], vec!["repository_facts"], vec!["record_activity"], vec!["repository_facts", "record_activity"]]
    {
        let (core, machine, root) = fixture();
        let (frames, mut rx) = mpsc::unbounded_channel();
        let line = Line::new(frames, &json!({"generation":1,"features":features}));
        core.lock().lines.insert(machine.id.clone(), super::super::LineState::Connected(line.clone()));
        let record = json!({"address":"t1","state":"live","status":{"state":"working"},"records":{},"reported":true,
            "repository":{"core_id":"origin","branch":"main"},"activity":{"core_id":"origin","scope":1},
            "labels":{"repository":"ordinary","activity":"ordinary","hc.activity_session":"ordinary"}});
        for method in ["terminal.get", "doc.get"] {
            let calling = core.clone();
            let target = machine.clone();
            let task = tokio::spawn(async move {
                super::super::forward_once(&calling, &target, method, json!({"terminal":"t1"})).await
            });
            let request = rx.recv().await.unwrap();
            let request: Value = serde_json::from_slice(&request[wire::HEADER..]).unwrap();
            assert_eq!(request["method"], method);
            let result = if method == "doc.get" { json!({"value":record,"version":1}) } else { record.clone() };
            line.take_reply(json!({"id":request["id"],"result":result}));
            let got = task.await.unwrap().unwrap();
            if method == "doc.get" {
                assert_eq!(got, result);
                continue;
            }
            assert_eq!(got["address"], "host-a/t1");
            assert_eq!(got.get("repository").is_some(), features.contains(&"repository_facts"));
            assert_eq!(got.get("activity").is_some(), features.contains(&"record_activity"));
            assert_eq!(got["labels"], record["labels"]);
        }
        // A missed snapshot may contain incidental fields from beyond an old hop.
        let missed = format!("{}\n", json!({"method":"missed","params":{"terminals":[record]}}));
        for half in missed.as_bytes().chunks(17) {
            mirror_events(&core, &machine, &line, half);
        }
        for (kind, data) in [
            ("repository", json!({"repository":{"core_id":"origin","branch":"next"}})),
            ("activity", json!({"activity":{"core_id":"origin","scope":2}})),
            (
                "status",
                json!({"status":{"state":"idle"},"records":{},"reported":true,"activity":{"core_id":"origin","scope":3}}),
            ),
        ] {
            mirror_events(
                &core,
                &machine,
                &line,
                format!("{}\n", json!({"method":"event","params":{"type":kind,"terminal":"t1","data":data}}))
                    .as_bytes(),
            );
        }
        let mirrored = core.lock().mirrors[&machine.id]["host-a/t1"].clone();
        assert_eq!(mirrored.get("repository").is_some(), features.contains(&"repository_facts"));
        assert_eq!(mirrored.get("activity").is_some(), features.contains(&"record_activity"));
        if features.contains(&"repository_facts") {
            assert_eq!(mirrored["repository"]["branch"], "next");
        }
        if features.contains(&"record_activity") {
            assert_eq!(mirrored["activity"]["scope"], 3);
        }
        assert_eq!(mirrored["labels"], record["labels"]);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn a_downgraded_hello_invalidates_cached_mirror_facts_before_a_new_snapshot() {
    let (core, machine, root) = fixture();
    mirror_replace(
        &core,
        &machine,
        vec![
            json!({"address":"t1","repository":{"branch":"old"},"activity":{"scope":1},"labels":{"repository":"ordinary"}}),
        ],
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let mut st = core.lock();
        core.subscribe(&mut st, crate::events::Filter::default(), crate::events::Sink::Waiter(tx));
        capabilities(&core, &mut st, &machine, &[]);
        let record = &st.mirrors[&machine.id]["host-a/t1"];
        assert!(record.get("repository").is_none() && record.get("activity").is_none());
        assert_eq!(record["labels"]["repository"], "ordinary");
    }
    let event = rx.try_recv().expect("immediate downgrade publication");
    assert_eq!(event["type"], "created");
    assert!(event["data"].get("activity").is_none());
    drop(core);
    std::fs::remove_dir_all(root).unwrap();
}
