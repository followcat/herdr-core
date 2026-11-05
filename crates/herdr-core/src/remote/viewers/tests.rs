use super::*;

#[test]
fn quiet_release_needs_request_nonobserver_and_both_confirmations() {
    for feature in [false, true] {
        let (frames, _rx) = mpsc::unbounded_channel();
        let line = Line::new(frames, &json!({"features":if feature {vec!["quiet_human_release"]} else {vec![]}}));
        for requested in [false, true] {
            for observe in [false, true] {
                let mut params = json!({"quiet_human_release":requested,"observe":observe});
                negotiate_quiet(&line, &mut params);
                for reply in [json!({}), json!({"quiet_human_release":false}), json!({"quiet_human_release":true})] {
                    assert_eq!(
                        quiet_release(&line, &params, &reply),
                        feature && requested && !observe && reply["quiet_human_release"] == true
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn lost_quiet_contract_is_rejected_before_mapping_resumed_output() {
    for feature in [false, true] {
        for confirmed in [false, true] {
            let (frames, mut rx) = mpsc::unbounded_channel();
            let line = Line::new(frames, &json!({"features":if feature {vec!["quiet_human_release"]} else {vec![]}}));
            let conn = Arc::new(Conn::default());
            conn.with(|out| out.open_paused(7));
            let calling = line.clone();
            let sink = conn.clone();
            let task = tokio::spawn(async move {
                calling
                    .call_into(
                        "host-a",
                        "terminal.attach",
                        json!({}),
                        Some(((sink, 7), Required { quiet: true, ..Default::default() })),
                        Duration::from_secs(1),
                    )
                    .await
            });
            let frame = rx.recv().await.unwrap();
            let request: Value = serde_json::from_slice(&frame[wire::HEADER..]).unwrap();
            line.take_reply(json!({"id":request["id"],"result":{"channel":9,"quiet_human_release":confirmed}}));
            let reply = task.await.unwrap();
            assert_eq!(reply.is_ok(), feature && confirmed);
            assert_eq!(lock(&line.streams).contains_key(&9), feature && confirmed);
            if !(feature && confirmed) {
                assert_eq!(rx.recv().await.unwrap(), wire::encode(9, wire::CLOSE, b""));
                assert_eq!(conn.outbox().queued(7), 0);
            }
        }
    }
}

#[tokio::test]
async fn a_false_quiet_contract_stays_false_after_a_capable_reconnect() {
    let root = std::path::PathBuf::from(format!(
        "/var/tmp/hc-c3-quiet-false-{}-{}",
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
        session: Some("c3-quiet".into()),
        id: "origin".into(),
        host: "host-a".into(),
        hostname: None,
        version: "fixture".into(),
    };
    let conn = Arc::new(Conn::default());
    conn.with(|out| out.open(7));
    let (old_frames, _old_rx) = mpsc::unbounded_channel();
    core.lock().relayed.insert(
        (conn_key(&conn), 7),
        Relayed {
            automatic_input: false,
            quiet_human_release: false,
            line: Line::new(old_frames, &json!({})),
            there: 9,
            conn: conn.clone(),
            machine: machine.id.clone(),
            params: json!({"terminal":"t1","quiet_human_release":false}),
        },
    );
    let (frames, mut rx) = mpsc::unbounded_channel();
    let line = Line::new(frames, &json!({"features":["quiet_human_release"]}));
    let task = tokio::spawn(reattach(core.clone(), machine, line.clone()));
    let request = rx.recv().await.unwrap();
    let request: Value = serde_json::from_slice(&request[wire::HEADER..]).unwrap();
    assert_eq!(request["params"]["quiet_human_release"], false);
    // Even an unsolicited true from the peer cannot upgrade an existing stream.
    line.take_reply(json!({"id":request["id"],"result":{"channel":10,"quiet_human_release":true}}));
    task.await.unwrap();
    assert!(!core.lock().relayed[&(conn_key(&conn), 7)].quiet_human_release);
    assert!(relay_in(&core, &conn, 7, wire::QUIET_HUMAN_RELEASE, b"ignored"));
    assert!(rx.try_recv().is_err());
    drop(core);
    std::fs::remove_dir_all(root).unwrap();
}
