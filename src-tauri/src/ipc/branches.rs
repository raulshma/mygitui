//! Branches, tags, remotes, and submodules: ref CRUD (queue mutations,
//! checkout progress forwarded to `op-progress`), remote config CRUD, and
//! the submodule lifecycle (`update`/`sync`/`add`/`deinit`). Tags report as
//! queue kind `branch` (ref-affecting); remote config ops as `fetch` (net
//! category). `tag_create_signed` and `submodule_add` ride the CLI lane
//! (libgit2 cannot sign; submodule add stages a gitlink + `.gitmodules`).

use tauri::State;

use crate::engine::git_engine::EngineError;
use crate::engine::types::{BranchInfo, RemoteBranchInfo, RemoteInfo, RepoId, TagInfo};
use crate::repo::RepoManager;

use super::{
    emit_checkout_progress, engine_err, enqueue_mutation, enqueue_read, enqueue_remote_mutation,
    enqueue_remote_read, finish_op, get_handle,
};

#[tauri::command(rename_all = "snake_case")]
pub async fn branches(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<BranchInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
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

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_create(
    repo_id: RepoId,
    name: String,
    from: Option<String>,
    checkout: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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
        move |ctx, repo| {
            engine.branch_create(repo, &name, from.as_deref(), checkout, &mut |progress| {
                emit_checkout_progress(&ctx, progress)
            })
        },
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_switch(
    repo_id: RepoId,
    name: String,
    force: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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
        move |ctx, repo| {
            engine.branch_switch(repo, &name, force, &mut |progress| {
                emit_checkout_progress(&ctx, progress)
            })
        },
    ))
    .await
}

/// Detach HEAD at a commit (`git checkout --detach <sha>`), the history
/// panel's "checkout commit" action. The workdir+index are forced to the
/// commit's tree, so the FE snapshots a `guard_checkpoint("pre-checkout")`
/// first (same flow as the hard resets).
#[tauri::command(rename_all = "snake_case")]
pub async fn commit_checkout(
    repo_id: RepoId,
    sha: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "checkout",
            move |git, _ws| git.checkout_detach(&sha),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "checkout",
        move |ctx, repo| {
            engine.checkout_commit(repo, &sha, &mut |progress| {
                emit_checkout_progress(&ctx, progress)
            })
        },
    ))
    .await
}

/// `r#into` unraws to the IPC arg key `into` (contracts.md M2).
#[tauri::command(rename_all = "snake_case")]
pub async fn branch_is_merged(
    repo_id: RepoId,
    name: String,
    r#into: String,
    state: State<'_, RepoManager>,
) -> Result<bool, String> {
    let handle = get_handle(&state, &repo_id)?;
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

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_delete(
    repo_id: RepoId,
    name: String,
    force: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_rename(
    repo_id: RepoId,
    old: String,
    new: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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

#[tauri::command(rename_all = "snake_case")]
pub async fn tag_create(
    repo_id: RepoId,
    name: String,
    target: Option<String>,
    message: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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

#[tauri::command(rename_all = "snake_case")]
pub async fn tag_delete(
    repo_id: RepoId,
    name: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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

#[tauri::command(rename_all = "snake_case")]
pub async fn tag_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<TagInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
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
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.tag_list(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("tag_list task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_branches(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<RemoteBranchInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
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
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.remote_branches(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("remote_branches task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_checkout_remote(
    repo_id: RepoId,
    remote: String,
    name: String,
    new_local: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
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
        move |ctx, repo| {
            engine.branch_checkout_remote(
                repo,
                &remote,
                &name,
                new_local.as_deref(),
                &mut |progress| emit_checkout_progress(&ctx, progress),
            )
        },
    ))
    .await
}

/// Create a signed annotated tag via the git CLI (`git tag -s`; libgit2
/// cannot sign). Runs hooks/GPG natively.
#[tauri::command(rename_all = "snake_case")]
pub async fn tag_create_signed(
    repo_id: RepoId,
    name: String,
    target: Option<String>,
    message: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
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
                .map_err(EngineError::cli)
        },
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remotes(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<RemoteInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remotes(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "fetch", move |_ctx, repo| {
        engine.remotes(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_add(
    repo_id: RepoId,
    name: String,
    url: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remote_add(&name, &url),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "fetch",
        move |_ctx, repo| engine.remote_add(repo, &name, &url),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_remove(
    repo_id: RepoId,
    name: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remote_remove(&name),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "fetch",
        move |_ctx, repo| engine.remote_remove(repo, &name),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_set_url(
    repo_id: RepoId,
    name: String,
    url: String,
    push: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remote_set_url(&name, &url, push),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "fetch",
        move |_ctx, repo| engine.remote_set_url(repo, &name, &url, push),
    ))
    .await
}

use crate::engine::types::SubmoduleInfo;

/// List every submodule with checked-out vs recorded state (contracts.md:
/// `SubmoduleInfo[]`; status strings mirror `git status` submodule wording).
#[tauri::command(rename_all = "snake_case")]
pub async fn submodules(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<SubmoduleInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodules(),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| engine.submodules(repo),
    ))
    .await
}

/// `git submodule update [--init] [--recursive] <path>` for one submodule.
/// Network fetches (missing clone / missing target commit) route credential
/// prompts through the auth broker attributed to this repo.
#[tauri::command(rename_all = "snake_case")]
pub async fn submodule_update(
    repo_id: RepoId,
    path: String,
    init: bool,
    recursive: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine updates all submodules (no per-path argument);
        // the device session brings its own credentials, so no auth broker.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodule_update(init, recursive),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| {
            crate::auth::with_netop_context(&repo_id, || {
                engine.submodule_update(repo, &path, init, recursive)
            })
        },
    ))
    .await
}

/// `git submodule sync [path]`: rewrite the submodule URL from `.gitmodules`
/// into the repository config. `None`/empty `path` syncs all submodules.
#[tauri::command(rename_all = "snake_case")]
pub async fn submodule_sync(
    repo_id: RepoId,
    path: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine syncs all submodules (no per-path argument);
        // `recursive: false` mirrors the local non-recursive sync.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodule_sync(false),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| engine.submodule_sync(repo, path.as_deref()),
    ))
    .await
}

/// `git submodule add [--name <name>] <url> <path>` (Phase 5, CLI lane):
/// clones the submodule, stages the gitlink and the `.gitmodules` entry.
/// URL/path run through the CLI clone's guard (`clone_args_ok`) before the
/// op is queued.
#[tauri::command(rename_all = "snake_case")]
pub async fn submodule_add(
    repo_id: RepoId,
    url: String,
    path: String,
    name: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    // Same up-front guard family as clone_blobless (defense in depth: the
    // engine impl re-checks).
    if !crate::maintenance::clone_args_ok(&url, &path) {
        return Err("invalid submodule arguments".into());
    }
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodule_add(&url, &path, name.as_deref()),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| engine.submodule_add(repo, &url, &path, name.as_deref()),
    ))
    .await
}

/// `git submodule deinit -f <path>` (Phase 5, CLI lane): unregister the
/// submodule and empty its workdir; `.gitmodules` and the index gitlink
/// stay (re-init via `submodule_update(init = true)`).
#[tauri::command(rename_all = "snake_case")]
pub async fn submodule_deinit(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodule_deinit(&path),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| engine.submodule_deinit(repo, &path),
    ))
    .await
}
