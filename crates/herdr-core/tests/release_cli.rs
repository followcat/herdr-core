#![allow(dead_code, unused_imports)] // Unit-test helpers have no harness in this fixture.

// Compile the same implementation with the test-only numeric version.
include!("../src/lib.rs");

fn main() {
    const { assert!(cfg!(test)) };
    assert!(!build_version().contains('-'));
    // Cargo may pass filters and libtest flags. Only explicit scratch-session
    // subprocesses enter the CLI; running this test target itself does no IO.
    if std::env::var_os("HC_TEST_RELEASE_CLI").is_some() {
        std::process::exit(cli::main());
    }
}
