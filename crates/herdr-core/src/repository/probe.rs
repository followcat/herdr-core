//! Stat-first Git observations. Called by a bounded worker, never the foreground watcher.

use super::{Repository, small_file};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

mod process;
use process::Budget;

const RECHECK: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub branch: Option<String>,
    pub ahead_behind: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    target: PathBuf,
    metadata: Option<(u64, SystemTime)>,
}

/// One checkout cache; the future service shares it across terminals in that checkout.
#[derive(Clone)]
pub struct Probe {
    home: PathBuf,
    xdg: PathBuf,
    key: Option<(PathBuf, PathBuf, PathBuf)>,
    stamps: Vec<Stamp>,
    checked: Option<Instant>,
    facts: Option<Facts>,
    processes: usize,
}

impl Probe {
    pub fn new(home: PathBuf, xdg: Option<PathBuf>) -> Self {
        let xdg = xdg.unwrap_or_else(|| home.join(".config"));
        Self { home, xdg, key: None, stamps: Vec::new(), checked: None, facts: None, processes: 0 }
    }

    /// Child count for the latest refresh; zero means the stat fingerprint sufficed.
    pub fn processes(&self) -> usize {
        self.processes
    }

    pub fn refresh(&mut self, repo: &Repository, now: Instant) -> io::Result<Facts> {
        self.processes = 0;
        let key = (repo.root.clone(), repo.git_dir.clone(), repo.common_dir.clone());
        if self.key.as_ref() == Some(&key)
            && self.checked.is_some_and(|at| now.saturating_duration_since(at) < RECHECK)
            && current(&self.stamps)?
        {
            return self.facts.clone().ok_or_else(|| io::Error::other("repository facts unavailable"));
        }
        self.key = Some(key);
        self.checked = Some(now);
        self.facts = None;
        let mut files = BTreeSet::from([
            repo.root.join(".git"),
            repo.git_dir.join("HEAD"),
            repo.git_dir.join("commondir"),
            repo.git_dir.join("config.worktree"),
            repo.common_dir.join("config"),
            repo.common_dir.join("packed-refs"),
            repo.common_dir.join("refs"),
            repo.common_dir.join("reftable/tables.list"),
            self.home.join(".gitconfig"),
            self.xdg.join("git/config"),
        ]);
        let mut budget = Budget::new(&self.home, &self.xdg, repo);
        let result = observe(repo, &mut budget, &mut files)
            .or_else(|e| if budget.spawn_failed { loose_head(repo) } else { Err(e) });
        self.processes = budget.processes;
        // Record even a failed probe, avoiding repeated expensive config failures every tick.
        self.stamps = stamps(files)?;
        if let Ok(facts) = &result {
            self.facts = Some(facts.clone());
        }
        result
    }
}

fn observe(repo: &Repository, budget: &mut Budget<'_>, files: &mut BTreeSet<PathBuf>) -> io::Result<Facts> {
    if !crate::platform::repository_subprocesses() {
        // Windows has no child containment here. Do not run even config readers without it.
        return loose_head(repo);
    }
    let identity = identity(repo, budget, files)?;
    let mut facts = Facts { branch: identity.branch.clone(), ahead_behind: None };
    if !identity.promisor
        && let Some((head, upstream)) = &identity.oids
    {
        let range = format!("{head}...{upstream}");
        let output = budget.run(&["rev-list", "--left-right", "--count", &range])?;
        if output.status.success() {
            facts.ahead_behind = counts(&output.stdout);
        }
        if identity != self::identity(repo, budget, files)? {
            return Err(io::Error::other("repository changed during observation"));
        }
    }
    Ok(facts)
}

fn loose_head(repo: &Repository) -> io::Result<Facts> {
    const HEAD_SPACE: &[char] = &[' ', '\t', '\n', '\r'];
    let branch = small_file(&repo.git_dir.join("HEAD"))?.and_then(|head| {
        let name = head.trim_end_matches(HEAD_SPACE).strip_prefix("ref:")?.trim_start_matches(HEAD_SPACE);
        let branch = name.strip_prefix("refs/heads/")?;
        (!branch.is_empty() && valid_ref(name)).then(|| branch.into())
    });
    Ok(Facts { branch, ahead_behind: None })
}

