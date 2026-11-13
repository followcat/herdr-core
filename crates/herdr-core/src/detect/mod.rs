//! Screen-derived status for programs that never speak OSC 7501 (fork).
//!
//! A pure engine: TOML manifests describe regions of the visible screen and
//! the matchers a rule needs; the best matching rule yields a [`Detection`].
//! No match means [`State::Unknown`]; only positive evidence may publish a
//! state. Nothing here touches the runtime, processes, or locks.

pub mod catalog;
pub mod identify;
pub mod manifest;
mod prefix;
mod regions;

pub use catalog::{Catalog, Limits};
pub use identify::{SUPPORTED, identify};
pub use manifest::Manifest;

/// Longest input field the engine will scan; oversized input is Unknown.
pub const MAX_INPUT_BYTES: usize = 1 << 18;

/// The screen text plus the two OSC-derived strings the rules can read.
/// `progress` stays empty: hc's relay does not capture OSC progress.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub screen: &'a str,
    pub title: &'a str,
    pub progress: &'a str,
}

/// What the screen evidence says the agent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Live prompt visible, ready for input. Published as `idle`.
    Idle,
    /// Mid-turn: spinners, tool footers, interrupt hints.
    Working,
    /// Waiting on a person: permission, question, or trust dialog.
    Blocked,
    /// No rule matched; nothing is published. Never collapses into idle.
    Unknown,
}

impl State {
    pub fn as_str(&self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::Blocked => "blocked",
            State::Unknown => "unknown",
        }
    }
}

/// One evaluation's outcome. `rule` names the winning rule for logs.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub state: State,
    /// `permission`, `question` or `auth` when a blocked rule says so.
    pub kind: Option<&'static str>,
    /// The screen is an agent-owned viewer (transcript, menu); the inferred
    /// status should not change on its account.
    pub skip: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub rule: Option<String>,
}

const NONE: Detection = Detection {
    state: State::Unknown,
    kind: None,
    skip: false,
    visible_idle: false,
    visible_blocker: false,
    visible_working: false,
    rule: None,
};

/// Run one manifest over the input. Highest priority rule wins; ties keep
/// the rule written first in the file. No match — or input over
/// [`MAX_INPUT_BYTES`] — is [`NONE`].
pub fn detect(manifest: &Manifest, input: &Input<'_>) -> Detection {
    detect_with_logical_screen(manifest, input, None)
}

/// Evaluate with a logical screen produced by the same VT snapshot. Only
/// opted-in gates use it. Without that view, rows remain hard-separated.
pub fn detect_with_logical_screen(manifest: &Manifest, input: &Input<'_>, logical: Option<&str>) -> Detection {
    if input.screen.len() > MAX_INPUT_BYTES
        || input.title.len() > MAX_INPUT_BYTES
        || input.progress.len() > MAX_INPUT_BYTES
        || logical.is_some_and(|s| s.len() > MAX_INPUT_BYTES)
    {
        return NONE;
    }
    let mut best: Option<&manifest::Rule> = None;
    for rule in &manifest.rules {
        if !rule.matches_with_logical_screen(input, logical) {
            continue;
        }
        match best {
            Some(prev) if prev.priority >= rule.priority => {}
            _ => best = Some(rule),
        }
    }
    let Some(rule) = best else { return NONE };
    Detection {
        state: rule.state,
        kind: rule.kind.map(|k| k.as_str()),
        skip: rule.skip_state_update,
        visible_idle: rule.visible_idle && rule.state == State::Idle,
        visible_blocker: rule.visible_blocker && rule.state == State::Blocked,
        visible_working: rule.visible_working && rule.state == State::Working,
        rule: Some(rule.id.clone()),
    }
}

/// The manifests compiled into the binary. `id` is both the agent key and
/// the file name under `<config dir>/detect/` for user overrides.
pub const BUNDLED: &[(&str, &str)] = &[
    ("claude", include_str!("manifests/claude.toml")),
    ("codex", include_str!("manifests/codex.toml")),
    ("command-code", include_str!("manifests/command-code.toml")),
    ("cursor", include_str!("manifests/cursor.toml")),
    ("devin", include_str!("manifests/devin.toml")),
    ("opencode", include_str!("manifests/opencode.toml")),
];

fn bundled_all() -> &'static [(&'static str, std::sync::Arc<Manifest>)] {
    static ALL: std::sync::OnceLock<Vec<(&'static str, std::sync::Arc<Manifest>)>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        BUNDLED
            .iter()
            .map(|(id, text)| {
                (*id, std::sync::Arc::new(Manifest::compile(text).unwrap_or_else(|e| panic!("bundled {id}: {e}"))))
            })
            .collect()
    })
}

/// A bundled manifest, compiled once and shared. Panics only if a checked-in
/// manifest is broken; `bundled_manifests_validate` in tests guards that.
pub fn bundled(id: &str) -> Option<std::sync::Arc<Manifest>> {
    bundled_all().iter().find(|(key, _)| *key == id).map(|(_, m)| m.clone())
}

#[cfg(test)]
mod tests;
