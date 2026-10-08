//! Network + branch/tag engine tests.
//!
//! Everything here runs against throwaway repositories under
//! `std::env::temp_dir()` using `file://` remotes only (no network, no
//! fixtures/) so parallel lanes can append to `tests.rs` safely.

use std::path::{Path, PathBuf};

use git2::{BranchType, IndexAddOption, Repository, RepositoryInitOptions};

use super::git_engine::{EngineError, FetchProgress, GitEngine, GitEngineM3, PushProgress};
use super::libgit2::Libgit2Engine;
use super::types::{FetchOptions, PullOptions, PushOptions};

const ENGINE: Libgit2Engine = Libgit2Engine;

// ---------------------------------------------------------------------------
// temp scaffolding
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("mygitui-net-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
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

/// Windows-safe file:// URL for a local path.
fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

fn init_repo(path: &Path, bare: bool) -> Repository {
    let mut opts = RepositoryInitOptions::new();
    opts.bare(bare).initial_head("main");
    let repo = Repository::init_opts(path, &opts).expect("init temp repo");
    if !bare {
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Net Test")?;
                c.set_str("user.email", "net@test.local")
            })
            .expect("configure identity");
    }
    repo
}

fn write_file(repo: &Repository, path: &str, content: &str) {
    let file = repo.workdir().unwrap().join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
}

/// Write + stage + commit on the current branch; returns the new sha.
fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) -> String {
    write_file(repo, path, content);
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let sig = repo.signature().expect("signature");
    let mut parents = Vec::new();
    if let Ok(head) = repo.head() {
        parents.push(head.peel_to_commit().expect("head commit"));
    }
    let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .expect("commit");
    oid.to_string()
}

fn head_sha(repo: &Repository) -> String {
    repo.head()
        .expect("head")
        .peel_to_commit()
        .expect("head commit")
        .id()
        .to_string()
}

/// Fixture-direct push bypassing the engine (setup must not depend on the
/// code under test).
fn raw_push(repo: &Repository, refspec: &str) {
    let mut remote = repo.find_remote("origin").expect("origin remote");
    remote.push(&[refspec], None).expect("raw push");
}

/// Bare `origin` + clone A (already pushed and tracking) + clone B, all on
/// `main` with one base commit.
struct NetFixture {
    origin: Repository,
    a: Repository,
    b: Repository,
    _dirs: Vec<TempDir>, // dropped last: repositories close before dirs delete
}

impl NetFixture {
    fn new(name: &str) -> NetFixture {
        let origin_dir = TempDir::new(&format!("{name}-origin"));
        let a_dir = TempDir::new(&format!("{name}-a"));
        let b_dir = TempDir::new(&format!("{name}-b"));

        let origin = init_repo(origin_dir.path(), true);
        let a = init_repo(a_dir.path(), false);
        a.remote("origin", &file_url(origin_dir.path()))
            .expect("add origin remote");
        commit_file(&a, "readme.md", "base\n", "base commit");
        raw_push(&a, "refs/heads/main:refs/heads/main");
        // A was not cloned: configure its upstream by hand and seed the
        // tracking ref with one direct fetch.
        a.config()
            .and_then(|mut c| {
                c.set_str("branch.main.remote", "origin")?;
                c.set_str("branch.main.merge", "refs/heads/main")
            })
            .expect("configure upstream A");
        let mut a_remote = a.find_remote("origin").expect("origin remote A");
        a_remote
            .fetch(&[] as &[&str], None, None)
            .expect("seed fetch A");
        drop(a_remote); // Remote's Drop keeps borrowing `a` until here

        let b = Repository::clone(&file_url(origin_dir.path()), b_dir.path()).expect("clone B");
        b.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Net Test")?;
                c.set_str("user.email", "net@test.local")
            })
            .expect("configure identity B");

        NetFixture {
            origin,
            a,
            b,
            _dirs: vec![origin_dir, a_dir, b_dir],
        }
    }
}

fn fetch_opts(prune: bool) -> FetchOptions {
    FetchOptions {
        remote: "origin".into(),
        prune,
        ..Default::default()
    }
}

