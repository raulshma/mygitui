//! Submodule inspection and management — M4 lane F3.
//!
//! Implementations live as inherent `*_impl` methods on `Libgit2Engine`
//! (the lane convention; the IPC commands call them directly, like the M3
//! checkpoint/merge commands do).
//!
//! Semantics notes:
//! - `submodules_impl` walks `repo.submodules()`. For every submodule:
//!   `recorded_sha` is the gitlink the superproject's index records
//!   (`Submodule::index_id`), `head_sha` is what is actually checked out in
//!   the submodule worktree (the opened sub-repository's HEAD; `None` when
//!   the submodule is not initialized / has an unborn HEAD). `initialized`
//!   means the submodule directory is an openable repository; a submodule
//!   whose directory exists but cannot be opened reports status "broken".
//! - `status` mirrors `git status` wording, joined with " / ":
//!   "new commits" (checked-out HEAD differs from the recorded gitlink),
//!   "modified content" (tracked worktree changes), "untracked content"
//!   (untracked files inside the submodule), "" when clean.
//! - `submodule_update_impl` = `git submodule update [--init] [--recursive]`
//!   for ONE submodule: optionally `init`, then `Submodule::update` (clone /
//!   fetch + checkout of the recorded gitlink; credentials come from the
//!   auth broker for network remotes), then the same for nested submodules
//!   when `recursive` (depth-capped).
//! - `submodule_sync_impl` = `git submodule sync`: copy the `.gitmodules`
//!   URL into the repository config, for one submodule or all. Per-submodule
//!   failures are collected (best-effort, like git's own loop) and the first
//!   one is returned.

use git2::{Repository, Status, StatusOptions, SubmoduleUpdateOptions};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::SubmoduleInfo;

/// Recursion cap for `submodule_update(recursive = true)`: nested submodule
/// trees deeper than this stop descending (defensive bound against
/// pathological / self-referencing module graphs).
const MAX_SUBMODULE_DEPTH: u32 = 3;

/// Worktree-side status flags that count as "modified content" (untracked
/// files are reported separately as "untracked content").
const WT_DIRTY: Status = Status::WT_MODIFIED
    .union(Status::WT_DELETED)
    .union(Status::WT_TYPECHANGE)
    .union(Status::WT_RENAMED)
    .union(Status::WT_UNREADABLE);

#[allow(dead_code)]
impl Libgit2Engine {
    /// List every submodule of `repo` with checked-out vs recorded state.
    pub(crate) fn submodules_impl(&self, repo: &Repository) -> EngineResult<Vec<SubmoduleInfo>> {
        // Not a worktree (bare repo) or .gitmodules unreadable: no submodules.
        let subs = match repo.submodules() {
            Ok(subs) => subs,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::with_capacity(subs.len());
        for sub in subs {
            let name = sub.name().unwrap_or_default().to_owned();
            // Normalize to forward slashes: the FE joins `root + "/" + path`
            // and passes the result around as a portable path.
            let path = sub.path().to_string_lossy().replace('\\', "/");
            let url = sub.url().ok().flatten().unwrap_or_default().to_owned();
            let recorded = sub.index_id();
            let recorded_sha = recorded.map(|oid| oid.to_string()).unwrap_or_default();

            let opened = sub.open();
            let initialized = opened.is_ok();
            let (head_sha, status) = match opened {
                Err(_) => (None, "broken".to_string()),
                Ok(sub_repo) => {
                    let head = sub_repo.head().ok().and_then(|h| h.target());
                    let status = submodule_status(&sub_repo, head, recorded);
                    (head.map(|oid| oid.to_string()), status)
                }
            };
            out.push(SubmoduleInfo {
                path,
                name,
                url,
                head_sha,
                recorded_sha,
                initialized,
                status,
            });
        }
        Ok(out)
    }

    /// `git submodule update [--init] [--recursive] <path>`: bring the
    /// submodule at `path` to the gitlink recorded in the superproject.
    pub(crate) fn submodule_update_impl(
        &self,
        repo: &Repository,
        path: &str,
        init: bool,
        recursive: bool,
    ) -> EngineResult<()> {
        let mut sub = find_submodule(repo, path)?;
        update_one(&mut sub, init)?;
        if recursive {
            // Descend into the freshly checked-out sub-repository.
            if let Ok(nested) = sub.open() {
                update_all_recursive(&nested, init, 1)?;
            }
        }
        Ok(())
    }

    /// `git submodule sync [path]`: rewrite each submodule's URL from
    /// `.gitmodules` into the repository config. `None`/empty `path` = all.
    pub(crate) fn submodule_sync_impl(
        &self,
        repo: &Repository,
        path: Option<&str>,
    ) -> EngineResult<()> {
        match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(path) => {
                let mut sub = find_submodule(repo, path)?;
                sub.sync()?;
            }
            None => {
                // Best-effort like `git submodule sync`: keep syncing the
                // remaining modules, report the first failure.
                let mut first_error: Option<EngineError> = None;
                for mut sub in repo.submodules()? {
                    if let Err(err) = sub.sync() {
                        if first_error.is_none() {
                            first_error = Some(err.into());
                        }
                    }
                }
                if let Some(err) = first_error {
                    return Err(err);
                }
            }
        }
        Ok(())
    }
}

