//! Staging, committing, signing detection, hook introspection.
//!
//! Lane C1. Whole-file staging talks to the index directly (git2 `Index`
//! add/remove path); hunk/line-granular staging is delegated to
//! `crate::diffcore`, which builds minimal `git apply --cached` patches.
//!
//! ARCHITECTURE NOTE (CLI routing): git2/libgit2 cannot execute hooks or
//! produce signed commits. When either is in play the engine returns
//! `EngineError::Unsupported` and the IPC layer (lane C3) falls back to
//! running system `git commit`, which runs hooks and GPG/SSH signing natively.

use std::path::Path;

use git2::{IndexAddOption, Repository, Signature};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{CommitOptions, GitSignature, HookInfo, SigningInfo, StageRequest, StageTarget};
use crate::diffcore::{self, Selection};

/// Hook names surfaced by `hooks_impl`.
const KNOWN_HOOKS: [&str; 5] = [
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "pre-push",
    "post-commit",
];

/// Hooks that can block or alter a commit: their presence (and executability)
/// forces the CLI path for `commit` unless `no_verify` is set. `post-commit`
/// is advisory (cannot change the result) and `pre-push` belongs to the push
/// lane, so neither gates `commit`.
const COMMIT_GATING_HOOKS: [&str; 3] = ["pre-commit", "prepare-commit-msg", "commit-msg"];

/// HEAD peeled to a commit object, or `None` on an unborn HEAD. libgit2
/// reports unborn HEAD as `EUNBORNBRANCH` (-9), not as "not found".
fn head_commit_object(repo: &Repository) -> EngineResult<Option<git2::Object<'_>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel(git2::ObjectType::Commit)?)),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// Path of a hook inside the repo's common git dir (linked worktrees share
/// hooks with their main repo, hence `commondir`).
fn hook_file(repo: &Repository, kind: &str) -> std::path::PathBuf {
    repo.commondir().join("hooks").join(kind)
}

/// (present, executable) for one hook path.
///
/// Windows caveat: NTFS has no exec bit; Git-for-Windows runs any hook file
/// that is present via the bundled sh, so `executable == present` there.
/// Unix additionally requires any execute bit in the file mode.
fn hook_state(path: &Path) -> (bool, bool) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (false, false);
    };
    if !meta.is_file() {
        return (false, false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = meta.permissions().mode() & 0o111 != 0;
        (true, executable)
    }
    #[cfg(windows)]
    {
        (true, true)
    }
}

/// Any commit-gating hook present and runnable? (See COMMIT_GATING_HOOKS.)
fn commit_hooks_active(repo: &Repository) -> bool {
    COMMIT_GATING_HOOKS
        .iter()
        .any(|kind| matches!(hook_state(&hook_file(repo, kind)), (true, true)))
}

/// Whole-file stage/unstage.
///
/// * stage: untracked/modified -> `index.add_path`; deleted from the workdir
///   -> `index.remove_path`.
/// * unstage: `git reset <path>` via `reset_default` against the HEAD tree;
///   on an unborn HEAD the entry is removed from the index instead.
fn stage_file(repo: &Repository, path: &str, unstage: bool) -> EngineResult<()> {
    if unstage {
        match head_commit_object(repo)? {
            Some(target) => repo.reset_default(Some(&target), [path])?,
            None => repo.reset_default(None, [path])?,
        }
        return Ok(());
    }

    let workdir = repo
        .workdir()
        .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?;
    let exists_in_workdir = std::fs::symlink_metadata(workdir.join(path)).is_ok();
    let mut index = repo.index()?;
    if exists_in_workdir {
        index.add_path(Path::new(path))?;
    } else {
        index.remove_path(Path::new(path))?;
    }
    index.write()?;
    Ok(())
}

impl Libgit2Engine {
    pub(crate) fn stage_impl(&self, repo: &Repository, req: &StageRequest) -> EngineResult<()> {
        // Hunk/line selections are grouped per path and applied as one
        // `git apply --cached` per path so hunk indices stay valid.
        let mut selections: std::collections::BTreeMap<String, Vec<Selection>> =
            std::collections::BTreeMap::new();
        for target in &req.targets {
            match target {
                StageTarget::File(path) => stage_file(repo, path, req.unstage)?,
                StageTarget::Hunk { path, hunk } => selections
                    .entry(path.clone())
                    .or_default()
                    .push(Selection::Hunk(*hunk)),
                StageTarget::Lines { path, hunk, ranges } => selections
                    .entry(path.clone())
                    .or_default()
                    .push(Selection::Lines(*hunk, ranges.clone())),
            }
        }
        for (path, sels) in &selections {
            diffcore::apply_selection(repo, path, sels, req.unstage)?;
        }
        Ok(())
    }

    pub(crate) fn stage_all_impl(&self, repo: &Repository, unstage: bool) -> EngineResult<()> {
        if unstage {
            // Mixed-reset semantics: index := HEAD tree, workdir untouched.
            match head_commit_object(repo)? {
                Some(target) => repo.reset_default(Some(&target), ["*"])?,
                None => {
                    // Unborn HEAD: unstage-all empties the index.
                    let paths: Vec<String> = repo
                        .index()?
                        .iter()
                        .map(|e| String::from_utf8_lossy(&e.path).into_owned())
                        .collect();
                    if !paths.is_empty() {
                        repo.reset_default(None, paths)?;
                    }
                }
            }
            return Ok(());
        }

        repo.workdir()
            .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?;
        // `git add -A`: add_all covers untracked + modified, update_all drops
        // tracked entries whose workdir file disappeared.
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        index.update_all(["*"].iter(), None)?;
        index.write()?;
        Ok(())
    }

