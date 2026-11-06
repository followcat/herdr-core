use super::*;
use crate::terminals::{NewTerminal, Registry};
use ghostty_vt::StatusReport;

fn terminal() -> Terminal {
    let mut reg = Registry::default();
    let id =
        reg.create(NewTerminal { cwd: "/home/user".into(), argv: vec!["sh".into()], ..Default::default() }, 0).unwrap();
    let mut t = reg.terminals.remove(&id).unwrap();
    prepare(&mut t, "host-a", "epoch-a", false);
    t
}

fn report(t: &mut Terminal, counter: &mut u64, id: &str, state: &'static str) {
    crate::status::apply(
        t,
        &StatusReport {
            id: id.into(),
            state,
            kind: None,
            progress: None,
            app: String::new(),
            title: String::new(),
            message: String::new(),
        },
    );
    observe(t, counter, "host-a", "epoch-a", Evidence::Fresh(id));
}

#[test]
fn acquisition_and_completion_follow_the_full_transition_table() {
    for (words, completes) in [
        (vec!["idle"], false),
        (vec!["done"], false),
        (vec!["idle", "done", "idle"], false),
        (vec!["done", "idle", "done"], false),
        (vec!["idle", "working", "done", "idle"], true),
        (vec!["done", "working", "idle", "done"], true),
        (vec!["blocked", "idle", "done"], false),
        (vec!["working", "blocked", "done", "idle"], true),
        (vec!["working", "error", "idle", "done"], false),
        (vec!["working", "unknown", "done", "idle"], false),
        (vec!["blocked", "idle"], false),
        (vec!["blocked", "done"], false),
        (vec!["working", "idle"], true),
        (vec!["working", "done"], true),
        (vec!["working", "blocked", "idle"], true),
        (vec!["working", "blocked", "done"], true),
        (vec!["working", "error", "idle"], false),
        (vec!["working", "unknown", "idle"], false),
        (vec!["idle", "blocked", "idle"], true),
        (vec!["idle", "working", "clear", "idle"], false),
    ] {
        for id in ["", "build/test"] {
            let (mut t, mut counter) = (terminal(), 0);
            for word in &words {
                report(&mut t, &mut counter, id, word);
            }
            let clock = &t.activity.as_ref().unwrap().records[id];
            assert_eq!(clock.completion_ordinal.is_some(), completes, "{id}: {words:?}");
            if completes {
                assert_eq!(clock.completion_ordinal, clock.state_ordinal);
            }
        }
    }
}

// Only the remembered raw word may change on a readiness alias.
fn expect_alias(t: &mut Terminal, counter: &mut u64, id: &str, word: &str, evidence: Evidence<'_>) {
    let before = t.activity.clone().unwrap();
    let count = *counter;
    let mut guards = t.activity_guard.clone();
    guards.records.get_mut(id).unwrap().word = Some(word.into());
    let status = if id.is_empty() { t.status.as_mut() } else { t.records.get_mut(id) }.unwrap();
    status.state = word.into();
    assert!(!observe(t, counter, &before.core_id, &before.epoch, evidence), "{id}: {word}");
    assert_eq!(*counter, count);
    assert_eq!(t.activity.as_ref(), Some(&before));
    assert_eq!(t.activity_guard, guards);
}

#[test]
fn readiness_aliases_preserve_exact_clocks_and_guards_until_new_work() {
    for id in ["", "build/test"] {
        for words in [
            vec!["idle"],
            vec!["done"],
            vec!["blocked", "idle"],
            vec!["working", "idle"],
            vec!["idle", "working", "blocked", "done"],
            vec!["working", "error", "idle"],
            vec!["working", "unknown", "done"],
        ] {
            let (mut t, mut counter) = (terminal(), 0);
            for word in &words {
                report(&mut t, &mut counter, id, word);
            }
            let last = *words.last().unwrap();
            let other = if last == "idle" { "done" } else { "idle" };
            // A viewer's local seen token cannot be advanced by an alias.
            let seen = t.activity.as_ref().unwrap().records[id].completion_ordinal;
            for word in [other, last, other] {
                expect_alias(&mut t, &mut counter, id, word, Evidence::Fresh(id));
                assert_eq!(t.activity.as_ref().unwrap().records[id].completion_ordinal, seen);
            }
            let before = counter;
            report(&mut t, &mut counter, id, "working");
            report(&mut t, &mut counter, id, "idle");
            let a = t.activity.as_ref().unwrap();
            assert_eq!(counter, before + 2);
            assert_eq!(a.revision, counter);
            assert_eq!(a.records[id], Clock { state_ordinal: Some(counter), completion_ordinal: Some(counter) });
            assert!(a.records[id].completion_ordinal > seen);
            prepare(&mut t, "host-a", "epoch-b", true);
            expect_alias(&mut t, &mut counter, id, "done", Evidence::Replay);
        }
    }
}

