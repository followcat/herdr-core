//! Who ended a terminal, and the journal that outlives its record (fork).
//!
//! A terminal's record says how it ended (`reason`) but not who asked. This
//! module carries the answer — an optional `ended_by` on the record — and
//! keeps one JSON line per end and per removal in the session's state
//! directory, because `terminal.rm` deletes the record itself.
//!
//! The journal never holds argv beyond the basename of `argv[0]`, an
//! environment, cwd contents or screen text: argv can carry secrets (real
//! launches have carried proxy credentials there), and the file outlives the
//! terminal.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Why a terminal ended, when its own program did not simply exit.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
pub struct EndedBy {
    /// The request that ended it (`terminal.kill`, `terminal.rm`), or what
    /// happened instead: `die_with_parent`, `server_stop`, `host_shutdown`.
    pub by: String,
    /// The caller the request carried, as it was named here (`t3`,
    /// `rohan/t3`). `ppid:<pid>` means the caller was not a terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    /// The process that sent the request over this core's socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// That process's `comm` (`hc`, `herdr-core`, a simulator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm: Option<String>,
    /// The terminal whose program that process runs in, found by walking its
    /// parents up to a program this core started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// The core the request came through, when it came from another machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<String>,
    /// `die_with_parent`: the parent that ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// That parent's own `ended_by`, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_by: Option<Box<EndedBy>>,
}

impl EndedBy {
    pub fn cause(by: impl Into<String>) -> EndedBy {
        EndedBy { by: by.into(), ..EndedBy::default() }
    }
}

/// One end or removal, as the journal holds it.
pub struct Entry<'a> {
    /// `ended` or `removed`.
    pub event: &'a str,
    pub address: &'a str,
    pub name: Option<&'a str>,
    pub labels: &'a BTreeMap<String, String>,
    /// The basename of `argv[0]` — never the rest of argv.
    pub program: &'a str,
    pub reason: Option<&'a str>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub ended_by: Option<&'a EndedBy>,
}

/// The journal file, its one previous file, and how big one may get.
pub const JOURNAL: &str = "ended.jsonl";
/// Rotation: a line that would push a file past this starts a fresh one.
pub const MAX_BYTES: u64 = 1 << 20;

pub fn path(state: &Path) -> PathBuf {
    state.join(JOURNAL)
}

/// The one previous file (`.1`), kept after rotation.
pub fn previous(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

/// `HC_ENDED_MAX_BYTES` overrides the bound (tests, and a person who wants a
/// short journal).
pub fn max_bytes() -> u64 {
    std::env::var("HC_ENDED_MAX_BYTES").ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(MAX_BYTES)
}

/// Only the two labels a client places a terminal by; the rest stay out.
fn placed(labels: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    ["herdr.space", "herdr.tab"]
        .into_iter()
        .filter_map(|key| labels.get(key).map(|value| (key.to_string(), value.clone())))
        .collect()
}

/// One journal line, ending in a newline. Pure, so the shape is testable.
pub fn render(entry: &Entry<'_>, time_ms: u64) -> String {
    let mut object = serde_json::Map::new();
    object.insert("time_ms".into(), time_ms.into());
    object.insert("event".into(), entry.event.into());
    object.insert("address".into(), entry.address.into());
    if let Some(name) = entry.name.filter(|n| !n.is_empty()) {
        object.insert("name".into(), name.into());
    }
    let labels = placed(entry.labels);
    if !labels.is_empty() {
        object.insert("labels".into(), serde_json::to_value(labels).unwrap_or_default());
    }
    object.insert("program".into(), entry.program.into());
    if let Some(reason) = entry.reason {
        object.insert("reason".into(), reason.into());
    }
    if entry.exit_code.is_some() {
        object.insert("exit_code".into(), entry.exit_code.into());
    }
    if entry.signal.is_some() {
        object.insert("signal".into(), entry.signal.into());
    }
    if let Some(ended_by) = entry.ended_by {
        object.insert("ended_by".into(), serde_json::to_value(ended_by).unwrap_or_default());
    }
    let mut line = serde_json::Value::Object(object).to_string();
    line.push('\n');
    line
}

/// Write one line. A journal that cannot be written must never fail the
/// request that ended the terminal: the failure is logged once and dropped.
pub fn record(state: &Path, entry: &Entry<'_>) {
    if let Err(e) = append(state, render(entry, crate::runtime::now_ms()).as_bytes()) {
        report(&e);
    }
}

fn report(error: &std::io::Error) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        eprintln!("core: cannot write the end journal: {error}");
    }
}

