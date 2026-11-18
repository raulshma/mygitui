//! Perf-gate benches (plan §19). Run: `cargo bench` (from src-tauri).
//!
//! Gates, measured here in release mode on representative workloads:
//! - incremental status  < 50 ms   (small workdir delta)
//! - full status         < 300 ms  (100k-file repos need the real fixture;
//!   this bench pins the code path scaling)
//! - log page + layout   < 16 ms   (500-commit page incl. lane assignment)
//! - diff build          < 50 ms   (200-file working tree)
//! - cold open + status  < 50 ms   (repo reopen + first full status, 500
//!   commits / 200 modified files)
//! - 10k-hunk diff       < 50 ms   (single file, synthetic ~130k-line text)
//! - regex log page      < 16 ms   (500 commits, `regex` grep, one page)
//! - file-history page   < 16 ms   (path-filtered page over a file touched
//!   in 250 of 500 commits)
//! - graph layout        (merge-heavy synthetic history: 40 branches merged
//!   into a 120-commit trunk — lane assignment on a branched DAG)
//! - log page @ 250k     < 16 ms   (500-commit page + layout over a 250k-
//!   commit linear history; the topological order is cached)
//! - bisect step         < 20 ms   (start + one mark + untimed reset, 500
//!   linear commits)
//!
//! CI (nightly.yml) runs `cargo bench -- --save-baseline nightly`; regressions
//! are reviewed against the previous baseline rather than hard-gated on
//! shared runners.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use git2::{Repository, Signature, Time};
use mygitui_lib::engine::git_engine::{GitEngine, GitEngineM3};
use mygitui_lib::engine::libgit2::Libgit2Engine;
use mygitui_lib::engine::types::{BisectMark, DiffSide, LogFilter};
use mygitui_lib::graph::types as graph;

/// One temp repo per process: 500 linear commits, half the files touched
/// in the worktree afterwards (for status/diff benches).
fn fixture_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mygitui-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = Repository::init(&dir).unwrap();
    let mut cfg = repo.config().unwrap();
    let _ = cfg.set_str("user.name", "Bench");
    let _ = cfg.set_str("user.email", "bench@local");
    let sig = Signature::new("Bench", "bench@local", &Time::new(1_700_000_000, 0)).unwrap();

    let mut tree_oids = Vec::with_capacity(500);
    let mut parent: Option<git2::Oid> = None;
    let mut index = repo.index().unwrap();
    for i in 0..500 {
        let path = format!("dir{}/file{}.txt", i % 50, i);
        let full = dir.join(&path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, format!("content {i}\nline two\n").repeat(4)).unwrap();
        index.add_path(std::path::Path::new(&path)).unwrap();
        let tree_oid = index.write_tree().unwrap();
        index.write().unwrap();
        tree_oids.push(tree_oid);
        let tree = repo.find_tree(tree_oid).unwrap();
        let parents: Vec<git2::Commit> = parent
            .map(|oid| vec![repo.find_commit(oid).unwrap()])
            .unwrap_or_default();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        let commit = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("commit {i}"),
                &tree,
                &parent_refs,
            )
            .unwrap();
        parent = Some(commit);
    }
    // Worktree delta for status/diff: touch 200 files.
    for i in (0..500).step_by(5) {
        let path = format!("dir{}/file{}.txt", i % 50, i);
        std::fs::write(dir.join(&path), format!("modified {i}\n").repeat(4)).unwrap();
    }
    let _ = black_box(tree_oids.len());
    dir
}

