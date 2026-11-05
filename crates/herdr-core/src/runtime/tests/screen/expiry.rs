use super::*;

async fn expire_many(catalog: crate::detect::Catalog, must_yield: bool) {
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    const COUNT: usize = 512;
    let (mut core, root) = test_core();
    let (record, mut written) = tokio::sync::mpsc::unbounded_channel();
    Arc::get_mut(&mut core).unwrap().registry_writer = RegistryWriter::start(move |bytes| {
        record.send(bytes.to_vec()).unwrap();
        Ok(())
    })
    .unwrap();
    let (first, _frames) = terminal_on_fake_holder(&core);
    {
        let mut st = core.lock();
        for index in 0..COUNT {
            let id = if index == 0 {
                first
            } else {
                st.reg
                    .create(crate::terminals::NewTerminal { argv: vec!["sh".into()], ..Default::default() }, 0)
                    .unwrap()
            };
            let t = st.reg.terminals.get_mut(&id).unwrap();
            t.holder = Some("fake".into());
            t.pid = Some(4321);
            crate::status::apply_inferred(t, "working", None);
            st.screen_status.entry(id).or_default().unconfirmed_until =
                Some(tokio::time::Instant::now() - Duration::from_millis(1));
        }
    }
    {
        let pass = core.screen_status_pass(&catalog);
        tokio::pin!(pass);
        if must_yield {
            // Deterministic responsiveness proof: the pass must hand back the
            // executor with roots remaining, and release Core.state when it does.
            let pending = poll_fn(|cx| Poll::Ready(pass.as_mut().poll(cx).is_pending())).await;
            assert!(pending, "expiry processed the whole registry without yielding");
            let st =
                core.state.try_lock().unwrap_or_else(|_| panic!("expiry retained the global lock across its yield"));
            let expired = st.reg.terminals.values().filter(|t| t.status.is_none()).count();
            assert!(expired > 0 && expired < COUNT, "no opportunity for unrelated core operations during the batch");
        }
        pass.await;
    }
    core.flush_saved().await.unwrap();
    assert!(written.try_recv().is_err(), "expiry serialized the registry immediately instead of coalescing");
    {
        let st = core.lock();
        assert_eq!(st.reg.terminals.values().filter(|t| t.status.is_none()).count(), COUNT);
        assert!(st.reg.terminals.values().all(|t| t.turn == 0 && t.news == 0));
        assert_eq!(st.events.seq, COUNT as u64);
    }
    // A real writer acknowledgement proves the entire burst produces one
    // deferred checkpoint, containing every withdrawal.
    let saver = tokio::spawn(core.clone().save_when_due());
    let bytes = tokio::time::timeout(Duration::from_secs(8), written.recv()).await.unwrap().unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["terminals"].as_object().unwrap().len(), COUNT);
    assert!(saved["terminals"].as_object().unwrap().values().all(|t| t.get("status").is_none_or(|s| s.is_null())));
    core.flush_saved().await.unwrap();
    assert!(written.try_recv().is_err(), "more than one checkpoint for the expiry burst");
    saver.abort();
    let _ = saver.await;
    drop(core);
    remove_root(&root);
}

#[tokio::test]
async fn empty_catalog_expiry_yields_the_lock_and_coalesces_persistence() {
    expire_many(
        crate::detect::Catalog::load(None, Some(&crate::detect::Limits { enabled: Some(false), agents: None })),
        true,
    )
    .await;
}

#[tokio::test]
async fn ordinary_expiry_also_coalesces_persistence() {
    expire_many(test_detect_catalog(), false).await;
}
