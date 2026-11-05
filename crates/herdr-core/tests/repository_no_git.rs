#![cfg(unix)]

mod common;
use common::Session;
use herdr_core::repository::{discover, probe::Probe};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
fn absent_git_keeps_a_bounded_loose_head_branch_and_recovers_after_revalidation() {
    if let Some(root) = std::env::var_os("C3_NO_GIT_ROOT") {
        let home = PathBuf::from(root);
        let root = home.join("acme-shop");
        let repo = discover(&root, None).unwrap().repository.unwrap();
        let mut probe = Probe::new(home.clone(), None);
        let now = Instant::now();
        let facts = probe.refresh(&repo, now).unwrap();
        assert_eq!(facts.branch.as_deref(), Some("main"));
        assert!(facts.ahead_behind.is_none());
        assert_eq!(probe.processes(), 0, "a failed spawn is not a child");
        assert_eq!(probe.refresh(&repo, now + Duration::from_secs(5)).unwrap(), facts);
        assert_eq!(probe.processes(), 0);
        std::os::unix::fs::symlink(std::env::var_os("C3_REAL_GIT").unwrap(), home.join("bin/git")).unwrap();
        let facts = probe.refresh(&repo, now + Duration::from_secs(60)).unwrap();
        assert_eq!(facts.branch.as_deref(), Some("main"));
        assert_eq!(facts.ahead_behind, Some((0, 0)), "Git is retried after the bounded cache interval");
        fs::remove_file(home.join("bin/git")).unwrap();
        for (offset, head) in [
            (65, "0123456789012345678901234567890123456789\n"),
            (70, "ref: refs/heads/../invalid\n"),
            (71, "ref: refs/heads/\n"),
        ] {
            fs::write(root.join(".git/HEAD"), head).unwrap();
            let facts = probe.refresh(&repo, now + Duration::from_secs(offset)).unwrap();
            assert!(facts.branch.is_none() && facts.ahead_behind.is_none());
        }
        fs::write(root.join(".git/HEAD"), "x".repeat(64 * 1024 + 1)).unwrap();
        assert!(probe.refresh(&repo, now + Duration::from_secs(75)).is_err(), "fallback retains the HEAD read bound");
        return;
    }
    let mut s = Session::new();
    s.env.push(("HC_SESSION".into(), "c3-no-git".into()));
    let root = s.root.join("acme-shop");
    fs::create_dir_all(&root).unwrap();
    let real =
        std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|p| p.join("git")).find(|p| p.is_file()).unwrap();
    for args in [
        vec!["init", "--quiet", "-b", "main"],
        vec!["config", "user.name", "alice"],
        vec!["config", "user.email", "alice@example.invalid"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "initial"],
        vec!["branch", "upstream"],
        vec!["branch", "--set-upstream-to=upstream"],
    ] {
        assert!(s.program(&real).arg("-C").arg(&root).args(args).status().unwrap().success());
    }
    let bin = s.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let output = s
        .program(std::env::current_exe().unwrap())
        .args(["--exact", "absent_git_keeps_a_bounded_loose_head_branch_and_recovers_after_revalidation"])
        .env("PATH", &bin)
        .env("C3_NO_GIT_ROOT", &s.root)
        .env("C3_REAL_GIT", &real)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
