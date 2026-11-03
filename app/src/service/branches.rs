use crate::engine::git_engine::EngineError;
use crate::engine::types::{
    BranchInfo, BranchTrashEntry, CommitSignature, MergetoolInfo, MergetoolResult, RebaseStep,
    ReflogEntry, RemoteBranchInfo, RepoId, TagInfo,
};
use crate::service::{
    engine_err, enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read,
    finish_op, get_handle, AppServices,
};

/// Entry cap for the remote reflog read (the remote engine has no local
/// `name` argument; see the `reflog` command's remote branch).
const REMOTE_REFLOG_LIMIT: usize = 500;

pub async fn branches(services: &AppServices, repo_id: RepoId) -> Result<Vec<BranchInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branches(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "branch", move |_ctx, repo| {
        engine.branches(repo)
    }))
    .await
}

pub async fn branch_create(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    from: Option<String>,
    checkout: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_create(&name, from.as_deref(), checkout),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_create(repo, &name, from.as_deref(), checkout),
    ))
    .await
}

pub async fn branch_switch(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    force: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_switch(&name, force),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_switch(repo, &name, force),
    ))
    .await
}

/// `r#into` unraws to the IPC arg key `into` (contracts.md M2).
pub async fn branch_is_merged(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    r#into: String,
) -> Result<bool, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_is_merged(&name, &r#into),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "branch", move |_ctx, repo| {
        engine.branch_is_merged(repo, &name, &r#into)
    }))
    .await
}

pub async fn branch_delete(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    force: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_delete(&name, force),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_delete(repo, &name, force),
    ))
    .await
}

pub async fn branch_rename(
    services: &AppServices,
    repo_id: RepoId,
    old: String,
    new: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_rename(&old, &new),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_rename(repo, &old, &new),
    ))
    .await
}

pub async fn tag_create(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    target: Option<String>,
    message: Option<String>,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.tag_create(&name, target.as_deref(), message.as_deref()),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.tag_create(repo, &name, target.as_deref(), message.as_deref()),
    ))
    .await
}

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

pub async fn tag_list(services: &AppServices, repo_id: RepoId) -> Result<Vec<TagInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.tag_list(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    crate::rt::spawn_blocking(move || {
        let repo = repo.lock();
        engine.tag_list(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("tag_list task failed: {e}"))?
}

pub async fn remote_branches(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<RemoteBranchInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.remote_branches(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    crate::rt::spawn_blocking(move || {
        let repo = repo.lock();
        engine.remote_branches(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("remote_branches task failed: {e}"))?
}

pub async fn branch_checkout_remote(
    services: &AppServices,
    repo_id: RepoId,
    remote: String,
    name: String,
    new_local: Option<String>,
) -> Result<String, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote_handle) = handle.remote() {
        // `remote` here is the IPC arg (the remote's config name), so the
        // handle binding is `remote_handle`.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote_handle,
            "branch",
            move |git, _ws| git.branch_checkout_remote(&remote, &name, new_local.as_deref()),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_checkout_remote(repo, &remote, &name, new_local.as_deref()),
    ))
    .await
}

/// Create a signed annotated tag via the git CLI (`git tag -s`; libgit2
/// cannot sign). Runs hooks/GPG natively.
pub async fn tag_create_signed(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    target: Option<String>,
    message: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The device's own git + gpg sign the tag (same CLI semantics).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.tag_create_signed(&name, target.as_deref(), &message),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, _repo| {
            crate::mergetool::tag_sign_via_cli(&root, &name, target.as_deref(), &message)
                .map_err(EngineError::Invalid)
        },
    ))
    .await
}

/// Configured external merge tools (pure config read).
pub async fn mergetool_info(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<MergetoolInfo, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if handle.remote().is_some() {
        return Err("mergetool_info is not supported for remote projects".into());
    }
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "merge", move |_ctx, repo| {
        Ok(crate::mergetool::mergetool_info(repo))
    }))
    .await
}