#[derive(PartialEq, Eq)]
struct Identity {
    branch: Option<String>,
    oids: Option<(String, String)>,
    config: Vec<(String, String)>,
    promisor: bool,
}

fn identity(repo: &Repository, budget: &mut Budget<'_>, files: &mut BTreeSet<PathBuf>) -> io::Result<Identity> {
    let output = budget.run(&["config", "--includes", "--null", "--list", "--show-origin"])?;
    if !output.status.success() {
        return Err(io::Error::other("cannot read repository config"));
    }
    let config = config(&output.stdout, budget.home, files)?;
    let output = budget.run(&["symbolic-ref", "--quiet", "HEAD"])?;
    let full_branch = if output.status.success() {
        let line = text(&output.stdout)?;
        // The branch name is data: remove only the one LF/CRLF terminator.
        let line = line.strip_suffix('\n').map_or(line, |line| line.strip_suffix('\r').unwrap_or(line));
        Some(line.to_owned())
    } else if output.status.code() == Some(1) {
        None
    } else {
        return Err(io::Error::other("cannot read repository HEAD"));
    };
    let branch = full_branch.as_deref().and_then(|s| s.strip_prefix("refs/heads/")).map(String::from);
    // Fail closed on every partial-clone signal, regardless of spelling or
    // boolean value. No object command should depend on Git-version-specific
    // lazy-fetch suppression or config interpretation.
    let promisor = config.iter().any(|(key, _)| {
        key == "extensions.partialclone"
            || (key.starts_with("remote.") && (key.ends_with(".promisor") || key.ends_with(".partialclonefilter")))
    });
    let upstream = branch.as_deref().and_then(|branch| upstream(&config, branch));
    for name in full_branch.iter().chain(upstream.iter()) {
        if valid_ref(name) {
            files.insert(repo.common_dir.join(name));
            files.insert(repo.git_dir.join(name));
        }
    }
    let oids = if let Some(upstream) = upstream.filter(|s| !promisor && valid_ref(s)) {
        let output = budget.run(&["rev-parse", "--revs-only", "--end-of-options", "HEAD", &upstream])?;
        output.status.success().then(|| oid_pair(&output.stdout)).flatten()
    } else {
        None
    };
    Ok(Identity { branch, oids, config, promisor })
}

fn config(bytes: &[u8], home: &Path, files: &mut BTreeSet<PathBuf>) -> io::Result<Vec<(String, String)>> {
    let fields: Vec<_> = text(bytes)?.split_terminator('\0').collect();
    if fields.len() % 2 != 0 || fields.len() > 16_384 {
        return Err(io::Error::other("invalid or oversized config listing"));
    }
    let mut result = Vec::new();
    for entry in fields.chunks_exact(2) {
        let origin = entry[0].strip_prefix("file:").map(PathBuf::from);
        if let Some(path) = &origin {
            files.insert(path.clone());
        }
        let (key, value) = entry[1].split_once('\n').unwrap_or((entry[1], "true"));
        if (key == "include.path" || (key.starts_with("includeif.") && key.ends_with(".path")))
            && let Some(origin) = &origin
        {
            let path = if let Some(rest) = value.strip_prefix("~/") {
                home.join(rest)
            } else if Path::new(value).is_absolute() {
                value.into()
            } else {
                origin.parent().unwrap_or(Path::new(".")).join(value)
            };
            files.insert(path);
        }
        result.push((key.into(), value.into()));
    }
    if files.len() > 256 {
        return Err(io::Error::other("too many repository config dependencies"));
    }
    Ok(result)
}

fn upstream(config: &[(String, String)], branch: &str) -> Option<String> {
    let last = |key: &str| config.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let remote = last(&format!("branch.{branch}.remote"))?;
    let merge = last(&format!("branch.{branch}.merge"))?;
    if remote == "." {
        return Some(merge.into());
    }
    let key = format!("remote.{remote}.fetch");
    let specs: Vec<_> = config.iter().filter(|(k, _)| k == &key).map(|(_, v)| v.as_str()).collect();
    if specs.is_empty() {
        return map_ref(&format!("+refs/heads/*:refs/remotes/{remote}/*"), merge);
    }
    specs.into_iter().find_map(|spec| map_ref(spec, merge))
}

