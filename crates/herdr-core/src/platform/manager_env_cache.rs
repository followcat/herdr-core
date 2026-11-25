//! Linux's short-lived cache for the user manager's environment.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Clients that poll or exec about once a second otherwise spawn one busctl
/// apiece. Two seconds collapses each burst while a desktop session change
/// still reaches new programs almost at once.
const CACHE_FOR: Duration = Duration::from_secs(2);
/// No manager, no busctl, and transient D-Bus errors are observations too,
/// but they must not stick for the whole window.
const RETRY_AFTER: Duration = Duration::from_millis(100);

type Environment = BTreeMap<String, String>;
type Fetcher = dyn Fn() -> Option<Environment> + Send + Sync;
type Clock = dyn Fn() -> Instant + Send + Sync;

/// The manager environment, shared for a short time by every program start.
pub(super) async fn manager_env() -> Option<Environment> {
    static CACHE: std::sync::OnceLock<ManagerEnvCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| ManagerEnvCache::new(super::systemd::manager_env, Instant::now)).get().await
}

struct ManagerEnvCache {
    fetcher: Arc<Fetcher>,
    clock: Arc<Clock>,
    state: Arc<Mutex<State>>,
}

enum State {
    Empty,
    Cached { value: Option<Environment>, at: Instant },
    Fetching(Arc<InFlight>),
}

impl ManagerEnvCache {
    fn new(
        fetcher: impl Fn() -> Option<Environment> + Send + Sync + 'static,
        clock: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> ManagerEnvCache {
        ManagerEnvCache {
            fetcher: Arc::new(fetcher),
            clock: Arc::new(clock),
            state: Arc::new(Mutex::new(State::Empty)),
        }
    }

    async fn get(&self) -> Option<Environment> {
        enum Lookup {
            Done(Option<Environment>),
            Wait(Arc<InFlight>),
            Start(Arc<InFlight>),
        }
        let lookup = {
            let now = (self.clock)();
            let mut state = lock(&self.state);
            match &*state {
                State::Cached { value, at } if usable(value, *at, now) => Lookup::Done(value.clone()),
                State::Fetching(inflight) => Lookup::Wait(Arc::clone(inflight)),
                _ => {
                    let inflight = Arc::new(InFlight::new());
                    *state = State::Fetching(Arc::clone(&inflight));
                    Lookup::Start(inflight)
                }
            }
        };
        match lookup {
            Lookup::Done(value) => value,
            Lookup::Wait(inflight) => inflight.wait().await,
            Lookup::Start(inflight) => {
                self.fetch(Arc::clone(&inflight));
                inflight.wait().await
            }
        }
    }

    /// The fetch belongs to the cache, not to any one caller: a caller that
    /// goes away cannot make the others start a second busctl.
    fn fetch(&self, inflight: Arc<InFlight>) {
        let fetcher = Arc::clone(&self.fetcher);
        let clock = Arc::clone(&self.clock);
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let value = tokio::task::spawn_blocking(move || fetcher()).await.ok().flatten();
            let at = (clock)();
            {
                let mut state = lock(&state);
                if matches!(&*state, State::Fetching(current) if Arc::ptr_eq(current, &inflight)) {
                    *state = State::Cached { value: value.clone(), at };
                }
            }
            inflight.complete(value);
        });
    }
}

fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn usable(value: &Option<Environment>, at: Instant, now: Instant) -> bool {
    let keep = if value.is_some() { CACHE_FOR } else { RETRY_AFTER };
    now.checked_duration_since(at).is_some_and(|age| age < keep)
}

/// The fetch's one result, shared with every caller waiting for it.
struct InFlight {
    outcome: tokio::sync::watch::Sender<Outcome>,
}

enum Outcome {
    Pending,
    Done(Option<Environment>),
}

impl InFlight {
    fn new() -> InFlight {
        let (outcome, _) = tokio::sync::watch::channel(Outcome::Pending);
        InFlight { outcome }
    }

    fn complete(&self, value: Option<Environment>) {
        self.outcome.send_replace(Outcome::Done(value));
    }

    async fn wait(&self) -> Option<Environment> {
        let mut outcome = self.outcome.subscribe();
        loop {
            if let Outcome::Done(value) = &*outcome.borrow() {
                return value.clone();
            }
            if outcome.changed().await.is_err() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    fn env(value: &str) -> BTreeMap<String, String> {
        [("VALUE".to_string(), value.to_string())].into_iter().collect()
    }

    struct TestClock {
        start: Instant,
        elapsed: Arc<Mutex<Duration>>,
    }

    impl TestClock {
        fn new() -> TestClock {
            TestClock { start: Instant::now(), elapsed: Arc::new(Mutex::new(Duration::ZERO)) }
        }

        fn clock(&self) -> impl Fn() -> Instant + Send + Sync + 'static {
            let start = self.start;
            let elapsed = Arc::clone(&self.elapsed);
            move || start + *elapsed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        fn advance(&self, by: Duration) {
            *self.elapsed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) += by;
        }
    }

    fn cache_with(
        fetcher: impl Fn() -> Option<BTreeMap<String, String>> + Send + Sync + 'static,
    ) -> (ManagerEnvCache, TestClock) {
        let clock = TestClock::new();
        let cache = ManagerEnvCache::new(fetcher, clock.clock());
        (cache, clock)
    }

    #[tokio::test]
    async fn calls_within_the_window_use_one_fetch() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&fetches);
        let (cache, clock) = cache_with(move || {
            counting.fetch_add(1, Ordering::SeqCst);
            Some(env("one"))
        });
        for _ in 0..50 {
            assert_eq!(cache.get().await, Some(env("one")));
        }
        clock.advance(CACHE_FOR - Duration::from_millis(1));
        assert_eq!(cache.get().await, Some(env("one")));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_call_after_the_window_fetches_again() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&fetches);
        let (cache, clock) = cache_with(move || {
            counting.fetch_add(1, Ordering::SeqCst);
            Some(env("fresh"))
        });
        assert_eq!(cache.get().await, Some(env("fresh")));
        clock.advance(CACHE_FOR);
        assert_eq!(cache.get().await, Some(env("fresh")));
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_first_calls_share_one_fetch() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&fetches);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let fetcher_gate = Arc::clone(&gate);
        let (cache, _clock) = cache_with(move || {
            counting.fetch_add(1, Ordering::SeqCst);
            let (lock, wake) = &*fetcher_gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = wake.wait(open).unwrap();
            }
            Some(env("shared"))
        });
        let release_gate = Arc::clone(&gate);
        let release = tokio::spawn(async move {
            let (lock, wake) = &*release_gate;
            *lock.lock().unwrap() = true;
            wake.notify_all();
        });
        let answers = tokio::join!(cache.get(), cache.get(), cache.get(), cache.get());
        release.await.unwrap();
        assert_eq!(answers, (Some(env("shared")), Some(env("shared")), Some(env("shared")), Some(env("shared"))));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_none_result_is_retried_after_a_short_delay() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&fetches);
        let (cache, clock) =
            cache_with(move || if counting.fetch_add(1, Ordering::SeqCst) == 0 { None } else { Some(env("manager")) });
        assert_eq!(cache.get().await, None);
        assert_eq!(cache.get().await, None, "inside the short retry delay");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        clock.advance(RETRY_AFTER);
        assert_eq!(cache.get().await, Some(env("manager")));
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }
}
