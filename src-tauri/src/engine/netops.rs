//! Remote management + fetch/pull/push with progress and credential plumbing.
//!
//! Credentials come from the platform auth broker; progress callbacks are
//! throttled to `PROGRESS_MIN_INTERVAL` except the final `done` tick. Local
//! `file://` remotes never ask for credentials, so tests run without auth.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use git2::build::CheckoutBuilder;
use git2::{
    BranchType, FetchOptions as GitFetchOptions, FetchPrune, PushOptions as GitPushOptions,
    RemoteCallbacks, Repository,
};

use crate::auth;

use super::branches::current_branch_name;
use super::git_engine::{EngineError, EngineResult, FetchProgress, PushProgress};
use super::libgit2::Libgit2Engine;
use super::types::{FetchOptions, NetStats, PullOptions, PushOptions, RebaseStep, RemoteInfo};

/// Minimum spacing between two progress emissions (the final tick is exempt).
const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// shared fetch machinery
// ---------------------------------------------------------------------------

/// State shared between the libgit2 callbacks and the caller-facing progress
/// sink. Callbacks never re-enter each other, so `RefCell` borrow panics are
/// impossible in practice.
struct FetchCtx<'a> {
    progress: &'a mut dyn FnMut(FetchProgress),
    last_emit: Instant,
    updated: Vec<(String, String)>,
    bytes: u64,
    objects_total: u32,
    objects_received: u32,
}

impl<'a> FetchCtx<'a> {
    fn new(progress: &'a mut dyn FnMut(FetchProgress)) -> Self {
        FetchCtx {
            progress,
            last_emit: Instant::now(),
            updated: Vec::new(),
            bytes: 0,
            objects_total: 0,
            objects_received: 0,
        }
    }

    fn snapshot(&self, done: bool) -> FetchProgress {
        FetchProgress {
            bytes: self.bytes,
            objects_total: self.objects_total,
            objects_received: self.objects_received,
            done,
        }
    }

    fn emit_throttled(&mut self) {
        if self.last_emit.elapsed() >= PROGRESS_MIN_INTERVAL {
            self.last_emit = Instant::now();
            let event = self.snapshot(false);
            (self.progress)(event);
        }
    }

    /// Unconditional final tick; guaranteed even when nothing was transferred.
    fn emit_final(&mut self) {
        let event = self.snapshot(true);
        (self.progress)(event);
    }
}

/// Expand a user-facing ref filter into a full refspec:
/// `main` and `refs/heads/main` both become
/// `+refs/heads/main:refs/remotes/<remote>/main` (the leading `+` mirrors the
/// default remote refspec so tracking refs follow rewritten remote history);
/// anything containing `:` passes through verbatim.
fn expand_refspec(remote: &str, spec: &str) -> String {
    if spec.contains(':') {
        return spec.to_owned();
    }
    let src = if spec.starts_with("refs/") {
        spec.to_owned()
    } else {
        format!("refs/heads/{spec}")
    };
    let short = src.strip_prefix("refs/heads/").unwrap_or(&src);
    format!("+{src}:refs/remotes/{remote}/{short}")
}

