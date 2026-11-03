use crate::engine::types::{DiffSide, FileDiff, RepoId};
use crate::service::{enqueue_read, enqueue_remote_read, finish_op, get_handle, AppServices};

/// One-shot diff. Large diffs are rendered virtually by the UI from this
/// same payload, so there is no separate paged variant.
pub async fn repo_diff(
    services: &AppServices,
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    paths: Option<Vec<String>>,
) -> Result<Vec<FileDiff>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "diff",
            move |git, _ws| git.diff(&old, &new, paths.as_deref()),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "diff", move |_ctx, repo| {
        engine.diff(repo, &old, &new, paths.as_deref())
    }))
    .await
}
