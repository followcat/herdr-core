//! Cached publication. All filesystem and Git work stays off Core.state and the foreground watcher.

use super::{
    AheadBehind, Checkout, Discovery, FactState, Repository, RepositoryFacts, discover,
    probe::{Facts, Probe},
};
use crate::runtime::Core;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod worker;
use worker::Pool;

#[cfg(test)]
mod tests;

const POSITIVE: Duration = Duration::from_secs(5);
const NEGATIVE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Discover(String),
    Probe(String),
}

enum Job {
    Discover { cwd: String, generation: u64 },
    Probe { key: String, generation: u64, context: Box<(Repository, Option<Probe>)> },
}

enum Completed {
    Discover { cwd: String, generation: u64, result: io::Result<Discovery> },
    Probe { key: String, generation: u64, probe: Probe, result: io::Result<Facts> },
}

struct Registration {
    cwd: String,
    foreground: Option<crate::foreground::Key>,
    dirty: bool,
}

struct Directory {
    generation: u64,
    result: Option<Discovery>,
    failed: bool,
    dirty: bool,
    started: Option<Instant>,
    checked: Option<Instant>,
    used: Instant,
}

struct Cached {
    generation: u64,
    repo: Repository,
    probe: Option<Probe>,
    facts: Option<Facts>,
    failed: bool,
    started: Option<Instant>,
    checked: Option<Instant>,
    used: Instant,
}

#[derive(Default)]
pub(crate) struct Scheduler {
    registrations: BTreeMap<u32, Registration>,
    directories: HashMap<String, Directory>,
    checkouts: HashMap<String, Cached>,
    queue: VecDeque<Key>,
    queued: HashSet<Key>,
    flight: HashSet<Key>,
    cursor: u32,
    generation: u64,
    next_start: Option<Instant>,
}

impl Scheduler {
    pub fn remove(&mut self, id: u32) {
        self.registrations.remove(&id);
    }

    pub fn invalidate(&mut self, id: u32) {
        if let Some(reg) = self.registrations.get_mut(&id) {
            reg.dirty = true;
        }
    }

    fn generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn enqueue(&mut self, key: Key) {
        if self.queue.len() < 128 && !self.queued.contains(&key) && !self.flight.contains(&key) {
            self.queued.insert(key.clone());
            self.queue.push_back(key);
        }
    }

    fn observe(
        &mut self,
        id: u32,
        cwd: &str,
        foreground: Option<crate::foreground::Key>,
        old: Option<&RepositoryFacts>,
        core: &str,
        now: Instant,
    ) -> RepositoryFacts {
        let changed =
            self.registrations.get(&id).is_some_and(|r| r.cwd != cwd || r.foreground != foreground || r.dirty);
        self.registrations.insert(id, Registration { cwd: cwd.into(), foreground, dirty: false });
        let generation = self.generation();
        let directory = self.directories.entry(cwd.into()).or_insert_with(|| Directory {
            generation,
            result: None,
            failed: false,
            dirty: true,
            started: None,
            checked: None,
            used: now,
        });
        directory.used = now;
        if changed {
            directory.generation = generation;
            directory.dirty = true;
        }
        let interval =
            if directory.result.as_ref().is_some_and(|d| d.repository.is_none()) { NEGATIVE } else { POSITIVE };
        if directory.dirty || elapsed(directory.started, now) >= interval {
            self.enqueue(Key::Discover(cwd.into()));
        }
        let mut projected =
            old.filter(|f| f.cwd == cwd).cloned().unwrap_or_else(|| RepositoryFacts::pending(core, cwd));
        projected.core_id = core.into();
        let had_observation = matches!(projected.state, FactState::Ready | FactState::Stale);
        let directory = &self.directories[cwd];
        if let Some(discovery) = &directory.result {
            projected.auto_label = Some(discovery.auto_label.clone());
            if let Some(repo) = &discovery.repository {
                let key = repo.checkout_key.clone();
                let same_context = projected
                    .checkout
                    .as_ref()
                    .is_some_and(|c| c.checkout_key == repo.checkout_key && c.repository_key == repo.repository_key);
                if !same_context {
                    projected.branch = None;
                    projected.ahead_behind = None;
                }
                projected.checkout = Some(Checkout::from(repo));
                if let Some(cached) = self.checkouts.get_mut(&key) {
                    cached.used = now;
                    projected.state = if cached.facts.is_none() {
                        if had_observation && same_context {
                            FactState::Stale
                        } else if cached.failed || directory.failed {
                            FactState::Unavailable
                        } else {
                            FactState::Pending
                        }
                    } else if cached.failed
                        || directory.failed
                        || elapsed(cached.checked, now) > Duration::from_secs(15)
                        || elapsed(directory.checked, now) > Duration::from_secs(15)
                    {
                        FactState::Stale
                    } else {
                        FactState::Ready
                    };
                    if let Some(facts) = &cached.facts {
                        projected.branch = facts.branch.clone();
                        projected.ahead_behind =
                            facts.ahead_behind.map(|(ahead, behind)| AheadBehind { ahead, behind });
                    }
                    if elapsed(cached.started, now) >= POSITIVE {
                        self.enqueue(Key::Probe(key));
                    }
                } else {
                    projected.state =
                        if had_observation && same_context { FactState::Stale } else { FactState::Pending };
                }
            } else {
                projected.checkout = None;
                projected.branch = None;
                projected.ahead_behind = None;
                projected.state = if directory.failed || elapsed(directory.checked, now) > Duration::from_secs(45) {
                    FactState::Stale
                } else {
                    FactState::Ready
                };
            }
        } else if directory.failed {
            projected.state = if projected.auto_label.is_some() { FactState::Stale } else { FactState::Unavailable };
        }
        self.evict();
        projected
    }

