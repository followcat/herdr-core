use super::*;

fn stat(pid: i32, pgrp: i32, started: u64) -> String {
    let mut fields = vec!["0".to_string(); 20];
    fields[0] = "S".into();
    fields[2] = pgrp.to_string();
    fields[19] = started.to_string();
    format!("{pid} (devin ) (test)) {}\n", fields.join(" "))
}

#[tokio::test(start_paused = true)]
async fn an_unrelated_reused_pid_cannot_confirm_a_restored_root() {
    let (core, root) = test_core();
    let catalog = test_detect_catalog();
    let (id, _) = screen_agent(&core, "devin");
    core.inferred_status(&mut core.lock(), id, Some(("blocked", None)));
    core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
    {
        let mut st = core.lock();
        st.reg.terminals.get_mut(&id).unwrap().foreground.as_mut().unwrap().started = Some(10);
        st.screen_status.entry(id).or_default().unconfirmed_until =
            Some(tokio::time::Instant::now() + crate::screen_status::RECONFIRM);
    }
    let baseline = {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        (t.turn, t.news, st.events.seq)
    };
    // Deterministic counterpart of the private-PID-namespace reproduction:
    // pid 100 was reused for another devin, whose actual group is not 100.
    let result = platform::read_linux_foreground(100, || Ok(stat(100, 0, 20)), || Ok(agent("devin")));
    assert!(result.is_err(), "metadata for an unrelated replacement was accepted");
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert_eq!(t.status.as_ref().unwrap().state, "blocked");
        assert!(st.screen_status[&id].unconfirmed_until.is_some());
        assert!(!st.screen_status[&id].foreground_observed);
        assert_eq!((t.turn, t.news, st.events.seq), baseline);
    }
    tokio::time::advance(crate::screen_status::RECONFIRM).await;
    core.screen_status_pass(&catalog).await;
    {
        let st = core.lock();
        let t = &st.reg.terminals[&id];
        assert!(t.status.is_none(), "the unconfirmed root cannot stand forever");
        assert_eq!((t.turn, t.news), (baseline.0, baseline.1));
    }
    drop(core);
    remove_root(&root);
}

#[test]
fn linux_metadata_requires_a_stable_group_leader_incarnation() {
    for (before, after) in [
        (stat(100, 100, 10), stat(100, 100, 20)),
        (stat(100, 100, 10), stat(100, 200, 10)),
        (stat(200, 100, 10), stat(200, 100, 10)),
    ] {
        let mut reads = [before, after].into_iter();
        let result = platform::read_linux_foreground(100, || Ok(reads.next().unwrap()), || Ok(agent("devin")));
        assert!(result.is_err(), "incarnation drift or an unrelated leader was accepted");
    }
    let fg = platform::read_linux_foreground(100, || Ok(stat(100, 100, 10)), || Ok(agent("devin"))).unwrap().unwrap();
    assert_eq!(fg.started, Some(10));
    assert!(serde_json::to_value(&fg).unwrap().get("started").is_none(), "the stamp is internal");
}

#[tokio::test(start_paused = true)]
async fn a_new_start_stamp_changes_the_owner_and_rejects_the_old_verdict() {
    for unconfirmed in [false, true] {
        let (core, root) = test_core();
        let catalog = test_detect_catalog();
        let (id, _) = screen_agent(&core, "devin");
        core.lock().reg.terminals.get_mut(&id).unwrap().foreground.as_mut().unwrap().started = Some(10);
        core.inferred_status(&mut core.lock(), id, Some(("working", None)));
        core.output(&mut core.lock(), id, b"WORKING NOW\r\n");
        {
            let mut st = core.lock();
            let w = st.screen_status.entry(id).or_default();
            w.foreground_observed = true;
            if unconfirmed {
                w.unconfirmed_until = Some(tokio::time::Instant::now() + crate::screen_status::RECONFIRM);
            }
        }
        let verdicts = crate::screen_status::evaluate(core.screen_status_collect(&catalog));
        assert_eq!(verdicts.len(), 1);
        let (turn, news) = {
            let st = core.lock();
            (st.reg.terminals[&id].turn, st.reg.terminals[&id].news)
        };
        {
            let mut st = core.lock();
            let mut now = agent("devin");
            now.started = Some(20);
            let was = st.reg.terminals.get_mut(&id).unwrap().foreground.replace(now);
            core.foreground_changed(&mut st, id, was.as_ref());
        }
        assert!(word(&core, id).is_none(), "same pid/agent with a new start time is a new owner");
        for (snap, verdict) in verdicts {
            core.screen_status_commit(&catalog, snap, verdict);
        }
        {
            let st = core.lock();
            let t = &st.reg.terminals[&id];
            assert!(t.status.is_none(), "the old snapshot cannot restore the previous owner's verdict");
            assert_eq!(t.turn, turn + u64::from(!unconfirmed));
            if unconfirmed {
                assert_eq!(t.news, news);
            }
        }
        drop(core);
        remove_root(&root);
    }
}
