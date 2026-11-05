#![cfg(unix)]

mod common;

use common::Session;
use herdr_core::repository::{discover, probe::Probe};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

fn session() -> Session {
    let mut session = Session::new();
    session.env.push(("HC_SESSION".into(), "c3-repository-probe".into()));
    session
}

fn git(s: &Session, root: &Path, args: &[&str]) -> String {
    let out = s.program("git").arg("-C").arg(root).args(args).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn repo(s: &Session) -> std::path::PathBuf {
    let root = s.root.join("acme-shop");
    fs::create_dir_all(&root).unwrap();
    git(s, &root, &["init", "--quiet", "-b", "main"]);
    git(s, &root, &["config", "user.name", "alice"]);
    git(s, &root, &["config", "user.email", "alice@example.invalid"]);
    git(s, &root, &["commit", "--quiet", "--allow-empty", "-m", "initial"]);
    root
}

#[test]
fn stat_first_cache_refreshes_local_divergence_and_same_oid_branch_switch() {
    let s = session();
    let root = repo(&s);
    git(&s, &root, &["branch", "upstream"]);
    git(&s, &root, &["checkout", "--quiet", "upstream"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "behind"]);
    git(&s, &root, &["checkout", "--quiet", "main"]);
    git(&s, &root, &["branch", "--set-upstream-to=upstream"]);
    for message in ["ahead-one", "ahead-two"] {
        git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", message]);
    }
    let repo = discover(&root, None).unwrap().repository.unwrap();
    let mut probe = Probe::new(s.root.clone(), Some(s.root.join("config")));
    let now = Instant::now();
    let facts = probe.refresh(&repo, now).unwrap();
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert_eq!(facts.ahead_behind, Some((2, 1)));
    assert!(probe.processes() <= 8);
    assert_eq!(probe.refresh(&repo, now + Duration::from_secs(5)).unwrap(), facts);
    assert_eq!(probe.processes(), 0, "unchanged 5-second tick must only stat");
    probe.refresh(&repo, now + Duration::from_secs(60)).unwrap();
    assert!(probe.processes() > 0, "force a bounded revalidation at 60 seconds");
    git(&s, &root, &["checkout", "--quiet", "-b", "feature"]);
    let facts = probe.refresh(&repo, now + Duration::from_secs(61)).unwrap();
    assert_eq!(facts.branch.as_deref(), Some("feature"));
    assert_eq!(facts.ahead_behind, None);
    git(&s, &root, &["checkout", "--quiet", "--detach"]);
    assert_eq!(probe.refresh(&repo, now + Duration::from_secs(62)).unwrap().branch, None);
}

#[test]
fn includes_packed_refs_missing_includes_and_custom_refspecs_are_observed() {
    let s = session();
    let root = repo(&s);
    let oid = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["update-ref", "refs/cache/main", &oid]);
    git(&s, &root, &["config", "branch.main.remote", "origin"]);
    git(&s, &root, &["config", "branch.main.merge", "refs/heads/main"]);
    git(&s, &root, &["config", "remote.origin.fetch", "+refs/heads/*:refs/cache/*"]);
    git(&s, &root, &["config", "include.path", "extra.cfg"]);
    git(&s, &root, &["pack-refs", "--all", "--prune"]);
    let repo = discover(&root, None).unwrap().repository.unwrap();
    let mut probe = Probe::new(s.root.clone(), None);
    let now = Instant::now();
    assert_eq!(probe.refresh(&repo, now).unwrap().ahead_behind, Some((0, 0)));
    fs::write(root.join(".git/extra.cfg"), "[branch \"main\"]\n remote = .\n merge = refs/heads/missing\n").unwrap();
    assert_eq!(probe.refresh(&repo, now + Duration::from_secs(5)).unwrap().ahead_behind, None);
    fs::write(root.join(".git/extra.cfg"), "[branch \"main\"]\n remote = .\n merge = refs/heads/main\n").unwrap();
    assert_eq!(probe.refresh(&repo, now + Duration::from_secs(10)).unwrap().ahead_behind, Some((0, 0)));
    fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    fs::write(root.join(".git/refs/heads/main"), "broken\n").unwrap();
    assert!(
        probe.refresh(&repo, now + Duration::from_secs(15)).is_err(),
        "broken loose refs cannot resurrect packed facts"
    );
}