#[test]
fn alias_after_clear_recreation_and_scope_reset_cannot_revive_a_marker() {
    for id in ["", "build/test"] {
        let (mut t, mut counter) = (terminal(), 0);
        report(&mut t, &mut counter, id, "working");
        report(&mut t, &mut counter, id, "done");
        assert!(t.activity.as_ref().unwrap().records[id].completion_ordinal.is_some());
        report(&mut t, &mut counter, id, "clear");
        assert!(t.activity.as_ref().unwrap().records.get(id).is_none_or(|c| c.completion_ordinal.is_none()));
        report(&mut t, &mut counter, id, "idle");
        assert!(t.activity.as_ref().unwrap().records[id].completion_ordinal.is_none());
        expect_alias(&mut t, &mut counter, id, "done", Evidence::Fresh(id));
        report(&mut t, &mut counter, id, "working");
        report(&mut t, &mut counter, id, "idle");
        assert!(t.activity.as_ref().unwrap().records[id].completion_ordinal.is_some());
        let scope = t.activity.as_ref().unwrap().scope;
        assert!(reset(&mut t, &mut counter, "host-a", "epoch-a"));
        assert_eq!(t.activity.as_ref().unwrap().scope, scope + 1);
        assert!(t.activity.as_ref().unwrap().records[id].completion_ordinal.is_none());
        assert!(t.activity_guard.records[id].acquiring);
        assert!(!t.activity_guard.records[id].work);
        expect_alias(&mut t, &mut counter, id, "done", Evidence::Fresh(id));
    }
}

#[test]
fn metadata_repetition_named_subtree_removal_and_clocks_ignore_wall_time_and_turn() {
    let (mut t, mut counter) = (terminal(), 0);
    report(&mut t, &mut counter, "", "working");
    let first = counter;
    t.status.as_mut().unwrap().title = "new title".into();
    t.status.as_mut().unwrap().kind = Some("question".into());
    t.status_ms = Some(u64::MAX);
    report(&mut t, &mut counter, "", "working");
    assert_eq!(counter, first);
    report(&mut t, &mut counter, "build/test", "working");
    let named = counter;
    t.status_ms = Some(0);
    report(&mut t, &mut counter, "", "blocked");
    report(&mut t, &mut counter, "", "idle");
    assert!(t.activity.as_ref().unwrap().records[""].completion_ordinal > Some(named));
    report(&mut t, &mut counter, "build", "clear");
    assert!(!t.activity.as_ref().unwrap().records.contains_key("build/test"));
    assert_eq!(t.activity.as_ref().unwrap().revision, counter);
    assert_eq!(t.turn, 1, "existing accounting is unchanged");
    t.status.as_mut().unwrap().source = Some("screen".into());
    assert!(crate::status::withdraw_inferred(&mut t).status);
    observe(&mut t, &mut counter, "host-a", "epoch-a", Evidence::Replay);
    assert!(t.activity.as_ref().unwrap().records[""].completion_ordinal.is_none());
    report(&mut t, &mut counter, "", "idle");
    expect_alias(&mut t, &mut counter, "", "done", Evidence::Fresh(""));
}

#[test]
fn a_clear_orders_the_root_edge_before_lexical_named_removals() {
    let (mut t, mut counter) = (terminal(), 0);
    report(&mut t, &mut counter, "", "working");
    report(&mut t, &mut counter, "build", "working");
    let before = counter;
    report(&mut t, &mut counter, "", "clear");
    let a = t.activity.as_ref().unwrap();
    assert_eq!(a.records[""].state_ordinal, Some(before + 1));
    assert_eq!(a.revision, before + 2);
    assert_eq!(a.records.len(), 1);
}

