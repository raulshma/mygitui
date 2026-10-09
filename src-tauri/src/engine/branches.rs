//! Branch and tag operations.
//!
//! Local branches only: remote-tracking refs surface through log decorations
//! and `repo_refs`; the branch panel gets its remote list from there.

use git2::build::CheckoutBuilder;
use git2::{BranchType, Repository};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{BranchInfo, GitSignature, RemoteBranchInfo, TagInfo};

/// Branch name of HEAD; also resolves unborn (symbolic-only) HEAD.
/// Twin of the private helper in libgit2.rs (that file is owned by C1).
pub(super) fn current_branch_name(repo: &Repository) -> Option<String> {
    if let Ok(head) = repo.head() {
        return head.shorthand().ok().map(str::to_owned);
    }
    let sym = repo
        .find_reference("HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_owned))?;
    sym.rsplit('/').next().map(str::to_owned)
}

/// Resolve a commit-ish (sha, branch, tag, revspec) to its commit.
fn resolve_commit<'r>(repo: &'r Repository, spec: &str) -> EngineResult<git2::Commit<'r>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?;
    obj.peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("`{spec}` is not a commit: {e}")))
}

/// Peel a local branch name to its tip commit; `Invalid` when missing/unborn.
fn local_branch_tip<'r>(repo: &'r Repository, name: &str) -> EngineResult<git2::Commit<'r>> {
    repo.find_branch(name, BranchType::Local)
        .map_err(|_| EngineError::Invalid(format!("branch `{name}` not found")))?
        .get()
        .peel_to_commit()
        .map_err(|_| EngineError::Invalid(format!("branch `{name}` is unborn")))
}

/// Friendlier error for branch/tag create/rename collisions.
fn map_already_exists(name: &str) -> impl FnOnce(git2::Error) -> EngineError + '_ {
    move |e| match e.code() {
        git2::ErrorCode::Exists => EngineError::Invalid(format!("`{name}` already exists")),
        _ => e.into(),
    }
}

/// Upstream name + ahead/behind + gone flag for one local branch.
///
/// `gone` = the upstream is configured (branch.<name>.remote/merge) but the
/// remote-tracking ref itself is gone, e.g. the branch was deleted on the
/// remote and pruned.
fn upstream_info(
    repo: &Repository,
    branch: &git2::Branch<'_>,
    local_oid: Option<git2::Oid>,
) -> EngineResult<(Option<String>, bool, u32, u32)> {
    match branch.upstream() {
        // Tracking ref exists: real ahead/behind numbers.
        Ok(up) => {
            let name = up.name().ok().flatten().map(str::to_owned);
            match (up.get().target(), local_oid) {
                (Some(up_oid), Some(local)) => {
                    let (ahead, behind) = repo.graph_ahead_behind(local, up_oid)?;
                    Ok((name, false, ahead as u32, behind as u32))
                }
                _ => Ok((name, true, 0, 0)),
            }
        }
        // Tracking ref unreachable: distinguish "no upstream configured"
        // (config lookup also fails) from "configured but gone".
        Err(_) => {
            let Some(name) = branch.name().ok().flatten() else {
                return Ok((None, false, 0, 0));
            };
            match repo.branch_upstream_name(&format!("refs/heads/{name}")) {
                Ok(buf) if !buf.as_str().map(str::is_empty).unwrap_or(true) => {
                    let full = buf.as_str().map(str::to_owned).unwrap_or_default();
                    let short = full
                        .strip_prefix("refs/remotes/")
                        .unwrap_or(&full)
                        .to_owned();
                    Ok((Some(short), true, 0, 0))
                }
                _ => Ok((None, false, 0, 0)),
            }
        }
    }
}

/// Update the working directory + index to `commit`'s tree, then point HEAD
/// at the branch ref (`git switch` semantics: workdir first, HEAD last).
/// `force` discards conflicting local modifications instead of refusing.
fn checkout_branch_target(
    repo: &Repository,
    refname: &str,
    commit: &git2::Commit<'_>,
    force: bool,
) -> EngineResult<()> {
    let mut cb = CheckoutBuilder::new();
    if force {
        cb.force();
    } else {
        cb.safe();
    }
    repo.checkout_tree(commit.tree()?.as_object(), Some(&mut cb))?;
    repo.set_head(refname)?;
    Ok(())
}