/// Core fetch: download + update tips + collect stats, running the given
/// explicit refspecs (empty slice = the remote's configured refspecs).
fn fetch_with_refspecs(
    repo: &Repository,
    remote_name: &str,
    refspecs: &[String],
    prune: bool,
    depth: Option<u32>,
    progress: &mut dyn FnMut(FetchProgress),
) -> EngineResult<NetStats> {
    let mut remote = repo
        .find_remote(remote_name)
        .map_err(|e| EngineError::Invalid(format!("remote `{remote_name}` not found: {e}")))?;

    let ctx = RefCell::new(FetchCtx::new(progress));
    let ctx_ref = &ctx;
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(auth::broker().credentials());
    cbs.transfer_progress(move |stats| {
        let mut ctx = ctx_ref.borrow_mut();
        ctx.bytes = stats.received_bytes() as u64;
        ctx.objects_total = stats.total_objects() as u32;
        ctx.objects_received = stats.received_objects() as u32;
        ctx.emit_throttled();
        true
    });
    // Local transports have no sideband; when one arrives, tick progress with
    // the latest known counters.
    cbs.sideband_progress(move |_msg| {
        ctx_ref.borrow_mut().emit_throttled();
        true
    });
    cbs.update_tips(move |refname, _old_oid, new_oid| {
        ctx_ref
            .borrow_mut()
            .updated
            .push((refname.to_owned(), new_oid.to_string()));
        true
    });

    let mut fo = GitFetchOptions::new();
    fo.remote_callbacks(cbs);
    fo.prune(if prune {
        FetchPrune::On
    } else {
        FetchPrune::Unspecified
    });
    if let Some(depth) = depth {
        fo.depth(depth as i32);
    }

    let refs: Vec<&str> = refspecs.iter().map(String::as_str).collect();
    remote.fetch(&refs, Some(&mut fo), None)?;
    // Release the callbacks (they borrow `ctx`) before consuming it.
    drop(fo);
    // Read the counters before disconnecting (remote stats are authoritative
    // over per-callback snapshots).
    let received_bytes = remote.stats().received_bytes() as u64;
    let objects = remote.stats().received_objects() as u32;
    remote.disconnect()?;

    let mut ctx = ctx.into_inner();
    ctx.bytes = received_bytes;
    ctx.objects_received = objects;
    ctx.emit_final();

    Ok(NetStats {
        received_bytes,
        objects,
        updated_refs: ctx.updated,
    })
}