/// Temp repo where `log.md` is modified in every second of 500 linear
/// commits (file-history page: 250 hits inside a 500-commit walk).
fn file_history_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mygitui-bench-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = Repository::init(&dir).unwrap();
    let mut cfg = repo.config().unwrap();
    let _ = cfg.set_str("user.name", "Bench");
    let _ = cfg.set_str("user.email", "bench@local");
    let sig = Signature::new("Bench", "bench@local", &Time::new(1_700_000_000, 0)).unwrap();
    let mut parent: Option<git2::Oid> = None;
    let mut index = repo.index().unwrap();
    for i in 0..500 {
        let mut log = String::new();
        if i % 2 == 0 {
            // Rewrite log.md every second commit; other commits touch a
            // different file so the path filter has real work.
            for line in 0..=(i / 2 + 1) {
                log.push_str(&format!("entry {line}\n"));
            }
            std::fs::write(dir.join("log.md"), &log).unwrap();
            index.add_path(std::path::Path::new("log.md")).unwrap();
        } else {
            std::fs::write(dir.join(format!("other{i}.txt")), format!("x {i}\n")).unwrap();
            index
                .add_path(std::path::Path::new(&format!("other{i}.txt")))
                .unwrap();
        }
        let tree_oid = index.write_tree().unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let parents: Vec<git2::Commit> = parent
            .map(|oid| vec![repo.find_commit(oid).unwrap()])
            .unwrap_or_default();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        parent = Some(
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("commit {i}"),
                &tree,
                &parent_refs,
            )
            .unwrap(),
        );
    }
    dir
}

/// ~10k separated hunks: commit text A, put text B in the worktree.
/// Each 13-line block (10 SHARED context lines + 3 changed) stays one hunk
/// at the default 3-line context; only the payload lines carry the side
/// prefix, otherwise every line differs and the diff collapses to one hunk.
fn big_diff_repo() -> PathBuf {
    fn text(prefix: &str, hunks: usize) -> String {
        let mut s = String::with_capacity(hunks * 13 * 20);
        for i in 0..hunks {
            for ctx in 0..10 {
                s.push_str(&format!("ctx {i} {ctx}\n"));
            }
            s.push_str(&format!("{prefix} one {i}\n"));
            s.push_str(&format!("{prefix} two {i}\n"));
            s.push_str(&format!("{prefix} three {i}\n"));
        }
        s
    }
    let dir = std::env::temp_dir().join(format!("mygitui-bench-bigdiff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = Repository::init(&dir).unwrap();
    let mut cfg = repo.config().unwrap();
    let _ = cfg.set_str("user.name", "Bench");
    let _ = cfg.set_str("user.email", "bench@local");
    let sig = Signature::new("Bench", "bench@local", &Time::new(1_700_000_000, 0)).unwrap();
    std::fs::write(dir.join("big.txt"), text("old", 10_000)).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("big.txt")).unwrap();
    let tree_oid = index.write_tree().unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "big A", &tree, &[])
        .unwrap();
    std::fs::write(dir.join("big.txt"), text("new", 10_000)).unwrap();
    dir
}

/// Merge-heavy synthetic history: trunk commits, then short branches merged
/// back via 2-parent merge commits. Wide lane state with bends and
/// convergences exercises `lane_of` and the pass-through bookkeeping
/// (item 14). Every commit shares one tree — the bench only reads commits.
const MERGE_TRUNK: usize = 120;
/// Branches merged back into the trunk (3 commits + 1 merge each).
const MERGE_BRANCHES: usize = 40;

