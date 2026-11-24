//! Thin Tauri commands for the GitHub Issues backend. All blocking gh work is
//! parked on the async runtime's blocking pool; structured [`ForgeError`]
//! rejections preserve HTTP/rate metadata for the frontend.

use tauri::State;

use crate::engine::types::RepoId;
use crate::forge::issues::{
    issue_activity_sync, issue_comment_sync, issue_comments_sync, issue_create_sync,
    issue_detail_sync, issue_identity_sync, issue_list_sync, issue_metadata_sync,
    issue_set_state_sync, issue_update_sync, IssueActivityBatch, IssueComment, IssueCommentPage,
    IssueDetail, IssueIdentity, IssueListPage, IssueListQuery, IssueMetadata,
};
use crate::forge::ForgeError;
use crate::repo::RepoManager;

use super::{forge_repo_of, get_handle};

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_identity(
    repo_id: RepoId,
    remote: String,
    state: State<'_, RepoManager>,
) -> Result<IssueIdentity, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || issue_identity_sync(&repo, Some(&remote)))
        .await
        .map_err(|err| ForgeError::other(format!("issues_identity task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_list(
    repo_id: RepoId,
    remote: String,
    query: IssueListQuery,
    state: State<'_, RepoManager>,
) -> Result<IssueListPage, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || issue_list_sync(&repo, Some(&remote), query))
        .await
        .map_err(|err| ForgeError::other(format!("issues_list task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_detail(
    repo_id: RepoId,
    remote: String,
    number: u64,
    state: State<'_, RepoManager>,
) -> Result<IssueDetail, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || issue_detail_sync(&repo, Some(&remote), number))
        .await
        .map_err(|err| ForgeError::other(format!("issues_detail task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_comments(
    repo_id: RepoId,
    remote: String,
    number: u64,
    page: Option<u32>,
    per_page: u32,
    state: State<'_, RepoManager>,
) -> Result<IssueCommentPage, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        issue_comments_sync(&repo, Some(&remote), number, page, per_page)
    })
    .await
    .map_err(|err| ForgeError::other(format!("issues_comments task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_metadata(
    repo_id: RepoId,
    remote: String,
    state: State<'_, RepoManager>,
) -> Result<IssueMetadata, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || issue_metadata_sync(&repo, Some(&remote)))
        .await
        .map_err(|err| ForgeError::other(format!("issues_metadata task failed: {err}")))?
}

#[allow(clippy::too_many_arguments)] // Fixed Issues IPC mutation contract.
#[tauri::command(rename_all = "snake_case")]
pub async fn issues_create(
    repo_id: RepoId,
    remote: String,
    title: String,
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    milestone: Option<u64>,
    state: State<'_, RepoManager>,
) -> Result<IssueDetail, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        issue_create_sync(
            &repo,
            Some(&remote),
            title,
            body,
            labels,
            assignees,
            milestone,
        )
    })
    .await
    .map_err(|err| ForgeError::other(format!("issues_create task failed: {err}")))?
}

#[allow(clippy::too_many_arguments)] // Fixed Issues IPC mutation contract.
#[tauri::command(rename_all = "snake_case")]
pub async fn issues_update(
    repo_id: RepoId,
    remote: String,
    number: u64,
    title: String,
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    milestone: Option<u64>,
    state: State<'_, RepoManager>,
) -> Result<IssueDetail, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        issue_update_sync(
            &repo,
            Some(&remote),
            number,
            title,
            body,
            labels,
            assignees,
            milestone,
        )
    })
    .await
    .map_err(|err| ForgeError::other(format!("issues_update task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_comment(
    repo_id: RepoId,
    remote: String,
    number: u64,
    body: String,
    state: State<'_, RepoManager>,
) -> Result<IssueComment, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        issue_comment_sync(&repo, Some(&remote), number, body)
    })
    .await
    .map_err(|err| ForgeError::other(format!("issues_comment task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_set_state(
    repo_id: RepoId,
    remote: String,
    number: u64,
    state: String,
    comment: Option<String>,
    repo_state: State<'_, RepoManager>,
) -> Result<IssueDetail, ForgeError> {
    let repo = forge_repo_of(&get_handle(&repo_state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        issue_set_state_sync(&repo, Some(&remote), number, state, comment)
    })
    .await
    .map_err(|err| ForgeError::other(format!("issues_set_state task failed: {err}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn issues_activity_since(
    repo_id: RepoId,
    remote: String,
    issue_since: Option<String>,
    comment_since: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<IssueActivityBatch, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        issue_activity_sync(&repo, Some(&remote), issue_since, comment_since)
    })
    .await
    .map_err(|err| ForgeError::other(format!("issues_activity_since task failed: {err}")))?
}
