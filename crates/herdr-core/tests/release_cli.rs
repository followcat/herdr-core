#![allow(dead_code, unused_imports)] // Unit-test helpers have no harness in this fixture.

// Compile the same implementation with the test-only numeric version.
include!("../src/lib.rs");

fn main() {
    const { assert!(cfg!(test)) };
    assert!(!build_version().contains('-'));
    // Cargo may pass filters and libtest flags. Only explicit scratch-session
    // subprocesses enter the CLI; running this test target itself does no IO.
    // A scratch-session subprocess enters the CLI directly. macOS launchd also
    // starts this fixture for a detached core, but with a clean environment that
    // lacks HC_TEST_RELEASE_CLI; it is identified only by its detach sentinel.
    let launchd = std::env::args_os().any(|arg| arg == "--hc-launchd");
    if launchd || std::env::var_os("HC_TEST_RELEASE_CLI").is_some() {
        std::process::exit(cli::main());
    }
}
