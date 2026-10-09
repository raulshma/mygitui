//! Remote management + fetch/pull/push with progress and credential plumbing.
//!
//! Credentials come from the platform auth broker; progress callbacks are
//! throttled to `PROGRESS_MIN_INTERVAL` except the final `done` tick. Local
//! `file://` remotes never ask for credentials, so tests run without auth.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Error raised when an op observes its cancellation token.
fn cancelled_err() -> EngineError {
    EngineError::Invalid("cancelled".to_owned())
}

/// Core fetch: download + update tips + collect stats, running the given
/// explicit refspecs (empty slice = the remote's configured refspecs).
/// `cancel` is checked between phases and inside the transfer-progress
/// callback (returning `false` aborts the pack transfer mid-flight).
fn fetch_with_refspecs(
    repo: &Repository,
    remote_name: &str,
    refspecs: &[String],
    prune: bool,
    depth: Option<u32>,
    progress: &mut dyn FnMut(FetchProgress),
    cancel: Option<&AtomicBool>,
) -> EngineResult<NetStats> {
    if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
        return Err(cancelled_err());
    }
    let mut remote = repo
        .find_remote(remote_name)
        .map_err(|e| EngineError::Invalid(format!("remote `{remote_name}` not found: {e}")))?;

    let ctx = RefCell::new(FetchCtx::new(progress));
    let ctx_ref = &ctx;
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(auth::broker().credentials());
    cbs.transfer_progress(move |stats| {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return false; // aborts the pack transfer
        }
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
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return false;
        }
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
    let fetch_result = remote.fetch(&refs, Some(&mut fo), None);
    // A cancelled transfer surfaces as a generic git error — translate it.
    if fetch_result.is_err() && cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
        return Err(cancelled_err());
    }
    fetch_result?;
    // Release the callbacks (they borrow `ctx`) before consuming it.
    drop(fo);
    if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
        return Err(cancelled_err());
    }
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
        self.fetch_impl_ext(repo, opts, progress, None)
    }

    /// Cancellation-aware fetch: `cancel` is polled between phases and inside
    /// the transfer callbacks. Wired by the op queue (M12); `fetch_impl` keeps
    /// the trait-facing signature.
    pub(crate) fn fetch_impl_ext(
        &self,
        repo: &Repository,
        opts: &FetchOptions,
        progress: &mut dyn FnMut(FetchProgress),
        cancel: Option<&AtomicBool>,
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
            cancel,
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
        let stats =
            fetch_with_refspecs(repo, &opts.remote, &[refspec], false, None, progress, None)?;

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
        self.push_impl_ext(repo, opts, progress, None)
    }

    /// Cancellation-aware push: `cancel` is polled between phases. Wired by
    /// the op queue (M12); `push_impl` keeps the trait-facing signature.
    pub(crate) fn push_impl_ext(
        &self,
        repo: &Repository,
        opts: &PushOptions,
        progress: &mut dyn FnMut(PushProgress),
        cancel: Option<&AtomicBool>,
    ) -> EngineResult<NetStats> {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(cancelled_err());
        }
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
        // Force-with-lease expectation for one branch refspec: the CURRENT
        // oid of the local remote-tracking ref — exactly what
        // `--force-with-lease=<ref>:<oid>` encodes. Refuses when the branch
        // was never fetched (we would be blind-forcing).
        let lease_expectation = |repo: &Repository,
                                 remote: &str,
                                 spec: &str,
                                 full: &str|
         -> EngineResult<(String, String)> {
            let Some(short) = full.strip_prefix("refs/heads/") else {
                return Err(EngineError::Invalid(format!("cannot lease `{spec}`")));
            };
            let tracking = format!("refs/remotes/{remote}/{short}");
            let tracking_ref = repo.find_reference(&tracking).map_err(|_| {
                EngineError::Invalid(format!(
                    "force-with-lease refused: no tracking ref `{tracking}` (fetch first)"
                ))
            })?;
            let oid = tracking_ref.target().ok_or_else(|| {
                    EngineError::Invalid(format!(
                        "force-with-lease refused: tracking ref `{tracking}` has no target (fetch first)"
                    ))
                })?;
            Ok((full.to_owned(), oid.to_string()))
        };

        // Real `--force-with-lease` flags (branch refspecs only; deletes and
        // tags never lease). Non-empty routes the push through the CLI —
        // libgit2 has no native lease, and the client-side approximation
        // cannot see a concurrent remote update between fetch and push.
        let mut leases: Vec<(String, String)> = Vec::new();
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
                // A `+` refspec is per-ref --force and SILENTLY CANCELS the
                // lease (verified against git 2.55: leased `+` refspecs push
                // straight through stale remotes). Leased refs therefore get
                // a plain refspec — `--force-with-lease=<ref>:<oid>` is what
                // authorizes the non-fast-forward update when it holds.
                let force = if opts.force_with_lease && full.starts_with("refs/heads/") {
                    leases.push(lease_expectation(repo, &opts.remote, spec, &full)?);
                    false
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
                    leases.push(lease_expectation(
                        repo,
                        &opts.remote,
                        &name,
                        &format!("refs/heads/{name}"),
                    )?);
                }
                // Same as above: no `+` for leased refs (the lease flag
                // carries the force authorization).
                let force = opts.force;
                refspecs.push(format!(
                    "{}refs/heads/{name}:refs/heads/{name}",
                    if force { "+" } else { "" }
                ));
                branch_name = Some(name);
            }
        }

        // Per-remote-ref new values for the stats map: source ref target for
        // updates, "(deleted)" for delete refspecs. (Force refspecs carry a
        // `+` marker on the source side — it is not part of the ref name.)
        let mut new_values: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for spec in &refspecs {
            if let Some((src, dst)) = spec.split_once(':') {
                let value = if src.is_empty() {
                    "(deleted)".to_owned()
                } else {
                    repo.find_reference(src.trim_start_matches('+'))
                        .ok()
                        .and_then(|r| r.target())
                        .map(|o| o.to_string())
                        .unwrap_or_default()
                };
                new_values.insert(dst.to_owned(), value);
            }
        }

        // Real lease push: hand the whole refspec set to the sanitized CLI.
        if !leases.is_empty() {
            let stats = cli_push_with_leases(
                repo,
                &opts.remote,
                &refspecs,
                &leases,
                &new_values,
                progress,
            )?;
            if let (Some(name), true) = (branch_name.as_deref(), opts.set_upstream) {
                let mut local = repo.find_branch(name, BranchType::Local)?;
                local.set_upstream(Some(&format!("{}/{}", opts.remote, name)))?;
            }
            return Ok(stats);
        }

        let mut remote = repo.find_remote(&opts.remote).map_err(|e| {
            EngineError::Invalid(format!("remote `{}` not found: {e}", opts.remote))
        })?;

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
        // Cancellation between the per-ref resolution phase and the pack
        // phase. (libgit2's push-transfer callback cannot abort mid-pack;
        // a cancel lands once the push returns.)
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(cancelled_err());
        }
        let push_result = remote.push(&spec_refs, Some(&mut po));
        if push_result.is_err() && cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(cancelled_err());
        }
        push_result?;
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