    fn complete(&mut self, completed: Completed, now: Instant) {
        match completed {
            Completed::Discover { cwd, generation, result } => {
                self.flight.remove(&Key::Discover(cwd.clone()));
                let Some(entry) = self.directories.get_mut(&cwd).filter(|e| e.generation == generation) else { return };
                match result {
                    Ok(discovery) => {
                        entry.failed = false;
                        entry.checked = Some(now);
                        let repo = discovery.repository.clone();
                        entry.result = Some(discovery);
                        if let Some(repo) = repo {
                            let key = repo.checkout_key.clone();
                            let generation = self.generation();
                            let cached = self.checkouts.entry(key.clone()).or_insert_with(|| Cached {
                                generation,
                                repo: repo.clone(),
                                probe: None,
                                facts: None,
                                failed: false,
                                started: None,
                                checked: None,
                                used: now,
                            });
                            if cached.repo != repo {
                                cached.generation = generation;
                                cached.repo = repo;
                                cached.probe = None;
                                cached.facts = None;
                                cached.started = None;
                                cached.checked = None;
                            }
                            if elapsed(cached.started, now) >= POSITIVE {
                                self.enqueue(Key::Probe(key));
                            }
                        }
                    }
                    Err(_) => entry.failed = true,
                }
            }
            Completed::Probe { key, generation, probe, result } => {
                self.flight.remove(&Key::Probe(key.clone()));
                let Some(cached) = self.checkouts.get_mut(&key).filter(|e| e.generation == generation) else { return };
                cached.probe = Some(probe);
                match result {
                    Ok(facts) => {
                        cached.facts = Some(facts);
                        cached.checked = Some(now);
                        cached.failed = false;
                    }
                    Err(_) => cached.failed = true,
                }
            }
        }
        self.evict();
    }

