#![cfg(unix)]

mod common;

use common::Session;
use herdr_core::repository::discover;
use std::fs;
use std::path::Path;

fn session() -> Session {
    let mut session = Session::new();
    session.env.push(("HC_SESSION".into(), "c3-repository".into()));
    session
}

fn marker(root: &Path, git: &Path, bare: bool) {
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(git.join("config"), format!("[core]\n bare = {bare}\n")).unwrap();
    fs::create_dir_all(root).unwrap();
}

#[test]
fn discovery_uses_repo_root_labels_and_distinguishes_common_repo_from_checkout() {
    let s = session();
    let repo = s.root.join("acme-shop");
    let git = repo.join(".git");
    marker(&repo, &git, false);
    fs::create_dir_all(repo.join("src")).unwrap();
    let main = discover(&repo.join("src"), Some(&s.root)).unwrap();
    assert_eq!(main.auto_label, "acme-shop");
    let main = main.repository.unwrap();
    assert_eq!(main.root, repo);
    assert_eq!(main.repo_name, "acme-shop");
    assert_eq!(main.checkout_name, "acme-shop");
    assert!(!main.is_linked_worktree && !main.is_bare);

    let checkout = s.root.join("feature");
    let private = git.join("worktrees/feature");
    marker(&checkout, &private, false);
    fs::write(checkout.join(".git"), "gitdir: ../acme-shop/.git/worktrees/feature\n").unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    let linked = discover(&checkout, None).unwrap();
    assert_eq!(linked.auto_label, "feature");
    let linked = linked.repository.unwrap();
    assert_eq!(linked.repository_key, main.repository_key);
    assert_ne!(linked.checkout_key, main.checkout_key);
    assert_eq!(linked.repo_name, main.repo_name);
    assert_eq!(linked.checkout_name, "feature");
    assert!(linked.is_linked_worktree);

    let alias = s.root.join("alias");
    std::os::unix::fs::symlink(&checkout, &alias).unwrap();
    let alias = discover(&alias, None).unwrap().repository.unwrap();
    assert_eq!(alias.checkout_key, linked.checkout_key);
    assert_eq!(alias.repository_key, linked.repository_key);
}

#[test]
fn bare_names_follow_standalone_and_embedded_container_semantics() {
    let s = session();
    let container = s.root.join("acme-shop");
    let bare = container.join(".bare");
    marker(&bare, &bare, true);
    let standalone = discover(&bare.join("refs"), None).unwrap();
    assert_eq!(standalone.auto_label, ".bare");
    let standalone = standalone.repository.unwrap();
    assert_eq!(standalone.repo_name, ".bare");
    assert!(standalone.is_bare && !standalone.is_linked_worktree);
    fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
    let embedded = discover(&container, None).unwrap();
    assert_eq!(embedded.auto_label, "acme-shop");
    let embedded = embedded.repository.unwrap();
    assert_eq!(embedded.repo_name, "acme-shop");
    assert_eq!(embedded.repository_key, standalone.repository_key);
    assert!(embedded.is_bare);

    let linked = container.join("feature");
    let private = bare.join("worktrees/feature");
    marker(&linked, &private, false);
    fs::write(linked.join(".git"), "gitdir: ../.bare/worktrees/feature\n").unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    let linked = discover(&linked, None).unwrap().repository.unwrap();
    assert_eq!(linked.repository_key, embedded.repository_key);
    assert!(linked.is_linked_worktree);
    assert!(!linked.is_bare, "a checkout of a bare repository is not the bare source");
}

#[test]
fn invalid_markers_do_not_hide_a_parent_repo_and_plain_labels_use_home() {
    let s = session();
    assert_eq!(discover(&s.root, Some(&s.root)).unwrap().auto_label, "~");
    let root = s.root.join("acme-shop");
    marker(&root, &root.join(".git"), false);
    let nested = root.join("src");
    fs::create_dir_all(nested.join(".git")).unwrap();
    let found = discover(&nested, None).unwrap();
    assert_eq!(found.auto_label, "acme-shop");
    assert_eq!(found.repository.unwrap().root, root);
    let plain = s.root.join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join(".git"), "not a gitdir marker\n").unwrap();
    let found = discover(&plain, None).unwrap();
    assert!(found.repository.is_none());
    assert_eq!(found.auto_label, "plain");
    fs::write(plain.join(".git"), "x".repeat(65 * 1024)).unwrap();
    assert!(discover(&plain, None).is_err(), "oversized markers are unavailable, not guessed");
}