fn append(state: &Path, line: &[u8]) -> std::io::Result<()> {
    let path = path(state);
    rotate(&path, line.len() as u64)?;
    let mut file = crate::platform::append_file(&path)?;
    file.write_all(line)
}

/// Past the bound, the current file becomes `.1` (replacing the older `.1`)
/// and the next line starts a fresh one.
fn rotate(path: &Path, incoming: u64) -> std::io::Result<()> {
    let Ok(meta) = std::fs::metadata(path) else { return Ok(()) };
    if meta.len() + incoming <= max_bytes() {
        return Ok(());
    }
    let older = previous(path);
    let _ = std::fs::remove_file(&older);
    match std::fs::rename(path, &older) {
        Ok(()) => Ok(()),
        // Gone between the metadata read and the rename: nothing to rotate.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The journal's lines, oldest first (the previous file, then the current
/// one). Unreadable lines stay as they are: a person asked what is there.
pub fn lines(state: &Path) -> std::io::Result<Vec<String>> {
    let path = path(state);
    let mut out = Vec::new();
    for file in [previous(&path), path] {
        match std::fs::read_to_string(&file) {
            Ok(text) => out.extend(text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// An end or a removal read back from the journal.
#[derive(Deserialize)]
pub struct Read {
    pub time_ms: u64,
    pub event: String,
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
    #[serde(default)]
    pub ended_by: Option<EndedBy>,
}

impl Read {
    /// One line for a person: when, what, where, how, and who asked.
    pub fn describe(&self) -> String {
        let name = self.name.as_deref().filter(|n| !n.is_empty()).map(|n| format!(" {n}")).unwrap_or_default();
        let how = match (self.reason.as_deref(), self.exit_code, self.signal) {
            (Some(reason), Some(code), _) => format!("{reason} (exit {code})"),
            (Some(reason), _, Some(signal)) => format!("{reason} (signal {signal})"),
            (Some(reason), _, _) => reason.to_string(),
            (None, _, _) => "-".into(),
        };
        let mut line = format!(
            "{} {:<7} {}{name} {} {how}",
            stamp(self.time_ms),
            self.event,
            self.address,
            basename(&self.program)
        );
        if let Some(by) = &self.ended_by {
            line.push_str(&format!(" by {}", by.describe()));
        }
        line
    }
}

impl EndedBy {
    /// `terminal.kill from t3 (hc pid 4821, in t3)`: what asked, and what it was.
    pub fn describe(&self) -> String {
        let mut text = self.by.clone();
        if let Some(caller) = &self.caller {
            text.push_str(&format!(" from {caller}"));
        }
        let mut how: Vec<String> = Vec::new();
        match (&self.comm, self.pid) {
            (Some(comm), Some(pid)) => how.push(format!("{comm} pid {pid}")),
            (_, Some(pid)) => how.push(format!("pid {pid}")),
            _ => {}
        }
        if let Some(terminal) = &self.terminal {
            how.push(format!("in {terminal}"));
        }
        if let Some(core) = &self.core {
            how.push(format!("over {core}"));
        }
        if !how.is_empty() {
            text.push_str(&format!(" ({})", how.join(", ")));
        }
        if let Some(parent) = &self.parent {
            text.push_str(&format!(" with {parent}"));
            if let Some(by) = &self.parent_by {
                text.push_str(&format!(" ({})", by.describe()));
            }
        }
        text
    }
}

/// The basename of a program path, either separator.
pub fn basename(argv0: &str) -> String {
    argv0.rsplit(['/', '\\']).find(|p| !p.is_empty()).unwrap_or(argv0).to_string()
}

/// An epoch time as `YYYY-MM-DDTHH:MM:SSZ` (UTC; no local-zone guess).
pub fn stamp(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(labels: &'a BTreeMap<String, String>) -> Entry<'a> {
        Entry {
            event: "ended",
            address: "t3",
            name: Some("build"),
            labels,
            program: "node",
            reason: Some("killed"),
            exit_code: None,
            signal: None,
            ended_by: None,
        }
    }

    #[test]
    fn a_line_carries_the_placement_labels_and_nothing_else_from_the_record() {
        let mut labels = BTreeMap::new();
        labels.insert("herdr.space".to_string(), "acme".to_string());
        labels.insert("herdr.tab".to_string(), "work".to_string());
        labels.insert("secret".to_string(), "do-not-write".to_string());
        let line = render(&entry(&labels), 1_000);
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["event"], "ended");
        assert_eq!(value["address"], "t3");
        assert_eq!(value["name"], "build");
        assert_eq!(value["program"], "node");
        assert_eq!(value["reason"], "killed");
        assert_eq!(value["labels"]["herdr.space"], "acme");
        assert_eq!(value["labels"]["herdr.tab"], "work");
        assert!(value["labels"].get("secret").is_none(), "{value}");
        assert!(value.get("ended_by").is_none(), "absent, never null");
        assert_eq!(value["time_ms"], 1_000);
        let mut bare = BTreeMap::new();
        bare.insert("where".to_string(), "elsewhere".to_string());
        let value: serde_json::Value = serde_json::from_str(render(&entry(&bare), 0).trim_end()).unwrap();
        assert!(value.get("labels").is_none());
    }

    #[test]
    fn a_caller_is_described_by_what_it_was() {
        let by = EndedBy {
            by: "terminal.kill".into(),
            caller: Some("t3".into()),
            pid: Some(4821),
            comm: Some("hc".into()),
            terminal: Some("t3".into()),
            ..EndedBy::default()
        };
        assert_eq!(by.describe(), "terminal.kill from t3 (hc pid 4821, in t3)");
        let cascade = EndedBy {
            by: "die_with_parent".into(),
            parent: Some("t1".into()),
            parent_by: Some(Box::new(by)),
            ..EndedBy::default()
        };
        assert_eq!(cascade.describe(), "die_with_parent with t1 (terminal.kill from t3 (hc pid 4821, in t3))");
    }

    #[test]
    fn a_line_for_a_person_names_when_what_where_how_and_who() {
        let line = r#"{"time_ms":1794997353000,"event":"removed","address":"t3","name":"build","program":"node","reason":"killed","signal":15,"ended_by":{"by":"terminal.rm","caller":"t1","pid":4821,"comm":"hc"}}"#;
        let read: Read = serde_json::from_str(line).unwrap();
        assert_eq!(
            read.describe(),
            "2026-11-18T10:22:33Z removed t3 build node killed (signal 15) by terminal.rm from t1 (hc pid 4821)"
        );
        let quiet: Read = serde_json::from_str(
            r#"{"time_ms":0,"event":"ended","address":"t4","program":"sh","reason":"exited","exit_code":0}"#,
        )
        .unwrap();
        assert_eq!(quiet.describe(), "1970-01-01T00:00:00Z ended   t4 sh exited (exit 0)");
    }

    #[test]
    fn a_stamp_is_utc_and_never_guesses_a_zone() {
        assert_eq!(stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(1_709_251_199_000), "2024-02-29T23:59:59Z");
        assert_eq!(stamp(1_794_997_353_999), "2026-11-18T10:22:33Z");
    }

    #[test]
    fn only_the_basename_of_argv0_is_used() {
        assert_eq!(basename("/usr/local/bin/node"), "node");
        assert_eq!(basename("C:\\Program Files\\nodejs\\node.exe"), "node.exe");
        assert_eq!(basename("node"), "node");
    }
}