fn pull_opts() -> PullOptions {
    PullOptions {
        remote: "origin".into(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// branch listing + guards
// ---------------------------------------------------------------------------

#[test]
fn branches_report_upstream_ahead_behind_and_gone() {
    let f = NetFixture::new("branches-info");

    let list = ENGINE.branches(&f.a).expect("branches");
    assert_eq!(list.len(), 1);
    assert_eq!(
        (
            list[0].name.as_str(),
            list[0].upstream.as_deref(),
            list[0].is_head,
            list[0].gone
        ),
        ("main", Some("origin/main"), true, false)
    );
    assert_eq!((list[0].ahead, list[0].behind), (0, 0));
    assert_eq!(list[0].sha, head_sha(&f.a));

    // Local-only commit → ahead 1.
    commit_file(&f.a, "a.md", "local\n", "a local");
    let list = ENGINE.branches(&f.a).unwrap();
    assert_eq!((list[0].ahead, list[0].behind), (1, 0));

    // Remote moves independently → diverged 1/1 after a fetch.
    commit_file(&f.b, "b.md", "remote\n", "b remote");
    raw_push(&f.b, "refs/heads/main:refs/heads/main");
    ENGINE.fetch(&f.a, &fetch_opts(false), &mut |_| {}).unwrap();
    let list = ENGINE.branches(&f.a).unwrap();
    assert_eq!((list[0].ahead, list[0].behind), (1, 1));

    // Upstream ref vanishes while the config stays → gone (0/0 numbers).
    let mut tracking =
        f.a.find_reference("refs/remotes/origin/main")
            .expect("tracking ref");
    tracking.delete().expect("delete tracking ref");
    let list = ENGINE.branches(&f.a).unwrap();
    assert_eq!(list[0].upstream.as_deref(), Some("origin/main"));
    assert!(list[0].gone, "tracking ref deleted ⇒ gone");
    assert_eq!((list[0].ahead, list[0].behind), (0, 0));
}

#[test]
fn branch_create_switch_rename_delete_guards() {
    let f = NetFixture::new("branch-ops");
    let base = commit_file(&f.a, "one.md", "one\n", "one");
    commit_file(&f.a, "two.md", "two\n", "two");

    let names = |repo: &Repository| -> Vec<String> {
        ENGINE
            .branches(repo)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect()
    };

    // Create without checkout: HEAD stays on main.
    ENGINE.branch_create(&f.a, "feature", None, false).unwrap();
    assert_eq!(names(&f.a), ["feature", "main"]);
    assert!(!ENGINE.branches(&f.a).unwrap()[0].is_head);

    // Duplicate name → Invalid.
    match ENGINE.branch_create(&f.a, "feature", None, false) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("already exists"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }

    // Create with checkout from an older commit: workdir follows.
    ENGINE
        .branch_create(&f.a, "old", Some(&base), true)
        .unwrap();
    assert_eq!(ENGINE.status(&f.a).unwrap().branch.as_deref(), Some("old"));
    assert!(
        !f.a.workdir().unwrap().join("two.md").exists(),
        "workdir rolled back to the branch point"
    );

    // Safe switch back restores two.md; "old" is merged (ancestor of main).
    ENGINE.branch_switch(&f.a, "main", false).unwrap();
    assert!(f.a.workdir().unwrap().join("two.md").exists());
    assert!(ENGINE.branch_is_merged(&f.a, "old", "main").unwrap());
    ENGINE.branch_delete(&f.a, "old", false).unwrap();

    // Missing branch switch → Invalid not-found.
    match ENGINE.branch_switch(&f.a, "nope", false) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("not found"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }

    // Deleting the current branch is refused even with force.
    for force in [false, true] {
        match ENGINE.branch_delete(&f.a, "main", force) {
            Err(EngineError::Invalid(msg)) => {
                assert!(msg.contains("current branch"), "{msg}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    // Unmerged branch: commit on feature, then try deleting from main.
    ENGINE.branch_switch(&f.a, "feature", false).unwrap();
    commit_file(&f.a, "feature.md", "feat\n", "feature work");
    ENGINE.branch_switch(&f.a, "main", false).unwrap();
    assert!(!ENGINE.branch_is_merged(&f.a, "feature", "main").unwrap());
    match ENGINE.branch_delete(&f.a, "feature", false) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("not fully merged"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    ENGINE.branch_delete(&f.a, "feature", true).unwrap();
    assert_eq!(names(&f.a), ["main"]);

    // Rename follows HEAD when renaming the current branch.
    ENGINE.branch_create(&f.a, "topic", None, false).unwrap();
    ENGINE.branch_switch(&f.a, "topic", false).unwrap();
    ENGINE.branch_rename(&f.a, "topic", "topic2").unwrap();
    assert_eq!(
        ENGINE.status(&f.a).unwrap().branch.as_deref(),
        Some("topic2")
    );
    assert_eq!(names(&f.a), ["main", "topic2"]);
    // Renaming a missing branch errors.
    assert!(ENGINE.branch_rename(&f.a, "ghost", "ghost2").is_err());
}

#[test]
fn tags_create_and_delete_annotated_and_lightweight() {
    let f = NetFixture::new("tags");
    let base = commit_file(&f.a, "a.md", "1\n", "c1");
    commit_file(&f.a, "a.md", "2\n", "c2");
    let head = head_sha(&f.a);

    // Annotated (default target HEAD, explicit target) + lightweight variants.
    ENGINE
        .tag_create(&f.a, "v1.0", None, Some("first release\n\nnotes"))
        .unwrap();
    ENGINE
        .tag_create(&f.a, "v1.0-base", Some(&base), Some("base point"))
        .unwrap();
    ENGINE.tag_create(&f.a, "lw", None, None).unwrap();
    // Blank message counts as lightweight.
    ENGINE
        .tag_create(&f.a, "lw-base", Some(&base), Some("   "))
        .unwrap();

    let refs = ENGINE.refs(&f.a).unwrap();
    let sha_of = |name: &str| {
        refs.iter()
            .find(|(r, _)| r == name)
            .unwrap_or_else(|| panic!("tag {name} missing from {refs:?}"))
            .1
            .clone()
    };
    // refs() peels tags to commits: both HEAD tags land on the tip.
    assert_eq!(sha_of("v1.0"), head);
    assert_eq!(sha_of("lw"), head);
    assert_eq!(sha_of("v1.0-base"), base);

    // Annotated tags create a real tag object; lightweight ones point at the
    // commit directly.
    let v1 = f.a.revparse_single("v1.0").unwrap();
    let tag = v1.as_tag().expect("v1.0 is annotated").to_owned();
    assert_eq!(tag.name().unwrap(), "v1.0");
    assert_eq!(tag.message().ok().flatten(), Some("first release\n\nnotes"));
    assert_eq!(tag.tagger().unwrap().name().unwrap(), "Net Test");
    assert!(f.a.revparse_single("lw").unwrap().as_tag().is_none());
    assert!(
        f.a.revparse_single("lw-base")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
            == base
    );

    // Duplicate tag errors.
    assert!(ENGINE
        .tag_create(&f.a, "v1.0", None, Some("again"))
        .is_err());

    // Delete + missing-tag guard.
    ENGINE.tag_delete(&f.a, "v1.0").unwrap();
    assert!(ENGINE.refs(&f.a).unwrap().iter().all(|(r, _)| r != "v1.0"));
    match ENGINE.tag_delete(&f.a, "v1.0") {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("not found"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// remotes
// ---------------------------------------------------------------------------

#[test]
fn remotes_list_add_set_url_remove() {
    let f = NetFixture::new("remotes");

    let remotes = ENGINE.remotes(&f.a).unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "origin");
    assert!(
        remotes[0].url.starts_with("file://"),
        "{:?}",
        remotes[0].url
    );
    assert_eq!(remotes[0].push_url, None);

    let other = TempDir::new("remotes-other");
    let url = file_url(other.path());
    ENGINE.remote_add(&f.a, "upstream", &url).unwrap();
    assert!(ENGINE.remote_add(&f.a, "upstream", &url).is_err());

    let find = |repo: &Repository, name: &str| {
        ENGINE
            .remotes(repo)
            .unwrap()
            .into_iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("remote {name} missing"))
    };
    assert_eq!(find(&f.a, "upstream").url, url);

    // Fetch url, then push url only.
    let url2 = format!("{url}-alt");
    ENGINE
        .remote_set_url(&f.a, "upstream", &url2, false)
        .unwrap();
    ENGINE
        .remote_set_url(&f.a, "upstream", "file:///push-target", true)
        .unwrap();
    let up = find(&f.a, "upstream");
    assert_eq!(up.url, url2);
    assert_eq!(up.push_url.as_deref(), Some("file:///push-target"));

    // Remove.
    ENGINE.remote_remove(&f.a, "upstream").unwrap();
    let remotes = ENGINE.remotes(&f.a).unwrap();
    assert_eq!(remotes.len(), 1);
    assert!(remotes.iter().all(|r| r.name != "upstream"));
    assert!(ENGINE.remote_remove(&f.a, "upstream").is_err());
    assert!(ENGINE
        .remote_set_url(&f.a, "upstream", url.as_str(), false)
        .is_err());
}

// ---------------------------------------------------------------------------
// fetch / progress / prune
// ---------------------------------------------------------------------------

#[test]
fn fetch_updates_tracking_refs_and_prunes_deletions() {
    let f = NetFixture::new("fetch-prune");

    // B pushes a topic branch; A fetches it via the default refspec.
    ENGINE.branch_create(&f.b, "topic", None, false).unwrap();
    ENGINE.branch_switch(&f.b, "topic", false).unwrap();
    let topic_tip = commit_file(&f.b, "topic.md", "t\n", "topic work");
    raw_push(&f.b, "refs/heads/topic:refs/heads/topic");

    let stats = ENGINE.fetch(&f.a, &fetch_opts(false), &mut |_| {}).unwrap();
    assert!(
        stats
            .updated_refs
            .iter()
            .any(|(r, s)| r == "refs/remotes/origin/topic" && s == &topic_tip),
        "{:?}",
        stats.updated_refs
    );
    assert!(ENGINE
        .refs(&f.a)
        .unwrap()
        .iter()
        .any(|(r, _)| r == "origin/topic"));

    // A local branch tracking origin/topic (for the gone check below).
    ENGINE
        .branch_create(&f.a, "topic", Some("origin/topic"), false)
        .unwrap();
    let mut local_topic = f.a.find_branch("topic", BranchType::Local).unwrap();
    local_topic.set_upstream(Some("origin/topic")).unwrap();
    drop(local_topic);

    // Delete the branch on the remote.
    raw_push(&f.b, ":refs/heads/topic");

    // Without prune the stale tracking ref survives; with prune it goes.
    ENGINE.fetch(&f.a, &fetch_opts(false), &mut |_| {}).unwrap();
    assert!(ENGINE
        .refs(&f.a)
        .unwrap()
        .iter()
        .any(|(r, _)| r == "origin/topic"));
    ENGINE.fetch(&f.a, &fetch_opts(true), &mut |_| {}).unwrap();
    assert!(
        !ENGINE
            .refs(&f.a)
            .unwrap()
            .iter()
            .any(|(r, _)| r == "origin/topic"),
        "prune must drop the deleted remote branch"
    );

    // And the branch panel reports it as gone.
    let topic = ENGINE
        .branches(&f.a)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "topic")
        .unwrap();
    assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));
    assert!(topic.gone);
}

#[test]
fn fetch_emits_progress_with_final_done_tick() {
    let f = NetFixture::new("fetch-progress");
    commit_file(&f.b, "p.md", "payload\n", "remote work");
    raw_push(&f.b, "refs/heads/main:refs/heads/main");

    let mut events: Vec<FetchProgress> = Vec::new();
    ENGINE
        .fetch(&f.a, &fetch_opts(false), &mut |p| events.push(p))
        .unwrap();
    assert!(!events.is_empty(), "at least the final tick must fire");
    assert!(events.last().unwrap().done, "final tick is done=true");

    // Up-to-date fetch still reports a final tick.
    events.clear();
    ENGINE
        .fetch(&f.a, &fetch_opts(false), &mut |p| events.push(p))
        .unwrap();
    assert!(!events.is_empty());
    assert!(events.last().unwrap().done);

    // Fetching a missing remote errors with Invalid.
    let bad = FetchOptions {
        remote: "nowhere".into(),
        ..Default::default()
    };
    match ENGINE.fetch(&f.a, &bad, &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("nowhere"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pull
// ---------------------------------------------------------------------------

#[test]
fn pull_fast_forwards_cleanly() {
    let f = NetFixture::new("pull-ff");
    let pushed = {
        commit_file(&f.b, "ff.md", "ff\n", "remote advance");
        raw_push(&f.b, "refs/heads/main:refs/heads/main");
        head_sha(&f.b)
    };

    let opts = pull_opts();
    let stats = ENGINE.pull(&f.a, &opts, &mut |_| {}).unwrap();
    assert_eq!(head_sha(&f.a), pushed, "fast-forward moved the branch");
    assert!(f.a.workdir().unwrap().join("ff.md").exists());
    // No merge commit: single parent.
    assert_eq!(
        f.a.head().unwrap().peel_to_commit().unwrap().parent_count(),
        1
    );
    assert!(
        stats
            .updated_refs
            .iter()
            .any(|(r, _)| r == "refs/remotes/origin/main"),
        "{:?}",
        stats.updated_refs
    );

    // Up-to-date pull is a no-op.
    ENGINE.pull(&f.a, &opts, &mut |_| {}).unwrap();
    assert_eq!(head_sha(&f.a), pushed);
}

#[test]
fn pull_diverged_creates_merge_commit_by_default() {
    let f = NetFixture::new("pull-merge");
    commit_file(&f.a, "local.md", "local change\n", "a local");
    let remote_tip = {
        commit_file(&f.b, "remote.md", "remote change\n", "b remote");
        raw_push(&f.b, "refs/heads/main:refs/heads/main");
        head_sha(&f.b)
    };

    let opts = pull_opts();
    ENGINE.pull(&f.a, &opts, &mut |_| {}).unwrap();

    let commit = f.a.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.parent_count(), 2, "merge commit created");
    assert!(
        commit
            .summary()
            .ok()
            .flatten()
            .unwrap()
            .starts_with("Merge branch 'main' of origin"),
        "{}",
        commit.summary().unwrap().unwrap()
    );
    assert_eq!(commit.parent_id(1).unwrap().to_string(), remote_tip);
    // Merge state cleaned up; the local branch is 2 ahead of its upstream
    // (local commit + merge commit) with nothing left behind.
    let status = ENGINE.status(&f.a).unwrap();
    assert!(!status.merging && !status.rebasing);
    assert_eq!((status.ahead, status.behind), (2, 0));
    // Both sides' files landed in the workdir.
    assert!(f.a.workdir().unwrap().join("local.md").exists());
    assert!(f.a.workdir().unwrap().join("remote.md").exists());
}

#[test]
fn pull_ff_only_rejects_diverged_histories() {
    let f = NetFixture::new("pull-ff-only");
    commit_file(&f.a, "local.md", "l\n", "a local");
    commit_file(&f.b, "remote.md", "r\n", "b remote");
    raw_push(&f.b, "refs/heads/main:refs/heads/main");
    let before = head_sha(&f.a);

    let opts = PullOptions {
        ff_only: true,
        ..pull_opts()
    };
    match ENGINE.pull(&f.a, &opts, &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => {
            assert!(msg.to_lowercase().contains("fast-forward"), "{msg}")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert_eq!(head_sha(&f.a), before, "ff-only failure must not move HEAD");

    // The same history merges fine without ff_only (sanity for the guard).
    ENGINE.pull(&f.a, &pull_opts(), &mut |_| {}).unwrap();
    assert_eq!(
        f.a.head().unwrap().peel_to_commit().unwrap().parent_count(),
        2
    );
}

#[test]
fn pull_rebase_diverged_replays_local_commits() {
    let f = NetFixture::new("pull-rebase");
    commit_file(&f.a, "local.md", "l\n", "a local");
    let remote_tip = {
        commit_file(&f.b, "remote.md", "r\n", "b remote");
        raw_push(&f.b, "refs/heads/main:refs/heads/main");
        head_sha(&f.b)
    };

    let opts = PullOptions {
        rebase: true,
        ..pull_opts()
    };
    ENGINE.pull(&f.a, &opts, &mut |_| {}).unwrap();

    // The local commit replays on top of the fetched tip: linear history.
    let head = f.a.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(
        head.parent_count(),
        1,
        "rebase must not create a merge commit"
    );
    assert_eq!(head.parent(0).unwrap().id().to_string(), remote_tip);
    assert_eq!(head.summary().ok(), Some(Some("a local")));
    assert!(f.a.workdir().unwrap().join("local.md").exists());
    assert!(f.a.workdir().unwrap().join("remote.md").exists());
    let status = ENGINE.status(&f.a).unwrap();
    assert!(!status.merging && !status.rebasing);
    assert_eq!((status.ahead, status.behind), (1, 0));
}

#[test]
fn pull_rebase_conflict_pauses_in_rebase_state_and_aborts_cleanly() {
    let f = NetFixture::new("pull-rebase-conflict");
    let before = commit_file(&f.a, "shared.md", "base\nlocal\n", "a local");
    commit_file(&f.b, "shared.md", "base\nremote\n", "b remote");
    raw_push(&f.b, "refs/heads/main:refs/heads/main");

    let opts = PullOptions {
        rebase: true,
        ..pull_opts()
    };
    match ENGINE.pull(&f.a, &opts, &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => {
            assert!(msg.contains("conflict"), "{msg}")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }

    // The rebase is paused exactly like an interactive rebase: state live,
    // conflicts exposed for the FE editor.
    let status = ENGINE.status(&f.a).unwrap();
    assert!(status.rebasing, "rebase state active after conflicted pull");
    assert!(
        ENGINE
            .conflicts(&f.a)
            .expect("conflicts")
            .iter()
            .any(|c| c.path == "shared.md"),
        "conflicted path exposed"
    );

    // Abort restores the pre-pull local state.
    ENGINE.rebase_abort(&f.a).expect("rebase abort");
    assert_eq!(head_sha(&f.a), before);
    let status = ENGINE.status(&f.a).unwrap();
    assert!(!status.rebasing && status.entries.is_empty());
}

#[test]
fn pull_conflict_leaves_merge_state_for_resolution() {
    let f = NetFixture::new("pull-conflict");
    commit_file(&f.a, "shared.md", "base\nlocal\n", "a local");
    commit_file(&f.b, "shared.md", "base\nremote\n", "b remote");
    raw_push(&f.b, "refs/heads/main:refs/heads/main");
    let before = head_sha(&f.a);

    match ENGINE.pull(&f.a, &pull_opts(), &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("conflict"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    // HEAD unmoved, merge state exposed for the (M3) resolution UI.
    assert_eq!(head_sha(&f.a), before);
    let status = ENGINE.status(&f.a).unwrap();
    assert!(status.merging, "MERGE_HEAD present after conflicted pull");
}

#[test]
fn pull_missing_remote_branch_errors() {
    let f = NetFixture::new("pull-missing");
    let opts = PullOptions {
        branch: "ghost".into(),
        ..pull_opts()
    };
    // Either the transport rejects the unmatched refspec or the post-fetch
    // tracking-ref lookup fails; both must name the branch.
    let err = ENGINE
        .pull(&f.a, &opts, &mut |_| {})
        .expect_err("pulling a missing branch must fail");
    let msg = format!("{err}");
    assert!(msg.contains("ghost"), "{msg}");
}

// ---------------------------------------------------------------------------
// push
// ---------------------------------------------------------------------------

#[test]
fn push_branch_sets_upstream_and_reports_progress() {
    let f = NetFixture::new("push-upstream");
    ENGINE.branch_create(&f.a, "feature", None, false).unwrap();
    ENGINE.branch_switch(&f.a, "feature", false).unwrap();
    commit_file(&f.a, "feat.md", "f\n", "feature work");
    let tip = head_sha(&f.a);

    let opts = PushOptions {
        remote: "origin".into(),
        branch: "feature".into(),
        set_upstream: true,
        ..Default::default()
    };
    let mut events: Vec<PushProgress> = Vec::new();
    let stats = ENGINE.push(&f.a, &opts, &mut |p| events.push(p)).unwrap();

    // Remote gained the branch at the local tip.
    assert_eq!(
        f.origin
            .find_reference("refs/heads/feature")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        tip
    );
    assert_eq!(
        stats.updated_refs,
        vec![("refs/heads/feature".into(), tip.clone())]
    );
    assert!(!events.is_empty());
    assert_eq!(events.last().unwrap().message, "done");

    // Upstream now configured: 0/0 divergence.
    let feature = ENGINE
        .branches(&f.a)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "feature")
        .unwrap();
    assert_eq!(feature.upstream.as_deref(), Some("origin/feature"));
    assert_eq!((feature.ahead, feature.behind), (0, 0));

    // Pushing a missing branch errors.
    let bad = PushOptions {
        remote: "origin".into(),
        branch: "ghost".into(),
        ..Default::default()
    };
    match ENGINE.push(&f.a, &bad, &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("ghost"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn push_force_overwrites_non_fast_forward() {
    let f = NetFixture::new("push-force");
    let first = commit_file(&f.a, "h.txt", "one\n", "one");
    let pushed = commit_file(&f.a, "h.txt", "one\ntwo\n", "two");
    raw_push(&f.a, "refs/heads/main:refs/heads/main");

    // Rewrite local history: reset to `first`, build a different successor.
    let old =
        f.a.find_commit(git2::Oid::from_str(&first).unwrap())
            .unwrap();
    f.a.reset(old.as_object(), git2::ResetType::Hard, None)
        .expect("reset");
    let rewritten = commit_file(&f.a, "h.txt", "one\ntwo-rewritten\n", "two rewritten");
    assert_ne!(rewritten, pushed);

    // Non-force push of a non-fast-forward is rejected.
    let opts = PushOptions {
        remote: "origin".into(),
        branch: "main".into(),
        ..Default::default()
    };
    match ENGINE.push(&f.a, &opts, &mut |_| {}) {
        Err(EngineError::Git(err)) => {
            let msg = err.to_string().to_lowercase();
            assert!(msg.contains("fast"), "{msg}");
        }
        other => panic!("expected rejected push, got {other:?}"),
    }
    // Remote still holds the original history.
    let remote_main = || {
        f.origin
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
    };
    assert_eq!(remote_main(), pushed);

    // Force push succeeds and the remote lands on the rewritten commit.
    let opts = PushOptions {
        force: true,
        ..opts
    };
    ENGINE.push(&f.a, &opts, &mut |_| {}).unwrap();
    assert_eq!(remote_main(), rewritten);

    // A fetch elsewhere follows the rewind (forced tracking-ref update).
    ENGINE.fetch(&f.b, &fetch_opts(false), &mut |_| {}).unwrap();
    assert!(
        ENGINE
            .refs(&f.b)
            .unwrap()
            .iter()
            .any(|(r, s)| r == "origin/main" && s == &rewritten),
        "tracking ref must follow rewritten remote history"
    );
}

// ---------------------------------------------------------------------------
// push: refs / tags / delete / force-with-lease
// ---------------------------------------------------------------------------

#[test]
fn push_tags_publishes_all_local_tags() {
    let f = NetFixture::new("push-tags");
    let head = head_sha(&f.a);
    ENGINE
        .tag_create(&f.a, "v1.0", None, Some("first release"))
        .unwrap();
    ENGINE
        .tag_create(&f.a, "v1.1", None, Some("second"))
        .unwrap();
    ENGINE.tag_create(&f.a, "lw", None, None).unwrap();

    let opts = PushOptions {
        remote: "origin".into(),
        tags: true,
        ..Default::default()
    };
    let stats = ENGINE.push(&f.a, &opts, &mut |_| {}).unwrap();

    let remote_tag = |name: &str| {
        f.origin
            .find_reference(&format!("refs/tags/{name}"))
            .unwrap_or_else(|e| panic!("tag {name} on origin: {e}"))
    };
    assert_eq!(
        remote_tag("v1.0")
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        head
    );
    assert_eq!(
        remote_tag("lw").peel_to_commit().unwrap().id().to_string(),
        head
    );
    assert_eq!(stats.updated_refs.len(), 3, "{:?}", stats.updated_refs);
}

#[test]
fn push_explicit_refs_and_delete_remote_branch() {
    let f = NetFixture::new("push-refs-delete");
    ENGINE.branch_create(&f.a, "feature", None, false).unwrap();
    ENGINE.branch_switch(&f.a, "feature", false).unwrap();
    let tip = commit_file(&f.a, "feat.md", "f\n", "feature work");

    // Explicit ref push (bare branch name in `refs`).
    let opts = PushOptions {
        remote: "origin".into(),
        refs: vec!["feature".into()],
        ..Default::default()
    };
    ENGINE.push(&f.a, &opts, &mut |_| {}).unwrap();
    assert_eq!(
        f.origin
            .find_reference("refs/heads/feature")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        tip
    );

    // Delete the remote branch (leave the local one intact).
    let opts = PushOptions {
        remote: "origin".into(),
        refs: vec!["feature".into()],
        delete: true,
        ..Default::default()
    };
    let stats = ENGINE.push(&f.a, &opts, &mut |_| {}).unwrap();
    assert!(
        f.origin.find_reference("refs/heads/feature").is_err(),
        "remote branch deleted"
    );
    assert!(
        stats
            .updated_refs
            .iter()
            .any(|(r, s)| r == "refs/heads/feature" && s == "(deleted)"),
        "{:?}",
        stats.updated_refs
    );
    assert!(
        f.a.find_branch("feature", BranchType::Local).is_ok(),
        "local branch survives"
    );

    // Delete without refs is a usage error.
    let bad = PushOptions {
        remote: "origin".into(),
        delete: true,
        ..Default::default()
    };
    match ENGINE.push(&f.a, &bad, &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("delete"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn push_force_with_lease_requires_tracking_ref_then_forces() {
    let f = NetFixture::new("push-lease");
    let first = commit_file(&f.a, "h.txt", "one\n", "one");
    let _pushed = commit_file(&f.a, "h.txt", "one\ntwo\n", "two");
    raw_push(&f.a, "refs/heads/main:refs/heads/main");

    // Rewrite local history so the push is non-fast-forward.
    let old =
        f.a.find_commit(git2::Oid::from_str(&first).unwrap())
            .unwrap();
    f.a.reset(old.as_object(), git2::ResetType::Hard, None)
        .unwrap();
    let rewritten = commit_file(&f.a, "h.txt", "one\ntwo-rewritten\n", "two rewritten");

    // Drop the tracking ref: lease must refuse (we would be blind-forcing).
    f.a.find_reference("refs/remotes/origin/main")
        .unwrap()
        .delete()
        .unwrap();
    let opts = PushOptions {
        remote: "origin".into(),
        branch: "main".into(),
        force_with_lease: true,
        ..Default::default()
    };
    match ENGINE.push(&f.a, &opts, &mut |_| {}) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("lease"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }

    // With the tracking ref present (a fetch happened), lease forces through.
    let mut a_remote = f.a.find_remote("origin").unwrap();
    a_remote.fetch(&[] as &[&str], None, None).unwrap();
    drop(a_remote);
    ENGINE.push(&f.a, &opts, &mut |_| {}).unwrap();
    assert_eq!(
        f.origin
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        rewritten
    );
}

#[test]
fn tags_list_reports_annotated_and_lightweight_metadata() {
    let f = NetFixture::new("tags-list");
    let base = commit_file(&f.a, "a.md", "1\n", "c1");
    commit_file(&f.a, "a.md", "2\n", "c2");
    let head = head_sha(&f.a);
    ENGINE
        .tag_create(&f.a, "v1.0", None, Some("first release\n\nnotes"))
        .unwrap();
    ENGINE.tag_create(&f.a, "lw", Some(&base), None).unwrap();

    let tags = ENGINE.tag_list(&f.a).expect("tag_list");
    assert_eq!(tags.len(), 2, "{tags:?}");
    assert_eq!(tags[0].name, "lw");
    assert!(!tags[0].annotated);
    assert_eq!(tags[0].target, base);
    assert_eq!(tags[0].sha, base, "lightweight: ref points at the commit");
    assert_eq!(tags[0].tagger, None);
    assert_eq!(tags[0].message, None);

    assert_eq!(tags[1].name, "v1.0");
    assert!(tags[1].annotated);
    assert_eq!(tags[1].target, head);
    assert_ne!(tags[1].sha, head, "annotated: ref points at the tag object");
    let tagger = tags[1].tagger.as_ref().expect("tagger");
    assert_eq!(tagger.name, "Net Test");
    assert_eq!(tags[1].message.as_deref(), Some("first release\n\nnotes"));
}

#[test]
fn remote_branches_and_checkout_creates_tracking_local() {
    let f = NetFixture::new("remote-checkout");
    // B pushes a topic branch; A fetches it.
    ENGINE.branch_create(&f.b, "topic", None, false).unwrap();
    ENGINE.branch_switch(&f.b, "topic", false).unwrap();
    let topic_tip = commit_file(&f.b, "topic.md", "t\n", "topic work");
    raw_push(&f.b, "refs/heads/topic:refs/heads/topic");
    ENGINE.fetch(&f.a, &fetch_opts(false), &mut |_| {}).unwrap();

    let remotes = ENGINE.remote_branches(&f.a).expect("remote_branches");
    let find = |name: &str| {
        remotes
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {remotes:?}"))
    };
    assert_eq!(
        (
            find("main").remote.as_str(),
            find("main").tracked_by.as_deref()
        ),
        ("origin", Some("main"))
    );
    assert_eq!(
        (
            find("topic").remote.as_str(),
            find("topic").tracked_by.as_deref()
        ),
        ("origin", None)
    );
    assert_eq!(find("topic").sha, topic_tip);

    // Checkout creates the local branch at the tracking tip with upstream set.
    let local = ENGINE
        .branch_checkout_remote(&f.a, "origin", "topic", None)
        .expect("checkout remote branch");
    assert_eq!(local, "topic");
    assert_eq!(
        ENGINE.status(&f.a).unwrap().branch.as_deref(),
        Some("topic")
    );
    assert!(f.a.workdir().unwrap().join("topic.md").exists());
    let topic = ENGINE
        .branches(&f.a)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "topic")
        .unwrap();
    assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));
    assert_eq!(topic.sha, topic_tip);

    // Existing local name → Invalid; custom name works.
    match ENGINE.branch_checkout_remote(&f.a, "origin", "topic", None) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("already exists"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    let renamed = ENGINE
        .branch_checkout_remote(&f.a, "origin", "topic", Some("topic-local"))
        .expect("checkout with custom name");
    assert_eq!(renamed, "topic-local");
    let custom = ENGINE
        .branches(&f.a)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "topic-local")
        .unwrap();
    assert_eq!(custom.upstream.as_deref(), Some("origin/topic"));

    // Missing remote branch → Invalid.
    match ENGINE.branch_checkout_remote(&f.a, "origin", "ghost", None) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("not found"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}
