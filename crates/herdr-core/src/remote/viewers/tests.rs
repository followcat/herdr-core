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