impl Libgit2Engine {
    pub(crate) fn branches_impl(&self, repo: &Repository) -> EngineResult<Vec<BranchInfo>> {
        let mut out = Vec::new();
        for entry in repo.branches(Some(BranchType::Local))? {
            let (branch, _) = entry?;
            let Some(name) = branch.name().ok().flatten().map(str::to_owned) else {
                continue; // non-UTF-8 branch name; log decorations still show it
            };
            let oid = branch.get().target();
            let (upstream, gone, ahead, behind) = upstream_info(repo, &branch, oid)?;
            out.push(BranchInfo {
                sha: oid.map(|o| o.to_string()).unwrap_or_default(),
                is_head: branch.is_head(),
                name,
                upstream,
                ahead,
                behind,
                gone,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub(crate) fn branch_create_impl(
        &self,
        repo: &Repository,
        name: &str,
        from: Option<&str>,
        checkout: bool,
    ) -> EngineResult<()> {
        let commit = match from {
            Some(spec) => resolve_commit(repo, spec)?,
            None => repo
                .head()
                .map_err(|_| EngineError::Invalid("cannot branch from an unborn HEAD".into()))?
                .peel_to_commit()?,
        };
        let branch = repo
            .branch(name, &commit, false)
            .map_err(map_already_exists(name))?;
        if checkout {
            let refname = branch.get().name().unwrap_or_default().to_owned();
            checkout_branch_target(repo, &refname, &commit, false)?;
        }
        Ok(())
    }

    pub(crate) fn branch_switch_impl(
        &self,
        repo: &Repository,
        name: &str,
        force: bool,
    ) -> EngineResult<()> {
        let branch = repo
            .find_branch(name, BranchType::Local)
            .map_err(|_| EngineError::Invalid(format!("branch `{name}` not found")))?;
        // Unborn targets carry no commit to check out.
        let commit = branch.get().peel_to_commit().map_err(|_| {
            EngineError::Invalid(format!(
                "branch `{name}` is unborn and cannot be checked out"
            ))
        })?;
        let refname = branch.get().name().unwrap_or_default().to_owned();
        checkout_branch_target(repo, &refname, &commit, force)?;
        Ok(())
    }

    pub(crate) fn branch_is_merged_impl(
        &self,
        repo: &Repository,
        name: &str,
        into: &str,
    ) -> EngineResult<bool> {
        let tip = local_branch_tip(repo, name)?;
        let base = resolve_commit(repo, into)?;
        match repo.merge_base(tip.id(), base.id()) {
            // Merged iff the branch tip is an ancestor of `into`.
            Ok(merge_base) => Ok(merge_base == tip.id()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn branch_delete_impl(
        &self,
        repo: &Repository,
        name: &str,
        force: bool,
    ) -> EngineResult<()> {
        let mut branch = repo
            .find_branch(name, BranchType::Local)
            .map_err(|_| EngineError::Invalid(format!("branch `{name}` not found")))?;
        if branch.is_head() {
            return Err(EngineError::Invalid(format!(
                "cannot delete the current branch `{name}`; switch away first"
            )));
        }
        // Git itself has no delete protection — the merged check is our guard
        // (`git branch -d` semantics); force bypasses it (`-D`).
        if !force && !self.branch_is_merged_impl(repo, name, "HEAD")? {
            return Err(EngineError::Invalid(format!(
                "branch `{name}` is not fully merged into HEAD; delete with force to bypass"
            )));
        }
        // SAFETY NET (M12): park the tip under a hidden trash ref
        // (`refs/mygitui/trash/<id>`) BEFORE the branch ref goes away, so a
        // mistaken delete stays recoverable from the Health panel. A failed
        // trash write aborts the delete (deleting while unrecoverable is
        // worse than not deleting).
        if let Some(oid) = branch.get().target() {
            super::trash::trash_create(repo, name, &oid.to_string())?;
        }
        branch.delete()?;
        Ok(())
    }

    pub(crate) fn branch_rename_impl(
        &self,
        repo: &Repository,
        old: &str,
        new: &str,
    ) -> EngineResult<()> {
        let mut branch = repo
            .find_branch(old, BranchType::Local)
            .map_err(|_| EngineError::Invalid(format!("branch `{old}` not found")))?;
        let was_head = branch.is_head();
        branch.rename(new, false).map_err(map_already_exists(new))?;
        if was_head {
            repo.set_head(&format!("refs/heads/{new}"))?;
        }
        Ok(())
    }

    pub(crate) fn tag_create_impl(
        &self,
        repo: &Repository,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> EngineResult<()> {
        let target_obj = match target {
            Some(spec) => repo
                .revparse_single(spec)
                .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?,
            None => repo
                .head()
                .map_err(|_| EngineError::Invalid("cannot tag an unborn HEAD".into()))?
                .peel_to_commit()?
                .as_object()
                .clone(),
        };
        match message.map(str::trim).filter(|m| !m.is_empty()) {
            Some(msg) => {
                let tagger = repo.signature().map_err(|e| {
                    EngineError::Invalid(format!("no tagger identity configured: {e}"))
                })?;
                repo.tag(name, &target_obj, &tagger, msg, false)
                    .map_err(map_already_exists(name))?;
            }
            None => {
                repo.tag_lightweight(name, &target_obj, false)
                    .map_err(map_already_exists(name))?;
            }
        }
        Ok(())
    }

    pub(crate) fn tag_delete_impl(&self, repo: &Repository, name: &str) -> EngineResult<()> {
        repo.tag_delete(name).map_err(|e| match e.code() {
            git2::ErrorCode::NotFound => EngineError::Invalid(format!("tag `{name}` not found")),
            _ => e.into(),
        })
    }

    pub(crate) fn tags_impl(&self, repo: &Repository) -> EngineResult<Vec<TagInfo>> {
        let mut out = Vec::new();
        for entry in repo.references_glob("refs/tags/*")? {
            let reference = entry?;
            let Ok(full) = reference.name().map(str::to_owned) else {
                continue;
            };
            let name = full.strip_prefix("refs/tags/").unwrap_or(&full).to_owned();
            let Some(oid) = reference.target() else {
                continue; // symbolic tag chain; skip (git rare)
            };
            // Annotated: the ref points at a tag object. Peeling it to the
            // tag type first distinguishes annotated from lightweight.
            let annotated = reference
                .peel(git2::ObjectType::Tag)
                .map(|obj| obj.id() == oid)
                .unwrap_or(false);
            let target = reference.peel_to_commit()?.id().to_string();
            let (tagger, message) = if annotated {
                let tag = repo.find_tag(oid)?;
                let tagger = tag.tagger().map(|sig| GitSignature {
                    name: sig.name().unwrap_or_default().to_owned(),
                    email: sig.email().unwrap_or_default().to_owned(),
                    time: sig.when().seconds(),
                    offset_minutes: sig.when().offset_minutes(),
                });
                let message = tag
                    .message()
                    .ok()
                    .flatten()
                    .map(str::to_owned)
                    .map(|m| m.trim_end().to_owned());
                (tagger, message)
            } else {
                (None, None)
            };
            out.push(TagInfo {
                name,
                sha: oid.to_string(),
                target,
                annotated,
                tagger,
                message,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub(crate) fn remote_branches_impl(
        &self,
        repo: &Repository,
    ) -> EngineResult<Vec<RemoteBranchInfo>> {
        // Reverse map: remote-tracking name -> local branch tracking it.
        let mut tracked_by: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for entry in repo.branches(Some(BranchType::Local))? {
            let (branch, _) = entry?;
            let Some(local) = branch.name().ok().flatten().map(str::to_owned) else {
                continue;
            };
            if let Ok(up) = branch.upstream() {
                if let Ok(full) = up.get().name().map(str::to_owned) {
                    tracked_by.insert(full, local);
                }
            }
        }
        let mut out = Vec::new();
        for entry in repo.branches(Some(BranchType::Remote))? {
            let (branch, _) = entry?;
            // Remote branch names include the remote prefix; skip HEAD refs.
            let Some(short) = branch.name().ok().flatten().map(str::to_owned) else {
                continue;
            };
            if short.ends_with("/HEAD") {
                continue;
            }
            let (remote, name) = short.split_once('/').unwrap_or((&short, ""));
            let sha = branch
                .get()
                .target()
                .map(|o| o.to_string())
                .unwrap_or_default();
            // Remote branch names are shorthands ("origin/main"); the map
            // from the local scan is keyed by the full ref.
            let full = format!("refs/remotes/{short}");
            out.push(RemoteBranchInfo {
                remote: remote.to_owned(),
                name: name.to_owned(),
                sha,
                tracked_by: tracked_by.get(&full).cloned(),
            });
        }
        out.sort_by(|a, b| (&a.remote, &a.name).cmp(&(&b.remote, &b.name)));
        Ok(out)
    }

    /// `git switch <name>` from a remote branch: create the local branch at
    /// the tracking tip with upstream configured, then check it out.
    pub(crate) fn branch_checkout_remote_impl(
        &self,
        repo: &Repository,
        remote: &str,
        name: &str,
        new_local: Option<&str>,
    ) -> EngineResult<String> {
        let tracking_ref = format!("refs/remotes/{remote}/{name}");
        let tracking = repo.find_reference(&tracking_ref).map_err(|_| {
            EngineError::Invalid(format!("remote branch `{remote}/{name}` not found"))
        })?;
        let commit = tracking.peel_to_commit()?;
        let local_name = new_local.unwrap_or(name);
        if repo.find_branch(local_name, BranchType::Local).is_ok() {
            return Err(EngineError::Invalid(format!(
                "branch `{local_name}` already exists; switch to it or pick another name"
            )));
        }
        let mut branch = repo.branch(local_name, &commit, false)?;
        branch.set_upstream(Some(&format!("{remote}/{name}")))?;
        let refname = format!("refs/heads/{local_name}");
        checkout_branch_target(repo, &refname, &commit, false)?;
        Ok(local_name.to_owned())
    }
}
