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
use std::sync::Arc;
use tokio::time::{Duration, Instant};

/// Per-terminal worker state, kept in `State::screen_status`. The entry
/// lives as long as the terminal: `State::remove` is the only delete, so
/// `generation` never resets while a terminal exists.
#[derive(Default)]
pub struct ScreenWatch {
    /// The `output_seq` whose snapshot was last evaluated to a verdict.
    last_seq: Option<u64>,
    /// When the current run of idle evidence began; `settle` runs from here.
    /// Cleared by any other verdict, by sync output, or by invalidation.
    idle_since: Option<Instant>,
    /// The invalidation generation. Part of the commit token, so a snapshot
    /// taken before a bump dies at commit even when its other fields happen
    /// to match again — a resize there and back, a foreground A→B→A.
    generation: u64,
    /// A root status restored from disk stands unconfirmed until a verdict
    /// from a post-adoption snapshot re-proves it; past this it is
    /// withdrawn. Not touched by `invalidate`: the deadline never extends,
    /// whatever the evidence does in between.
    pub(crate) unconfirmed_until: Option<Instant>,
}

impl ScreenWatch {
    /// The screen can no longer back cached or in-flight verdicts: forget
    /// pending work and move the generation so they die at commit.
    pub fn invalidate(&mut self) {
        self.last_seq = None;
        self.idle_since = None;
        self.generation = self.generation.wrapping_add(1);
    }
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
    /// The terminal's invalidation generation at snapshot: any later bump —
    /// redraws aside, resize, stitch, foreground or agent change, a native
    /// report, program end — means the screen the rules saw is gone.
    generation: u64,
}

/// One terminal's owned snapshot, taken under one lock and evaluated off it.
pub(crate) struct Snap {
    id: u32,
    token: Token,
    manifest: Arc<Manifest>,
    rows: Vec<String>,
    title: String,
}

/// The worker's tick.
const CADENCE: Duration = Duration::from_millis(500);
/// How long idle evidence must hold before it publishes; working and blocked
/// publish at once.
const SETTLE: Duration = Duration::from_millis(1500);

/// Terminals snapshotted per pass; the rest wait a tick.
const MAX_PER_PASS: usize = 32;