/// Move `local_branch` to `target`, updating the worktree (fast-forward only
/// by construction; also used to seed an unborn branch after a first pull).
fn fast_forward_to(repo: &Repository, local_branch: &str, target: git2::Oid) -> EngineResult<()> {
    let commit = repo.find_commit(target)?;
    let tree = commit.tree()?;
    let mut cb = CheckoutBuilder::new();
    cb.safe();
    repo.checkout_tree(tree.as_object(), Some(&mut cb))?;
    repo.reference(
        &format!("refs/heads/{local_branch}"),
        target,
        true,
        "pull: fast-forward",
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// GitEngine impls
// ---------------------------------------------------------------------------

impl Libgit2Engine {
    pub(crate) fn remotes_impl(&self, repo: &Repository) -> EngineResult<Vec<RemoteInfo>> {
        let mut out = Vec::new();
        // Iterator items are Result<Option<&str>, _>: flatten the Result, then
        // skip non-UTF-8 names.
        for name in repo.remotes()?.iter().flatten().flatten() {
            let remote = repo.find_remote(name)?;
            out.push(RemoteInfo {
                name: name.to_owned(),
                url: remote.url().map(str::to_owned).unwrap_or_default(),
                push_url: remote.pushurl().ok().flatten().map(str::to_owned),
            });
        }
        Ok(out)
    }

    pub(crate) fn remote_add_impl(
        &self,
        repo: &Repository,
        name: &str,
        url: &str,
    ) -> EngineResult<()> {
        repo.remote(name, url)
            .map_err(|e| EngineError::Invalid(format!("cannot add remote `{name}`: {e}")))?;
        Ok(())
    }

    pub(crate) fn remote_remove_impl(&self, repo: &Repository, name: &str) -> EngineResult<()> {
        repo.remote_delete(name)
            .map_err(|e| EngineError::Invalid(format!("cannot remove remote `{name}`: {e}")))?;
        Ok(())
    }

    pub(crate) fn remote_set_url_impl(
        &self,
        repo: &Repository,
        name: &str,
        url: &str,
        push: bool,
    ) -> EngineResult<()> {
        repo.find_remote(name)
            .map_err(|e| EngineError::Invalid(format!("remote `{name}` not found: {e}")))?;
        let which = if push { "push" } else { "fetch" };
        let res = if push {
            repo.remote_set_pushurl(name, Some(url))
        } else {
            repo.remote_set_url(name, url)
        };
        res.map_err(|e| {
            EngineError::Invalid(format!("cannot set {which} url for remote `{name}`: {e}"))
        })?;
        Ok(())
    }

    pub(crate) fn fetch_impl(
        &self,
        repo: &Repository,
        opts: &FetchOptions,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        let refspecs: Vec<String> = opts
            .refs
            .iter()
            .map(|spec| expand_refspec(&opts.remote, spec))
            .collect();
        fetch_with_refspecs(
            repo,
            &opts.remote,
            &refspecs,
            opts.prune,
            opts.depth,
            progress,
        )
    }

    pub(crate) fn pull_impl(
        &self,
        repo: &Repository,
        opts: &PullOptions,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        let current = current_branch_name(repo).ok_or_else(|| {
            EngineError::Invalid("cannot pull: no current branch (detached or unborn HEAD)".into())
        })?;
        let branch = if opts.branch.is_empty() {
            current.clone()
        } else {
            opts.branch.clone()
        };
        let tracking_ref = format!("refs/remotes/{}/{branch}", opts.remote);

        // 1. Fetch just the branch being pulled into its tracking ref (the
        // leading `+` keeps the tracking ref movable when the remote rewrote
        // history, matching the configured `+refs/heads/*` refspec).
        let refspec = format!("+refs/heads/{branch}:{tracking_ref}");
        let stats = fetch_with_refspecs(repo, &opts.remote, &[refspec], false, None, progress)?;

        // 2. Merge the fetched tip into the current branch.
        let tracking = repo.find_reference(&tracking_ref).map_err(|e| {
            EngineError::Invalid(format!(
                "remote branch `{}/{branch}` does not exist: {e}",
                opts.remote
            ))
        })?;
        let annotated = repo.reference_to_annotated_commit(&tracking)?;
        let fetched_oid = tracking.peel_to_commit()?.id();
        let (analysis, _) = repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(stats);
        }
        if analysis.is_fast_forward() || analysis.is_unborn() {
            fast_forward_to(repo, &current, fetched_oid)?;
            return Ok(stats);
        }
        if opts.ff_only {
            return Err(EngineError::Invalid(
                "pull was not a fast-forward: the branches have diverged".into(),
            ));
        }
        if opts.rebase {
            return self.pull_rebase(repo, &current, fetched_oid, stats);
        }

        // Default: merge, favoring fast-forward (handled above).
        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        repo.merge(&[&annotated], None, Some(&mut checkout))?;
        let mut index = repo.index()?;
        if index.has_conflicts() {
            return Err(EngineError::Invalid(format!(
                "pull produced conflicts with `{}/{branch}`: resolve them, then commit to finish the merge",
                opts.remote
            )));
        }
        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let sig = repo.signature()?;
        let head_commit = repo.head()?.peel_to_commit()?;
        let fetched_commit = repo.find_commit(fetched_oid)?;
        let message = format!("Merge branch '{branch}' of {}", opts.remote);
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &message,
            &tree,
            &[&head_commit, &fetched_commit],
        )?;
        // The merge commit exists — drop MERGE_HEAD/MERGE_MSG like `git commit`.
        repo.cleanup_state()?;
        Ok(stats)
    }

    /// Diverged `pull --rebase`: replay the local-only commits onto the
    /// fetched tip through the interactive rebase engine (plan = picks,
    /// onto = fetched tip). A conflict pauses exactly like an interactive
    /// rebase — the FE rebase monitor + conflict editor drive resolution —
    /// and surfaces here as a friendly Invalid error; the fetch stats are
    /// already banked.
    fn pull_rebase(
        &self,
        repo: &Repository,
        current: &str,
        fetched_oid: git2::Oid,
        stats: NetStats,
    ) -> EngineResult<NetStats> {
        let head = repo
            .head()
            .map_err(|_| EngineError::Invalid("cannot rebase: HEAD is unborn".into()))?
            .peel_to_commit()?;
        let base = repo.merge_base(head.id(), fetched_oid).map_err(|_| {
            EngineError::Invalid(format!(
                "cannot rebase `{current}`: no merge base with the fetched tip (histories are unrelated)"
            ))
        })?;
        // Local-only commits, oldest first (pick order).
        let mut walk = repo.revwalk()?;
        walk.push(head.id())?;
        walk.hide(fetched_oid)?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL)?;
        let mut shas: Vec<String> = walk
            .filter_map(Result::ok)
            .filter(|oid| *oid != base)
            .map(|oid| oid.to_string())
            .collect();
        shas.reverse();
        if shas.is_empty() {
            // Not actually diverged (defensive; merge_analysis said so).
            return Ok(stats);
        }
        let plan: Vec<RebaseStep> = shas
            .into_iter()
            .map(|sha| RebaseStep {
                sha,
                action: "pick".to_owned(),
                new_message: None,
            })
            .collect();
        let state = self.rebase_start_impl(repo, &plan, Some(&fetched_oid.to_string()))?;
        if state.active {
            return Err(EngineError::Invalid(format!(
                "pull --rebase hit a conflict on `{current}`: resolve it in the rebase view, then continue (or abort)",
            )));
        }
        Ok(stats)
    }

    pub(crate) fn push_impl(
        &self,
        repo: &Repository,
        opts: &PushOptions,
        progress: &mut dyn FnMut(PushProgress),
    ) -> EngineResult<NetStats> {
        let mut branch_name: Option<String> = None;

        // Resolve the refspecs for this push mode:
        //  * delete  -> `:refs/...` for every ref in `refs`
        //  * refs    -> each ref (bare names become refs/heads/...; tags pass)
        //  * tags    -> every local tag
        //  * default -> `branch` (or the current branch)
        // `full_ref` mirrors git's push refspec shorthand resolution.
        let full_ref = |spec: &str| -> String {
            if spec.starts_with("refs/") {
                spec.to_owned()
            } else if repo.find_reference(&format!("refs/tags/{spec}")).is_ok() {
                format!("refs/tags/{spec}")
            } else {
                format!("refs/heads/{spec}")
            }
        };
        // The remote-tracking ref git would use as the force-with-lease
        // expectation for a branch refspec.
        let require_tracking_ref =
            |repo: &Repository, remote: &str, spec: &str, full: &str| -> Result<(), EngineError> {
                // Lease: the remote must still match the last-fetched
                // tracking ref. libgit2 has no native lease, so the
                // tracking ref IS the expectation — refuse when we have
                // never fetched the branch (we would be blindly forcing).
                let Some(short) = full.strip_prefix("refs/heads/") else {
                    return Err(EngineError::Invalid(format!("cannot lease `{spec}`")));
                };
                let tracking = format!("refs/remotes/{remote}/{short}");
                if repo.find_reference(&tracking).is_err() {
                    return Err(EngineError::Invalid(format!(
                        "force-with-lease refused: no tracking ref `{tracking}` (fetch first)"
                    )));
                }
                Ok(())
            };

        let mut refspecs: Vec<String> = Vec::new();
        if opts.delete {
            if opts.refs.is_empty() {
                return Err(EngineError::Invalid(
                    "delete push requires at least one ref in `refs`".into(),
                ));
            }
            for spec in &opts.refs {
                refspecs.push(format!(":{}", full_ref(spec)));
            }
        } else {
            if opts.tags {
                for entry in repo.references_glob("refs/tags/*")? {
                    let reference = entry?;
                    if let Ok(name) = reference.name() {
                        refspecs.push(format!("{name}:{name}"));
                    }
                }
                if refspecs.is_empty() {
                    return Err(EngineError::Invalid("no local tags to push".into()));
                }
            }
            for spec in &opts.refs {
                let full = full_ref(spec);
                let force = if opts.force_with_lease && full.starts_with("refs/heads/") {
                    require_tracking_ref(&repo, &opts.remote, spec, &full)?;
                    true
                } else {
                    opts.force
                };
                refspecs.push(format!("{}{full}:{full}", if force { "+" } else { "" }));
            }
            if refspecs.is_empty() {
                let name = if opts.branch.is_empty() {
                    current_branch_name(repo).ok_or_else(|| {
                        EngineError::Invalid(
                            "cannot push: no current branch (detached or unborn HEAD)".into(),
                        )
                    })?
                } else {
                    opts.branch.clone()
                };
                repo.find_branch(&name, BranchType::Local)
                    .map_err(|e| EngineError::Invalid(format!("branch `{name}` not found: {e}")))?
                    .get()
                    .peel_to_commit()
                    .map_err(|_| {
                        EngineError::Invalid(format!("branch `{name}` is unborn; nothing to push"))
                    })?;
                if opts.force_with_lease {
                    require_tracking_ref(
                        &repo,
                        &opts.remote,
                        &name,
                        &format!("refs/heads/{name}"),
                    )?;
                }
                let force = opts.force || opts.force_with_lease;
                refspecs.push(format!(
                    "{}refs/heads/{name}:refs/heads/{name}",
                    if force { "+" } else { "" }
                ));
                branch_name = Some(name);
            }
        }

        let mut remote = repo.find_remote(&opts.remote).map_err(|e| {
            EngineError::Invalid(format!("remote `{}` not found: {e}", opts.remote))
        })?;

        // Per-remote-ref new values for the stats map: source ref target for
        // updates, "(deleted)" for delete refspecs.
        let mut new_values: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for spec in &refspecs {
            if let Some((src, dst)) = spec.split_once(':') {
                let value = if src.is_empty() {
                    "(deleted)".to_owned()
                } else {
                    repo.find_reference(src)
                        .ok()
                        .and_then(|r| r.target())
                        .map(|o| o.to_string())
                        .unwrap_or_default()
                };
                new_values.insert(dst.to_owned(), value);
            }
        }

        struct PushCtx<'a> {
            progress: &'a mut dyn FnMut(PushProgress),
            last_emit: Instant,
            current: u32,
            total: u32,
            bytes: u64,
        }
        let ctx = RefCell::new(PushCtx {
            progress,
            last_emit: Instant::now(),
            current: 0,
            total: 0,
            bytes: 0,
        });
        let ctx_ref = &ctx;
        let updated = RefCell::new(Vec::new());
        let updated_ref = &updated;

        let mut cbs = RemoteCallbacks::new();
        cbs.credentials(auth::broker().credentials());
        cbs.push_transfer_progress(move |current, total, bytes| {
            let mut ctx = ctx_ref.borrow_mut();
            ctx.current = current as u32;
            ctx.total = total as u32;
            ctx.bytes = bytes as u64;
            if ctx.last_emit.elapsed() >= PROGRESS_MIN_INTERVAL {
                ctx.last_emit = Instant::now();
                // Destructure for disjoint borrows: the callee is `progress`
                // while the arguments read the counter fields.
                let PushCtx {
                    progress,
                    current,
                    total,
                    bytes,
                    ..
                } = &mut *ctx;
                (*progress)(PushProgress {
                    current: *current,
                    total: *total,
                    bytes: *bytes,
                    message: String::new(),
                });
            }
        });
        cbs.push_update_reference(move |refname, status| {
            if let Some(reason) = status {
                return Err(git2::Error::from_str(&format!(
                    "push of `{refname}` rejected: {reason}"
                )));
            }
            // New value per ref: the local source ref's target for updates,
            // "(deleted)" for delete refspecs.
            let new_value = new_values
                .get(refname)
                .cloned()
                .unwrap_or_else(|| "(unknown)".to_owned());
            updated_ref
                .borrow_mut()
                .push((refname.to_owned(), new_value));
            Ok(())
        });

        let mut po = GitPushOptions::new();
        po.remote_callbacks(cbs);
        let spec_refs: Vec<&str> = refspecs.iter().map(String::as_str).collect();
        remote.push(&spec_refs, Some(&mut po))?;
        // Release the callbacks (they borrow `ctx` and `updated`) before
        // consuming them.
        drop(po);

        // Unconditional final tick (push progress has no done flag; message
        // marks completion).
        let push_ctx = ctx.into_inner();
        (push_ctx.progress)(PushProgress {
            current: push_ctx.current,
            total: push_ctx.total,
            bytes: push_ctx.bytes,
            message: "done".to_owned(),
        });

        if let (Some(name), true) = (branch_name.as_deref(), opts.set_upstream) {
            let mut local = repo.find_branch(name, BranchType::Local)?;
            local.set_upstream(Some(&format!("{}/{}", opts.remote, name)))?;
        }

        Ok(NetStats {
            received_bytes: push_ctx.bytes,
            objects: push_ctx.total,
            updated_refs: updated.into_inner(),
        })
    }
}
