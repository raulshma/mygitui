//! Interactive-rebase engine tests (lane D2). Temp repos only — every test
//! builds its own commit chain and rewrites it via the rebase sequencer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use git2::{IndexAddOption, Oid, Repository};

use super::git_engine::EngineError;
use super::libgit2::Libgit2Engine;
use super::types::RebaseStep;

const ENGINE: Libgit2Engine = Libgit2Engine;

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

struct TempRepo {
    _dir: PathBuf,
    repo: Repository,
}

impl TempRepo {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("mygitui-rebase-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let repo = Repository::init(&dir).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Rebase Test")?;
                c.set_str("user.email", "rebase@test.local")?;
                // Pin line-ending behavior: a global core.autocrlf would leak
                // into checkout output and make content asserts flaky.
                c.set_str("core.autocrlf", "false")
            })
            .expect("configure identity");
        TempRepo { _dir: dir, repo }
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.repo.workdir().unwrap().join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(file, content).expect("write file");
    }

    fn commit(&self, message: &str) -> String {
        let mut index = self.repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = self.repo.find_tree(tree_oid).expect("tree");
        // Fixed, increasing timestamps: replayed commits always carry the
        // current committer time, so they can never byte-identically collide
        // with an original commit (git object dedup would map old == new).
        static TICK: AtomicI64 = AtomicI64::new(1_700_000_000);
        let t = TICK.fetch_add(60, Ordering::SeqCst);
        let sig = git2::Signature::new("Rebase Test", "rebase@test.local", &git2::Time::new(t, 0))
            .expect("signature");
        let parents: Vec<git2::Commit<'_>> = match self.repo.head() {
            Ok(head) => vec![head.peel_to_commit().expect("head commit")],
            Err(_) => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
            .to_string()
    }

    fn stage(&self, path: &str) {
        let mut index = self.repo.index().expect("index");
        index.add_path(Path::new(path)).expect("stage add");
        index.write().expect("stage write");
    }

    fn head_sha(&self) -> String {
        self.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
    }

    fn head_message(&self) -> String {
        self.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap_or_default()
            .to_owned()
    }

    fn branch_at(&self, name: &str, sha: &str) {
        let commit = self.repo.find_commit(oid(sha)).expect("branch base");
        self.repo.branch(name, &commit, false).expect("branch");
    }

    fn set_head_ref(&self, full_name: &str) {
        self.repo.set_head(full_name).expect("set head");
    }

    fn checkout(&self, sha: &str) {
        let commit = self.repo.find_commit(oid(sha)).expect("checkout commit");
        let mut opts = git2::build::CheckoutBuilder::new();
        opts.force();
        self.repo
            .checkout_tree(commit.as_object(), Some(&mut opts))
            .expect("checkout");
    }

    fn file_content(&self, path: &str) -> String {
        std::fs::read_to_string(self.repo.workdir().unwrap().join(path)).expect("read file")
    }

    fn index_has_conflicts(&self) -> bool {
        self.repo.index().expect("index").has_conflicts()
    }
}

impl std::ops::Deref for TempRepo {
    type Target = Repository;

