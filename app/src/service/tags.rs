//! Tag service surface: `tag_list` / `tag_create` / `tag_delete`.
//!
//! Mirrors the wrapper pattern of [`crate::service::branches`] (this module's
//! exemplar): listings are queued reads, ref-affecting changes are queued
//! mutations, and remote repos go through the remote engine's git surface,
//! which has the full tag family. Signing routes through the device's own
//! git CLI (libgit2 cannot sign) — the same path as
//! [`crate::service::branches::tag_create_signed`].
//!
//! Engine rules encoded here (see `engine/branches.rs::tag_create_impl`): a
//! trimmed-non-empty message creates an annotated tag, everything else is
//! lightweight; a signed tag is always annotated, so it requires a message.
//! Ops report on the queue as `branch` (ref-affecting), the tag *listing*
//! reads as `refs` (silent, like every queued read).

use crate::engine::types::{RepoId, TagInfo};
use crate::service::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle, AppServices,
};

/// Every tag with its metadata (annotated tags carry tagger + message).
pub async fn tag_list(services: &AppServices, repo_id: RepoId) -> Result<Vec<TagInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "refs",
            move |git, _ws| git.tag_list(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "refs", move |_ctx, repo| {
        engine.tag_list(repo)
    }))
    .await
}

/// Create a tag. `target` (`None` = HEAD) resolves any commit-ish; `message`
/// makes the tag annotated (the engine treats a trimmed-empty message as
/// lightweight); `sign` creates a signed annotated tag via the git CLI and
/// therefore requires a message.
pub async fn tag_create(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    target: Option<String>,
    message: Option<String>,
    sign: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        if sign {
            let message = signed_message(message.as_deref())?;
            return finish_op(enqueue_remote_mutation(
                &handle,
                remote,
                "branch",
                move |git, _ws| git.tag_create_signed(&name, target.as_deref(), &message),
            ))
            .await;
        }
        let message = prepare_message(message.as_deref());
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.tag_create(&name, target.as_deref(), message.as_deref()),
        ))
        .await;
    }
    if sign {
        let message = signed_message(message.as_deref())?;
        let root = handle.root.clone();
        let repo = handle.repo()?;
        return finish_op(enqueue_mutation(
            &handle,
            &repo,
            "branch",
            move |_ctx, _repo| {
                crate::mergetool::tag_sign_via_cli(&root, &name, target.as_deref(), &message)
                    .map_err(crate::engine::git_engine::EngineError::Invalid)
            },
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    let message = prepare_message(message.as_deref());
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.tag_create(repo, &name, target.as_deref(), message.as_deref()),
    ))
    .await
}

/// Delete a tag by name.
pub async fn tag_delete(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.tag_delete(&name),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.tag_delete(repo, &name),
    ))
    .await
}

/// The engine's message rule: trimmed-empty means lightweight (no message).
fn prepare_message(message: Option<&str>) -> Option<String> {
    message
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
}

/// A signed tag is annotated by definition, so it needs a message.
fn signed_message(message: Option<&str>) -> Result<String, String> {
    prepare_message(message).ok_or_else(|| "a signed tag requires a message".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{prepare_message, signed_message};

    #[test]
    fn prepare_message_trims_and_drops_blank() {
        assert_eq!(
            prepare_message(Some("  v1 notes  ")).as_deref(),
            Some("v1 notes")
        );
        assert_eq!(prepare_message(Some("   ")), None);
        assert_eq!(prepare_message(None), None);
    }

    #[test]
    fn signed_message_requires_a_real_message() {
        assert_eq!(signed_message(Some("release")).unwrap(), "release");
        let err = signed_message(Some("   ")).unwrap_err();
        assert!(err.contains("signed tag requires a message"), "{err}");
        assert!(signed_message(None).is_err());
    }
}