#[test]
fn durable_guards_distinguish_real_work_from_unknown_prehistory_and_replay() {
    let (mut genuine, mut counter) = (terminal(), 0);
    report(&mut genuine, &mut counter, "", "working");
    let bytes = serde_json::to_vec(&genuine).unwrap();
    let mut restored: Terminal = serde_json::from_slice(&bytes).unwrap();
    prepare(&mut restored, "host-a", "epoch-b", true);
    restored.status.as_mut().unwrap().state = "idle".into();
    observe(&mut restored, &mut counter, "host-a", "epoch-b", Evidence::Replay);
    assert!(restored.activity.as_ref().unwrap().records[""].completion_ordinal.is_some());
    assert_eq!(restored.activity.as_ref().unwrap().epoch, "epoch-b");

    let mut migrated = genuine.clone();
    migrated.activity = None;
    migrated.activity_guard = Default::default();
    prepare(&mut migrated, "host-a", "epoch-b", true);
    assert!(migrated.activity.as_ref().unwrap().records[""].state_ordinal.is_none());
    // A fresh child report must not turn a replayed root into observed work.
    report(&mut migrated, &mut counter, "build", "working");
    migrated.status.as_mut().unwrap().state = "idle".into();
    observe(&mut migrated, &mut counter, "host-a", "epoch-b", Evidence::Replay);
    assert!(migrated.activity.as_ref().unwrap().records[""].completion_ordinal.is_none());
    report(&mut migrated, &mut counter, "", "working");
    report(&mut migrated, &mut counter, "", "idle");
    assert!(migrated.activity.as_ref().unwrap().records[""].completion_ordinal.is_some());
}

#[test]
fn app_replacement_resets_completion_before_repeated_or_aliased_readiness() {
    for id in ["", "build/test"] {
        let (mut t, mut counter) = (terminal(), 0);
        report(&mut t, &mut counter, id, "working");
        let status = if id.is_empty() { t.status.as_mut() } else { t.records.get_mut(id) }.unwrap();
        status.app = "agent-a".into();
        observe(&mut t, &mut counter, "host-a", "epoch-a", Evidence::Fresh(id));
        report(&mut t, &mut counter, id, "idle");
        assert!(t.activity.as_ref().unwrap().records[id].completion_ordinal.is_some());
        for word in ["idle", "done"] {
            let (mut replaced, mut count) = (t.clone(), counter);
            let status = if id.is_empty() { replaced.status.as_mut() } else { replaced.records.get_mut(id) }.unwrap();
            status.app = "agent-b".into();
            status.state = word.into();
            let scope = replaced.activity.as_ref().unwrap().scope;
            assert!(observe(&mut replaced, &mut count, "host-a", "epoch-a", Evidence::Fresh(id)));
            assert!(replaced.activity.as_ref().unwrap().scope > scope);
            assert!(replaced.activity.as_ref().unwrap().records[id].completion_ordinal.is_none());
            let other = if word == "idle" { "done" } else { "idle" };
            expect_alias(&mut replaced, &mut count, id, other, Evidence::Fresh(id));
            assert!(
                !serde_json::to_value(crate::terminals::Public(replaced))
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .contains_key("activity_guard")
            );
        }
    }
}

#[test]
fn checked_exhaustion_drops_activity_instead_of_wrapping() {
    let (mut t, mut counter) = (terminal(), u64::MAX);
    report(&mut t, &mut counter, "", "working");
    assert!(t.activity.is_none());
    assert_eq!(counter, u64::MAX);
    let (mut t, mut counter) = (terminal(), 0);
    report(&mut t, &mut counter, "", "idle");
    counter = u64::MAX;
    report(&mut t, &mut counter, "", "done");
    assert!(t.activity.is_none(), "readiness aliases cannot bypass exhaustion");
    assert_eq!(counter, u64::MAX);
}

