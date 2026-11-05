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
        let (mut t, mut counter) = (terminal(), 0);
        for word in &words {
            report(&mut t, &mut counter, "", word);
        }
        let clock = &t.activity.as_ref().unwrap().records[""];
        assert_eq!(clock.completion_ordinal.is_some(), completes, "{words:?}");
        if completes {
            assert_eq!(clock.completion_ordinal, clock.state_ordinal);
        }
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
fn app_replacement_resets_completion_even_with_the_same_status_word() {
    let (mut t, mut counter) = (terminal(), 0);
    t.status = Some(crate::status::Status {
        state: "working".into(),
        app: "agent-a".into(),
        kind: None,
        progress: None,
        title: String::new(),
        msg: String::new(),
        source: None,
    });
    observe(&mut t, &mut counter, "host-a", "epoch-a", Evidence::Fresh(""));
    report(&mut t, &mut counter, "", "idle");
    // report() omitted app: unknown is not proof of a different app.
    assert!(t.activity.as_ref().unwrap().records[""].completion_ordinal.is_some());
    let scope = t.activity.as_ref().unwrap().scope;
    t.status.as_mut().unwrap().app = "agent-b".into();
    assert!(observe(&mut t, &mut counter, "host-a", "epoch-a", Evidence::Fresh("")));
    assert!(t.activity.as_ref().unwrap().scope > scope);
    assert!(t.activity.as_ref().unwrap().records[""].completion_ordinal.is_none());
    assert!(
        !serde_json::to_value(crate::terminals::Public(t)).unwrap().as_object().unwrap().contains_key("activity_guard")
    );
}

#[test]
fn checked_exhaustion_drops_activity_instead_of_wrapping() {
    let (mut t, mut counter) = (terminal(), u64::MAX);
    report(&mut t, &mut counter, "", "working");
    assert!(t.activity.is_none());
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
    }
    drop(core);
    std::fs::remove_dir_all(root).unwrap();
}