/// "new commits" / "modified content" / "untracked content", " / "-joined;
/// "" when clean, "broken" when the sub-repository cannot be interrogated.
fn submodule_status(
    sub_repo: &Repository,
    head: Option<git2::Oid>,
    recorded: Option<git2::Oid>,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if head != recorded {
        parts.push("new commits");
    }
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);
    match sub_repo.statuses(Some(&mut opts)) {
        Ok(statuses) => {
            let mut modified = false;
            let mut untracked = false;
            for entry in statuses.iter() {
                let flags = entry.status();
                if flags.contains(Status::WT_NEW) {
                    untracked = true;
                }
                if flags.intersects(WT_DIRTY) {
                    modified = true;
                }
            }
            if modified {
                parts.push("modified content");
            }
            if untracked {
                parts.push("untracked content");
            }
        }
        // A sub-repository that opens but cannot be statused is broken.
        Err(_) => return "broken".to_string(),
    }
    parts.join(" / ")
}

/// Find a submodule by name (usually the path), falling back to a path scan
/// (names can diverge from paths when a module was added twice).
fn find_submodule<'r>(repo: &'r Repository, path: &str) -> EngineResult<git2::Submodule<'r>> {
    if let Ok(sub) = repo.find_submodule(path) {
        return Ok(sub);
    }
    for sub in repo.submodules()? {
        if sub.path().to_string_lossy() == path {
            return Ok(sub);
        }
    }
    Err(EngineError::Invalid(format!(
        "submodule `{path}` not found"
    )))
}

/// Init-then-update one submodule to its recorded gitlink. Fetching (clone
/// or a fetch of the missing target commit) uses the auth broker's
/// credential callbacks; local `file://` remotes never ask for credentials.
fn update_one(sub: &mut git2::Submodule<'_>, init: bool) -> EngineResult<()> {
    if init {
        // overwrite=false: a no-op for already-initialized modules.
        sub.init(false)?;
    }
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(crate::auth::broker().credentials());
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);
    let mut opts = SubmoduleUpdateOptions::new();
    opts.fetch(fetch); // also re-enables allow_fetch
    sub.update(init, Some(&mut opts))?;
    Ok(())
}

/// Recursively update every submodule of `repo` (the `--recursive` part).
fn update_all_recursive(repo: &Repository, init: bool, depth: u32) -> EngineResult<()> {
    if depth > MAX_SUBMODULE_DEPTH {
        return Ok(());
    }
    for mut sub in repo.submodules()? {
        update_one(&mut sub, init)?;
        if let Ok(nested) = sub.open() {
            update_all_recursive(&nested, init, depth + 1)?;
        }
    }
    Ok(())
}