/// Push through the sanitized CLI when a real `--force-with-lease` is
/// requested: libgit2 has no lease, so `git push
/// --force-with-lease=<full-ref>:<expected-oid>` (one flag per branch
/// refspec) makes the REMOTE enforce the expectation atomically — a remote
/// that moved after our last fetch rejects the push with "stale info".
///
/// Progress is a single final tick (the CLI exposes no per-object
/// callbacks); `updated_refs` is parsed from git's stdout report, falling
/// back to the local source targets when the format surprises us.
fn cli_push_with_leases(
    repo: &Repository,
    remote: &str,
    refspecs: &[String],
    leases: &[(String, String)],
    new_values: &std::collections::HashMap<String, String>,
    progress: &mut dyn FnMut(PushProgress),
) -> EngineResult<NetStats> {
    let workdir = repo.workdir().unwrap_or_else(|| repo.path());
    let mut args: Vec<String> = Vec::with_capacity(2 + leases.len() + refspecs.len());
    args.push("push".to_owned());
    for (full, oid) in leases {
        args.push(format!("--force-with-lease={full}:{oid}"));
    }
    args.push(remote.to_owned());
    args.extend(refspecs.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match crate::maintenance::git_run(workdir, &arg_refs) {
        Ok(report) => {
            (progress)(PushProgress {
                current: 0,
                total: 0,
                bytes: 0,
                message: "done".to_owned(),
            });
            let updated = parse_push_report(&report);
            Ok(NetStats {
                received_bytes: 0,
                objects: updated.len() as u32,
                updated_refs: if updated.is_empty() {
                    new_values
                        .iter()
                        .map(|(dst, value)| (dst.clone(), value.clone()))
                        .collect()
                } else {
                    updated
                },
            })
        }
        // `git push failed: <git's own message>`.
        Err(detail) => {
            if detail.to_lowercase().contains("stale info") {
                Err(EngineError::Invalid(
                    "force-with-lease refused: stale tracking info: fetch first".to_owned(),
                ))
            } else {
                // "[rejected]", non-fast-forward, hook failures: surface git.
                Err(EngineError::Invalid(detail))
            }
        }
    }
}

/// Best-effort parse of `git push`'s stdout report, one line per ref:
/// `   1234567..89abcde  main -> main` or
/// ` + 1234567...89abcde main -> main (forced update)`.
/// Returns (remote ref, new value) pairs.
fn parse_push_report(report: &str) -> Vec<(String, String)> {
    let mut updated = Vec::new();
    for line in report.lines() {
        let Some((left, right)) = line.split_once("->") else {
            continue;
        };
        // Drop trailing annotations ("(forced update)", "(fast-forward)").
        let dst = right.split_whitespace().next().unwrap_or("");
        if dst.is_empty() {
            continue;
        }
        // The range token is the last `old..new` / `old...new` on the left.
        let Some(new_sha) = left.split_whitespace().rev().find_map(|token| {
            token
                .split_once("..")
                .map(|(_, new)| new.trim_start_matches('.').to_owned())
        }) else {
            continue;
        };
        if new_sha.is_empty() {
            continue;
        }
        updated.push((dst.to_owned(), new_sha));
    }
    updated
}

#[cfg(test)]
mod tests {
    //! Force-with-lease integration tests. The lease path shells out to real
    //! git against `file://` remotes, so these need git on PATH. The shared
    //! fixture scaffolding lives in net_tests.rs, which this module does not
    //! own — it is re-created here, trimmed to what the lease tests need.

    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    use git2::{IndexAddOption, Repository, RepositoryInitOptions};

    use super::super::git_engine::{EngineError, GitEngine};
    use super::super::libgit2::Libgit2Engine;
    use super::super::types::{FetchOptions, PushOptions};
    use super::parse_push_report;

    const ENGINE: Libgit2Engine = Libgit2Engine;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-lease-{}-{name}", std::process::id()));
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

    fn file_url(path: &Path) -> String {
        let p = path.to_string_lossy().replace('\\', "/");
        if p.starts_with('/') {
            format!("file://{p}")
        } else {
            format!("file:///{p}")
        }
    }

    fn init_repo(path: &Path, bare: bool) -> Repository {
        let mut opts = RepositoryInitOptions::new();
        opts.bare(bare).initial_head("main");
        let repo = Repository::init_opts(path, &opts).expect("init temp repo");
        if !bare {
            repo.config()
                .and_then(|mut c| {
                    c.set_str("user.name", "Lease Test")?;
                    c.set_str("user.email", "lease@test.local")
                })
                .expect("configure identity");
        }
        repo
    }

    fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) -> String {
        let file = repo.workdir().unwrap().join(path);
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
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
            .to_string()
    }

    fn head_sha(repo: &Repository) -> String {
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
    }

    fn remote_main_sha(origin: &Repository) -> String {
        origin
            .find_reference("refs/heads/main")
            .expect("remote main")
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
    }

    /// Bare `origin` + working clone `a` with one pushed base commit and a
    /// seeded tracking ref.
    struct LeaseFixture {
        origin: Repository,
        a: Repository,
        /// Kept so the temp dirs outlive the test body (drop order: repos
        /// first, dirs deleted after a short delay).
        #[allow(dead_code)]
        origin_dir: TempDir,
        #[allow(dead_code)]
        a_dir: TempDir,
    }

    impl LeaseFixture {
        fn new(name: &str) -> LeaseFixture {
            let origin_dir = TempDir::new(&format!("{name}-origin"));
            let a_dir = TempDir::new(&format!("{name}-a"));
            let origin = init_repo(origin_dir.path(), true);
            let a = init_repo(a_dir.path(), false);
            a.remote("origin", &file_url(origin_dir.path()))
                .expect("remote");
            commit_file(&a, "base.md", "base\n", "base commit");
            let mut a_remote = a.find_remote("origin").expect("origin");
            a_remote
                .push(&["refs/heads/main:refs/heads/main"], None)
                .expect("seed push");
            a_remote
                .fetch(&[] as &[&str], None, None)
                .expect("seed fetch");
            drop(a_remote);
            LeaseFixture {
                origin,
                a,
                origin_dir,
                a_dir,
            }
        }

        fn lease_opts() -> PushOptions {
            PushOptions {
                remote: "origin".into(),
                branch: "main".into(),
                force_with_lease: true,
                ..Default::default()
            }
        }

        fn fetch_opts() -> FetchOptions {
            FetchOptions {
                remote: "origin".into(),
                ..Default::default()
            }
        }
    }

    /// A second clone that advances the remote behind our back: commits one
    /// file locally, then pushes it.
    fn advance_remote(f: &LeaseFixture, tag: &str) -> (TempDir, Repository) {
        let b_dir = TempDir::new(tag);
        let b = Repository::clone(&file_url(f.origin_dir.path()), b_dir.path()).expect("clone b");
        b.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Lease Test")?;
                c.set_str("user.email", "lease@test.local")
            })
            .expect("configure identity b");
        commit_file(&b, "b.md", "remote\n", "b remote");
        b.find_remote("origin")
            .unwrap()
            .push(&["refs/heads/main:refs/heads/main"], None)
            .expect("b push");
        (b_dir, b)
    }

    /// Reset `a` onto `base` and commit a diverging replacement.
    fn rewrite_local(f: &LeaseFixture, base: &str) -> String {
        f.a.reset(
            f.a.find_commit(git2::Oid::from_str(base).unwrap())
                .unwrap()
                .as_object(),
            git2::ResetType::Hard,
            None,
        )
        .unwrap();
        commit_file(&f.a, "a.md", "local\n", "a rewritten")
    }

    /// Remote moved forward (another clone pushed): a lease push from a stale
    /// clone must be refused as stale info. The previous approximation only
    /// checked that the tracking ref existed — which still held here — so
    /// only the real remote-side lease catches this.
    #[test]
    fn lease_push_fails_when_remote_moved_without_fetch() {
        let f = LeaseFixture::new("stale");
        let base = head_sha(&f.a);

        let (_b_dir, b) = advance_remote(&f, "stale-b");

        rewrite_local(&f, &base);

        let err = ENGINE
            .push(&f.a, &LeaseFixture::lease_opts(), &mut |_| {})
            .expect_err("stale lease must be refused");
        match err {
            EngineError::Invalid(msg) => {
                assert!(
                    msg.contains("stale tracking info") && msg.contains("fetch first"),
                    "got: {msg}"
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
        // The push did not land: the remote still holds b's tip.
        assert_eq!(remote_main_sha(&f.origin), head_sha(&b));
    }

    /// After a fetch the tracking ref matches the remote: the lease holds and
    /// the (non-fast-forward) push forces through.
    #[test]
    fn lease_push_succeeds_after_fetch() {
        let f = LeaseFixture::new("fresh");
        let base = head_sha(&f.a);

        let (_b_dir, _b) = advance_remote(&f, "fresh-b");

        let rewritten = rewrite_local(&f, &base);

        // Without a fetch the lease fails...
        assert!(ENGINE
            .push(&f.a, &LeaseFixture::lease_opts(), &mut |_| {})
            .is_err());
        // ...after a fetch it forces through.
        ENGINE
            .fetch(&f.a, &LeaseFixture::fetch_opts(), &mut |_| {})
            .expect("fetch refreshes the lease");
        let stats = ENGINE
            .push(&f.a, &LeaseFixture::lease_opts(), &mut |_| {})
            .expect("lease push lands after fetch");
        assert_eq!(remote_main_sha(&f.origin), rewritten);
        assert!(
            stats
                .updated_refs
                .iter()
                .any(|(r, s)| r == "refs/heads/main" && s == &rewritten),
            "push report parsed from the CLI output: {:?}",
            stats.updated_refs
        );
    }

    /// Plain force (no lease) keeps the libgit2 path and still overwrites.
    #[test]
    fn plain_force_push_still_works_without_lease() {
        let f = LeaseFixture::new("plain-force");
        let base = head_sha(&f.a);

        let (_b_dir, _b) = advance_remote(&f, "plain-force-b");

        let rewritten = rewrite_local(&f, &base);

        let opts = PushOptions {
            remote: "origin".into(),
            branch: "main".into(),
            force: true,
            ..Default::default()
        };
        ENGINE.push(&f.a, &opts, &mut |_| {}).expect("plain force");
        assert_eq!(remote_main_sha(&f.origin), rewritten);
    }

    #[test]
    fn parse_push_report_extracts_updated_refs() {
        let report = concat!(
            "To file:///tmp/origin\n",
            "   1234567..89abcde  main -> main\n",
            " + 1111111...2222222 topic -> topic (forced update)\n",
            " * [new branch]      feat -> feat\n",
        );
        let updated = parse_push_report(report);
        assert_eq!(
            updated,
            vec![
                ("main".to_owned(), "89abcde".to_owned()),
                ("topic".to_owned(), "2222222".to_owned()),
            ],
            "new-branch lines carry no range and are skipped: {updated:?}"
        );
    }

    /// A cancelled push refuses before touching the remote.
    #[test]
    fn cancelled_push_refuses_up_front() {
        let f = LeaseFixture::new("cancel");
        let cancel = AtomicBool::new(true);
        let err = ENGINE
            .push_impl_ext(
                &f.a,
                &LeaseFixture::lease_opts(),
                &mut |_| {},
                Some(&cancel),
            )
            .expect_err("cancelled op must not run");
        match err {
            EngineError::Invalid(msg) => assert_eq!(msg, "cancelled"),
            other => panic!("expected Invalid(cancelled), got {other:?}"),
        }
    }
}
