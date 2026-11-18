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

/// Argument guard for the CLI-lane subcommands that take user-supplied
/// values (archive spec/destination, bundle out-path/refs, lfs lock paths,
/// note refs/shas). Arguments reach git as argv entries (never a shell), but
/// the IPC contract promises a metacharacter guard: reject shell
/// metacharacters, control characters, and option-shaped values (a leading
/// `-` would be parsed as a flag). Formerly `archive_arg_ok` next to the
/// archive command; moved here so every CLI lane (engine + IPC) guards
/// through one home.
pub(crate) fn cli_arg_ok(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.chars().any(|c| {
            matches!(
                c,
                ';' | '&'
                    | '|'
                    | '$'
                    | '`'
                    | '('
                    | ')'
                    | '<'
                    | '>'
                    | '"'
                    | '\''
                    | '\n'
                    | '\r'
                    | '\0'
            )
        })
}

/// URL/destination guard for the CLI clone family (`clone_blobless` IPC
/// command, `submodule_add`): argv entries never touch a shell, but a
/// leading `-` would be parsed as a git flag — clone accepts option-shaped
/// transfer configs right after its subcommand (`--upload-pack=<cmd>` runs
/// it), so option-shaped URLs are rejected up front. The `;` check keeps the
/// original metacharacter contract.
pub(crate) fn clone_args_ok(url: &str, destination: &str) -> bool {
    !url.is_empty() && !url.starts_with('-') && !url.contains(';') && !destination.is_empty()
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
        } else if let Some(v) = line.strip_prefix("size-pack: ") {
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

use crate::engine::types::{LfsLock, LfsStatus, NoteInfo, SparseInfo};

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

/// Runs a `git lfs` subcommand (`pull` | `push` | `install` | `fetch`).
pub fn lfs_run(workdir: &Path, subcommand: &str) -> Result<String, String> {
    let allowed = ["pull", "push", "install", "fetch"];
    if !allowed.contains(&subcommand) {
        return Err(format!("unsupported git lfs subcommand `{subcommand}`"));
    }
    git_run(workdir, &["lfs", subcommand])
}

// ---------------------------------------------------------------------------
// git lfs locks (Phase 5 item 48a)
// ---------------------------------------------------------------------------

/// `git lfs locks --json` item (subset we surface; unknown fields ignored).
#[derive(Deserialize)]
struct LfsLockJson {
    #[serde(default)]
    id: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    owner: Option<LfsOwnerJson>,
}

#[derive(Deserialize)]
struct LfsOwnerJson {
    #[serde(default)]
    name: String,
}

/// Parse `git lfs locks --json` output into [`LfsLock`] entries; falls back
/// to the plain-text lines (tab-separated `id`, `owner`, `path` — older lfs
/// builds / non-JSON output) when JSON parsing fails. A data row is
/// recognized by its numeric id, so a header line never slips through.
pub(crate) fn parse_lfs_locks(raw: &str) -> Vec<LfsLock> {
    if let Ok(items) = serde_json::from_str::<Vec<LfsLockJson>>(raw) {
        return items
            .into_iter()
            .map(|lock| LfsLock {
                id: lock.id,
                path: lock.path,
                owner: lock.owner.map(|o| o.name).unwrap_or_default(),
            })
            .collect();
    }
    let mut out = Vec::new();
    for line in raw.lines() {
        let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
        if fields.len() < 3 || fields[0].parse::<u64>().is_err() {
            continue;
        }
        out.push(LfsLock {
            id: fields[0].to_owned(),
            owner: fields[1].to_owned(),
            path: fields[2].to_owned(),
        });
    }
    out
}

/// argv for `git lfs locks --json` (pure; unit-tested — the real run needs
/// git-lfs installed).
pub(crate) fn lfs_locks_args() -> Vec<String> {
    vec!["lfs".into(), "locks".into(), "--json".into()]
}

/// argv for `git lfs lock <path>` (pure; same reasoning as
/// [`lfs_locks_args`]).
pub(crate) fn lfs_lock_args(path: &str) -> Vec<String> {
    vec!["lfs".into(), "lock".into(), path.into()]
}

/// argv for `git lfs unlock <path>` (pure).
pub(crate) fn lfs_unlock_args(path: &str) -> Vec<String> {
    vec!["lfs".into(), "unlock".into(), path.into()]
}

/// Shared path guard for the lfs lock/unlock lanes: same rules as
/// [`cli_arg_ok`] with a friendly message. Called before spawning so an
/// option-shaped path never reaches git's own flag parser.
fn lfs_path_checked(path: &str) -> Result<(), String> {
    if cli_arg_ok(path) {
        return Ok(());
    }
    Err(format!("invalid lfs lock path `{path}`"))
}

/// `git lfs locks` — list file locks (JSON preferred; plain-text fallback,
/// see [`parse_lfs_locks`]).
pub fn lfs_locks(workdir: &Path) -> Result<Vec<LfsLock>, String> {
    let argv: Vec<String> = lfs_locks_args();
    let raw = git_run_argv(workdir, &argv).or_else(|_| git_run(workdir, &["lfs", "locks"]))?;
    Ok(parse_lfs_locks(&raw))
}

/// `git lfs lock <path>` — lock a file; returns git-lfs's output line.
pub fn lfs_lock(workdir: &Path, path: &str) -> Result<String, String> {
    lfs_path_checked(path)?;
    let argv: Vec<String> = lfs_lock_args(path);
    git_run_argv(workdir, &argv)
}

/// `git lfs unlock <path>` — release a lock; returns git-lfs's output line.
pub fn lfs_unlock(workdir: &Path, path: &str) -> Result<String, String> {
    lfs_path_checked(path)?;
    let argv: Vec<String> = lfs_unlock_args(path);
    git_run_argv(workdir, &argv)
}

/// [`git_run`] taking owned strings (the argv builders above produce them).
fn git_run_argv(workdir: &Path, argv: &[String]) -> Result<String, String> {
    let arg_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    git_run(workdir, &arg_refs)
}

// ---------------------------------------------------------------------------
// git bundle (Phase 5 item 48d)
// ---------------------------------------------------------------------------

/// `git bundle create <out_path> (--all | refs...)` — a portable packfile of
/// the selected refs (mirror of the archive CLI lane). The caller validates
/// `out_path`/refs with [`cli_arg_ok`] (the archive arg guard family);
/// `all` and `refs` are mutually exclusive exactly like the CLI.
pub fn bundle_create(
    workdir: &Path,
    out_path: &str,
    refs: &[String],
    all: bool,
) -> Result<(), String> {
    if all && !refs.is_empty() {
        return Err("bundle takes either `all` or explicit refs, not both".into());
    }
    if !all && refs.is_empty() {
        return Err("bundle requires either `all` or at least one ref".into());
    }
    if !cli_arg_ok(out_path) {
        return Err(format!("invalid bundle destination `{out_path}`"));
    }
    for r in refs {
        if !cli_arg_ok(r) {
            return Err(format!("invalid bundle ref `{r}`"));
        }
    }
    let mut args: Vec<&str> = vec!["bundle", "create", out_path];
    if all {
        args.push("--all");
    }
    let ref_refs: Vec<&str> = refs.iter().map(String::as_str).collect();
    args.extend(ref_refs.iter().copied());
    git_run(workdir, &args)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// git notes (Phase 5 item 48e)
// ---------------------------------------------------------------------------

/// The notes ref `git notes` uses when none is given.
pub(crate) const DEFAULT_NOTES_REF: &str = "refs/notes/commits";

/// The `--ref=<r>` flag for the notes CLI edges; `None`/empty resolves to
/// [`DEFAULT_NOTES_REF`]. `r` is guarded with [`cli_arg_ok`] (an
/// option-shaped ref would be parsed as a git flag).
fn notes_ref_flag(note_ref: Option<&str>) -> Result<String, String> {
    let r = note_ref
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(DEFAULT_NOTES_REF);
    if !cli_arg_ok(r) {
        return Err(format!("invalid notes ref `{r}`"));
    }
    Ok(format!("--ref={r}"))
}

/// Sha guard shared by the notes commands: commit-ishes are fine, but an
/// option-shaped value must never reach git's flag parser.
fn note_sha_checked(sha: &str) -> Result<(), String> {
    if cli_arg_ok(sha) {
        return Ok(());
    }
    Err(format!("invalid commit sha `{sha}`"))
}

/// Every note attached to `sha`, one entry per notes ref that holds a note
/// for it (`git notes --ref <r> show <sha>`; a missing note fails the
/// subcommand, which simply skips that ref).
pub fn commit_notes(workdir: &Path, sha: &str) -> Result<Vec<NoteInfo>, String> {
    note_sha_checked(sha)?;
    let refs = git_run(
        workdir,
        &["for-each-ref", "--format=%(refname)", "refs/notes/"],
    )?;
    let mut out = Vec::new();
    for r in refs.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let flag = notes_ref_flag(Some(r))?;
        if let Ok(text) = git_run(workdir, &["notes", &flag, "show", sha]) {
            out.push(NoteInfo {
                r#ref: r.to_owned(),
                text,
            });
        }
    }
    Ok(out)
}

/// `git notes add [--ref <r>] -F <file> <sha>` — the note text travels via
/// the commit-message temp-file helper (multi-line safe, argv-only, never a
/// shell; the file is removed afterwards like every `-F` consumer).
pub fn note_add(
    workdir: &Path,
    sha: &str,
    text: &str,
    note_ref: Option<&str>,
) -> Result<(), String> {
    note_sha_checked(sha)?;
    let flag = notes_ref_flag(note_ref)?;
    let file = crate::cli::write_message_file("note", text)?;
    let result = git_run(
        workdir,
        &["notes", &flag, "add", "-F", &file.to_string_lossy(), sha],
    );
    let _ = std::fs::remove_file(&file);
    result.map(|_| ())
}

/// `git notes remove [--ref <r>] <sha>` — deletes the note under that ref.
pub fn note_remove(workdir: &Path, sha: &str, note_ref: Option<&str>) -> Result<(), String> {
    note_sha_checked(sha)?;
    let flag = notes_ref_flag(note_ref)?;
    git_run(workdir, &["notes", &flag, "remove", sha])?;
    Ok(())
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

    fn commit_file(repo: &git2::Repository, path: &str, content: &str, message: &str) -> String {
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
            .expect("commit")
            .to_string()
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

    // -- lfs (Phase 5 item 48a) ----------------------------------------------

    #[test]
    fn lfs_run_rejects_arbitrary_subcommands() {
        let dir = TempDir::new("lfs-allow");
        init_repo(dir.path());
        // The allowlist stays pull/push/install/fetch: anything else (here
        // `env`, which would dump git-lfs's environment) is rejected before
        // git is spawned.
        for subcommand in ["env", "locks", "lock", "unlock", "version", ""] {
            let err = lfs_run(dir.path(), subcommand).expect_err(subcommand);
            assert!(
                err.contains("unsupported git lfs subcommand"),
                "{subcommand}: {err}"
            );
        }
        // The allowed ones pass the guard (they fail here with git-lfs not
        // installed / not a lfs repo — not with the unsupported message).
        for subcommand in ["pull", "push", "install", "fetch"] {
            let err = lfs_run(dir.path(), subcommand).unwrap_err();
            assert!(
                !err.contains("unsupported git lfs subcommand"),
                "{subcommand}: {err}"
            );
        }
    }

    #[test]
    fn lfs_argv_builders_are_pinned() {
        assert_eq!(lfs_locks_args(), vec!["lfs", "locks", "--json"]);
        assert_eq!(lfs_lock_args("a.bin"), vec!["lfs", "lock", "a.bin"]);
        assert_eq!(lfs_unlock_args("a.bin"), vec!["lfs", "unlock", "a.bin"]);
    }

    #[test]
    fn lfs_lock_arg_guards_reject_option_shaped_and_empty_paths() {
        let dir = TempDir::new("lfs-guard");
        init_repo(dir.path());
        // Guards fire before any spawn: a leading `-` would be parsed as a
        // git flag (option injection), metacharacters break the IPC contract.
        for path in ["", "-x", "--upload-pack=touch /tmp/x", "a;b", "$(danger)"] {
            let err = lfs_lock(dir.path(), path).expect_err(path);
            assert!(err.contains("invalid lfs lock path"), "{path}: {err}");
            let err = lfs_unlock(dir.path(), path).expect_err(path);
            assert!(err.contains("invalid lfs lock path"), "{path}: {err}");
        }
    }

    #[test]
    fn parse_lfs_locks_json_and_text_fallback() {
        // Primary shape: `git lfs locks --json`.
        let json = r#"[{"id":"12","path":"a.bin","owner":{"name":"bob"},
                        "locked_at":"2026-01-01T00:00:00Z"}]"#;
        let locks = parse_lfs_locks(json);
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].id, "12");
        assert_eq!(locks[0].path, "a.bin");
        assert_eq!(locks[0].owner, "bob");

        // Missing owner object parses to "".
        assert_eq!(parse_lfs_locks(r#"[{"id":"1","path":"p"}]"#)[0].owner, "");

        // Text fallback: tab-separated id/owner/path, header skipped.
        let text = "ID\tOwner\tPath\n7\talice\tbig.bin\n8\t\tsub dir/b.bin\n";
        let locks = parse_lfs_locks(text);
        assert_eq!(locks.len(), 2);
        assert_eq!(locks[0].id, "7");
        assert_eq!(locks[0].owner, "alice");
        assert_eq!(locks[0].path, "big.bin");
        assert_eq!(locks[1].owner, "");
        assert_eq!(locks[1].path, "sub dir/b.bin");

        // Garbage parses to nothing (never panics the listing).
        assert!(parse_lfs_locks("").is_empty());
        assert!(parse_lfs_locks("not json at all").is_empty());
    }

    // -- bundle (Phase 5 item 48d) -------------------------------------------

    /// `git bundle verify` via std::process (independent of the runner under
    /// test), mirroring the spec's verification suggestion.
    fn bundle_verify(workdir: &Path, bundle: &Path) -> Result<(), String> {
        let out = std::process::Command::new("git")
            .args(["bundle", "verify"])
            .arg(bundle)
            .current_dir(workdir)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    #[test]
    fn bundle_create_round_trips_via_verify() {
        let dir = TempDir::new("bundle-all");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");

        // --all form.
        let all_path = dir.path().join("all.bundle");
        bundle_create(dir.path(), all_path.to_str().unwrap(), &[], true).expect("bundle --all");
        assert!(all_path.metadata().expect("bundle file").len() > 0);
        bundle_verify(dir.path(), &all_path).expect("git bundle verify --all bundle");

        // Explicit-refs form (HEAD is a valid bundle rev).
        let refs_path = dir.path().join("refs.bundle");
        bundle_create(
            dir.path(),
            refs_path.to_str().unwrap(),
            &["HEAD".to_owned()],
            false,
        )
        .expect("bundle with refs");
        bundle_verify(dir.path(), &refs_path).expect("git bundle verify refs bundle");
    }

    #[test]
    fn bundle_create_rejects_bad_input_before_spawning() {
        let dir = TempDir::new("bundle-guard");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");

        // Exactly one of (all, refs) — never both, never neither.
        let both = bundle_create(
            dir.path(),
            dir.path().join("x.bundle").to_str().unwrap(),
            &["HEAD".to_owned()],
            true,
        )
        .unwrap_err();
        assert!(both.contains("not both"), "{both}");
        let neither = bundle_create(dir.path(), "x.bundle", &[], false).unwrap_err();
        assert!(neither.contains("at least one ref"), "{neither}");

        // Destination and ref guards (option-shaped values would be parsed
        // as git flags).
        let err = bundle_create(dir.path(), "-o", &[], true).unwrap_err();
        assert!(err.contains("invalid bundle destination"), "{err}");
        let err = bundle_create(dir.path(), "x.bundle", &["--all".to_owned()], false).unwrap_err();
        assert!(err.contains("invalid bundle ref"), "{err}");

        // Nothing was written by the rejected runs.
        assert!(!dir.path().join("x.bundle").exists());
    }

    // -- notes (Phase 5 item 48e) --------------------------------------------

    #[test]
    fn notes_add_list_remove_round_trip() {
        let dir = TempDir::new("notes");
        let repo = init_repo(dir.path());
        let sha = commit_file(&repo, "a.txt", "one\n", "base");

        // Empty listing before anything is added.
        assert!(commit_notes(dir.path(), &sha)
            .expect("empty list")
            .is_empty());

        // Add under the default ref (multi-line text via the message file).
        // The runner trims trailing whitespace, so the stored text loses its
        // final newline (git notes content itself is unchanged).
        note_add(dir.path(), &sha, "review note\nsecond line\n", None).expect("note add");
        let notes = commit_notes(dir.path(), &sha).expect("list");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].r#ref, "refs/notes/commits");
        assert_eq!(notes[0].text, "review note\nsecond line");

        // A second notes ref holds an independent note for the same sha.
        note_add(dir.path(), &sha, "audit", Some("refs/notes/audit")).expect("note add alt");
        let notes = commit_notes(dir.path(), &sha).expect("list both");
        assert_eq!(notes.len(), 2);
        assert!(notes
            .iter()
            .any(|n| n.r#ref == "refs/notes/audit" && n.text == "audit"));

        // Remove round-trips per ref.
        note_remove(dir.path(), &sha, None).expect("remove default");
        let notes = commit_notes(dir.path(), &sha).expect("list after remove");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].r#ref, "refs/notes/audit");
        note_remove(dir.path(), &sha, Some("refs/notes/audit")).expect("remove alt");
        assert!(commit_notes(dir.path(), &sha)
            .expect("final list")
            .is_empty());
    }

    #[test]
    fn notes_commands_guard_sha_and_ref_before_spawning() {
        let dir = TempDir::new("notes-guard");
        init_repo(dir.path());
        // Option-shaped values would be parsed as git flags.
        let err = commit_notes(dir.path(), "--all").unwrap_err();
        assert!(err.contains("invalid commit sha"), "{err}");
        let err = note_add(dir.path(), "-x", "t", None).unwrap_err();
        assert!(err.contains("invalid commit sha"), "{err}");
        let err = note_add(dir.path(), "abc", "t", Some("--exec=touch")).unwrap_err();
        assert!(err.contains("invalid notes ref"), "{err}");
        let err = note_remove(dir.path(), "abc", Some(";bad")).unwrap_err();
        assert!(err.contains("invalid notes ref"), "{err}");
        // Empty/None ref resolves to the default.
        assert_eq!(
            notes_ref_flag(None).expect("default ref"),
            "--ref=refs/notes/commits"
        );
    }
}
