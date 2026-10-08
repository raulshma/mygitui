pub mod engine;
pub mod graph;

use tauri::{Emitter, Manager};

mod actions;
#[cfg(test)]
mod actions_tests;
mod auth;
mod cli;
mod diffcore;
mod forge;
#[cfg(test)]
mod forge_tests;
mod ipc_commands;
mod keyring_store;
mod ops;
mod pty;
#[cfg(test)]
mod pty_tests;
mod repo;
mod watcher;

/// M0 bootstrap: single-instance with CLI arg forwarding, updater wiring.
/// The L1 lane adds `keyring_store` (secret storage for AI provider keys).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Must be the last plugin registered: it decides whether this instance
        // runs or defers to the existing one.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second launch (`mygitui <path>`) forwards its argv to the
            // running instance; the frontend opens it as a repo tab (M1).
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.emit("cli-args", argv);
                let _ = window.set_focus();
            }
        }))
        // M2: the auth broker emits `auth-request` events through this
        // handle; must be registered before the first net op, hence before
        // `.manage` hands out the RepoManager.
        .setup(|app| {
            auth::init(app.handle().clone());
            Ok(())
        })
        .manage(repo::RepoManager::new())
        // M4: custom action runs (action_run/action_cancel + action-output
        // events) track their live children here.
        .manage(actions::ActionRegistry::default())
        // M5: embedded terminal sessions (pty_create/pty_write/pty_resize/
        // pty_kill + pty-output/pty-exit events) live here; dropping the
        // registry on app exit kills every remaining shell.
        .manage(pty::PtyRegistry::default())
        .invoke_handler(tauri::generate_handler![
            keyring_store::secrets_get,
            keyring_store::secrets_set,
            keyring_store::secrets_delete,
            ipc_commands::repo_open,
            ipc_commands::repo_close,
            ipc_commands::repo_status,
            ipc_commands::repo_refs,
            ipc_commands::repo_blame,
            ipc_commands::repo_diff,
            ipc_commands::repo_diff_stream,
            ipc_commands::repo_log_stream,
            ipc_commands::repo_file_history,
            ipc_commands::repo_clone,
            ipc_commands::stage,
            ipc_commands::stage_all,
            ipc_commands::commit,
            ipc_commands::signing_info,
            ipc_commands::hooks_list,
            ipc_commands::branches,
            ipc_commands::branch_create,
            ipc_commands::branch_switch,
            ipc_commands::branch_is_merged,
            ipc_commands::branch_delete,
            ipc_commands::branch_rename,
            ipc_commands::tag_create,
            ipc_commands::tag_delete,
            ipc_commands::remotes,
            ipc_commands::remote_add,
            ipc_commands::remote_remove,
            ipc_commands::remote_set_url,
            ipc_commands::fetch,
            ipc_commands::pull,
            ipc_commands::push,
            ipc_commands::auth_respond,
            ipc_commands::checkpoint_create,
            ipc_commands::checkpoints,
            ipc_commands::checkpoint_restore,
            ipc_commands::checkpoint_gc,
            ipc_commands::ops_preview,
            ipc_commands::guard_checkpoint,
            ipc_commands::merge_branch,
            ipc_commands::merge_abort,
            ipc_commands::conflicts,
            ipc_commands::conflict_resolve,
            ipc_commands::cherry_pick,
            ipc_commands::revert,
            ipc_commands::reset,
            ipc_commands::rebase_start,
            ipc_commands::rebase_state,
            ipc_commands::rebase_continue,
            ipc_commands::rebase_abort,
            ipc_commands::stash_list,
            ipc_commands::stash_push,
            ipc_commands::stash_apply,
            ipc_commands::stash_drop,
            ipc_commands::stash_branch,
            ipc_commands::worktrees,
            ipc_commands::worktree_add,
            ipc_commands::worktree_remove,
            ipc_commands::reflog,
            ipc_commands::repo_read_file,
            ipc_commands::submodules,
            ipc_commands::submodule_update,
            ipc_commands::submodule_sync,
            ipc_commands::gitignore_add,
            ipc_commands::gitignore_templates,
            ipc_commands::repo_clean,
            ipc_commands::action_run,
            ipc_commands::action_cancel,
            pty::pty_create,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            ipc_commands::forge_status,
            ipc_commands::forge_context,
            ipc_commands::pr_create,
            ipc_commands::pr_list,
            ipc_commands::pr_checks,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mygitui");
}
