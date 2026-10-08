//! Bisect + autosquash + rebase-exec tests (M10). Throwaway repos, same
//! pattern as merge_tests.rs.

use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, RepositoryInitOptions};

use super::git_engine::{EngineError, GitEngine, GitEngineM3};
use super::libgit2::Libgit2Engine;
use super::types::{BisectMark, RebaseStep};

const ENGINE: Libgit2Engine = Libgit2Engine;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir =
            std::env::temp_dir().join(format!("mygitui-bisect-{}-{name}", std::process::id()));
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
        let dir = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

struct Fixture {
    repo: Repository,
    _dir: Option<TempDir>,
}

fn init_fixture(name: &str) -> Fixture {
    let dir = TempDir::new(name);
    let mut opts = RepositoryInitOptions::new();
    opts.bare(false).initial_head("main");
    let repo = Repository::init_opts(dir.path(), &opts).expect("init temp repo");
    repo.config()
        .and_then(|mut c| {
            c.set_str("user.name", "Bisect Test")?;
            c.set_str("user.email", "bisect@test.local")?;
            c.set_bool("core.autocrlf", false)
        })
        .expect("configure identity");
    Fixture {
        repo,
        _dir: Some(dir),
    }
}

fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) -> String {
    let file = repo.workdir().unwrap().join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
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

/// Linear fixture: c1..c5; `bug.txt` flips to "broken" at c4.
/// Returns (fixture, [c1..c5] shas).
fn linear_fixture(name: &str) -> (Fixture, [String; 5]) {
    let fx = init_fixture(name);
    let c1 = commit_file(&fx.repo, "app.txt", "one\n", "c1");
    let c2 = commit_file(&fx.repo, "app.txt", "two\n", "c2");
    let c3 = commit_file(&fx.repo, "app.txt", "three\n", "c3");
    let c4 = commit_file(&fx.repo, "bug.txt", "broken\n", "c4 introduces the bug");
    let c5 = commit_file(&fx.repo, "app.txt", "five\n", "c5");
    (fx, [c1, c2, c3, c4, c5])
}

#[test]
fn bisect_finds_first_bad_commit() {
    let (fx, [c1, _c2, c3, c4, c5]) = linear_fixture("find");

    let state = ENGINE
        .bisect_start(&fx.repo, Some(&c5), Some(&c1))
        .expect("start");
    assert!(state.active);
    assert_eq!(state.bad, c5);
    assert_eq!(state.good.as_deref(), Some(c1.as_str()));
    assert_eq!(state.remaining, 3, "candidates c2..c5 minus bounds");
    // Probe detached somewhere strictly between good and bad.
    let probe1 = head_sha(&fx.repo);
    assert_ne!(probe1, c1);
    assert_ne!(probe1, c5);
    let head_ref = fx.repo.head().unwrap();
    assert!(
        head_ref.symbolic_target().ok().flatten().is_none(),
        "probe runs detached"
    );

    // Mark the probe good → next probe must be at or after it.
    let state = ENGINE
        .bisect_mark(&fx.repo, BisectMark::Good)
        .expect("mark good");
    assert_eq!(state.good.as_deref(), Some(probe1.as_str()));
    let probe2 = head_sha(&fx.repo);
    assert_ne!(probe2, probe1, "probe moved");

    // Drive to exhaustion: mark each probe until remaining == 0. The good
    // test is walk membership: is the probe an ancestor of c3?
    let mut marks = 0;
    let mut state = state;
    while state.remaining > 0 && marks < 10 {
        let probe = head_sha(&fx.repo);
        let good = {
            let c3_oid = git2::Oid::from_str(&c3).unwrap();
            let probe_oid = git2::Oid::from_str(&probe).unwrap();
            let base = fx.repo.merge_base(c3_oid, probe_oid).unwrap();
            base == probe_oid
        };
        let _ = probe1;
        state = ENGINE
            .bisect_mark(
                &fx.repo,
                if good {
                    BisectMark::Good
                } else {
                    BisectMark::Bad
                },
            )
            .expect("mark");
        marks += 1;
    }
    assert_eq!(state.remaining, 0, "search exhausted");
    assert_eq!(state.bad, c4, "first bad commit found");
}

#[test]
fn bisect_skip_excludes_commit_and_reset_restores_branch() {
    let (fx, [c1, _c2, _c3, _c4, c5]) = linear_fixture("skip-reset");
    let orig_branch = head_sha(&fx.repo);
    assert_eq!(orig_branch, c5);

    ENGINE
        .bisect_start(&fx.repo, Some(&c5), Some(&c1))
        .expect("start");
    let probe = head_sha(&fx.repo);
    let state = ENGINE
        .bisect_mark(&fx.repo, BisectMark::Skip)
        .expect("skip");
    assert_eq!(state.skipped, vec![probe.clone()]);
    assert!(
        !state.remaining.to_string().is_empty(),
        "state reports remaining"
    );

    ENGINE.bisect_reset(&fx.repo).expect("reset");
    assert_eq!(head_sha(&fx.repo), c5, "back on main at c5");
    assert!(
        !fx.repo
            .head()
            .map(|h| h.kind() == Some(git2::ReferenceType::Symbolic))
            .unwrap_or(false),
        "re-attached"
    );
    assert!(!fx.repo.path().join("mygitui").join("bisect.json").exists());
    let state = ENGINE.bisect_state(&fx.repo).expect("state");
    assert!(!state.active);

    // Reset without an active bisect errors.
    match ENGINE.bisect_reset(&fx.repo) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("no bisect"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn bisect_start_guards() {
    let (fx, [c1, _c2, _c3, _c4, c5]) = linear_fixture("guards");

    // good == bad rejected.
    match ENGINE.bisect_start(&fx.repo, Some(&c5), Some(&c5)) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("same commit"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    // Unknown ref rejected.
    assert!(ENGINE.bisect_start(&fx.repo, Some("ghost"), None).is_err());

    // Start at HEAD when bad is None.
    let state = ENGINE
        .bisect_start(&fx.repo, None, Some(&c1))
        .expect("start from HEAD");
    assert_eq!(state.bad, c5);

    // Double start rejected.
    match ENGINE.bisect_start(&fx.repo, Some(&c5), Some(&c1)) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("already in progress"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn autosquash_plan_orders_fixups_after_targets() {
    let fx = init_fixture("autosquash");
    let base = commit_file(&fx.repo, "f.txt", "base\n", "base");
    let target = commit_file(&fx.repo, "f.txt", "fix target\n", "feat: widget");
    let unrelated = commit_file(&fx.repo, "g.txt", "other\n", "chore: other");
    let fixup = commit_file(&fx.repo, "f.txt", "fix target+\n", "fixup! feat: widget");

    let plan = ENGINE
        .autosquash_plan(&fx.repo, &base)
        .expect("autosquash plan");
    assert_eq!(plan.len(), 3, "{plan:?}");
    assert_eq!(plan[0].sha, target);
    assert_eq!(plan[0].action, "pick");
    assert_eq!(plan[1].sha, fixup);
    assert_eq!(plan[1].action, "fixup");
    assert_eq!(plan[1].new_message.as_deref(), Some("feat: widget"));
    assert_eq!(plan[2].sha, unrelated);
    assert_eq!(plan[2].action, "pick");

    // The plan runs through the real engine: history rewritten, fixup folded
    // (message = target's), unrelated commit preserved on top.
    let state = ENGINE
        .rebase_start(&fx.repo, &plan, None)
        .expect("rebase run");
    assert!(!state.active);
    let head = fx.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.summary().unwrap().unwrap(), "chore: other");
    let folded = head.parent(0).unwrap();
    assert_eq!(folded.summary().unwrap().unwrap(), "feat: widget");
    let entry = folded
        .tree()
        .unwrap()
        .get_path(std::path::Path::new("f.txt"))
        .unwrap();
    let blob = fx.repo.find_blob(entry.id()).unwrap();
    assert_eq!(blob.content(), b"fix target+\n", "fixup content folded in");
}

#[test]
fn rebase_exec_runs_and_pauses_on_failure() {
    let fx = init_fixture("exec");
    let base = commit_file(&fx.repo, "f.txt", "one\n", "base");
    let c1 = commit_file(&fx.repo, "f.txt", "two\n", "first");

    // Passing exec: appended between commits (runs, then pick continues).
    let plan = vec![
        RebaseStep {
            sha: c1.clone(),
            action: "pick".into(),
            new_message: None,
        },
        RebaseStep {
            sha: String::new(),
            action: "exec".into(),
            new_message: Some("echo ok > exec-marker.txt".into()),
        },
    ];
    let state = ENGINE.rebase_start(&fx.repo, &plan, None).expect("run");
    assert!(!state.active, "{state:?}");
    assert!(fx.repo.workdir().unwrap().join("exec-marker.txt").exists());

    // Failing exec pauses; continue re-runs (and fails again); abort restores.
    let plan = vec![RebaseStep {
        sha: String::new(),
        action: "exec".into(),
        new_message: Some("exit 3".into()),
    }];
    let before_failed = head_sha(&fx.repo);
    let state = ENGINE
        .rebase_start(&fx.repo, &plan, Some(&base))
        .expect("run");
    assert!(state.active);
    assert!(state.paused_for_exec);
    assert!(state.exec_error.as_deref().unwrap_or("").contains("exit"));
    let state = ENGINE.rebase_continue(&fx.repo).expect("continue");
    assert!(state.active, "still paused after a failing re-run");
    ENGINE.rebase_abort(&fx.repo).expect("abort");
    assert_eq!(
        head_sha(&fx.repo),
        before_failed,
        "abort restored pre-rebase HEAD"
    );

    // Empty exec command rejected at start.
    let bad = vec![RebaseStep {
        sha: String::new(),
        action: "exec".into(),
        new_message: Some("  ".into()),
    }];
    match ENGINE.rebase_start(&fx.repo, &bad, None) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("exec"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn describe_uses_tags_and_falls_back() {
    let fx = init_fixture("describe");
    let c1 = commit_file(&fx.repo, "f.txt", "one\n", "c1");
    let c2 = commit_file(&fx.repo, "f.txt", "two\n", "c2");
    ENGINE
        .tag_create(&fx.repo, "v1.0", None, Some("release"))
        .unwrap();
    let c3 = commit_file(&fx.repo, "f.txt", "three\n", "c3");

    // Ancestor of the tag: describe --tags walks FORWARD, so c1 has no tag
    // to reach — falls back to the short sha.
    let before = ENGINE.describe(&fx.repo, &c1).expect("describe c1");
    assert_eq!(before, &c1[..7], "fallback for pre-tag commits: {before}");
    // The tagged commit describes exactly.
    let at_tag = ENGINE.describe(&fx.repo, &c2).expect("describe c2");
    assert_eq!(at_tag, "v1.0", "exact tag match: {at_tag}");
    // After the tag: v1.0-N-g<short>.
    let after_tag = ENGINE.describe(&fx.repo, &c3).expect("describe c3");
    assert!(
        after_tag.starts_with("v1.0-") && after_tag.ends_with(&format!("-g{}", &c3[..7])),
        "describe --tags format: {after_tag}"
    );
}