fn merge_heavy_repo() -> PathBuf {
    /// Commits per branch before its merge.
    const BRANCH_COMMITS: usize = 3;

    let dir = std::env::temp_dir().join(format!("mygitui-bench-mergeheavy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = Repository::init(&dir).unwrap();
    let mut cfg = repo.config().unwrap();
    let _ = cfg.set_str("user.name", "Bench");
    let _ = cfg.set_str("user.email", "bench@local");
    let sig = Signature::new("Bench", "bench@local", &Time::new(1_700_000_000, 0)).unwrap();

    let mut index = repo.index().unwrap();
    std::fs::write(dir.join("trunk.txt"), "trunk\n").unwrap();
    index.add_path(std::path::Path::new("trunk.txt")).unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    // Trunk on an explicit ref (HEAD may stay unborn; the engine walks
    // branch tips from the refs cache either way).
    let mut main = repo
        .commit(Some("refs/heads/main"), &sig, &sig, "trunk 0", &tree, &[])
        .unwrap();
    let mut trunk_tips = Vec::with_capacity(MERGE_TRUNK);
    for i in 1..MERGE_TRUNK {
        main = repo
            .commit(
                Some("refs/heads/main"),
                &sig,
                &sig,
                &format!("trunk {i}"),
                &tree,
                &[&repo.find_commit(main).unwrap()],
            )
            .unwrap();
        trunk_tips.push(main);
    }
    for b in 0..MERGE_BRANCHES {
        // Spread the branch points across the trunk.
        let base = trunk_tips[b * (MERGE_TRUNK - 1) / MERGE_BRANCHES];
        let branch_ref = format!("refs/heads/branch{b}");
        let mut tip = base;
        for c in 0..BRANCH_COMMITS {
            tip = repo
                .commit(
                    Some(&branch_ref),
                    &sig,
                    &sig,
                    &format!("branch{b} c{c}"),
                    &tree,
                    &[&repo.find_commit(tip).unwrap()],
                )
                .unwrap();
        }
        let branch_commit = repo.find_commit(tip).unwrap();
        let main_commit = repo.find_commit(main).unwrap();
        main = repo
            .commit(
                Some("refs/heads/main"),
                &sig,
                &sig,
                &format!("merge branch{b}"),
                &tree,
                &[&main_commit, &branch_commit],
            )
            .unwrap();
    }
    dir
}

/// 250k-commit linear history sharing one tree: the bulk gate for page +
/// layout at scale (item 14). Setup mirrors the 500-commit fixture but
/// skips blob/index churn — the bench only reads commits.
fn bulk_history_repo() -> PathBuf {
    const COMMITS: usize = 250_000;
    let dir = std::env::temp_dir().join(format!("mygitui-bench-bulk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = Repository::init(&dir).unwrap();
    let mut cfg = repo.config().unwrap();
    let _ = cfg.set_str("user.name", "Bench");
    let _ = cfg.set_str("user.email", "bench@local");
    let sig = Signature::new("Bench", "bench@local", &Time::new(1_700_000_000, 0)).unwrap();
    std::fs::write(dir.join("bulk.txt"), "bulk\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("bulk.txt")).unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let mut parent: Option<git2::Oid> = None;
    for i in 0..COMMITS {
        let parents: Vec<git2::Commit> = parent
            .map(|oid| vec![repo.find_commit(oid).unwrap()])
            .unwrap_or_default();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        parent = Some(
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("commit {i}"),
                &tree,
                &parent_refs,
            )
            .unwrap(),
        );
    }
    dir
}

/// Item 14: layout over the whole merge-heavy walk (lane assignment on a
/// branched DAG — the O(1) lane lookup and per-row edge bitmap hot path).
fn bench_graph_layout(c: &mut Criterion) {
    let dir = merge_heavy_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let filter = LogFilter::default();
    // Setup: the whole (trunk + branches + merges) walk in one page.
    let (commits, _) = engine
        .log(&repo, &filter, MERGE_TRUNK + MERGE_BRANCHES * 4, None)
        .unwrap();
    let total = commits.len();
    assert!(total >= MERGE_TRUNK + MERGE_BRANCHES * 4);
    let mut group = c.benchmark_group("graph");
    group.throughput(criterion::Throughput::Elements(total as u64));
    group.bench_function("layout_merge_heavy", |b| {
        b.iter_batched(
            graph::LaneState::default,
            |mut lanes| {
                let rows = graph::layout_page(&commits, &mut lanes);
                assert_eq!(rows.len(), total);
            },
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Item 14: one 500-commit page + layout over a 250k-commit linear history.
/// The topological order is built once and cached — the warm-up call
/// (outside the timed closure) populates it, and the timed loop measures a
/// cached-order page exactly like the streaming hot path.
fn bench_log_250k(c: &mut Criterion) {
    let dir = bulk_history_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let filter = LogFilter::default();
    let _ = engine.log(&repo, &filter, 500, None).unwrap();
    let mut group = c.benchmark_group("log");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("page500_plus_layout_250k", |b| {
        b.iter_batched(
            graph::LaneState::default,
            |mut lanes| {
                let t0 = Instant::now();
                let (commits, _) = engine.log(&repo, &filter, 500, None).unwrap();
                let rows = graph::layout_page(&commits, &mut lanes);
                assert_eq!(commits.len(), 500);
                assert_eq!(rows.len(), 500);
                t0.elapsed()
            },
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

fn bench_status(c: &mut Criterion) {
    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let mut group = c.benchmark_group("status");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("full_500_files_200_modified", |b| {
        b.iter(|| {
            let t0 = Instant::now();
            let s = engine.status(&repo, None).unwrap();
            assert!(s.entries.len() >= 100);
            t0.elapsed()
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

fn bench_log_layout(c: &mut Criterion) {
    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let filter = LogFilter::default();
    let _lanes = graph::LaneState::default();
    let mut group = c.benchmark_group("log");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("page500_plus_layout", |b| {
        b.iter_batched(
            graph::LaneState::default,
            |mut l| {
                let t0 = Instant::now();
                let (commits, _) = engine.log(&repo, &filter, 500, None).unwrap();
                let rows = graph::layout_page(&commits, &mut l);
                assert_eq!(commits.len(), 500);
                assert_eq!(rows.len(), 500);
                t0.elapsed()
            },
            criterion::BatchSize::SmallInput,
        )
    });
    // Layout alone from a continuous state (the streaming hot path).
    let (commits, _) = engine.log(&repo, &filter, 500, None).unwrap();
    group.bench_function("layout_only_page500", |b| {
        b.iter_batched(
            graph::LaneState::default,
            |mut l| {
                let rows = graph::layout_page(&commits, &mut l);
                assert_eq!(rows.len(), 500);
            },
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

fn bench_diff(c: &mut Criterion) {
    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let mut group = c.benchmark_group("diff");
    group.bench_function("worktree_vs_head_200_files", |b| {
        b.iter(|| {
            let t0 = Instant::now();
            let files = engine
                .diff(&repo, &DiffSide::Head, &DiffSide::Worktree, None, false)
                .unwrap();
            assert!(!files.is_empty());
            t0.elapsed()
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);

    // Synthetic single-file diff with ~10k separated hunks (M12 gate):
    // 13-line blocks (3 changed + 10 unchanged) keep the hunks apart at the
    // default 3-line context.
    let big = big_diff_repo();
    let repo = Repository::open(&big).unwrap();
    let mut group = c.benchmark_group("diff");
    group.throughput(criterion::Throughput::Elements(10_000));
    group.bench_function("synthetic_10k_hunks", |b| {
        b.iter(|| {
            let t0 = Instant::now();
            let files = engine
                .diff(&repo, &DiffSide::Head, &DiffSide::Worktree, None, false)
                .unwrap();
            assert_eq!(files.len(), 1, "{:?} files", files.len());
            assert!(
                files[0].hunks.len() >= 10_000 - 20,
                "expected ~10k hunks, got {}",
                files[0].hunks.len()
            );
            t0.elapsed()
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&big);
}

/// M9–M11 ops on the same fixture: tag listing, describe, pickaxe filter.
fn bench_refs_ops(c: &mut Criterion) {
    use mygitui_lib::engine::git_engine::GitEngineM3;

    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    // A few annotated + lightweight tags across history.
    for i in [0, 100, 250, 499] {
        let oid = repo
            .revparse_single(&format!("HEAD~{}", 499 - i))
            .unwrap()
            .id();
        let commit = repo.find_commit(oid).unwrap();
        if i % 2 == 0 {
            repo.tag(
                &format!("v0.{i}"),
                commit.as_object(),
                &commit.author(),
                "bench",
                false,
            )
            .ok();
        } else {
            repo.tag_lightweight(&format!("v0.{i}"), commit.as_object(), false)
                .ok();
        }
    }
    let head = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let mut group = c.benchmark_group("refs");
    group.bench_function("tag_list_500_commits", |b| {
        b.iter(|| {
            let tags = engine.tag_list(&repo).unwrap();
            assert_eq!(tags.len(), 4);
        })
    });
    group.bench_function("describe_head", |b| {
        b.iter(|| {
            let text = engine.describe(&repo, &head).unwrap();
            assert!(text.starts_with("v0."));
        })
    });
    group.bench_function("log_pickaxe_substring", |b| {
        b.iter(|| {
            let filter = LogFilter {
                pickaxe: Some("content 42".into()),
                ..Default::default()
            };
            let (commits, _) = engine.log(&repo, &filter, 100, None).unwrap();
            assert!(!commits.is_empty());
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

/// M12: drop the repo handle, reopen from disk, take the first full status —
/// what a fresh tab does on startup. Budget: < 50 ms on 500 commits.
fn bench_cold_open(c: &mut Criterion) {
    let dir = fixture_repo();
    let engine = Libgit2Engine::new();
    let mut repo = Some(Repository::open(&dir).unwrap());
    let mut group = c.benchmark_group("cold");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("open_plus_status", |b| {
        b.iter(|| {
            drop(repo.take()); // close the handle: next open is a cold open
            let r = Repository::open(&dir).unwrap();
            let s = engine.status(&r, None).unwrap();
            assert!(s.entries.len() >= 100);
            repo = Some(r);
        })
    });
    group.finish();
    drop(repo);
    let _ = std::fs::remove_dir_all(&dir);
}

/// M12: one regex-grep log page over the 500-commit fixture.
/// Budget: < 16 ms ("commit 4xx" matches 100 of 500 commits).
fn bench_search(c: &mut Criterion) {
    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let filter = LogFilter {
        regex: true,
        text: Some("commit 4[0-9]{2}".into()),
        ..Default::default()
    };
    let mut group = c.benchmark_group("search");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("log_filter_regex_page", |b| {
        b.iter(|| {
            let (commits, _) = engine.log(&repo, &filter, 500, None).unwrap();
            assert_eq!(commits.len(), 100);
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

/// M12: one path-filtered page over a file touched in 250 of 500 commits.
/// Budget: < 16 ms.
fn bench_history(c: &mut Criterion) {
    let dir = file_history_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let filter = LogFilter {
        path: Some("log.md".into()),
        ..Default::default()
    };
    let mut group = c.benchmark_group("history");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("file_history_page", |b| {
        b.iter(|| {
            let (commits, _) = engine.log(&repo, &filter, 200, None).unwrap();
            assert_eq!(commits.len(), 200);
            for c in &commits {
                assert!(!c.summary.is_empty());
            }
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

/// M12: `bisect_start` + one `bisect_mark` on the 500-commit fixture (the
/// timed part; the reset that makes the next iteration independent runs
/// untimed via `iter_custom`). Budget: < 20 ms.
fn bench_bisect(c: &mut Criterion) {
    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let head = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let first = repo
        .revparse_single(&format!("{}~499", &head[..7]))
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let mut group = c.benchmark_group("bisect");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("mark_step", |b| {
        b.iter_custom(|iters| {
            let mut timed = Duration::ZERO;
            for _ in 0..iters {
                let t0 = Instant::now();
                let state = engine.bisect_start(&repo, Some(&head), Some(&first));
                let state = state.unwrap();
                black_box(&state);
                let state = engine.bisect_mark(&repo, BisectMark::Good).unwrap();
                black_box(&state);
                timed += t0.elapsed();
                engine.bisect_reset(&repo).unwrap(); // untimed cleanup
            }
            timed
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

criterion_group!(
    benches,
    bench_status,
    bench_log_layout,
    bench_graph_layout,
    bench_log_250k,
    bench_diff,
    bench_refs_ops,
    bench_cold_open,
    bench_search,
    bench_history,
    bench_bisect
);
criterion_main!(benches);
