pub mod engine;
pub mod graph;

use tauri::{Emitter, Manager, webview::PageLoadEvent};
use tauri_plugin_window_state::StateFlags;

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
mod maintenance;
mod mergetool;
mod opencode;
mod ops;
mod popout;
mod process;
#[cfg(test)]
mod prop_tests;
mod pty;
#[cfg(test)]
mod pty_tests;
mod repo;
mod watcher;

/// Arguments captured at first launch (`mygitui <path>`), consumed once by
/// the frontend via `cli_args_initial`. The single-instance plugin handles
/// the *second*-launch path by emitting `cli-args` directly.
#[derive(Default)]
pub struct CliArgs(pub std::sync::Mutex<Vec<String>>);

/// M0 bootstrap: single-instance with CLI arg forwarding, updater wiring.
/// The L1 lane adds `keyring_store` (secret storage for AI provider keys).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // M12: first-launch `mygitui <path>` — previously argv was only read on
    // the second-instance path, so the first launch silently ignored it.
    let cli_args: Vec<String> = std::env::args().skip(1).collect();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // M12: `mygitui://open?path=...` deep links. The plugin re-fires
        // second-instance URLs through the same scheme; the frontend routes
        // them in `src/lib/entry/deeplink.ts`.
        .plugin(tauri_plugin_deep_link::init())
        // Restores window position/size/maximized state across launches
        // (state file under the app config dir; saved on close). The
        // VISIBLE flag is excluded on purpose: the windows below are
        // created hidden (main: `"visible": false` in tauri.conf.json,
        // popouts: `.visible(false)` in popout.rs) so the restore's
        // position/size/maximize round-trip happens before the window is
        // ever on screen — with the default flags the plugin would call
        // `show()` mid-restore and the user would watch the webview
        // repaint twice (the "UI doesn't fill the window width" twitch).
        // The page-load hook at the bottom of this builder shows each
        // window once its document has actually loaded.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                .build(),
        )
        // Must be the last plugin registered: it decides whether this instance
        // runs or defers to the existing one.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second launch (`mygitui <path>`) forwards its argv to the
            // running instance; the frontend opens it as a repo tab (M1).
            // argv[0] is this executable's own path — never a repo argument.
            let args: Vec<String> = argv.iter().skip(1).cloned().collect();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.emit("cli-args", args);
                let _ = window.set_focus();
            }
        }))
        // M2: the auth broker emits `auth-request` events through this
        // handle; must be registered before the first net op, hence before
        // `.manage` hands out the RepoManager.
        .setup(move |app| {
            auth::init(app.handle().clone());
            *app.state::<CliArgs>().0.lock().unwrap() = cli_args;
            // Failsafe for the hidden-until-loaded windows: if the page
            // never fires a load event (broken asset pipeline, webview
            // error page without one), don't leave the user with an
            // invisible main window.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if let Some(window) = handle.get_webview_window("main") {
                    if !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                    }
                }
            });
            Ok(())
        })
        .manage(CliArgs::default())
        .manage(repo::RepoManager::new())
        // M4: custom action runs (action_run/action_cancel + action-output
        // events) track their live children here.
        .manage(actions::ActionRegistry::default())
        // M5: embedded terminal sessions (pty_create/pty_write/pty_resize/
        // pty_kill + pty-output/pty-exit events) live here; dropping the
        // registry on app exit kills every remaining shell.
        .manage(pty::PtyRegistry::default())
        // AI: the managed `opencode serve` child (opencode_serve_start/
        // status/stop); dropped on app exit, killing the server.
        .manage(opencode::OpenCodeServerRegistry::default())
        .invoke_handler(tauri::generate_handler![
            keyring_store::secrets_get,
            keyring_store::secrets_set,
            keyring_store::secrets_delete,
            ipc_commands::repo_open,
            ipc_commands::cli_args_initial,
            ipc_commands::os_accent_color,
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
            ipc_commands::tag_list,
            ipc_commands::tag_create_signed,
            ipc_commands::mergetool_info,
            ipc_commands::mergetool_run,
            ipc_commands::discard,
            ipc_commands::remote_branches,
            ipc_commands::branch_checkout_remote,
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
            ipc_commands::sequencer_abort,
            ipc_commands::conflicts,
            ipc_commands::conflict_resolve,
            ipc_commands::cherry_pick,
            ipc_commands::revert,
            ipc_commands::reset,
            ipc_commands::rebase_start,
            ipc_commands::rebase_state,
            ipc_commands::rebase_continue,
            ipc_commands::rebase_abort,
            ipc_commands::bisect_start,
            ipc_commands::bisect_state,
            ipc_commands::bisect_mark,
            ipc_commands::bisect_reset,
            ipc_commands::bisect_log,
            ipc_commands::describe,
            ipc_commands::autosquash_plan,
            ipc_commands::commit_signature,
            ipc_commands::branch_trash_list,
            ipc_commands::branch_trash_restore,
            ipc_commands::worktree_prune,
            ipc_commands::op_cancel,
            ipc_commands::repo_health,
            ipc_commands::maintenance_run,
            ipc_commands::archive,
            ipc_commands::sparse_info,
            ipc_commands::sparse_apply,
            ipc_commands::lfs_status,
            ipc_commands::lfs_run,
            ipc_commands::clone_blobless,
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
            ipc_commands::gitignore_apply_template,
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
            ipc_commands::commit_activity,
            ipc_commands::contributor_stats,
            opencode::opencode_detect,
            opencode::opencode_serve_status,
            opencode::opencode_serve_start,
            opencode::opencode_serve_stop,
            popout::open_popout,
        ])
        // Windows are created hidden so the window-state plugin's restore
        // (position/size/maximize) lands before anything is on screen; this
        // shows each window — main and popouts alike — once its document
        // has finished loading, at its final size. The main window also
        // takes focus, replacing the focus grab the plugin's VISIBLE flag
        // used to perform on restore.
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished {
                let window = webview.window();
                let _ = window.show();
                if window.label() == "main" {
                    let _ = window.set_focus();
                }
            }
        })
        // Tauri does NOT drop managed state on exit (verified live: the
        // pty registry's Drop comment notwithstanding) — kill the managed
        // opencode serve tree explicitly on the graceful-exit path.
        .build(tauri::generate_context!())
        .expect("error while building mygitui")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(registry) = app.try_state::<opencode::OpenCodeServerRegistry>() {
                    registry.stop();
                }
            }
        });
}
