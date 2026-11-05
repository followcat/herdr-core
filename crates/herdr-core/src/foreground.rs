//! What runs in a terminal right now: its foreground process and that
//! process's folder. Raw facts, read about twice a second; clients name and
//! draw programs themselves.

use crate::runtime::Core;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Foreground {
    /// The foreground process group's leader.
    pub pid: i32,
    /// Its name as the OS has it (`node`, `vim`, `sh`).
    pub name: String,
    pub argv: Vec<String>,
    /// Its working directory now (resolved by the OS).
    pub cwd: String,
    /// Linux process start ticks: an internal incarnation stamp, never wire data.
    #[serde(skip)]
    #[schemars(skip)]
    pub started: Option<u64>,
    /// The agent identified when this foreground was observed (fork): `about`
    /// runs `identify` — and its filesystem reads — off the runtime lock, so
    /// code under `Core.state` only ever reads this stored field. Skipped on
    /// the wire: a runtime fact, recomputed on each observation.
    #[serde(skip)]
    #[schemars(skip)]
    pub agent: Option<&'static str>,
}

pub(crate) type Key = (i32, Option<u64>, Option<&'static str>);

impl Foreground {
    pub(crate) fn key(&self) -> Key {
        (self.pid, self.started, self.agent)
    }
}

/// Readable process facts, for callers that do not need the error distinction.
/// The watcher uses the platform result directly: errors cannot erase facts.
pub fn about(pid: i32) -> Option<Foreground> {
    crate::platform::observe_foreground(pid).ok().flatten()
}

impl Core {
    /// Keeps every live terminal's `foreground` current, with an event on each change.
    pub async fn watch_foreground(self: Arc<Self>) {
        let ms = std::env::var("HC_FOREGROUND_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
        loop {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let live: Vec<u32> = self.lock().reg.terminals.values().filter(|t| t.is_live()).map(|t| t.id()).collect();
            for id in live {
                let generation = self.lock().screen_status.get(&id).map_or(0, |w| w.generation);
                let observation = match self.foreground(id).await {
                    Some(pgid) => {
                        let result = crate::platform::observe_foreground(pgid);
                        #[cfg(test)]
                        if result.is_err() {
                            self.observations.foreground_read_errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        result.ok()
                    }
                    None => None,
                };
                let Some(now) = observation else {
                    #[cfg(test)]
                    self.observations.foreground_unknown.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    continue;
                };
                let mut st = self.lock();
                // A response/OS read crossing detach or adoption cannot make
                // the next screen's saved foreground freshly observed.
                if st.screen_status.get(&id).map_or(0, |w| w.generation) != generation {
                    continue;
                }
                let Some(t) = st.reg.terminals.get_mut(&id).filter(|t| t.is_live()) else {
                    continue;
                };
                if let Some(f) = &now {
                    t.last_cwd = Some(f.cwd.clone());
                }
                let mut was = std::mem::replace(&mut t.foreground, now.clone());
                // Restore the skipped stamp without fabricating a new owner
                // or wire event. A known previous stamp is never overwritten.
                if let (Some(was), Some(now)) = (&mut was, &now)
                    && was.started.is_none()
                {
                    was.started = now.started;
                }
                if was != now {
                    st.repositories.invalidate(id);
                    self.save_soon();
                    self.emit(&mut st, "foreground", id, serde_json::to_value(&now).unwrap_or_default());
                }
                self.activity_foreground(&mut st, id);
                self.foreground_changed(&mut st, id, was.as_ref());
            }
        }
    }
}
