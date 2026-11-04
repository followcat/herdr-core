//! The manifests available to detection: bundled ones plus user overrides in
//! `<config dir>/detect/*.toml`, narrowed by `[detect]` config. Loading reads
//! a directory once; everything after stays in `Arc`s.
//!
//! Precedence: a valid `<agent>.toml` replaces the bundled manifest for that
//! agent. A bad or misplaced override never removes the bundled rule set — it
//! lands in `warnings` for the caller to log.

use super::identify::SUPPORTED;
use super::{BUNDLED, manifest::Manifest};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

/// `[detect]` in config.toml. Absent fields mean defaults.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// `false` switches screen detection off entirely.
    pub enabled: Option<bool>,
    /// Restricts detection to these agent ids when present.
    pub agents: Option<Vec<String>>,
}

/// Largest accepted manifest file; bigger overrides are skipped.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 1 << 18;

pub struct Catalog {
    manifests: Vec<(String, Arc<Manifest>)>,
    /// Human-readable override problems, for the caller's log.
    pub warnings: Vec<String>,
}

impl Catalog {
    /// Bundled manifests, optionally limited. `enabled = false` gives an
    /// empty catalog; `agents` keeps only the ids it names.
    pub fn load(dir: Option<&Path>, limits: Option<&Limits>) -> Catalog {
        let mut warnings = Vec::new();
        if limits.is_some_and(|l| l.enabled == Some(false)) {
            return Catalog { manifests: Vec::new(), warnings };
        }
        let allowed: Option<BTreeSet<String>> = limits.and_then(|l| l.agents.as_ref()).map(|list| {
            list.iter()
                .map(|id| id.trim().to_lowercase())
                .filter(|id| {
                    let ok = SUPPORTED.contains(&id.as_str());
                    if !ok {
                        warnings.push(format!("detect.agents: {id:?} is not a supported agent, ignoring"));
                    }
                    ok
                })
                .collect()
        });
        let allow = |id: &str| allowed.as_ref().is_none_or(|set| set.contains(id));

        let mut manifests: Vec<(String, Arc<Manifest>)> = Vec::new();
        for (id, text) in BUNDLED {
            if allow(id) {
                manifests.push((
                    (*id).to_string(),
                    Arc::new(Manifest::compile(text).unwrap_or_else(|e| panic!("bundled {id}: {e}"))),
                ));
            }
        }
        if let Some(dir) = dir {
            load_overrides(dir, &allow, &mut manifests, &mut warnings);
        }
        Catalog { manifests, warnings }
    }

    /// The manifest for `agent` by canonical id. Aliases are metadata, not
    /// lookup keys: `identify` already canonicalizes, and an override's
    /// declared aliases must not shadow another agent's entry or dodge the
    /// `[detect] agents` allowlist.
    pub fn get(&self, agent: &str) -> Option<Arc<Manifest>> {
        self.manifests.iter().find(|(id, _)| id == agent).map(|(_, m)| m.clone())
    }

    pub fn agents(&self) -> impl Iterator<Item = &str> {
        self.manifests.iter().map(|(id, _)| id.as_str())
    }
}

fn load_overrides(
    dir: &Path,
    allow: &dyn Fn(&str) -> bool,
    manifests: &mut Vec<(String, Arc<Manifest>)>,
    warnings: &mut Vec<String>,
) {
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warnings.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension() != Some(std::ffi::OsStr::new("toml")) {
            continue;
        }
        let name = path.display().to_string();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if !SUPPORTED.contains(&stem) {
            warnings.push(format!("{name}: no supported agent named {stem:?}; file is ignored"));
            continue;
        }
        if !allow(stem) {
            continue;
        }
        match read_override(&path) {
            Ok(m) if m.id == stem => {
                if let Some(slot) = manifests.iter_mut().find(|(id, _)| id == stem) {
                    slot.1 = Arc::new(m);
                } else {
                    manifests.push((stem.to_string(), Arc::new(m)));
                }
            }
            Ok(m) => warnings.push(format!("{name}: id {:?} must equal the file name {stem:?}", m.id)),
            Err(e) => warnings.push(format!("{name}: {e}; bundled rules kept")),
        }
    }
}

fn read_override(path: &Path) -> Result<Manifest, String> {
    use std::io::Read;
    // Check before opening: a fifo or device could block the read forever.
    if !std::fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".to_string());
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!("over {MAX_MANIFEST_BYTES} bytes"));
    }
    Manifest::compile(&String::from_utf8(buf).map_err(|e| e.to_string())?)
}
