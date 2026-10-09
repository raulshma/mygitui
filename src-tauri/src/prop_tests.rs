//! M12 lane B: property-based roundtrips (diff hunk-apply / checkpoint
//! restore), exercised through the crate's public engine APIs only:
//! `GitEngine` (diff, stage_all, commit), `GitEngineM3` (checkpoint_*), and
//! the diffcore staging core (`apply_selection` — the same entry the IPC
//! stage command uses).
//!
//! Cases are deliberately bounded (≤ 40 lines, ≤ 6 files) so the whole
//! suite stays fast; strategies draw from small alphabets to force
//! duplicate lines / repeated hunks, which is where line-number arithmetic
//! tends to break.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use proptest::prelude::*;

use git2::{Repository, RepositoryInitOptions};

use crate::diffcore::{apply_selection, Selection};
use crate::engine::git_engine::{GitEngine, GitEngineM3};
use crate::engine::libgit2::Libgit2Engine;
use crate::engine::types::{CommitOptions, DiffSide};

const ENGINE: Libgit2Engine = Libgit2Engine;

// ---------------------------------------------------------------------------
// scaffolding (temp repos, mirrors the *_tests.rs pattern)
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

/// Per-iteration unique suffix: proptest runs many cases through the same
/// `prop_repo` name, and a previous case's DELAYED cleanup (below) would
/// otherwise delete the next case's freshly created directory mid-setup.
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mygitui-prop-{}-{name}-{id}", std::process::id()));
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
        // Best-effort delayed cleanup: repository handles can outlive the
        // remover by a few milliseconds on Windows.
        let dir = self.0.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

fn prop_repo(name: &str) -> (Repository, TempDir) {
    let dir = TempDir::new(name);
    let mut opts = RepositoryInitOptions::new();
    opts.bare(false).initial_head("main");
    let repo = Repository::init_opts(dir.path(), &opts).expect("init temp repo");
    repo.config()
        .and_then(|mut c| {
            c.set_str("user.name", "Prop Test")?;
            c.set_str("user.email", "prop@test.local")?;
            // Byte-exact file contents: neutralize a global core.autocrlf.
            c.set_bool("core.autocrlf", false)
        })
        .expect("configure identity");
    (repo, dir)
}

fn write_file(repo: &Repository, path: &str, content: &str) {
    let file = repo.workdir().unwrap().join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
}

fn remove_file(repo: &Repository, path: &str) {
    let file = repo.workdir().unwrap().join(path);
    if file.exists() {
        std::fs::remove_file(file).expect("remove file");
    }
}

fn stage_commit(repo: &Repository, message: &str) -> String {
    ENGINE.stage_all(repo, false).expect("stage_all");
    ENGINE
        .commit(
            repo,
            &CommitOptions {
                message: message.to_string(),
                amend: false,
                no_verify: true,
                allow_empty: false,
                author: None,
            },
        )
        .expect("commit")
}

fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// strategies
// ---------------------------------------------------------------------------

/// Line pool small enough that duplicates are frequent (the point).
fn lines_pool() -> Vec<String> {
    [
        "alpha",
        "beta",
        "gamma",
        "x",
        "y",
        "dup",
        "dup",
        "common line shared by many",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn any_lines() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(proptest::sample::select(lines_pool()), 0..=40usize)
}

fn nonempty_lines() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(proptest::sample::select(lines_pool()), 1..=40usize)
}

/// One property case: a base state (2..=6 files), a per-file mutation spec
/// (keep / modify with fresh content / delete) and 0..=2 extra untracked
/// files for the mutated state.
#[derive(Debug, Clone)]
struct CheckpointCase {
    files: Vec<(String, String)>,
    ops: Vec<(u8, String)>, // 0 = keep, 1 = modify, 2 = delete
    extras: Vec<String>,
}