#[test]
fn maximum_legal_records_have_complete_clocks_and_eviction_removes_only_the_evicted_clock() {
    let (mut t, mut counter) = (terminal(), u64::MAX - 1024);
    for n in 0..255 {
        let id = format!("{n:03}/{}", "a".repeat(124));
        assert_eq!(id.len(), 128);
        report(&mut t, &mut counter, &id, "working");
        report(&mut t, &mut counter, &id, "done");
    }
    let a = t.activity.as_ref().unwrap();
    assert_eq!(a.records.len(), 256);
    assert!(serde_json::to_vec(a).unwrap().len() > 32 * 1024, "no deferred fence-size cap truncates legal Activity");
    let evicted = t.records.keys().next().unwrap().clone();
    report(&mut t, &mut counter, "new", "working");
    let a = t.activity.as_ref().unwrap();
    assert_eq!(a.records.len(), 256);
    assert!(!a.records.contains_key(&evicted));
    assert_eq!(a.revision, counter);
    assert!(a.records.contains_key(""));
    report(&mut t, &mut counter, &evicted, "idle");
    assert!(t.activity.as_ref().unwrap().records[&evicted].completion_ordinal.is_none());
    expect_alias(&mut t, &mut counter, &evicted, "done", Evidence::Fresh(&evicted));
    let mut reg = Registry { activity_ordinal: 0, ..Default::default() };
    reg.terminals.insert(1, t);
    restore(&mut reg, "host-a", "next-epoch");
    assert_eq!(
        reg.activity_ordinal, counter,
        "resume above every saved record even if the registry counter was missing"
    );
}

#[test]
fn foreground_scope_changes_publish_without_a_status_change_and_hydration_is_not_replacement() {
    use crate::{
        events::{Filter, Sink},
        paths::Paths,
        runtime::Core,
    };
    let root = std::path::PathBuf::from(format!(
        "/var/tmp/hc-c3-activity-{}-{}",
        std::process::id(),
        crate::platform::random_hex()
    ));
    let paths = Paths {
        socket: root.join("core.sock"),
        holder_socket: root.join("holder.sock"),
        state: root.clone(),
        config: root.join("config.toml"),
    };
    paths.create_dirs().unwrap();
    let core =
        Core::start(paths, crate::api::CoreInfo { id: "host-a".into(), name: "alice-dev".into(), hostname: None })
            .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let mut st = core.lock();
        let id = st
            .reg
            .create(NewTerminal { argv: vec!["fixture".into()], cwd: "/home/user".into(), ..Default::default() }, 0)
            .unwrap();
        st.reg.terminals.get_mut(&id).unwrap().foreground = Some(crate::foreground::Foreground {
            pid: 100,
            name: "fixture".into(),
            argv: vec!["fixture".into()],
            cwd: "/home/user".into(),
            started: None,
            agent: None,
        });
        core.activity_foreground(&mut st, id);
        let t = st.reg.terminals.get_mut(&id).unwrap();
        let working = crate::status::apply(
            t,
            &StatusReport {
                id: String::new(),
                state: "working",
                kind: None,
                progress: None,
                app: String::new(),
                title: String::new(),
                message: String::new(),
            },
        );
        core.status_changed(&mut st, id, working);
        let t = st.reg.terminals.get_mut(&id).unwrap();
        let change = crate::status::apply(
            t,
            &StatusReport {
                id: String::new(),
                state: "idle",
                kind: None,
                progress: None,
                app: String::new(),
                title: String::new(),
                message: String::new(),
            },
        );
        core.status_changed(&mut st, id, change);
        assert!(st.reg.terminals[&id].activity.as_ref().unwrap().records[""].completion_ordinal.is_some());
        let before = st.reg.terminals[&id].activity.clone();
        core.subscribe(&mut st, Filter::default(), Sink::Waiter(tx));
        let f = st.reg.terminals.get_mut(&id).unwrap().foreground.as_mut().unwrap();
        f.started = Some(5);
        f.cwd = "/home/user/acme-shop".into();
        core.activity_foreground(&mut st, id);
        assert_eq!(st.reg.terminals[&id].activity, before);
        assert!(rx.try_recv().is_err(), "stamp hydration/cwd do not change scope");
        st.reg.terminals.get_mut(&id).unwrap().foreground.as_mut().unwrap().pid = 101;
        core.activity_foreground(&mut st, id);
        let event = rx.try_recv().expect("scope publication");
        assert_eq!(event["type"], "activity");
        assert_eq!(event["data"]["activity"], serde_json::to_value(st.reg.terminals[&id].activity.as_ref()).unwrap());
        assert_eq!(st.reg.terminals[&id].status.as_ref().unwrap().state, "idle");
        assert!(event["data"]["activity"]["records"][""]["completion_ordinal"].is_null());
        let Registry { terminals, activity_ordinal, .. } = &mut st.reg;
        expect_alias(terminals.get_mut(&id).unwrap(), activity_ordinal, "", "done", Evidence::Fresh(""));
    }
    drop(core);
    std::fs::remove_dir_all(root).unwrap();
}