/// Launch `git mergetool` for one conflicted path. Deliberately NOT on the
/// op queue: the external tool runs interactively for minutes, and blocking
/// every repo op behind it would freeze the app (watcher events still fire
/// when the tool writes files).
pub async fn mergetool_run(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
    tool: Option<String>,
) -> Result<MergetoolResult, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if handle.remote().is_some() {
        // An interactive local merge tool cannot run against a workdir on
        // another device; refuse explicitly instead of failing on the
        // nonexistent local path.
        return Err("mergetool_run is not supported for remote projects".into());
    }
    let workdir = handle.root.clone();
    crate::rt::spawn_blocking(move || {
        crate::mergetool::mergetool_run(&workdir, &path, tool.as_deref())
    })
    .await
    .map_err(|e| format!("mergetool_run task failed: {e}"))?
}

// ---------- M12: signature verify / branch trash / prune / bisect log ----------

/// Verification status of one commit's signature (queued read, like the
/// other engine reads).
pub async fn commit_signature(
    services: &AppServices,
    repo_id: RepoId,
    sha: String,
) -> Result<CommitSignature, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "commit",
            move |git, _ws| git.commit_signature(&sha),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "commit", move |_ctx, repo| {
        engine.commit_signature(repo, &sha)
    }))
    .await
}

/// Branches preserved under `refs/mygitui/trash/*`, newest first (read).
pub async fn branch_trash_list(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<BranchTrashEntry>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_trash_list(),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "branch", move |_ctx, repo| {
        engine.branch_trash_list(repo)
    }))
    .await
}

/// Restore a trashed branch (optionally renamed); returns the branch name.
pub async fn branch_trash_restore(
    services: &AppServices,
    repo_id: RepoId,
    id: String,
    new_name: Option<String>,
) -> Result<String, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine's restore takes no rename; compose the rename
        // from branch_rename. The entry id is `<name>-<timestamp>`, so the
        // original name strips back from the numeric suffix (mirrors the
        // engine's own `strip_trash_suffix`).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| {
                git.branch_trash_restore(&id)?;
                let name = id
                    .trim_end_matches(|c: char| c.is_ascii_digit())
                    .trim_end_matches('-')
                    .to_string();
                match new_name {
                    Some(new) => {
                        git.branch_rename(&name, &new)?;
                        Ok(new)
                    }
                    None => Ok(name),
                }
            },
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_trash_restore(repo, &id, new_name.as_deref()),
    ))
    .await
}

pub async fn reflog(
    services: &AppServices,
    repo_id: RepoId,
    name: Option<String>,
) -> Result<Vec<ReflogEntry>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine reads HEAD's reflog only; a specific ref gets a
        // clear error instead of silently wrong data.
        if name
            .as_deref()
            .map(str::trim)
            .is_some_and(|n| !n.is_empty() && n != "HEAD")
        {
            return Err("reflog for a specific ref is not supported for remote projects".into());
        }
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "reflog",
            move |git, _ws| git.reflog(REMOTE_REFLOG_LIMIT),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "reflog", move |_ctx, repo| {
        engine.reflog(repo, name.as_deref())
    }))
    .await
}

/// `git describe --tags` for a commit-ish.
pub async fn describe(
    services: &AppServices,
    repo_id: RepoId,
    spec: String,
) -> Result<String, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "describe",
            move |git, _ws| git.describe(&spec),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "describe",
        move |_ctx, repo| engine.describe(repo, &spec),
    ))
    .await
}

/// Builds an autosquash rebase plan for `base..HEAD` (feeds `rebase_start`).
pub async fn autosquash_plan(
    services: &AppServices,
    repo_id: RepoId,
    base: String,
) -> Result<Vec<RebaseStep>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.autosquash_plan(&base),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "rebase", move |_ctx, repo| {
        engine.autosquash_plan(repo, &base)
    }))
    .await
}