fn checkpoint_case() -> impl Strategy<Value = CheckpointCase> {
    (2usize..=6usize)
        .prop_flat_map(|n| {
            (
                proptest::collection::vec(nonempty_lines(), n..=n),
                proptest::collection::vec(proptest::sample::select(vec![0u8, 1, 2]), n..=n),
                proptest::collection::vec(nonempty_lines(), n..=n),
                proptest::collection::vec(any_lines(), 0..=2usize),
            )
        })
        .prop_map(|(contents, ops, new_contents, extras)| {
            let files: Vec<(String, String)> = contents
                .into_iter()
                .enumerate()
                .map(|(i, lines)| (format!("f{i}.txt"), join_lines(&lines)))
                .collect();
            let ops: Vec<(u8, String)> = ops
                .into_iter()
                .zip(new_contents)
                .map(|(op, lines)| (op, join_lines(&lines)))
                .collect();
            let extras: Vec<String> = extras.into_iter().map(|lines| join_lines(&lines)).collect();
            CheckpointCase { files, ops, extras }
        })
}

// ---------------------------------------------------------------------------
// properties
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// (a) Diff/hunk-apply roundtrip: whatever the (A, B) text pair, staging
    /// EVERY hunk the engine's worktree diff reports must land the index on
    /// exactly B — the "select all hunks ≡ stage the file" invariant the
    /// diff viewer relies on.
    #[test]
    fn diff_hunk_apply_reconstructs_new_text(a in nonempty_lines(), b in any_lines()) {
        let (repo, _dir) = prop_repo("diffapply");
        let path = "prop.txt";
        write_file(&repo, path, &join_lines(&a));
        stage_commit(&repo, "base A");
        write_file(&repo, path, &join_lines(&b));

        let files = ENGINE
            .diff(&repo, &DiffSide::Head, &DiffSide::Worktree, None)
            .expect("worktree diff");
        let fd = files
            .iter()
            .find(|f| f.path == path)
            .expect("diff includes the changed file");
        let selections: Vec<Selection> = (0..fd.hunks.len() as u32).map(Selection::Hunk).collect();
        apply_selection(&repo, path, &selections, false).expect("apply all hunks");

        let expected = join_lines(&b);
        let index = repo.index().expect("index");
        match index.get_path(Path::new(path), 0) {
            Some(entry) => {
                let blob = repo.find_blob(entry.id).expect("staged blob");
                prop_assert_eq!(blob.content(), expected.as_bytes());
            }
            None => prop_assert!(
                expected.is_empty(),
                "staging all hunks dropped a non-empty file from the index"
            ),
        }
    }

    /// (b) Checkpoint-restore roundtrip: restore must bring the worktree
    /// back to the checkpointed state EXACTLY — modifications reverted,
    /// deletions restored, post-checkpoint untracked files removed.
    #[test]
    fn checkpoint_restore_returns_exact_prior_state(case in checkpoint_case()) {
        let (repo, _dir) = prop_repo("ckpt");
        // state1: base worktree, committed, then checkpointed.
        for (name, content) in &case.files {
            write_file(&repo, name, content);
        }
        stage_commit(&repo, "state1");
        let info = ENGINE.checkpoint_create(&repo, "prop").expect("checkpoint");

        // state2: apply mutations (incl. deletions) + untracked extras.
        for ((name, _), (op, content)) in case.files.iter().zip(&case.ops) {
            match op {
                0 => {} // keep
                1 => write_file(&repo, name, content),
                _ => remove_file(&repo, name),
            }
        }
        for (j, content) in case.extras.iter().enumerate() {
            write_file(&repo, &format!("extra{j}.txt"), content);
        }

        ENGINE
            .checkpoint_restore(&repo, &info.id)
            .expect("checkpoint restore");

        // Worktree must equal state1 exactly.
        for (name, content) in &case.files {
            let file = repo.workdir().unwrap().join(name);
            let restored = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("restored worktree missing {name}: {e}"));
            prop_assert_eq!(&restored, content);
        }
        for j in 0..case.extras.len() {
            let extra = repo.workdir().unwrap().join(format!("extra{j}.txt"));
            prop_assert!(!extra.exists(), "untracked extra{j}.txt survived restore");
        }
    }
}