#[test]
fn hostile_config_cannot_execute_helpers_and_large_includes_stay_confined() {
    use std::os::unix::fs::PermissionsExt;
    let s = session();
    let root = repo(&s);
    let sentinel = s.root.join("ran");
    let helper = s.root.join("helper");
    fs::write(&helper, format!("#!/bin/sh\ntouch '{}'\nexit 1\n", sentinel.display())).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    for key in ["core.fsmonitor", "credential.helper", "core.sshCommand", "core.pager"] {
        git(&s, &root, &["config", key, helper.to_str().unwrap()]);
    }
    git(&s, &root, &["config", "core.hooksPath", s.root.to_str().unwrap()]);
    fs::copy(&helper, s.root.join("post-checkout")).unwrap();
    git(&s, &root, &["branch", "upstream"]);
    git(&s, &root, &["branch", "--set-upstream-to=upstream"]);
    let repo = discover(&root, None).unwrap().repository.unwrap();
    let mut probe = Probe::new(s.root.clone(), None);
    assert_eq!(probe.refresh(&repo, Instant::now()).unwrap().ahead_behind, Some((0, 0)));
    assert!(!sentinel.exists(), "no configured executable may run");
    git(&s, &root, &["config", "include.path", "large.cfg"]);
    let huge = root.join(".git/large.cfg");
    fs::write(&huge, "[large]\n value = ").unwrap();
    fs::OpenOptions::new().write(true).open(&huge).unwrap().set_len(600 * 1024 * 1024).unwrap();
    let started = Instant::now();
    assert!(probe.refresh(&repo, started).is_err(), "resource failure affects repository facts only");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!sentinel.exists());
    fs::remove_file(&huge).unwrap();
    assert_eq!(probe.refresh(&repo, started + Duration::from_secs(60)).unwrap().ahead_behind, Some((0, 0)));
}

#[test]
fn linked_checkout_and_unborn_and_promisor_repos_keep_independent_facts() {
    let s = session();
    let root = repo(&s);
    let linked = s.root.join("feature");
    git(&s, &root, &["worktree", "add", "--quiet", "-b", "feature", linked.to_str().unwrap()]);
    let mut probe = Probe::new(s.root.clone(), None);
    let main = discover(&root, None).unwrap().repository.unwrap();
    let feature = discover(&linked, None).unwrap().repository.unwrap();
    assert_eq!(probe.refresh(&main, Instant::now()).unwrap().branch.as_deref(), Some("main"));
    assert_eq!(probe.refresh(&feature, Instant::now()).unwrap().branch.as_deref(), Some("feature"));
    git(&s, &root, &["config", "remote.origin.promisor", "true"]);
    assert_eq!(probe.refresh(&main, Instant::now()).unwrap().ahead_behind, None);
    let unborn = s.root.join("unborn");
    fs::create_dir_all(&unborn).unwrap();
    git(&s, &unborn, &["init", "--quiet", "-b", "main"]);
    let unborn = discover(&unborn, None).unwrap().repository.unwrap();
    let facts = probe.refresh(&unborn, Instant::now()).unwrap();
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert_eq!(facts.ahead_behind, None);
}

#[test]
fn partial_clone_signals_never_fetch_a_missing_upstream_object_or_launch_its_helper() {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var_os("C3_GIT_OLD_LAZY_FETCH").is_none() {
        let s = session();
        let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("git"))
            .find(|p| p.is_file())
            .unwrap();
        let shim = s.root.join("git");
        // Model Git versions that ignore this environment variable. The safety
        // regression must not pass merely because the installed Git honors it.
        fs::write(&shim, format!("#!/bin/sh\nunset GIT_NO_LAZY_FETCH\nexec '{}' \"$@\"\n", real.display())).unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        let output = s
            .program(std::env::current_exe().unwrap())
            .args(["--exact", "partial_clone_signals_never_fetch_a_missing_upstream_object_or_launch_its_helper"])
            .env("C3_GIT_OLD_LAZY_FETCH", "1")
            .env("PATH", format!("{}:{}", s.root.display(), std::env::var("PATH").unwrap()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let s = session();
    let root = repo(&s);
    let initial = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "missing parent"]);
    let missing = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "upstream"]);
    let upstream = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["update-ref", "refs/remotes/origin/main", &upstream]);
    git(&s, &root, &["reset", "--quiet", "--hard", &initial]);
    fs::remove_file(root.join(".git/objects").join(&missing[..2]).join(&missing[2..])).unwrap();
    let sentinel = s.root.join("helper-ran");
    let helper = s.root.join("offline-helper");
    fs::write(&helper, format!("#!/bin/sh\nprintf ran > '{}'\nexit 1\n", sentinel.display())).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    for (key, value) in [
        ("remote.origin.url", "ssh://host-a/acme-shop"),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
        ("branch.main.remote", "origin"),
        ("branch.main.merge", "refs/heads/main"),
        ("core.sshCommand", helper.to_str().unwrap()),
        ("core.askPass", helper.to_str().unwrap()),
        ("credential.helper", helper.to_str().unwrap()),
    ] {
        git(&s, &root, &["config", key, value]);
    }
    let repo = discover(&root, None).unwrap().repository.unwrap();
    // Presence is deliberately conservative, including false/unknown values: no
    // count depends on a version-specific interpretation of partial-clone config.
    for (key, value) in [
        ("remote.origin.promisor", "yes"),
        ("remote.origin.promisor", "on"),
        ("remote.origin.promisor", "1"),
        ("remote.origin.promisor", "true"),
        ("remote.origin.promisor", ""),
        ("remote.origin.promisor", "false"),
        ("remote.origin.partialCloneFilter", "blob:none"),
        ("extensions.partialClone", "origin"),
    ] {
        git(&s, &root, &["config", key, value]);
        let mut probe = Probe::new(s.root.clone(), None);
        let observed = probe.refresh(&repo, Instant::now());
        assert!(!sentinel.exists(), "configured helper ran for {key}={value}");
        let facts = observed.unwrap();
        assert_eq!(facts.branch.as_deref(), Some("main"));
        assert!(facts.ahead_behind.is_none(), "partial-clone counts must remain unknown");
        assert_eq!(probe.processes(), 2, "only config and symbolic HEAD, no object traversal");
        git(&s, &root, &["config", "--unset-all", key]);
    }
}

