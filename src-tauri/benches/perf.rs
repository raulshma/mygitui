//! Perf-gate benches (plan §19). Run: `cargo bench` (from src-tauri).
//!
//! Gates, measured here in release mode on representative workloads:
//! - incremental status  < 50 ms   (small workdir delta)
//! - full status         < 300 ms  (100k-file repos need the real fixture;
//!   this bench pins the code path scaling)
//! - log page + layout   < 16 ms   (500-commit page incl. lane assignment)
//! - diff build          < 50 ms   (200-file working tree)
//!
//! CI (nightly.yml) runs `cargo bench -- --save-baseline nightly`; regressions
//! are reviewed against the previous baseline rather than hard-gated on
//! shared runners.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion};
use git2::{Repository, Signature, Time};
use mygitui_lib::engine::git_engine::GitEngine;
use mygitui_lib::engine::libgit2::Libgit2Engine;
use mygitui_lib::engine::types::{DiffSide, LogFilter};
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

fn bench_status(c: &mut Criterion) {
    let dir = fixture_repo();
    let repo = Repository::open(&dir).unwrap();
    let engine = Libgit2Engine::new();
    let mut group = c.benchmark_group("status");
    group.throughput(criterion::Throughput::Elements(500));
    group.bench_function("full_500_files_200_modified", |b| {
        b.iter(|| {
            let t0 = Instant::now();
            let s = engine.status(&repo).unwrap();
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
                .diff(&repo, &DiffSide::Head, &DiffSide::Worktree, None)
                .unwrap();
            assert!(!files.is_empty());
            t0.elapsed()
        })
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&dir);
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

criterion_group!(
    benches,
    bench_status,
    bench_log_layout,
    bench_diff,
    bench_refs_ops
);
criterion_main!(benches);
