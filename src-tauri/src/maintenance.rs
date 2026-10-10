//! Repo health + maintenance (lane M11) — sizes, object counts, and the
//! `git` maintenance commands, all through the sanitized CLI layer
//! (libgit2 has no gc/repack/commit-graph write).
//!
//! Every runner takes the repo workdir and returns combined output; the IPC
//! layer op-queues them (long-running) and toasts the summary. `git gc`
//! writes the commit-graph + packs refs — the "blazingly fast" win on huge
//! repos (libgit2's revwalk reads the commit-graph).

use std::path::Path;

use serde::{Deserialize, Serialize};

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
    let fsck = read_fsck_cache(git_dir);
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
        fsck_dangling: fsck.as_ref().map(|c| c.dangling),
        fsck_samples: fsck.map(|c| c.samples).unwrap_or_default(),
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
    let mut cmd = crate::process::command("git");
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
    /// M12: dangling-object census (`fsck_run`, not plain `git_run` — the
    /// result must be parsed AND cached for the health panel). Wired by the
    Fsck,
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
            MaintenanceOp::Fsck => &["fsck", "--no-progress", "--dangling"],
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
// fsck (dangling-object census; M12)
// ---------------------------------------------------------------------------

/// Cap on dangling shas kept as samples (the count stays exact).
pub const FSCK_SAMPLE_CAP: usize = 10;

/// Cached result of the last fsck run, persisted under
/// `<gitdir>/mygitui/fsck.json` (fsck walks every object, so the health
/// panel shows the last on-demand run instead of computing it per open).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsckCache {
    /// Unix seconds when fsck last ran.
    pub checked_at: i64,
    pub dangling: u64,
    pub samples: Vec<String>,
}

fn fsck_cache_path(git_dir: &Path) -> std::path::PathBuf {
    git_dir.join("mygitui").join("fsck.json")
}

/// Last cached fsck result, when any (`None` = never checked).
pub fn read_fsck_cache(git_dir: &Path) -> Option<FsckCache> {
    let text = std::fs::read_to_string(fsck_cache_path(git_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// `git fsck --no-progress --dangling`: count dangling objects and keep up
/// to [`FSCK_SAMPLE_CAP`] shas, persisting the result to
/// `<gitdir>/mygitui/fsck.json` for [`repo_health`]. Dangling lines can
/// arrive on either stream, so both are parsed; a run whose output carries
/// no dangling lines still refreshes the cache (zero is an answer).
pub fn fsck_run(workdir: &Path, git_dir: &Path) -> Result<(u64, Vec<String>), String> {
    let mut cmd = crate::process::command("git");
    cmd.current_dir(workdir).args(MaintenanceOp::Fsck.args());
    crate::cli::apply_sanitized_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|err| format!("failed to spawn git fsck: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let mut dangling = 0u64;
    let mut samples = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        if let Some(rest) = line.strip_prefix("dangling ") {
            dangling += 1;
            if samples.len() < FSCK_SAMPLE_CAP {
                samples.push(rest.trim().to_owned());
            }
        }
    }

    if !output.status.success() && dangling == 0 {
        // Real corruption (not just dangling objects): surface git's report.
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("git fsck failed: {detail}"));
    }

    let cache = FsckCache {
        checked_at: now_secs(),
        dangling,
        samples: samples.clone(),
    };
    write_fsck_cache(git_dir, &cache);
    Ok((dangling, samples))
}

/// Best-effort cache write (a read-only gitdir just means no caching).
fn write_fsck_cache(git_dir: &Path, cache: &FsckCache) {
    let path = fsck_cache_path(git_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, json);
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-maint-{}-{name}", std::process::id()));
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

    fn init_repo(path: &Path) -> git2::Repository {
        let repo = git2::Repository::init(path).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Maint Test")?;
                c.set_str("user.email", "maint@test.local")
            })
            .expect("configure identity");
        repo
    }

    fn commit_file(repo: &git2::Repository, path: &str, content: &str, message: &str) {
        let file = repo.workdir().unwrap().join(path);
        std::fs::write(file, content).expect("write file");
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .ok();
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("tree");
        let sig = repo.signature().expect("signature");
        let mut parents = Vec::new();
        if let Ok(head) = repo.head() {
            parents.push(head.peel_to_commit().expect("head commit"));
        }
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit");
    }

    #[test]
    fn fsck_op_args_are_pinned() {
        assert_eq!(
            MaintenanceOp::Fsck.args(),
            &["fsck", "--no-progress", "--dangling"]
        );
    }

    #[test]
    fn fsck_run_counts_dangling_and_caches_result() {
        let dir = TempDir::new("fsck");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");

        // A blob written straight into the odb and referenced by nothing is
        // exactly what fsck reports as "dangling blob <sha>".
        let odb = repo.odb().expect("odb");
        odb.write(git2::ObjectType::Blob, b"dangling payload\n")
            .expect("write unreferenced blob");

        let git_dir = dir.path().join(".git");
        let (dangling, samples) = fsck_run(dir.path(), &git_dir).expect("fsck runs");
        assert!(dangling >= 1, "at least the orphan blob dangles");
        assert!(!samples.is_empty(), "samples captured: {samples:?}");
        assert!(
            samples.iter().all(|s| s.len() >= 7),
            "samples are shas: {samples:?}"
        );

        // Cache file written and readable.
        let cache_path = git_dir.join("mygitui").join("fsck.json");
        assert!(
            cache_path.exists(),
            "cache file at {}",
            cache_path.display()
        );
        let cache = read_fsck_cache(&git_dir).expect("cache parses");
        assert_eq!(cache.dangling, dangling);
        assert_eq!(cache.samples, samples);
        assert!(cache.checked_at > 0);

        // repo_health fills the fsck fields from the cache.
        let health = repo_health(dir.path(), &git_dir);
        assert_eq!(health.fsck_dangling, Some(dangling));
        assert_eq!(health.fsck_samples, samples);
    }

    #[test]
    fn repo_health_reports_null_fsck_until_first_run() {
        let dir = TempDir::new("fsck-null");
        init_repo(dir.path());
        let git_dir = dir.path().join(".git");
        let health = repo_health(dir.path(), &git_dir);
        assert_eq!(health.fsck_dangling, None, "never checked");
        assert!(health.fsck_samples.is_empty());
        assert!(!git_dir.join("mygitui").join("fsck.json").exists());
    }

    #[test]
    fn fsck_on_clean_repo_reports_zero_dangling() {
        let dir = TempDir::new("fsck-clean");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");
        let git_dir = dir.path().join(".git");
        let (dangling, samples) = fsck_run(dir.path(), &git_dir).expect("fsck runs");
        assert_eq!(dangling, 0, "fully referenced repo: {samples:?}");
        assert!(samples.is_empty());
        assert_eq!(read_fsck_cache(&git_dir).unwrap().dangling, 0);
    }
}
