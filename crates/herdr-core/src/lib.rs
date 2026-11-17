pub mod activity;
pub mod addr;
pub mod api;
pub mod attach;
pub mod cli;
pub mod client;
pub mod config;
pub mod detect;
pub mod events;
pub mod exec;
pub mod files;
pub mod foreground;
pub mod holder;
pub mod holder_proto;
pub mod input;
mod kept;
pub mod keys;
pub mod locale;
pub mod machines;
pub mod notes;
pub mod paths;
pub mod picker;
pub mod platform;
pub mod put;
pub mod relay;
pub mod releases;
pub mod remote;
#[doc(hidden)]
pub mod repository;
mod restart;
pub mod rpc;
pub mod runtime;
pub mod screen_status;
pub mod server;
pub mod status;
pub mod stitch;
pub mod sweep;
pub mod templates;
pub mod terminals;
pub mod viewers;
pub mod wait;
pub mod wire;

/// The folder tests put their scratch data in: `HC_TEST_TMPDIR` if set, else
/// /var/tmp, whose short path keeps socket paths under the macOS limit.
#[cfg(test)]
pub(crate) fn test_tmp() -> std::path::PathBuf {
    std::env::var_os("HC_TEST_TMPDIR").map_or_else(|| "/var/tmp".into(), Into::into)
}

/// The version compiled into every CLI, API and installation decision.
/// Test targets also exercise official release paths on fork branches.
pub fn build_version() -> &'static str {
    let version = env!("CARGO_PKG_VERSION");
    if cfg!(test) { version.split('-').next().unwrap() } else { version }
}