/// A restored inferred root gets this long to be re-proven by a fresh
/// verdict after adoption, then it is withdrawn: well past one pass plus
/// the idle settle, whatever evidence appears in between.
pub(crate) const RECONFIRM: Duration = Duration::from_secs(30);

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
            // Nothing can ever be re-proven here: any root the registry
            // restored is withdrawn now, then the worker just stops.
            let mut st = self.lock();
            let restored: Vec<u32> = st
                .screen_status
                .iter()
                .filter(|(_, w)| w.unconfirmed_until.is_some())
                .map(|(id, _)| *id)
                .collect();
            for id in restored {
                self.inferred_status(&mut st, id, None);
            }
            return;
        }
        loop {
            tokio::time::sleep(CADENCE).await;
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
    /// acquisition, rotating on `State::screen_cursor` so terminals past
    /// `MAX_PER_PASS` are reached within ⌈terminals/cap⌉ passes — including
    /// their due idle confirms, which step performs under the same lock.
    /// `self.lock()` like every other path: it recovers a poisoned mutex.
    pub(crate) fn screen_status_collect(&self, catalog: &Catalog) -> Vec<Snap> {
        let mut out = Vec::new();
        let mut scanned = 0usize;
        while out.len() < MAX_PER_PASS {
            let mut st = self.lock();
            if st.screen_frozen || scanned >= st.reg.terminals.len() {
                break;
            }
            let cursor = st.screen_cursor;
            let id = st
                .reg
                .terminals
                .range((std::ops::Bound::Excluded(cursor), std::ops::Bound::Unbounded))
                .next()
                .or_else(|| st.reg.terminals.range(..=cursor).next())
                .map(|(id, _)| *id);
            let Some(id) = id else { break };
            st.screen_cursor = id;
            scanned += 1;
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
        // A restored root past its re-proof deadline is withdrawn. Checked
        // before every gate below so it fires while stitching, under
        // synchronized output, and on terminals not snapshotted this pass.
        if st.screen_status.get(&id).and_then(|w| w.unconfirmed_until).is_some_and(|at| at <= Instant::now()) {
            self.inferred_status(st, id, None);
            return None;
        }
        if !live || st.link(id).is_none() || agent.is_none() {
            // Gone, holderless, or no supported agent in front: whatever the
            // screen said about the old program is withdrawn — never "done".
            self.inferred_status(st, id, None);
            if let Some(w) = st.screen_status.get_mut(&id) {
                w.invalidate();
            }
            return None;
        }
        if reported {
            // Native authority owns the root now; `report_status` already
            // invalidated on the report, and inference stays off for the
            // program's lifetime.
            return None;
        }
        if st.stitching.contains(&id) {
            // A read is scrolling the app; completion invalidates the cache.
            return None;
        }
        let agent = agent.expect("checked above");
        let manifest = match catalog.get(agent) {
            Some(manifest) => manifest,
            None => {
                // This catalog has no rules for the agent in front: nothing
                // here can re-prove a restored root, so it is withdrawn now
                // rather than at its deadline.
                if st.screen_status.get(&id).is_some_and(|w| w.unconfirmed_until.is_some()) {
                    self.inferred_status(st, id, None);
                }
                return None;
            }
        };
        let (sync, size) = st.screens.get(&id).map(|s| (s.dec_mode(2026), s.size()))?;
        if sync {
            // Synchronized output: mid-batch rows are not evidence.
            if let Some(w) = st.screen_status.get_mut(&id) {
                w.idle_since = None;
            }
            return None;
        }
        let seq = st.output_seq.get(&id).copied().unwrap_or(0);
        if st.screen_status.get(&id).is_some_and(|w| w.last_seq == Some(seq)) {
            // Nothing new on screen: idle evidence past its settle deadline
            // confirms without another extraction.
            let due = {
                let watch = st.screen_status.entry(id).or_default();
                let due = watch.idle_since.is_some_and(|since| since.elapsed() >= SETTLE);
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
        let generation = st.screen_status.get(&id).map_or(0, |w| w.generation);
        let screen = st.screens.get(&id).expect("checked above");
        Some(Snap {
            id,
            token: Token { program, foreground, seq, size, manifest: Arc::as_ptr(&manifest) as usize, generation },
            manifest,
            rows: screen.screen_rows(),
            title: screen.title(),
        })
    }

    /// One verdict's commit, under its own lock: rejected when the token no
    /// longer holds (redraw, resize, agent change, a native report racing
    /// us) or the terminal went ineligible. Applies at most one change.
    pub(crate) fn screen_status_commit(&self, catalog: &Catalog, snap: Snap, detection: Detection) {
        let mut st = self.lock();
        let Some(t) = st.reg.terminals.get(&snap.id) else { return };
        let agent = t.foreground.as_ref().and_then(|f| f.agent);
        let now = Token {
            program: t.pid.map(|pid| (pid, t.program_started_ms)),
            foreground: t.foreground.as_ref().map(|f| (f.pid, f.agent)),
            seq: st.output_seq.get(&snap.id).copied().unwrap_or(0),
            size: st.screens.get(&snap.id).map_or((0, 0), |s| s.size()),
            manifest: agent.and_then(|a| catalog.get(a)).map_or(0, |m| Arc::as_ptr(&m) as usize),
            generation: st.screen_status.get(&snap.id).map_or(0, |w| w.generation),
        };
        let eligible = t.is_live()
            && !t.reported
            && !st.screen_frozen
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
                    // Already idle: nothing new — except a restored idle root,
                    // which still owes the settle before it is re-proven.
                    if said_idle && watch.unconfirmed_until.is_none() {
                        watch.idle_since = None;
                        None
                    } else {
                        let since = *watch.idle_since.get_or_insert(Instant::now());
                        if since.elapsed() >= SETTLE {
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
        st.invalidate_screen(id);
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