fn map_ref(spec: &str, name: &str) -> Option<String> {
    let (source, target) = spec.trim_start_matches('+').split_once(':')?;
    if source.starts_with('^') {
        return None;
    }
    match (source.split_once('*'), target.split_once('*')) {
        (None, None) => (source == name).then(|| target.into()),
        (Some((before, after)), Some((prefix, suffix))) => {
            let middle = name.strip_prefix(before)?.strip_suffix(after)?;
            Some(format!("{prefix}{middle}{suffix}"))
        }
        _ => None,
    }
}

fn valid_ref(name: &str) -> bool {
    // check-ref-format for a complete refs/... name, without normalization,
    // refspec-pattern or allow-onelevel relaxations. No Git child is needed.
    name.starts_with("refs/")
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.bytes().any(|b| b <= b' ' || b == 127 || b"~^:?*[\\".contains(&b))
        && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

fn oid_pair(bytes: &[u8]) -> Option<(String, String)> {
    let lines: Vec<_> = std::str::from_utf8(bytes).ok()?.lines().collect();
    if lines.len() != 2
        || lines.iter().any(|s| !matches!(s.len(), 40 | 64) || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return None;
    }
    Some((lines[0].into(), lines[1].into()))
}

fn counts(bytes: &[u8]) -> Option<(u64, u64)> {
    let fields: Vec<_> = std::str::from_utf8(bytes).ok()?.split_whitespace().collect();
    if fields.len() == 2 { Some((fields[0].parse().ok()?, fields[1].parse().ok()?)) } else { None }
}

fn text(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn stamp(path: PathBuf) -> io::Result<Stamp> {
    let metadata = match fs::metadata(&path) {
        Ok(m) => Some((m.len(), m.modified()?)),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => None,
        Err(e) => return Err(e),
    };
    Ok(Stamp { target: fs::canonicalize(&path).unwrap_or_else(|_| path.clone()), path, metadata })
}

fn stamps(files: BTreeSet<PathBuf>) -> io::Result<Vec<Stamp>> {
    let stamps: Vec<_> = files.into_iter().map(stamp).collect::<io::Result<_>>()?;
    let bytes: usize = stamps.iter().map(|s| s.path.as_os_str().len() + s.target.as_os_str().len()).sum();
    if stamps.len() > 256 || bytes > 128 * 1024 {
        return Err(io::Error::other("repository dependency fingerprint exceeds its cache budget"));
    }
    Ok(stamps)
}

fn current(previous: &[Stamp]) -> io::Result<bool> {
    for was in previous {
        if stamp(was.path.clone())? != *was {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod ref_tests {
    use super::valid_ref;

    #[test]
    fn full_ref_format_obeys_all_git_rules_without_normalization() {
        for name in [
            "refs/heads/main",
            "refs/heads/a./b",
            "refs/heads/topic.LOCK",
            "refs/heads/-topic",
            "refs/heads/@",
            "refs/heads/a@b",
            "refs/heads/a{b}",
            "refs/heads/开发/分支",
            "refs/heads/topic\u{2003}",
            "refs/remotes/origin/main",
            "refs/tags/v1.2",
        ] {
            assert!(valid_ref(name), "valid full ref: {name:?}");
        }
        for name in [
            "main",
            "refs",
            "refs/",
            "refs//heads/main",
            "refs/heads/main/",
            "refs/heads/.main",
            "refs/heads/main/.hidden",
            "refs/heads/a.lock/main",
            "refs/heads/main.lock",
            "refs/heads/a..b",
            "refs/heads/main.",
            "refs/heads/a:b",
            "refs/heads/a~b",
            "refs/heads/a^b",
            "refs/heads/a?b",
            "refs/heads/a*b",
            "refs/heads/a[b",
            "refs/heads/a\\b",
            "refs/heads/a@{b",
            "refs/heads/a b",
            "refs/heads/a\tb",
            "refs/heads/a\nb",
            "refs/heads/a\u{7f}b",
            "refs/heads/a\0b",
        ] {
            assert!(!valid_ref(name), "invalid full ref: {name:?}");
        }
    }
}