    fn next(&mut self, now: Instant) -> Option<Job> {
        if self.flight.len() >= 2 || self.next_start.is_some_and(|at| now < at) {
            return None;
        }
        for _ in 0..self.queue.len() {
            let key = self.queue.pop_front()?;
            self.queued.remove(&key);
            let job = match &key {
                Key::Discover(cwd) => {
                    let Some(entry) = self.directories.get_mut(cwd) else { continue };
                    if elapsed(entry.started, now) < Duration::from_secs(1) {
                        self.enqueue(key);
                        continue;
                    }
                    entry.started = Some(now);
                    entry.dirty = false;
                    Job::Discover { cwd: cwd.clone(), generation: entry.generation }
                }
                Key::Probe(name) => {
                    let Some(entry) = self.checkouts.get_mut(name) else { continue };
                    if elapsed(entry.started, now) < Duration::from_secs(1) {
                        self.enqueue(key);
                        continue;
                    }
                    entry.started = Some(now);
                    Job::Probe {
                        key: name.clone(),
                        generation: entry.generation,
                        context: Box::new((entry.repo.clone(), entry.probe.clone())),
                    }
                }
            };
            self.flight.insert(key);
            self.next_start = Some(now + Duration::from_millis(500));
            return Some(job);
        }
        None
    }

    fn evict(&mut self) {
        while self.directories.len() > 1024 {
            let key = self
                .directories
                .iter()
                .filter(|(k, _)| {
                    let key = Key::Discover((*k).clone());
                    !self.queued.contains(&key) && !self.flight.contains(&key)
                })
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.directories.remove(&key);
        }
        while self.checkouts.len() > 256 {
            let key = self
                .checkouts
                .iter()
                .filter(|(k, _)| {
                    let key = Key::Probe((*k).clone());
                    !self.queued.contains(&key) && !self.flight.contains(&key)
                })
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.checkouts.remove(&key);
        }
    }
}

fn elapsed(at: Option<Instant>, now: Instant) -> Duration {
    at.map_or(Duration::MAX, |at| now.saturating_duration_since(at))
}

fn perform(job: Job, home: &std::path::Path, xdg: Option<&std::path::Path>) -> Completed {
    match job {
        Job::Discover { cwd, generation } => {
            let result = discover(std::path::Path::new(&cwd), Some(home));
            Completed::Discover { cwd, generation, result }
        }
        Job::Probe { key, generation, context } => {
            let (repo, probe) = *context;
            let mut probe = probe.unwrap_or_else(|| Probe::new(home.into(), xdg.map(PathBuf::from)));
            let result = probe.refresh(&repo, Instant::now());
            Completed::Probe { key, generation, probe, result }
        }
    }
}

impl Core {
    pub async fn watch_repositories(self: Arc<Self>) {
        let home = crate::platform::user_home();
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let pool = match Pool::new(move |job| perform(job, &home, xdg.as_deref())) {
            Ok(pool) => pool,
            Err(e) => {
                eprintln!("repository workers unavailable: {e}");
                return;
            }
        };
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tick.tick().await;
            let now = Instant::now();
            let completed: Vec<_> = pool.results.try_iter().take(2).collect();
            let mut st = self.lock();
            if st.stopping {
                break;
            }
            for result in completed {
                st.repositories.complete(result, now);
            }
            let cursor = st.repositories.cursor;
            let ids: Vec<_> = st
                .reg
                .terminals
                .range((std::ops::Bound::Excluded(cursor), std::ops::Bound::Unbounded))
                .chain(st.reg.terminals.range(..=cursor))
                .take(64)
                .map(|(&id, _)| id)
                .collect();
            for id in ids {
                st.repositories.cursor = id;
                let terminal = &st.reg.terminals[&id];
                if !terminal.is_live() || terminal.pid.is_none() {
                    st.repositories.registrations.remove(&id);
                    continue;
                }
                let cwd = terminal.foreground.as_ref().map_or(&terminal.cwd, |f| &f.cwd).clone();
                let foreground = terminal.foreground.as_ref().map(crate::foreground::Foreground::key);
                let old = terminal.repository.clone();
                let facts = st.repositories.observe(id, &cwd, foreground, old.as_ref(), &self.info.id, now);
                if old.as_ref() != Some(&facts) {
                    st.reg.terminals.get_mut(&id).unwrap().repository = Some(facts.clone());
                    self.save_soon();
                    self.emit(&mut st, "repository", id, serde_json::json!({"repository":facts}));
                }
            }
            let job = st.repositories.next(now);
            drop(st);
            if let Some(job) = job
                && let Err(job) = pool.submit(job)
            {
                // All worker slots are accounted for; a disconnected pool must not spin.
                drop(job);
                break;
            }
        }
    }
}
