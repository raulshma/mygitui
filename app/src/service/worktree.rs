//! Worktree service lane: the queue wrappers the UI calls, under one name.
//!
//! History: this module started as an empty placeholder because the original
//! wrappers (`worktrees` / `worktree_add` / `worktree_remove`) landed next to
//! the stash commands in [`crate::service::stash`], with `worktree_prune`
//! next to the other history-rewriting commands in [`crate::service::merge`]
//! — the surface was too small to justify moving bodies across files. That
//! is still true, so this module does not duplicate any queue logic: it
//! re-exports the four wrappers unchanged (same functions, so `crate::service`
//! glob re-exports stay unambiguous) and adds the one composition the
//! worktrees panel needs beyond them — [`worktree_add_named`], the single
//! branch-name add form, whose pure pieces ([`branch_arg`],
//! [`worktree_path`]) are unit-tested here.

pub use crate::service::merge::worktree_prune;
pub use crate::service::stash::{worktree_add, worktree_remove, worktrees};

use std::path::Path;

use crate::engine::types::RepoId;
use crate::service::{get_handle, AppServices};

/// Add a linked worktree for one branch name: the panel's whole add form is
/// "branch name + optional folder", so this resolves what [`worktree_add`]
/// needs from it.
///
/// - `name`: an existing local branch is checked out; anything else becomes
///   a fresh branch created from HEAD (see [`branch_arg`]).
/// - `path`: the worktree folder. Empty/whitespace falls back to a sibling
///   directory named after the branch — the `git worktree add ../<name>`
///   convention (see [`worktree_path`]).
pub async fn worktree_add_named(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    path: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a worktree needs a branch name".into());
    }
    let handle = get_handle(&services.repos, &repo_id)?;
    let path = worktree_path(&handle.root, name, &path)?;
    let local_branches: Vec<String> = crate::service::branches::branches(services, repo_id.clone())
        .await?
        .into_iter()
        .map(|branch| branch.name)
        .collect();
    match branch_arg(name, &local_branches) {
        BranchArg::Existing(name) => {
            worktree_add(services, repo_id, path, Some(name.to_owned()), None).await
        }
        BranchArg::New(name) => {
            worktree_add(services, repo_id, path, None, Some(name.to_owned())).await
        }
    }
}

/// Which engine argument the branch-name field maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchArg<'a> {
    /// Check out this existing local branch (`worktree_add`'s `branch`).
    Existing(&'a str),
    /// Create a new branch with this name from HEAD (`new_branch`).
    New(&'a str),
}

/// Pure mapping behind [`worktree_add_named`]: an existing local branch is
/// checked out, anything else becomes a fresh branch. The engine rejects
/// `branch` and `new_branch` together, so the decision has to be exclusive.
pub(crate) fn branch_arg<'a>(name: &'a str, local_branches: &[String]) -> BranchArg<'a> {
    if local_branches.iter().any(|branch| branch == name) {
        BranchArg::Existing(name)
    } else {
        BranchArg::New(name)
    }
}

/// Resolve the worktree folder: an explicit non-empty path wins, otherwise a
/// sibling directory named after the branch is derived from the repo root.
/// Errors when the root has no parent (filesystem root / relative path).
pub(crate) fn worktree_path(root: &Path, name: &str, requested: &str) -> Result<String, String> {
    let requested = requested.trim();
    if !requested.is_empty() {
        return Ok(requested.to_owned());
    }
    let parent = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("cannot derive a worktree folder next to {}", root.display()))?;
    Ok(parent.join(name).to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{branch_arg, worktree_path, BranchArg};
    use std::path::Path;

    #[test]
    fn branch_arg_checks_out_existing_and_creates_missing() {
        let local = vec!["main".to_owned(), "feature/x".to_owned()];
        assert_eq!(branch_arg("main", &local), BranchArg::Existing("main"));
        assert_eq!(
            branch_arg("feature/x", &local),
            BranchArg::Existing("feature/x")
        );
        assert_eq!(branch_arg("spike", &local), BranchArg::New("spike"));
        assert_eq!(branch_arg("main", &[]), BranchArg::New("main"));
    }

    #[test]
    fn worktree_path_prefers_the_explicit_folder() {
        let root = Path::new("/repos/mygitui");
        assert_eq!(
            worktree_path(root, "spike", " /tmp/wt ").unwrap(),
            "/tmp/wt"
        );
    }

    #[test]
    fn worktree_path_derives_a_sibling_folder_from_the_branch_name() {
        let root = Path::new("/repos/mygitui");
        // Compare as paths: the returned String carries the host's
        // separators (`\` on Windows).
        let expected = Path::new("/repos").join("spike");
        assert_eq!(
            Path::new(&worktree_path(root, "spike", "").unwrap()),
            expected
        );
        assert_eq!(
            Path::new(&worktree_path(root, "spike", "   ").unwrap()),
            expected
        );
    }

    #[test]
    fn worktree_path_errors_when_the_root_has_no_parent() {
        let err = worktree_path(Path::new("/"), "spike", "").unwrap_err();
        assert!(err.contains("cannot derive a worktree folder"), "{err}");
    }
}
