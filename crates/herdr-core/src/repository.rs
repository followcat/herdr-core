//! Machine-side repository observations. No RPC or foreground-watcher IO.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub mod probe;
pub(crate) mod service;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FactState {
    Pending,
    Ready,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AheadBehind {
    pub ahead: u64,
    pub behind: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Checkout {
    pub repository_key: String,
    pub checkout_key: String,
    pub repo_name: String,
    pub checkout_name: String,
    pub repo_root: String,
    pub is_linked_worktree: bool,
    pub is_bare: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepositoryFacts {
    /// Origin core, never a relay alias. Keys and paths are opaque on viewers.
    pub core_id: String,
    /// Machine-side directory this observation concerns.
    pub cwd: String,
    pub state: FactState,
    pub auto_label: Option<String>,
    pub checkout: Option<Checkout>,
    pub branch: Option<String>,
    /// Unknown counts are null; observed zero counts remain zero.
    pub ahead_behind: Option<AheadBehind>,
}

impl RepositoryFacts {
    fn pending(core_id: &str, cwd: &str) -> Self {
        Self {
            core_id: core_id.into(),
            cwd: cwd.into(),
            state: FactState::Pending,
            auto_label: None,
            checkout: None,
            branch: None,
            ahead_behind: None,
        }
    }
}

impl From<&Repository> for Checkout {
    fn from(repo: &Repository) -> Self {
        Self {
            repository_key: repo.repository_key.clone(),
            checkout_key: repo.checkout_key.clone(),
            repo_name: repo.repo_name.clone(),
            checkout_name: repo.checkout_name.clone(),
            repo_root: repo.root.to_string_lossy().into_owned(),
            is_linked_worktree: repo.is_linked_worktree,
            is_bare: repo.is_bare,
        }
    }
}

const SMALL_FILE: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub repository_key: String,
    pub checkout_key: String,
    pub repo_name: String,
    pub checkout_name: String,
    pub is_linked_worktree: bool,
    pub is_bare: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub auto_label: String,
    pub repository: Option<Repository>,
}

/// Nearest valid Git root, including linked worktrees and bare repositories.
/// The caller supplies this machine's home; discovering facts never changes cwd.
pub fn discover(cwd: &Path, home: Option<&Path>) -> io::Result<Discovery> {
    let mut root = match fs::metadata(cwd) {
        Ok(meta) if meta.is_dir() => cwd.to_path_buf(),
        Ok(_) => cwd.parent().unwrap_or(cwd).to_path_buf(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => cwd.parent().unwrap_or(cwd).to_path_buf(),
        Err(e) => return Err(e),
    };
    // A malformed/deep path cannot make a discovery job grow without bound.
    for _ in 0..512 {
        if let Some(git) = git_dir(&root)?
            && is_file(&git.join("HEAD"))?
        {
            let git = canonical(&git);
            let common = match small_file(&git.join("commondir"))? {
                Some(value) => resolve(&git, value.trim()),
                None => git.clone(),
            };
            let common = canonical(&common);
            let is_bare = bare(&git)?;
            let checkout_name = basename(&root).unwrap_or_else(|| fallback(cwd, home));
            let name_root = match common.file_name().and_then(|s| s.to_str()) {
                Some(".git") => common.parent().unwrap_or(&root),
                Some(".bare") => {
                    let container = common.parent().unwrap_or(&common);
                    if git_dir(container)?.is_some_and(|p| canonical(&p) == common) { container } else { &common }
                }
                _ => &common,
            };
            return Ok(Discovery {
                auto_label: basename(&root).unwrap_or_else(|| fallback(cwd, home)),
                repository: Some(Repository {
                    repository_key: common.to_string_lossy().into_owned(),
                    checkout_key: canonical(&root).to_string_lossy().into_owned(),
                    repo_name: basename(name_root).unwrap_or_else(|| "repo".into()),
                    checkout_name,
                    is_linked_worktree: git != common,
                    is_bare,
                    root,
                    git_dir: git,
                    common_dir: common,
                }),
            });
        }
        if !root.pop() {
            return Ok(Discovery { auto_label: fallback(cwd, home), repository: None });
        }
    }
    Err(io::Error::other("repository discovery exceeded 512 ancestors"))
}

fn git_dir(root: &Path) -> io::Result<Option<PathBuf>> {
    let marker = root.join(".git");
    match fs::metadata(&marker) {
        Ok(meta) if meta.is_dir() => return Ok(Some(marker)),
        Ok(meta) if meta.is_file() => {
            return Ok(small_file(&marker)?.and_then(|s| {
                let value = s.trim().strip_prefix("gitdir:")?.trim();
                (!value.is_empty()).then(|| resolve(root, value))
            }));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if is_file(&root.join("HEAD"))? && root.join("objects").is_dir() && root.join("refs").is_dir() && bare(root)? {
        return Ok(Some(root.to_path_buf()));
    }
    Ok(None)
}

fn bare(git: &Path) -> io::Result<bool> {
    let Some(config) = small_file(&git.join("config"))? else { return Ok(false) };
    let mut core = false;
    let mut bare = false;
    for line in config.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.split_once(']')) {
            core = section.0.trim().eq_ignore_ascii_case("core");
        } else if core
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("bare")
        {
            bare = value.split_whitespace().next().is_some_and(|v| v.eq_ignore_ascii_case("true"));
        }
    }
    Ok(bare)
}

pub(super) fn small_file(path: &Path) -> io::Result<Option<String>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(SMALL_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > SMALL_FILE {
        return Err(io::Error::other("repository marker exceeds 64 KiB"));
    }
    String::from_utf8(bytes).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn is_file(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => Ok(false),
        Err(e) => Err(e),
    }
}

fn resolve(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() { path.into() } else { base.join(path) }
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.into())
}

fn basename(path: &Path) -> Option<String> {
    path.file_name()?.to_str().filter(|s| !s.is_empty()).map(String::from)
}

fn fallback(cwd: &Path, home: Option<&Path>) -> String {
    if home == Some(cwd) { "~".into() } else { basename(cwd).unwrap_or_else(|| cwd.to_string_lossy().into_owned()) }
}
