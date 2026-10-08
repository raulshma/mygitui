//! Submodule engine tests (lane F3).
//!
//! `git submodule add` has no git2 binding, so the scaffolding shells out to
//! the system `git` CLI against throwaway repositories under
//! `std::env::temp_dir()` (same pattern as stash_tests.rs / net_tests.rs).
//! The methods under test are the inherent `*_impl` twins called directly by
//! the M4 IPC commands.

use std::path::{Path, PathBuf};

use super::libgit2::Libgit2Engine;

const ENGINE: Libgit2Engine = Libgit2Engine;

// ---------------------------------------------------------------------------
// temp scaffolding
// ---------------------------------------------------------------------------

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let dir =
            std::env::temp_dir().join(format!("mygitui-submod-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create sandbox");
        Sandbox(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// The child repository that becomes the submodule (`<sandbox>/child`).
    fn child(&self) -> PathBuf {
        self.0.join("child")
    }

    /// The superproject (`<sandbox>/parent`).
    fn parent(&self) -> PathBuf {
        self.0.join("parent")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best-effort delayed cleanup: on Windows, repository handles can
        // outlive the drop that spawned the remover by a few milliseconds.
        let dir = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

/// Run git with an isolated identity and local-path submodule transports
/// allowed (git >= 2.38.1 gates `protocol.file` behind an explicit opt-in).
fn run_git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "core.autocrlf=false",
            "-c",
            "user.name=Sub Test",
            "-c",
            "user.email=sub@test.local",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Write a file, stage everything, commit on the current branch.
fn commit_file(dir: &Path, rel: &str, content: &str, message: &str) {
    let file = dir.join(rel);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "-m", message]);
}

/// Init both repos, give the child a commit, add it to the parent as `sub`.
/// `git submodule add` stages the gitlink + `.gitmodules` and checks the
/// child out into `<parent>/sub`. Returns the child's HEAD sha.
fn setup_with_submodule(name: &str) -> (Sandbox, String) {
    let sandbox = Sandbox::new(name);
    let child = sandbox.child();
    let parent = sandbox.parent();
    std::fs::create_dir_all(&child).expect("mkdir child");
    std::fs::create_dir_all(&parent).expect("mkdir parent");

    run_git(&child, &["init", "-b", "main"]);
    commit_file(&child, "lib.txt", "v1\n", "child v1");
    let child_head = head_sha(&child);

    run_git(&parent, &["init", "-b", "main"]);
    commit_file(&parent, "README.md", "super\n", "parent init");
    run_git(&parent, &["submodule", "add", "../child", "sub"]);
    (sandbox, child_head)
}

/// HEAD sha of the repository at `dir` (via the CLI, works for gitlinks too).
fn head_sha(dir: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("rev-parse");
    assert!(out.status.success(), "rev-parse failed in {dir:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn open(path: &Path) -> git2::Repository {
    git2::Repository::open(path).expect("open repo")
}

fn list(engine: &Libgit2Engine, repo: &git2::Repository) -> Vec<super::types::SubmoduleInfo> {
    engine.submodules_impl(repo).expect("submodules_impl")
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[test]
fn submodules_lists_added_submodule() {
    let (sandbox, child_head) = setup_with_submodule("list");
    let parent = open(&sandbox.parent());

    let subs = list(&ENGINE, &parent);
    assert_eq!(subs.len(), 1, "one submodule: {subs:?}");
    let sub = &subs[0];
    assert_eq!(sub.path, "sub");
    assert_eq!(sub.name, "sub");
    assert!(!sub.url.is_empty(), "url recorded from .gitmodules");
    assert!(sub.initialized, "freshly added submodule is initialized");
    // Freshly added: the gitlink in the index and the checked-out HEAD agree.
    assert_eq!(sub.recorded_sha, child_head);
    assert_eq!(sub.head_sha.as_deref(), Some(child_head.as_str()));
    assert_eq!(sub.status, "", "clean right after add");
}

#[test]
fn submodules_reports_new_commits() {
    let (sandbox, child_head) = setup_with_submodule("new-commits");
    // Advance the checked-out submodule past the recorded gitlink.
    let sub_dir = sandbox.parent().join("sub");
    commit_file(&sub_dir, "lib.txt", "v2\n", "child v2");

    let parent = open(&sandbox.parent());
    let subs = list(&ENGINE, &parent);
    assert_eq!(subs.len(), 1);
    let sub = &subs[0];
    assert_eq!(sub.recorded_sha, child_head, "index still records v1");
    assert_ne!(
        sub.head_sha.as_deref(),
        Some(child_head.as_str()),
        "worktree HEAD moved to v2"
    );
    assert!(sub.status.contains("new commits"), "got: {}", sub.status);
}

#[test]
fn submodule_update_moves_head_to_recorded_sha() {
    let (sandbox, child_head) = setup_with_submodule("update");
    let sub_dir = sandbox.parent().join("sub");
    commit_file(&sub_dir, "lib.txt", "v2\n", "child v2");

    let parent = open(&sandbox.parent());
    ENGINE
        .submodule_update_impl(&parent, "sub", false, false)
        .expect("submodule_update_impl");

    let subs = list(&ENGINE, &parent);
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].head_sha.as_deref(), Some(child_head.as_str()));
    assert_eq!(subs[0].status, "", "back at the recorded gitlink = clean");
}

#[test]
fn submodule_sync_rewrites_config_url() {
    let (sandbox, _child_head) = setup_with_submodule("sync");
    // Point .gitmodules at a new url, then let sync copy it into the repo
    // config. Absolute URLs are copied verbatim (relative ones get resolved
    // against remote/fallback bases, which is libgit2-internal).
    let base = sandbox.path().to_string_lossy().replace('\\', "/");
    let modules = sandbox.parent().join(".gitmodules");
    std::fs::write(
        &modules,
        format!("[submodule \"sub\"]\n\tpath = sub\n\turl = {base}/child-renamed\n"),
    )
    .expect("rewrite .gitmodules");

    let parent = open(&sandbox.parent());
    ENGINE.submodule_sync_impl(&parent, None).expect("sync all");

    let url = parent
        .config()
        .and_then(|c| c.get_string("submodule.sub.url"))
        .expect("config url after sync");
    assert_eq!(url, format!("{base}/child-renamed"));

    // Syncing one explicit path works too.
    std::fs::write(
        &modules,
        format!("[submodule \"sub\"]\n\tpath = sub\n\turl = {base}/child-third\n"),
    )
    .expect("rewrite .gitmodules");
    ENGINE
        .submodule_sync_impl(&parent, Some("sub"))
        .expect("sync one");
    let url = parent
        .config()
        .and_then(|c| c.get_string("submodule.sub.url"))
        .expect("config url after sync one");
    assert_eq!(url, format!("{base}/child-third"));
}

#[test]
fn submodule_update_unknown_path_errors() {
    let (sandbox, _child_head) = setup_with_submodule("unknown");
    let parent = open(&sandbox.parent());
    let err = ENGINE
        .submodule_update_impl(&parent, "nope", false, false)
        .expect_err("unknown submodule errors");
    assert!(err.to_string().contains("nope"), "got: {err}");
}

#[test]
fn deinitialized_submodule_reports_not_initialized() {
    let (sandbox, child_head) = setup_with_submodule("deinit");
    // `deinit -f` empties the submodule worktree (the clone stays in
    // .git/modules); the entry can then no longer be opened.
    run_git(&sandbox.parent(), &["submodule", "deinit", "-f", "sub"]);

    let parent = open(&sandbox.parent());
    let subs = list(&ENGINE, &parent);
    assert_eq!(subs.len(), 1);
    let sub = &subs[0];
    assert_eq!(sub.recorded_sha, child_head, "gitlink still recorded");
    assert!(!sub.initialized, "deinitialized = not openable");
    assert_ne!(sub.status, "", "open failure is surfaced, not silent");
}
