//! Screen-status inference for programs that never speak OSC 7501 (fork).
//!
//! One worker, one pass per tick. Each pass takes, per terminal and under
//! one short lock apiece, an owned screen snapshot plus a staleness token;
//! evaluation of the phase-1 manifests runs in a single awaited
//! `spawn_blocking`; commits re-lock per terminal and apply only verdicts
//! whose token still holds. `Core::output` only marks activity — nothing
//! here evaluates on the output path. Native reports keep full authority:
//! `reported` ends inference for the program's lifetime, and
//! `Core::report_status` invalidates pending work on any native report.

use crate::detect::{Catalog, Detection, Input, Manifest, State as Verdict, detect};
use crate::foreground::Foreground;
use crate::runtime::{Core, State};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::{Duration, Instant};

/// Per-terminal worker state, kept in `State::screen_status`.
#[derive(Default)]
pub struct ScreenWatch {
    /// The `output_seq` whose snapshot was last evaluated to a verdict.
    last_seq: Option<u64>,
    /// When the current run of idle evidence began; `settle` runs from here.
    /// Cleared by any other verdict, by sync output, or by invalidation.
    idle_since: Option<Instant>,
}

/// What must still be true at commit for a verdict to apply: the same
/// program, the same foreground agent, the same screen contents and size,
/// and the same manifest the rules were read from. Any drift means the
/// screen the rules saw is gone.
#[derive(PartialEq)]
struct Token {
    /// The hc-owned program: pid plus its start stamp (a restart is a new program).
    program: Option<(u32, Option<u64>)>,
    /// Foreground identity: the leader's pid and its identified agent. A cwd
    /// move alone does not change it.
    foreground: Option<(i32, Option<&'static str>)>,
    seq: u64,
    size: (u16, u16),
    /// The manifest generation: its `Arc` address while the catalog lives.
    manifest: usize,
}

/// One terminal's owned snapshot, taken under one lock and evaluated off it.
pub(crate) struct Snap {
    id: u32,
    token: Token,
    manifest: Arc<Manifest>,
    rows: Vec<String>,
    title: String,
}

fn env_ms(var: &str, default: u64) -> Duration {
    Duration::from_millis(std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default))
}

/// The worker's tick; `HC_DETECT_MS` overrides.
fn cadence() -> Duration {
    env_ms("HC_DETECT_MS", 500)
}

/// How long idle evidence must hold before it publishes; working and blocked
/// publish at once. `HC_DETECT_SETTLE_MS` overrides.
fn settle() -> Duration {
    env_ms("HC_DETECT_SETTLE_MS", 1500)
}

/// Terminals snapshotted per pass; the rest wait a tick.
const MAX_PER_PASS: usize = 32;

impl Core {
    /// The detection worker, spawned once beside `watch_foreground`: a pass
    /// every `cadence`, driven by `output_seq` changes plus settle deadlines.
    pub async fn watch_screen_status(self: Arc<Self>) {
        let settings = self.settings();
        let catalog = Catalog::load(Some(&self.paths.detect_dir()), settings.detect.as_ref());
        for warning in &catalog.warnings {
            eprintln!("core: detect: {warning}");
        }
        if catalog.agents().next().is_none() {
            return;
        }
        loop {
            tokio::time::sleep(cadence()).await;
            self.screen_status_pass(&catalog).await;
        }
    }

    /// One pass: collect snapshots under per-terminal locks, evaluate them
    /// all in a single awaited blocking task, then commit under per-terminal
    /// re-locks. Nothing overlaps passes — the caller is the only driver.
    pub(crate) async fn screen_status_pass(&self, catalog: &Catalog) {
        let snaps = self.screen_status_collect(catalog);
        if snaps.is_empty() {
            return;
        }
        let Ok(verdicts) = tokio::task::spawn_blocking(move || evaluate(snaps)).await else { return };
        for (snap, detection) in verdicts {
            self.screen_status_commit(catalog, snap, detection);
        }
    }

    /// Every terminal that needs work this pass, one terminal per lock
    /// acquisition. Side facts under the same lock: withdrawal on foreground
    /// loss, the unchanged-screen idle confirm, stale-cache cleanup.
    /// `try_lock` keeps the pass bounded — a busy core just waits a tick.
    pub(crate) fn screen_status_collect(&self, catalog: &Catalog) -> Vec<Snap> {
        let mut out = Vec::new();
        let mut processed = HashSet::new();
        while out.len() < MAX_PER_PASS {
            let Ok(mut st) = self.state.try_lock() else { break };
            let Some(id) = st.reg.terminals.keys().find(|id| !processed.contains(*id)).copied() else { break };
            processed.insert(id);
            if let Some(snap) = self.step(&mut st, id, catalog) {
                out.push(snap);
            }
        }
        out
    }

