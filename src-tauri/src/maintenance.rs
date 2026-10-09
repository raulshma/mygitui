//! Repo health + maintenance (lane M11) — sizes, object counts, and the
//! `git` maintenance commands, all through the sanitized CLI layer
//! (libgit2 has no gc/repack/commit-graph write).
//!
//! Every runner takes the repo workdir and returns combined output; the IPC
//! layer op-queues them (long-running) and toasts the summary. `git gc`
//! writes the commit-graph + packs refs — the "blazingly fast" win on huge
//! repos (libgit2's revwalk reads the commit-graph).

use std::path::Path;

use serde::Serialize;

use crate::engine::types::RepoHealth;

/// `.git` size in bytes + top-level breakdown (pure fs walk, bounded at 3
/// depth levels to keep it cheap on huge repos).
fn dir_size(path: &Path, depth: u8) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() && depth < 3 {
            total += dir_size(&entry.path(), depth + 1);
        }
    }
    total
}

/// Health snapshot for one repo (pure read; no CLI).
pub fn repo_health(workdir: &Path, git_dir: &Path) -> RepoHealth {
    let git_size_bytes = dir_size(git_dir, 0);
    let worktree_size_bytes = dir_size(workdir, 0);
    let mut health = RepoHealth {
        git_size_bytes,
        worktree_size_bytes,
        loose_objects: 0,
        packed_objects: None,
        pack_files: 0,
        has_commit_graph: git_dir.join("objects/info/commit-graph").exists()
            || git_dir.join("objects/info/commit-graphs").exists(),
        commit_graph_bytes: 0,
        packed_refs: git_dir.join("packed-refs").exists(),
        last_gc: None,
        fsck_dangling: None,
        fsck_samples: Vec::new(),
    };
    let objects = git_dir.join("objects");
    // Loose objects: two-hex-char fanout dirs.
    if let Ok(fanout) = std::fs::read_dir(&objects) {
        for dir in fanout.flatten() {
            let name = dir.file_name().to_string_lossy().into_owned();
            if name.len() == 2 && dir.path().is_dir() {
                if let Ok(files) = std::fs::read_dir(dir.path()) {
                    health.loose_objects += files.count() as u64;
                }
            }
        }
    }
    // Pack files + (approximate) packed object count from .idx names —
    // counting requires parsing the idx; report pack file count and let
    // `git count-objects -v` (run separately) give exact numbers on demand.
    let pack_dir = objects.join("pack");
    if let Ok(files) = std::fs::read_dir(&pack_dir) {
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if name.ends_with(".pack") {
                health.pack_files += 1;
            }
        }
    }
    let cg = objects.join("info/commit-graph");
    if cg.exists() {
        health.commit_graph_bytes = cg.metadata().map(|m| m.len()).unwrap_or(0);
    }
    let gc_log = git_dir.join("gc.log");
    if let Ok(meta) = gc_log.metadata() {
        if let Ok(modified) = meta.modified() {
            if let Ok(elapsed) = modified.duration_since(std::time::UNIX_EPOCH) {
                health.last_gc = Some(elapsed.as_secs() as i64);
            }
        }
    }
    health
}

/// Sanitized `git <args>` runner (shared rules with cli.rs).
pub fn git_run(workdir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(workdir).args(args);
    crate::cli::apply_sanitized_env(&mut cmd);
    let Ok(output) = cmd.output() else {
        // A missing git binary errors immediately; long-running ops are
        // op-queued so the UI stays responsive.
        return Err(format!("failed to spawn git {args:?}"));
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        Ok(stdout.trim().to_string())
    } else {
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        Err(format!(
            "git {} failed: {detail}",
            args.first().unwrap_or(&"")
        ))
    }
}

/// The maintenance operations the Health panel exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceOp {
    Gc,
    Prune,
    CommitGraph,
    PackRefs,
}

impl MaintenanceOp {
    pub fn args(self) -> &'static [&'static str] {
        match self {
            // Aggressive enough to matter, gentle enough for interactive use:
            // no --aggressive (hours on big repos), auto-detaches packs.
            MaintenanceOp::Gc => &["gc", "--auto"],
            MaintenanceOp::Prune => &["prune"],
            MaintenanceOp::CommitGraph => {
                &["commit-graph", "write", "--reachable", "--split=replace"]
            }
            MaintenanceOp::PackRefs => &["pack-refs", "--all", "--prune"],
        }
    }
}

/// Exact object counts via `git count-objects -v` (fills the packed side of
/// the health panel).
pub fn count_objects(workdir: &Path) -> Result<(u64, u64), String> {
    let out = git_run(workdir, &["count-objects", "-v"])?;
    let mut count = 0u64;
    let mut size = 0u64;
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("count: ") {
            count = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("size_pack: ") {
            size = v.trim().parse().unwrap_or(0);
        }
    }
    Ok((count, size))
}

// ---------------------------------------------------------------------------
// sparse checkout + LFS (CLI edges)
// ---------------------------------------------------------------------------

use crate::engine::types::{LfsStatus, SparseInfo};

/// Sparse-checkout state (`git sparse-checkout list`; `--cone` detection
/// from config `core.sparseCheckoutCone`).
pub fn sparse_info(workdir: &Path) -> SparseInfo {
    let list = git_run(workdir, &["sparse-checkout", "list"]).unwrap_or_default();
    let cone = git_run(workdir, &["config", "--bool", "core.sparseCheckoutCone"])
        .map(|v| v == "true")
        .unwrap_or(false);
    let patterns: Vec<String> = list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    SparseInfo {
        enabled: !patterns.is_empty(),
        cone,
        patterns,
    }
}

/// `git sparse-checkout set/add --cone <patterns>` (cone mode only — the
/// safe, git-recommended mode; raw patterns need `--no-cone` power users).
pub fn sparse_apply(workdir: &Path, patterns: &[String], add: bool) -> Result<(), String> {
    if patterns.is_empty() {
        // Disabling: back to full checkout.
        return git_run(workdir, &["sparse-checkout", "disable"]).map(|_| ());
    }
    let mut args: Vec<&str> = vec!["sparse-checkout"];
    args.push(if add { "add" } else { "set" });
    args.push("--cone");
    for pattern in patterns {
        args.push(pattern);
    }
    git_run(workdir, &args).map(|_| ())
}

/// `git lfs` presence + tracked patterns (parsed from every tracked
/// `.gitattributes` — the root one and simple per-dir ones via ls-files).
pub fn lfs_status(workdir: &Path) -> LfsStatus {
    let version = git_run(workdir, &["lfs", "version"]).ok();
    let mut tracked = Vec::new();
    if let Ok(list) = git_run(workdir, &["ls-files", "*.gitattributes"]) {
        for attributes_file in list.lines() {
            let path = workdir.join(attributes_file.trim());
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.contains("filter=lfs") {
                        let pattern = line.split_whitespace().next().unwrap_or_default();
                        if !pattern.is_empty() && !tracked.contains(&pattern.to_owned()) {
                            tracked.push(pattern.to_owned());
                        }
                    }
                }
            }
        }
    }
    LfsStatus {
        installed: version.is_some(),
        version,
        tracked_patterns: tracked,
    }
}

/// Runs a `git lfs` subcommand (`pull` | `push` | `install`).
pub fn lfs_run(workdir: &Path, subcommand: &str) -> Result<String, String> {
    let allowed = ["pull", "push", "install", "fetch"];
    if !allowed.contains(&subcommand) {
        return Err(format!("unsupported git lfs subcommand `{subcommand}`"));
    }
    git_run(workdir, &["lfs", subcommand])
}
