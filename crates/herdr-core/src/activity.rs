//! Origin record chronology, independent of wall time, event seq, turns and parent news.
//!
//! First-completion table (per record, within a scope):
//! - initial/unknown -> idle/done: readiness, no completion;
//! - startup blocked -> idle/done: readiness, no completion;
//! - freshly observed working -> idle/done: completion;
//! - working -> blocked -> idle/done: completion (the work guard survives blocked);
//! - working -> error/unknown -> idle/done: no completion (wrong predecessor);
//! - after acquiring idle/done, blocked -> idle/done: completion;
//! - repeated word or metadata-only report: no new ordinal/completion;
//! - clear, subtree removal/eviction, withdrawal, exit/loss or owner replacement:
//!   clear the marker, never complete work. A clear starts acquisition again.
//!
//! The private acquisition/work guard and observed owner persist. Replay may use
//! that guard but cannot manufacture a working observation. Old registries get
//! unknown prehistory, not chronology reconstructed from their current words.

use crate::{
    status::Status,
    terminals::{Registry, Terminal},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Activity {
    pub core_id: String,
    /// A fresh process epoch, including after restore. Compare only within an origin/epoch.
    pub epoch: String,
    pub scope: u64,
    pub revision: u64,
    /// Root is "" (retained even when null); other keys match Terminal.records.
    pub records: BTreeMap<String, Clock>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Clock {
    pub state_ordinal: Option<u64>,
    pub completion_ordinal: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guards {
    records: BTreeMap<String, Guard>,
    owner: Option<Owner>,
    owner_observed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Guard {
    word: Option<String>,
    app: String,
    acquiring: bool,
    work: bool,
}

impl Default for Guard {
    fn default() -> Self {
        Self { word: None, app: String::new(), acquiring: true, work: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Owner {
    pid: i32,
    started: Option<u64>,
    agent: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum Evidence<'a> {
    Fresh(&'a str),
    Replay,
}

fn statuses(t: &Terminal) -> BTreeMap<String, Option<&Status>> {
    std::iter::once((String::new(), t.status.as_ref()))
        .chain(t.records.iter().map(|(id, s)| (id.clone(), Some(s))))
        .collect()
}

fn baselines(t: &Terminal) -> BTreeMap<String, Guard> {
    statuses(t)
        .into_iter()
        .map(|(id, s)| {
            (
                id,
                Guard {
                    word: s.map(|s| s.state.clone()),
                    app: s.map(|s| s.app.clone()).unwrap_or_default(),
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn prepare(t: &mut Terminal, core: &str, epoch: &str, restored: bool) {
    if t.activity.is_none() {
        t.activity = Some(Activity {
            core_id: core.into(),
            epoch: epoch.into(),
            scope: 1,
            revision: 0,
            records: statuses(t).into_keys().map(|id| (id, Clock::default())).collect(),
        });
        t.activity_guard.records = if restored { baselines(t) } else { BTreeMap::new() };
    }
    let activity = t.activity.as_mut().unwrap();
    activity.core_id = core.into();
    activity.epoch = epoch.into();
    activity.records.entry(String::new()).or_default();
    // A registry with clocks but no guards is unknown prehistory too.
    if restored && t.activity_guard.records.is_empty() {
        t.activity_guard.records = baselines(t);
    }
}

pub(crate) fn initialize(t: &mut Terminal, core: &str, epoch: &str) {
    prepare(t, core, epoch, false);
}

pub(crate) fn restore(reg: &mut Registry, core: &str, epoch: &str) {
    for t in reg.terminals.values_mut() {
        prepare(t, core, epoch, true);
        let a = t.activity.as_ref().unwrap();
        reg.activity_ordinal = reg.activity_ordinal.max(a.revision);
        for c in a.records.values() {
            reg.activity_ordinal =
                reg.activity_ordinal.max(c.state_ordinal.unwrap_or(0)).max(c.completion_ordinal.unwrap_or(0));
        }
    }
}

fn next(counter: &mut u64) -> Option<u64> {
    *counter = counter.checked_add(1)?;
    Some(*counter)
}

/// Reset current markers without treating unchanged saved words as fresh work.
pub(crate) fn reset(t: &mut Terminal, counter: &mut u64, core: &str, epoch: &str) -> bool {
    prepare(t, core, epoch, true);
    let Some(revision) = next(counter) else { return t.activity.take().is_some() };
    let Some(scope) = t.activity.as_ref().unwrap().scope.checked_add(1) else { return t.activity.take().is_some() };
    let records = baselines(t);
    let a = t.activity.as_mut().unwrap();
    a.scope = scope;
    a.revision = revision;
    a.records = records.keys().map(|id| (id.clone(), Clock::default())).collect();
    t.activity_guard.records = records;
    true
}

pub(crate) fn observe(t: &mut Terminal, counter: &mut u64, core: &str, epoch: &str, evidence: Evidence<'_>) -> bool {
    if *counter == u64::MAX {
        return t.activity.take().is_some();
    }
    prepare(t, core, epoch, false);
    let before = t.activity.clone();
    let replacements: Vec<String> = statuses(t)
        .into_iter()
        .filter_map(|(id, s)| {
            let s = s?;
            let old = t.activity_guard.records.get(&id)?;
            (!old.app.is_empty() && !s.app.is_empty() && old.app != s.app).then_some(id)
        })
        .collect();
    if !replacements.is_empty() {
        reset(t, counter, core, epoch);
        if t.activity.is_none() {
            return true;
        }
        for id in replacements {
            t.activity_guard.records.get_mut(&id).unwrap().word = None;
        }
    }
    // Own words only: mutation below must not borrow the terminal's statuses.
    let mut words: BTreeMap<String, (Option<String>, String)> = statuses(t)
        .into_iter()
        .map(|(id, s)| (id, (s.map(|s| s.state.clone()), s.map(|s| s.app.clone()).unwrap_or_default())))
        .collect();
    let ids: BTreeSet<String> = words.keys().chain(t.activity.as_ref().unwrap().records.keys()).cloned().collect();
    for id in ids {
        let Some((word, app)) = words.remove(&id) else {
            let Some(revision) = next(counter) else {
                t.activity = None;
                return true;
            };
            let a = t.activity.as_mut().unwrap();
            a.records.remove(&id);
            a.revision = revision;
            t.activity_guard.records.remove(&id);
            continue;
        };
        let guard = t.activity_guard.records.entry(id.clone()).or_default();
        if !app.is_empty() {
            guard.app = app;
        }
        if word.as_deref() == Some("working") && matches!(evidence,Evidence::Fresh(fresh) if fresh==id) {
            guard.work = true;
        }
        if guard.word == word {
            continue;
        }
        let Some(ordinal) = next(counter) else {
            t.activity = None;
            return true;
        };
        let endpoint = matches!(word.as_deref(), Some("idle" | "done"));
        let completed = endpoint
            && matches!(guard.word.as_deref(), Some("working" | "blocked"))
            && (!guard.acquiring || guard.work);
        let a = t.activity.as_mut().unwrap();
        a.revision = ordinal;
        a.records.insert(id, Clock { state_ordinal: Some(ordinal), completion_ordinal: completed.then_some(ordinal) });
        guard.word = word;
        if endpoint {
            guard.acquiring = false;
        }
        if guard.word.is_none() {
            *guard = Guard::default();
        }
    }
    t.activity != before
}

impl crate::runtime::Core {
    pub(crate) fn update_activity(&self, st: &mut crate::runtime::State, id: u32, evidence: Evidence<'_>) -> bool {
        let Registry { terminals, activity_ordinal, .. } = &mut st.reg;
        let t = terminals.get_mut(&id).expect("activity terminal");
        let guard = t.activity_guard.clone();
        let changed = observe(t, activity_ordinal, &self.info.id, &self.activity_epoch, evidence);
        if changed || t.activity_guard != guard {
            self.save_soon();
        }
        changed
    }

    pub(crate) fn publish_activity(&self, st: &mut crate::runtime::State, id: u32) {
        let activity = st.reg.terminals[&id].activity.clone();
        self.emit(st, "activity", id, serde_json::json!({"activity":activity}));
    }

    pub(crate) fn reset_activity(&self, st: &mut crate::runtime::State, id: u32) -> bool {
        let Registry { terminals, activity_ordinal, .. } = &mut st.reg;
        let t = terminals.get_mut(&id).expect("activity terminal");
        let changed = reset(t, activity_ordinal, &self.info.id, &self.activity_epoch);
        t.activity_guard.owner = None;
        t.activity_guard.owner_observed = false;
        if changed {
            self.save_soon();
        }
        changed
    }

    pub(crate) fn activity_foreground(&self, st: &mut crate::runtime::State, id: u32) {
        let Registry { terminals, activity_ordinal, .. } = &mut st.reg;
        let t = terminals.get_mut(&id).expect("foreground terminal");
        let guard = t.activity_guard.clone();
        let changed = foreground(t, activity_ordinal, &self.info.id, &self.activity_epoch);
        if changed || guard != t.activity_guard {
            self.save_soon();
        }
        if changed {
            self.publish_activity(st, id);
        }
    }
}

/// Confirmed OS observations only; failed/unknown reads never reach here.
pub(crate) fn foreground(t: &mut Terminal, counter: &mut u64, core: &str, epoch: &str) -> bool {
    prepare(t, core, epoch, true);
    let mut now =
        t.foreground.as_ref().map(|f| Owner { pid: f.pid, started: f.started, agent: f.agent.map(String::from) });
    let guards = &mut t.activity_guard;
    if !guards.owner_observed {
        if now.is_some() {
            guards.owner_observed = true;
            guards.owner = now;
        }
        return false;
    }
    if let (Some(old), Some(now)) = (&mut guards.owner, &mut now)
        && old.pid == now.pid
    {
        if old.started.is_none() {
            old.started = now.started;
        }
        if now.started.is_none() {
            now.started = old.started;
        }
        if old.agent.is_none() {
            old.agent = now.agent.clone();
        }
    }
    if guards.owner == now {
        return false;
    }
    guards.owner = now;
    reset(t, counter, core, epoch)
}

#[cfg(test)]
mod tests;