    pub(crate) fn commit_impl(
        &self,
        repo: &Repository,
        opts: &CommitOptions,
    ) -> EngineResult<String> {
        // CLI routing (see module note): signing and commit hooks need the
        // real git binary; the IPC layer catches Unsupported and shells out.
        if self.signing_info_impl(repo)?.active {
            return Err(EngineError::Unsupported(
                "signing active: routed to CLI".into(),
            ));
        }
        if !opts.no_verify && commit_hooks_active(repo) {
            return Err(EngineError::Unsupported(
                "hooks present: routed to CLI".into(),
            ));
        }
        if opts.message.trim().is_empty() {
            return Err(EngineError::Invalid(
                "commit message is empty (git refuses empty messages)".into(),
            ));
        }

        let head = match repo.head() {
            Ok(head) => Some(head.peel_to_commit()?),
            Err(e)
                if matches!(
                    e.code(),
                    git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
                ) =>
            {
                None
            }
            Err(e) => return Err(e.into()),
        };

        let mut index = repo.index()?;
        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;

        // git refuses "nothing to commit" unless --allow-empty. Amending is
        // always meaningful (message/author change), so it skips the check.
        if !opts.allow_empty && !opts.amend {
            let unchanged = match &head {
                Some(head) => head.tree_id() == tree_oid,
                None => index.is_empty(),
            };
            if unchanged {
                return Err(EngineError::Invalid(
                    "nothing to commit (use allow_empty to override)".into(),
                ));
            }
        }

        let committer = repo.signature().map_err(|e| {
            EngineError::Invalid(format!(
                "committer identity not configured (set user.name / user.email): {e}"
            ))
        })?;
        let override_author = opts
            .author
            .as_ref()
            .map(|a| signature_from_model(a))
            .transpose()?;
        // git amend semantics: keep the original author unless overridden.
        let author = match (&override_author, &head) {
            (Some(a), _) => a.clone(),
            (None, Some(head)) if opts.amend => Signature::new(
                head.author().name().unwrap_or_default(),
                head.author().email().unwrap_or_default(),
                &git2::Time::new(
                    head.author().when().seconds(),
                    head.author().when().offset_minutes(),
                ),
            )?,
            _ => committer.clone(),
        };

        let parents: Vec<git2::Commit<'_>> = if opts.amend {
            let head =
                head.ok_or_else(|| EngineError::Invalid("cannot amend: HEAD is unborn".into()))?;
            head.parents().collect()
        } else {
            head.into_iter().collect()
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

        let oid = if opts.amend {
            // `git_commit_create` refuses to move a ref onto a commit whose
            // first parent is not the current tip — and amending a ROOT
            // commit has no parents at all. So: write the object first,
            // then force-move the ref HEAD points at (the branch when
            // attached, HEAD itself when detached) — `git commit --amend`.
            let head_ref = repo.find_reference("HEAD")?;
            let oid = repo.commit(
                None,
                &author,
                &committer,
                &opts.message,
                &tree,
                &parent_refs,
            )?;
            let ref_name = head_ref
                .symbolic_target()
                .ok()
                .flatten()
                .unwrap_or("HEAD")
                .to_string();
            repo.reference(&ref_name, oid, true, "commit: amend")?;
            oid
        } else {
            repo.commit(
                Some("HEAD"),
                &author,
                &committer,
                &opts.message,
                &tree,
                &parent_refs,
            )?
        };

        // `git commit` consumes the in-progress operation's state
        // (CHERRY_PICK_HEAD / REVERT_HEAD / MERGE_MSG); libgit2's commit does
        // not. A cherry-pick/revert conflict resolved through this path must
        // terminate the sequencer, or RepoStatus::sequencer stays lit
        // forever. (A real merge stays until its MERGE_HEAD becomes the
        // second parent — documented gap in merge.rs; the helper is a no-op
        // while a merge is in progress.)
        super::merge::clear_sequencer_files(repo);
        Ok(oid.to_string())
    }
    pub(crate) fn signing_info_impl(&self, repo: &Repository) -> EngineResult<SigningInfo> {
        let config = repo.config()?;
        let active = config.get_bool("commit.gpgsign").unwrap_or(false);
        let format = config
            .get_string("gpg.format")
            .unwrap_or_else(|_| "openpgp".to_string());
        let key_id = config.get_string("user.signingkey").ok();
        Ok(SigningInfo {
            active,
            format,
            key_id,
        })
    }

    pub(crate) fn hooks_impl(&self, repo: &Repository) -> EngineResult<Vec<HookInfo>> {
        Ok(KNOWN_HOOKS
            .iter()
            .map(|kind| {
                let (present, executable) = hook_state(&hook_file(repo, kind));
                HookInfo {
                    kind: (*kind).to_string(),
                    present,
                    executable,
                }
            })
            .collect())
    }
}

/// Model signature -> git2 signature (author override path).
fn signature_from_model(sig: &GitSignature) -> EngineResult<Signature<'static>> {
    Signature::new(
        &sig.name,
        &sig.email,
        &git2::Time::new(sig.time, sig.offset_minutes),
    )
    .map_err(|e| {
        EngineError::Invalid(format!(
            "invalid author identity `{}` <{}>: {}",
            sig.name, sig.email, e
        ))
    })
}