    /// One terminal's step, under the caller's lock: eligibility, the
    /// unchanged-screen confirm, or an owned snapshot for the off-lock pass.
    fn step(&self, st: &mut State, id: u32, catalog: &Catalog) -> Option<Snap> {
        // Facts first; the terminal borrow ends before anything mutates.
        let Some((live, reported, agent, program, foreground)) = st.reg.terminals.get(&id).map(|t| {
            (
                t.is_live(),
                t.reported,
                t.foreground.as_ref().and_then(|f| f.agent),
                t.pid.map(|pid| (pid, t.program_started_ms)),
                t.foreground.as_ref().map(|f| (f.pid, f.agent)),
            )
        }) else {
            st.screen_status.remove(&id);
            return None;
        };
        if !live || st.link(id).is_none() || agent.is_none() {
            // Gone, holderless, or no supported agent in front: whatever the
            // screen said about the old program is withdrawn — never "done".
            self.inferred_status(st, id, None);
            st.screen_status.remove(&id);
            return None;
        }
        if reported {
            // Native authority owns the root now; inference stays off for
            // this program's lifetime.
            st.screen_status.remove(&id);
            return None;
        }
        if st.stitching.contains(&id) {
            // A read is scrolling the app; completion invalidates the cache.
            return None;
        }
        let agent = agent.expect("checked above");
        let Some(manifest) = catalog.get(agent) else {
            st.screen_status.remove(&id);
            return None;
        };
        let Some((sync, size)) = st.screens.get(&id).map(|s| (s.dec_mode(2026), s.size())) else {
            st.screen_status.remove(&id);
            return None;
        };
        if sync {
            // Synchronized output: mid-batch rows are not evidence.
            st.screen_status.entry(id).or_default().idle_since = None;
            return None;
        }
        let seq = st.output_seq.get(&id).copied().unwrap_or(0);
        if st.screen_status.get(&id).is_some_and(|w| w.last_seq == Some(seq)) {
            // Nothing new on screen: idle evidence past its settle deadline
            // confirms without another extraction.
            let due = {
                let watch = st.screen_status.entry(id).or_default();
                let due = watch.idle_since.is_some_and(|since| since.elapsed() >= settle());
                if due {
                    watch.idle_since = None;
                }
                due
            };
            if due {
                self.inferred_status(st, id, Some(("idle", None)));
            }
            return None;
        }
        let screen = st.screens.get(&id).expect("checked above");
        Some(Snap {
            id,
            token: Token { program, foreground, seq, size, manifest: Arc::as_ptr(&manifest) as usize },
            manifest,
            rows: screen.screen_rows(),
            title: screen.title(),
        })
    }

    /// One verdict's commit, under its own lock: rejected when the token no
    /// longer holds (redraw, resize, agent change, a native report racing
    /// us) or the terminal went ineligible. Applies at most one change.
    pub(crate) fn screen_status_commit(&self, catalog: &Catalog, snap: Snap, detection: Detection) {
        let Ok(mut st) = self.state.try_lock() else { return };
        let Some(t) = st.reg.terminals.get(&snap.id) else { return };
        let agent = t.foreground.as_ref().and_then(|f| f.agent);
        let now = Token {
            program: t.pid.map(|pid| (pid, t.program_started_ms)),
            foreground: t.foreground.as_ref().map(|f| (f.pid, f.agent)),
            seq: st.output_seq.get(&snap.id).copied().unwrap_or(0),
            size: st.screens.get(&snap.id).map_or((0, 0), |s| s.size()),
            manifest: agent.and_then(|a| catalog.get(a)).map_or(0, |m| Arc::as_ptr(&m) as usize),
        };
        let eligible = t.is_live()
            && !t.reported
            && st.link(snap.id).is_some()
            && !st.stitching.contains(&snap.id)
            && agent.is_some()
            && st.screens.get(&snap.id).is_some_and(|s| !s.dec_mode(2026));
        if !eligible || now != snap.token {
            return;
        }
        let said_idle =
            st.reg.terminals[&snap.id].status.as_ref().is_some_and(|s| s.source.is_some() && s.state == "idle");
        let publish = {
            let watch = st.screen_status.entry(snap.id).or_default();
            watch.last_seq = Some(snap.token.seq);
            match detection.state {
                Verdict::Working => {
                    watch.idle_since = None;
                    Some(("working", None))
                }
                Verdict::Blocked => {
                    watch.idle_since = None;
                    Some(("blocked", detection.kind))
                }
                Verdict::Idle => {
                    if said_idle {
                        watch.idle_since = None;
                        None
                    } else {
                        let since = *watch.idle_since.get_or_insert(Instant::now());
                        if since.elapsed() >= settle() {
                            watch.idle_since = None;
                            Some(("idle", None))
                        } else {
                            None
                        }
                    }
                }
                // No match, or a skip rule (transcript viewers, menus):
                // pending idle dies; the published verdict stands. Unknown
                // never times out into idle — only evidence sets idle_since.
                Verdict::Unknown => {
                    watch.idle_since = None;
                    None
                }
            }
        };
        if let Some((word, kind)) = publish {
            self.inferred_status(&mut st, snap.id, Some((word, kind)));
        }
    }

    /// The foreground watcher saw a new foreground on `id`. An agent or
    /// leader-pid change drops cached work and withdraws the old owner's
    /// verdict — process loss never means "done". A cwd move alone keeps both.
    pub(crate) fn foreground_changed(&self, st: &mut State, id: u32, was: Option<&Foreground>) {
        let key = |f: Option<&Foreground>| f.map(|f| (f.pid, f.agent));
        if key(was) == key(st.reg.terminals.get(&id).and_then(|t| t.foreground.as_ref())) {
            return;
        }
        st.screen_status.remove(&id);
        self.inferred_status(st, id, None);
    }
}

/// The whole pass's evaluation, run inside the worker's one `spawn_blocking`:
/// normalize each owned snapshot — trailing padding trimmed, indentation and
/// row boundaries kept — and run its manifest. Pure; no locks, no runtime.
pub(crate) fn evaluate(snaps: Vec<Snap>) -> Vec<(Snap, Detection)> {
    snaps
        .into_iter()
        .map(|snap| {
            let mut rows: Vec<&str> = snap.rows.iter().map(|row| row.trim_end()).collect();
            while rows.last() == Some(&"") {
                rows.pop();
            }
            let screen = rows.join("\n");
            let detection = detect(&snap.manifest, &Input { screen: &screen, title: &snap.title, progress: "" });
            (snap, detection)
        })
        .collect()
}