#[test]
fn an_empty_protocol_allowlist_blocks_even_an_explicitly_allowed_ext_helper() {
    use std::os::unix::fs::PermissionsExt;
    let s = session();
    let sentinel = s.root.join("ran");
    let helper = s.root.join("offline-helper");
    fs::write(&helper, format!("#!/bin/sh\nprintf ran > '{}'\nexit 1\n", sentinel.display())).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    for restricted in [false, true] {
        let mut command = s.program("git");
        command.current_dir(&s.root).args([
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.ext.allow=always",
            "ls-remote",
            &format!("ext::{}", helper.display()),
        ]);
        if restricted {
            command.env("GIT_ALLOW_PROTOCOL", "");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert_eq!(sentinel.exists(), !restricted, "empty means an allowlist with no protocols, not unset");
        if restricted {
            assert!(String::from_utf8_lossy(&output.stderr).contains("transport 'ext' not allowed"));
        } else {
            fs::remove_file(&sentinel).unwrap();
        }
    }
}

#[test]
fn every_git_child_keeps_transports_disabled_across_a_config_change() {
    use std::os::unix::fs::PermissionsExt;
    if let Some(home) = std::env::var_os("C3_PROTOCOL_RACE_ROOT") {
        let home = std::path::PathBuf::from(home);
        let repository = discover(&home.join("acme-shop"), None).unwrap().repository.unwrap();
        let result = Probe::new(home, None).refresh(&repository, Instant::now());
        assert!(result.is_err(), "changed identity cannot publish count results");
        return;
    }
    let s = session();
    let root = repo(&s);
    let initial = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "missing parent"]);
    let missing = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["commit", "--quiet", "--allow-empty", "-m", "upstream"]);
    let upstream = git(&s, &root, &["rev-parse", "HEAD"]);
    git(&s, &root, &["update-ref", "refs/remotes/origin/main", &upstream]);
    git(&s, &root, &["reset", "--quiet", "--hard", &initial]);
    fs::remove_file(root.join(".git/objects").join(&missing[..2]).join(&missing[2..])).unwrap();
    for (key, value) in [
        ("remote.origin.url", "ssh://host-a/acme-shop"),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
        ("branch.main.remote", "origin"),
        ("branch.main.merge", "refs/heads/main"),
    ] {
        git(&s, &root, &["config", key, value]);
    }
    let sentinel = s.root.join("ext-ran");
    let helper = s.root.join("offline-ext");
    fs::write(&helper, format!("#!/bin/sh\nprintf ran > '{}'\nexit 1\n", sentinel.display())).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    let real =
        std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|p| p.join("git")).find(|p| p.is_file()).unwrap();
    let bin = s.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("git");
    fs::write(
        &shim,
        format!(
            r#"#!/bin/sh
if [ "${{GIT_ALLOW_PROTOCOL+x}}" = x ] && [ "$GIT_ALLOW_PROTOCOL" = '' ]; then
    printf 'blocked\n' >> '{home}/policy'
else
    printf 'open\n' >> '{home}/policy'
fi
for arg do
    if [ "$arg" = rev-list ]; then
        printf paused > '{home}/paused'
        while [ ! -e '{home}/resume' ]; do /bin/sleep 0.002; done
        break
    fi
done
unset GIT_NO_LAZY_FETCH
exec '{real}' "$@"
"#,
            home = s.root.display(),
            real = real.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = s
        .program(std::env::current_exe().unwrap())
        .args(["--exact", "every_git_child_keeps_transports_disabled_across_a_config_change"])
        .env("C3_PROTOCOL_RACE_ROOT", &s.root)
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let waiting = Instant::now();
    while !s.root.join("paused").exists() {
        if child.try_wait().unwrap().is_some() || waiting.elapsed() > Duration::from_secs(3) {
            fs::write(s.root.join("resume"), "").unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "probe did not reach rev-list: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // The probe's cached config says no promisor; the count child sees this.
    for (key, value) in [
        ("remote.origin.promisor", "yes"),
        ("remote.origin.url", &format!("ext::{}", helper.display())),
        ("protocol.ext.allow", "always"),
    ] {
        git(&s, &root, &["config", key, value]);
    }
    fs::write(s.root.join("resume"), "").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!sentinel.exists(), "a config race must never launch the ext helper");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let policy = fs::read_to_string(s.root.join("policy")).unwrap();
    assert!(policy.lines().count() >= 4, "config, symbolic HEAD, OIDs and count must all run");
    assert!(
        policy.lines().all(|line| line == "blocked"),
        "every child needs the unconditional transport restriction: {policy}"
    );
}