    fn deref(&self) -> &Repository {
        &self.repo
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        // Best-effort cleanup; on Windows a lingering handle would block the
        // delete, so defer it like the engine test fixture does.
        let dir = self._dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

fn oid(sha: &str) -> Oid {
    Oid::from_str(sha).expect("valid oid")
}

fn pick(sha: &str) -> RebaseStep {
    step("pick", sha, None)
}

fn step(action: &str, sha: &str, new_message: Option<&str>) -> RebaseStep {
    RebaseStep {
        sha: sha.to_owned(),
        action: action.to_owned(),
        new_message: new_message.map(str::to_owned),
    }
}

/// base + three commits, each adding one file (`f1.txt`/`f2.txt`/`f3.txt`).
/// Returns the three commit shas (oldest first); HEAD sits at the newest.
fn chain_of_three(name: &str) -> (TempRepo, String, String, String) {
    let temp = TempRepo::new(name);
    temp.write("base.txt", "base\n");
    temp.commit("base");
    temp.write("f1.txt", "one\n");
    let c1 = temp.commit("one");
    temp.write("f2.txt", "two\n");
    let c2 = temp.commit("two");
    temp.write("f3.txt", "three\n");
    let c3 = temp.commit("three");
    (temp, c1, c2, c3)
}

fn first_parent_summaries(repo: &Repository) -> Vec<String> {
    let mut out = Vec::new();
    let mut next = repo
        .head()
        .ok()
        .map(|h| h.peel_to_commit().expect("head commit"));
    while let Some(commit) = next {
        out.push(
            commit
                .message()
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned(),
        );
        next = commit.parent(0).ok();
    }
    out
}

fn expect_invalid(err: EngineError, needle: &str) {
    match err {
        EngineError::Invalid(msg) => {
            assert!(msg.contains(needle), "expected `{needle}` in: {msg}");
        }
        other => panic!("expected EngineError::Invalid, got: {other}"),
    }
}

// ---------------------------------------------------------------------------
// reorder
// ---------------------------------------------------------------------------

#[test]
fn reorder_plan_rewrites_history_and_map() {
    let (temp, c1, c2, c3) = chain_of_three("reorder");

    let plan = vec![pick(&c3), pick(&c1), pick(&c2)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("rebase start");

    assert!(!state.active, "greedy run finishes without conflicts");
    assert_eq!(state.current, 3);
    assert!(!state.paused_for_edit);

    // rewritten map keeps plan order: (c3,..), (c1,..), (c2,=HEAD).
    assert_eq!(state.rewritten.len(), 3);
    assert_eq!(state.rewritten[0].0, c3);
    assert_eq!(state.rewritten[1].0, c1);
    assert_eq!(state.rewritten[2].0, c2);
    assert_eq!(state.rewritten[2].1, temp.head_sha());
    // Fixed test timestamps guarantee replays never byte-identically collide
    // with the originals: every rewrite must produce a fresh sha.
    for (old, new) in &state.rewritten {
        assert_ne!(old, new, "rewrites must produce new shas");
    }

    // New first-parent order: the branch is rebuilt from the plan base
    // (parent of c3 = original c2) in plan order — three' -> one' -> two'
    // newest-first, on the untouched base..c2 prefix.
    assert_eq!(
        first_parent_summaries(&temp),
        ["two", "one", "three", "two", "one", "base"]
    );
    for file in ["base.txt", "f1.txt", "f2.txt", "f3.txt"] {
        assert!(
            temp.repo.workdir().unwrap().join(file).exists(),
            "{file} missing"
        );
    }

    // Finished rebase leaves no state behind.
    assert!(!ENGINE.rebase_state_impl(&temp).expect("state").active);
}

// ---------------------------------------------------------------------------
// squash / fixup
// ---------------------------------------------------------------------------

#[test]
fn squash_and_fixup_fold_into_previous() {
    // squash combines messages; fixup keeps the previous one.
    let (temp, c1, c2, c3) = chain_of_three("squash");
    let plan = vec![pick(&c1), pick(&c2), step("squash", &c3, None)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("squash rebase");
    assert!(!state.active);
    // c3 folded into c2's rewrite: only c1 and c2 entries, c2's target is HEAD.
    assert_eq!(state.rewritten.len(), 2);
    assert_eq!(state.rewritten[0].0, c1);
    assert_eq!(state.rewritten[1].0, c2);
    assert_eq!(state.rewritten[1].1, temp.head_sha());
    assert_eq!(temp.head_message(), "two\n\nthree");
    assert_eq!(
        first_parent_summaries(&temp),
        ["two", "one", "base"],
        "squash leaves two rewritten commits"
    );
    assert!(temp.repo.workdir().unwrap().join("f3.txt").exists());

    let (temp, c1, c2, c3) = chain_of_three("fixup");
    let plan = vec![pick(&c1), pick(&c2), step("fixup", &c3, None)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("fixup rebase");
    assert!(!state.active);
    assert_eq!(state.rewritten.len(), 2);
    assert_eq!(state.rewritten[1].1, temp.head_sha());
    assert_eq!(
        temp.head_message(),
        "two",
        "fixup discards the incoming message"
    );
    assert_eq!(first_parent_summaries(&temp), ["two", "one", "base"]);
    assert!(temp.repo.workdir().unwrap().join("f3.txt").exists());
}

// ---------------------------------------------------------------------------
// reword / drop
// ---------------------------------------------------------------------------

#[test]
fn reword_changes_commit_message() {
    let (temp, c1, c2, c3) = chain_of_three("reword");

    let plan = vec![
        pick(&c1),
        step("reword", &c2, Some("two (edited)")),
        pick(&c3),
    ];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("reword rebase");
    assert!(!state.active);
    assert_eq!(
        first_parent_summaries(&temp),
        ["three", "two (edited)", "one", "base"]
    );
    assert_eq!(state.rewritten.len(), 3);
    assert_eq!(state.rewritten[1].0, c2);
    assert_eq!(state.rewritten[2].1, temp.head_sha());
}

#[test]
fn drop_skips_commit_without_rewrite_entry() {
    let (temp, c1, c2, c3) = chain_of_three("drop");

    let plan = vec![pick(&c1), step("drop", &c2, None), pick(&c3)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("drop rebase");
    assert!(!state.active);
    assert_eq!(state.rewritten.len(), 2, "drop records nothing");
    assert_eq!(state.rewritten[0].0, c1);
    assert_eq!(state.rewritten[1].0, c3);
    assert_eq!(first_parent_summaries(&temp), ["three", "one", "base"]);
    let workdir = temp.repo.workdir().unwrap();
    assert!(
        !workdir.join("f2.txt").exists(),
        "dropped file must be absent"
    );
    assert!(workdir.join("f3.txt").exists());
}

// ---------------------------------------------------------------------------
// edit pause
// ---------------------------------------------------------------------------

#[test]
fn edit_pauses_then_continue_finishes() {
    let (temp, c1, c2, c3) = chain_of_three("edit");

    let plan = vec![pick(&c1), step("edit", &c2, None), pick(&c3)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("edit rebase");
    assert!(state.active);
    assert_eq!(state.current, 1, "paused at the edit step");
    assert!(state.paused_for_edit);
    // The edit step's pick already landed before the pause.
    assert_eq!(state.rewritten.len(), 2);
    assert_eq!(state.rewritten[1].0, c2);
    assert_eq!(first_parent_summaries(&temp), ["two", "one", "base"]);
    // rebase_state reports the same pause.
    let live = ENGINE.rebase_state_impl(&temp).expect("state");
    assert!(live.active && live.paused_for_edit && live.current == 1);

    // Continue: unpause, apply the rest.
    let state = ENGINE.rebase_continue_impl(&temp).expect("continue");
    assert!(!state.active);
    assert_eq!(state.current, 3);
    assert_eq!(
        first_parent_summaries(&temp),
        ["three", "two", "one", "base"]
    );
    assert!(temp.repo.workdir().unwrap().join("f3.txt").exists());
    assert!(!ENGINE.rebase_state_impl(&temp).expect("state").active);
}

// ---------------------------------------------------------------------------
// conflict flow
// ---------------------------------------------------------------------------

/// base("0") -> c1("1") -> c2("2") on one line of `file.txt`.
fn conflicting_chain(name: &str) -> (TempRepo, String, String, String) {
    let temp = TempRepo::new(name);
    temp.write("file.txt", "0\n");
    temp.commit("base");
    temp.write("file.txt", "1\n");
    let c1 = temp.commit("one");
    temp.write("file.txt", "2\n");
    let c2 = temp.commit("two");
    let head = temp.head_sha();
    (temp, c1, c2, head)
}

#[test]
fn conflict_pauses_until_resolved_then_continue_completes() {
    let (temp, c1, c2, orig_head) = conflicting_chain("conflict");

    // Replay [c2, c1]: c1's line rewrite conflicts against c2's.
    let plan = vec![pick(&c2), pick(&c1)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("rebase start");
    assert!(state.active);
    assert_eq!(state.current, 1, "paused at the conflicting step");
    assert!(!state.paused_for_edit);
    assert!(temp.index_has_conflicts(), "index carries conflict stages");

    // Continue without resolving: stays blocked at the same step, no error.
    let blocked = ENGINE
        .rebase_continue_impl(&temp)
        .expect("continue blocked");
    assert!(blocked.active && blocked.current == 1 && !blocked.paused_for_edit);
    assert!(temp.index_has_conflicts());

    // Resolve like the FE would (write content, stage it), then continue.
    temp.write("file.txt", "resolved\n");
    temp.stage("file.txt");
    let state = ENGINE
        .rebase_continue_impl(&temp)
        .expect("continue resolved");
    assert!(!state.active);
    assert_eq!(state.rewritten.len(), 2);
    assert_eq!(state.rewritten[0].0, c2);
    assert_eq!(state.rewritten[1].0, c1);
    assert_eq!(state.rewritten[1].1, temp.head_sha());
    assert_eq!(
        temp.head_message(),
        "one",
        "resolved step commits pick semantics"
    );
    assert_eq!(temp.file_content("file.txt"), "resolved\n");
    // Rebuilt from the plan base (parent of c2 = the original c1, itself
    // "one"), then the c2 replay, then the resolved c1 replay.
    assert_eq!(first_parent_summaries(&temp), ["one", "two", "one", "base"]);
    assert_ne!(temp.head_sha(), orig_head);
}

#[test]
fn abort_restores_original_history() {
    let (temp, c1, c2, orig_head) = conflicting_chain("abort");
    assert_eq!(orig_head, c2);

    let plan = vec![pick(&c2), pick(&c1)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("rebase start");
    assert!(state.active, "conflict expected");
    assert!(temp.index_has_conflicts());

    ENGINE.rebase_abort_impl(&temp).expect("abort");
    assert_eq!(temp.head_sha(), orig_head, "branch reset to orig_head");
    assert_eq!(temp.file_content("file.txt"), "2\n", "workdir restored");
    assert_eq!(
        first_parent_summaries(&temp),
        ["two", "one", "base"],
        "original history intact"
    );
    assert!(!ENGINE.rebase_state_impl(&temp).expect("state").active);
    // Aborting with nothing in progress is an error.
    match ENGINE.rebase_abort_impl(&temp) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("no rebase in progress")),
        other => panic!("expected Invalid for stray abort, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// onto
// ---------------------------------------------------------------------------

#[test]
fn rebase_onto_replays_on_new_base_and_moves_branch() {
    let temp = TempRepo::new("onto");
    temp.write("base.txt", "b\n");
    let base = temp.commit("base");
    temp.branch_at("feature", &base);
    temp.set_head_ref("refs/heads/feature");
    temp.write("f1.txt", "one\n");
    let f1 = temp.commit("f one");
    temp.write("f2.txt", "two\n");
    let f2 = temp.commit("f two");
    temp.set_head_ref("refs/heads/master");
    temp.checkout(&base);
    temp.write("m1.txt", "m one\n");
    let m1 = temp.commit("m one");
    temp.set_head_ref("refs/heads/feature");
    temp.checkout(&f2);

    let plan = vec![pick(&f1), pick(&f2)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, Some("master"))
        .expect("onto rebase");
    assert!(!state.active);

    // Branch moved to the rewritten head, HEAD reattached to it.
    let head = temp.repo.head().expect("head");
    assert_eq!(head.shorthand(), Ok("feature"));
    assert_eq!(
        head.target().expect("head oid").to_string(),
        temp.head_sha()
    );
    assert_eq!(state.rewritten[1].1, temp.head_sha());

    // Replay order: f2' -> f1' -> m1 -> base.
    assert_eq!(
        first_parent_summaries(&temp),
        ["f two", "f one", "m one", "base"]
    );
    for file in ["base.txt", "f1.txt", "f2.txt", "m1.txt"] {
        assert!(
            temp.repo.workdir().unwrap().join(file).exists(),
            "{file} missing"
        );
    }
    // master untouched.
    let master = temp
        .repo
        .find_reference("refs/heads/master")
        .expect("master");
    assert_eq!(master.target().expect("master oid").to_string(), m1);
}

// ---------------------------------------------------------------------------
// validation + guards
// ---------------------------------------------------------------------------

#[test]
fn validation_rejects_bad_plans_and_double_start() {
    let (temp, c1, c2) = {
        let temp = TempRepo::new("validate");
        temp.write("base.txt", "base\n");
        temp.commit("base");
        temp.write("f1.txt", "one\n");
        let c1 = temp.commit("one");
        temp.write("f2.txt", "two\n");
        let c2 = temp.commit("two");
        (temp, c1, c2)
    };

    expect_invalid(
        ENGINE.rebase_start_impl(&temp, &[], None).unwrap_err(),
        "empty",
    );
    expect_invalid(
        ENGINE
            .rebase_start_impl(&temp, &[step("rebase", &c1, None)], None)
            .unwrap_err(),
        "unknown action",
    );
    expect_invalid(
        ENGINE
            .rebase_start_impl(&temp, &[step("squash", &c2, None)], None)
            .unwrap_err(),
        "previous commit",
    );
    expect_invalid(
        ENGINE
            .rebase_start_impl(&temp, &[step("reword", &c1, None)], None)
            .unwrap_err(),
        "reword",
    );
    expect_invalid(
        ENGINE
            .rebase_start_impl(
                &temp,
                &[pick("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")],
                None,
            )
            .unwrap_err(),
        "cannot resolve",
    );

    // A second start while a rebase is active is rejected.
    let plan = vec![pick(&c1), step("edit", &c2, None)];
    let state = ENGINE
        .rebase_start_impl(&temp, &plan, None)
        .expect("first start");
    assert!(state.active);
    expect_invalid(
        ENGINE
            .rebase_start_impl(&temp, &[pick(&c1)], None)
            .unwrap_err(),
        "already in progress",
    );
    ENGINE.rebase_abort_impl(&temp).expect("cleanup abort");
}

#[test]
fn dirty_worktree_rejected_but_untracked_allowed() {
    let (temp, c1, _c2) = {
        let temp = TempRepo::new("dirty");
        temp.write("base.txt", "base\n");
        temp.commit("base");
        temp.write("f1.txt", "one\n");
        let c1 = temp.commit("one");
        temp.write("f2.txt", "two\n");
        let c2 = temp.commit("two");
        (temp, c1, c2)
    };

    // Modified tracked file in the worktree.
    temp.write("f1.txt", "dirty\n");
    expect_invalid(
        ENGINE
            .rebase_start_impl(&temp, &[pick(&c1)], None)
            .unwrap_err(),
        "clean working tree",
    );
    // Staged changes are equally dirty.
    temp.stage("f1.txt");
    expect_invalid(
        ENGINE
            .rebase_start_impl(&temp, &[pick(&c1)], None)
            .unwrap_err(),
        "clean working tree",
    );

    // Untracked files do not block a rebase.
    let temp = TempRepo::new("dirty-untracked");
    temp.write("base.txt", "base\n");
    temp.commit("base");
    temp.write("f1.txt", "one\n");
    let c1 = temp.commit("one");
    temp.write("untracked.txt", "scratch\n");
    let state = ENGINE
        .rebase_start_impl(&temp, &[pick(&c1)], None)
        .expect("untracked must not block");
    assert!(!state.active);
}

#[test]
fn state_reports_inactive_without_rebase() {
    let temp = TempRepo::new("state-inactive");
    temp.write("a.txt", "a\n");
    temp.commit("base");

    let state = ENGINE.rebase_state_impl(&temp).expect("state");
    assert!(!state.active);
    assert!(state.plan.is_empty());
    assert_eq!(state.current, 0);
    assert!(!state.paused_for_edit);
    assert!(state.rewritten.is_empty());

    // Continue with nothing in progress is an error.
    match ENGINE.rebase_continue_impl(&temp) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("no rebase in progress")),
        other => panic!("expected Invalid for stray continue, got: {other:?}"),
    }
}
