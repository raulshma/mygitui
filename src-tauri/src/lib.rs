pub mod engine;
pub mod graph;

use tauri::{webview::PageLoadEvent, Emitter, Manager};
use tauri_plugin_window_state::StateFlags;

mod actions;
#[cfg(test)]
mod actions_tests;
mod auth;
mod cli;
mod credential_helper;
mod diagnostics;
mod diffcore;
mod editors;
mod forge;
#[cfg(test)]
mod forge_tests;
mod ipc;
mod keyring_store;
mod maintenance;
mod mergetool;
mod opencode;
mod ops;
mod popout;
mod process;
mod projects;
#[cfg(test)]
mod prop_tests;
mod pty;
#[cfg(test)]
mod pty_tests;
pub mod remote;
mod repo;
mod shell_env;
mod updater;
mod watcher;
mod workspace;
#[cfg(test)]
mod workspace_tests;

/// Arguments captured at first launch (`mygitui <path>`), consumed once by
/// the frontend via `cli_args_initial`. The single-instance plugin handles
/// the *second*-launch path by emitting `cli-args` directly.
#[derive(Default)]
pub struct CliArgs(pub parking_lot::Mutex<Vec<String>>);

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
            // Tracing: stderr layer (`tauri dev` console; release builds
            // keep it but it is invisible without a console) + rotating
            // JSON-less line log under the app data dir
            // (`<app-data>/logs/mygitui.log`, see `diagnostics`). Both
            // layers share one env filter — RUST_LOG overrides, e.g.
            // RUST_LOG=mygitui_lib=debug to see device reconnect churn.
            // Lives in setup (not before the builder) because the log path
            // needs the app handle to resolve the app data dir.
            diagnostics::init_tracing(app.handle());
            auth::init(app.handle().clone());
            // Remote (SSH) devices: host-key confirms and secret asks reach
            // the frontend as events through this emitter; must be installed
            // before any device can connect.
            let remote_app = app.handle().clone();
            remote::prompts::init(move |event, payload| {
                let _ = remote_app.emit(event, payload.clone());
            });
            *app.state::<CliArgs>().0.lock() = cli_args;
            // Custom title bar (TitleBar.svelte): on Windows/Linux drop the
            // native chrome entirely so the app bar owns dragging and the
            // window controls. macOS keeps a decorated window and instead
            // uses the config's `titleBarStyle: "Overlay"` + `hiddenTitle`
            // (parsed on every platform, applied on macOS only) so the
            // native traffic lights float over the app bar. Must run while
            // the window is still hidden — config windows exist by setup
            // time and nothing is on screen yet, so no flash; popouts are
            // decorated by their builder in popout.rs instead.
            #[cfg(not(target_os = "macos"))]
            for window in app.webview_windows().values() {
                if let Err(err) = window.set_decorations(false) {
                    tracing::warn!("failed to drop native window chrome: {err}");
                }
            }
            // Warm the login-shell environment probe in the background: a
            // GUI-launched app has no shell profile in its env (nvm & co
            // missing), and action/editor spawns overlay the resolved env —
            // better paid at boot than on the first action run (bounded by
            // shell_env's timeout).
            std::thread::spawn(crate::shell_env::warm);
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
        // pty_kill + pty-output/pty-exit events) live here; the
        // RunEvent::Exit handler below kills every remaining shell (Tauri
        // does NOT drop managed state, so the registry's Drop never fires).
        .manage(pty::PtyRegistry::default())
        // AI: the managed `opencode serve` child (opencode_serve_start/
        // status/stop); killed by the RunEvent::Exit handler below.
        .manage(opencode::OpenCodeServerRegistry::default())
        // Remote (SSH) devices: live-session pool (`device_*` commands),
        // remote shell sessions (`rshell_*`), and remote action runs.
        .manage(remote::transport::DeviceRegistry::new())
        .manage(remote::term::RemoteShellRegistry::default())
        .manage(remote::actions::RemoteActionRegistry::default())
        // Self-update cycle: `updater_check` parks the plugin's `Update`
        // handle here for `updater_download_and_install` to consume (see
        // `updater.rs` for the flow).
        .manage(updater::PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            keyring_store::secrets_get,
            keyring_store::secrets_set,
            keyring_store::secrets_delete,
            ipc::status::repo_open,
            ipc::misc::cli_args_initial,
            ipc::misc::os_accent_color,
            ipc::misc::export_text,
            ipc::status::repo_close,
            ipc::status::repo_status,
            ipc::status::repo_summary,
            ipc::log::repo_commit_message,
            ipc::status::repo_refs,
            ipc::diff::repo_blame,
            ipc::diff::repo_diff,
            ipc::diff::repo_diff_stream,
            ipc::diff::repo_merge_base,
            ipc::log::repo_log_stream,
            ipc::log::repo_file_history,
            ipc::net::repo_clone,
            ipc::mutations::stage,
            ipc::mutations::stage_all,
            ipc::mutations::commit,
            ipc::mutations::signing_info,
            ipc::mutations::hooks_list,
            ipc::branches::branches,
            ipc::branches::branch_create,
            ipc::branches::branch_switch,
            ipc::branches::commit_checkout,
            ipc::branches::branch_is_merged,
            ipc::branches::branch_delete,
            ipc::branches::branch_rename,
            ipc::branches::tag_create,
            ipc::branches::tag_delete,
            ipc::branches::tag_list,
            ipc::branches::tag_create_signed,
            ipc::merge::mergetool_info,
            ipc::merge::mergetool_run,
            ipc::mutations::discard,
            ipc::branches::remote_branches,
            ipc::branches::branch_checkout_remote,
            ipc::branches::remotes,
            ipc::branches::remote_add,
            ipc::branches::remote_remove,
            ipc::branches::remote_set_url,
            ipc::net::fetch,
            ipc::net::pull,
            ipc::net::push,
            ipc::net::auth_respond,
            ipc::mutations::checkpoint_create,
            ipc::mutations::checkpoints,
            ipc::mutations::checkpoint_restore,
            ipc::mutations::checkpoint_gc,
            ipc::mutations::ops_preview,
            ipc::mutations::guard_checkpoint,
            ipc::merge::merge_branch,
            ipc::merge::merge_abort,
            ipc::merge::sequencer_abort,
            ipc::merge::conflicts,
            ipc::merge::conflict_resolve,
            ipc::merge::cherry_pick,
            ipc::merge::revert,
            ipc::merge::reset,
            ipc::merge::rebase_start,
            ipc::merge::rebase_state,
            ipc::merge::rebase_continue,
            ipc::merge::rebase_skip,
            ipc::merge::rebase_abort,
            ipc::merge::bisect_start,
            ipc::merge::bisect_state,
            ipc::merge::bisect_mark,
            ipc::merge::bisect_reset,
            ipc::merge::bisect_log,
            ipc::merge::describe,
            ipc::merge::autosquash_plan,
            ipc::mutations::commit_signature,
            ipc::mutations::branch_trash_list,
            ipc::mutations::branch_trash_restore,
            ipc::stash::worktree_prune,
            ipc::stash::worktree_lock,
            ipc::stash::worktree_unlock,
            ipc::mutations::op_cancel,
            ipc::health::repo_health,
            diagnostics::diagnostics_bundle,
            ipc::health::maintenance_run,
            ipc::health::archive,
            ipc::health::repo_bundle,
            ipc::health::sparse_info,
            ipc::health::sparse_apply,
            ipc::health::lfs_status,
            ipc::health::lfs_run,
            ipc::health::lfs_locks,
            ipc::health::lfs_lock,
            ipc::health::lfs_unlock,
            ipc::net::clone_blobless,
            ipc::stash::stash_list,
            ipc::stash::stash_push,
            ipc::stash::stash_show,
            ipc::stash::stash_rename,
            ipc::stash::stash_apply,
            ipc::stash::stash_drop,
            ipc::stash::stash_branch,
            ipc::stash::worktrees,
            ipc::stash::worktree_add,
            ipc::stash::worktree_remove,
            ipc::log::reflog,
            ipc::diff::repo_read_file,
            ipc::diff::repo_blob,
            workspace::repo_list_dir,
            workspace::repo_stat_file,
            workspace::repo_write_file,
            workspace::repo_create_entry,
            workspace::repo_rename_entry,
            workspace::repo_delete_entry,
            workspace::repo_file_index,
            ipc::branches::submodules,
            ipc::branches::submodule_update,
            ipc::branches::submodule_sync,
            ipc::branches::submodule_add,
            ipc::branches::submodule_deinit,
            ipc::log::commit_notes,
            ipc::log::note_add,
            ipc::log::note_remove,
            ipc::workspace::gitignore_add,
            ipc::workspace::gitignore_templates,
            ipc::workspace::gitignore_apply_template,
            ipc::mutations::repo_clean,
            ipc::misc::action_run,
            ipc::misc::action_cancel,
            pty::pty_create,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            ipc::forge::github_configure,
            ipc::forge::forge_status,
            ipc::forge::forge_hosts,
            ipc::forge::forge_context,
            ipc::forge::forge_snapshot,
            ipc::forge::forge_commit_checks,
            ipc::forge::forge_pr_files,
            ipc::forge::forge_pr_review_threads,
            ipc::forge::pr_create,
            ipc::forge::pr_list,
            ipc::forge::pr_checks,
            ipc::forge::pr_merge,
            ipc::forge::pr_close,
            ipc::forge::pr_reopen,
            ipc::forge::pr_ready,
            ipc::forge::pr_checkout,
            ipc::forge::pr_comment,
            ipc::forge::pr_review,
            ipc::forge::pr_edit,
            ipc::forge::pr_template,
            ipc::issues::issues_identity,
            ipc::issues::issues_list,
            ipc::issues::issues_detail,
            ipc::issues::issues_comments,
            ipc::issues::issues_metadata,
            ipc::issues::issues_create,
            ipc::issues::issues_update,
            ipc::issues::issues_comment,
            ipc::issues::issues_set_state,
            ipc::issues::issues_activity_since,
            ipc::forge::notifications_list,
            ipc::forge::notification_mark_read,
            ipc::forge::notifications_mark_all_read,
            ipc::forge::workflow_runs,
            ipc::forge::workflow_run_detail,
            ipc::forge::workflow_run_logs,
            ipc::forge::workflow_run_cancel,
            ipc::forge::workflow_run_rerun,
            ipc::forge::workflow_dispatch,
            ipc::forge::workflow_list,
            ipc::misc::project_detect,
            ipc::log::commit_activity,
            ipc::log::contributor_stats,
            opencode::opencode_detect,
            opencode::opencode_serve_status,
            opencode::opencode_serve_start,
            opencode::opencode_serve_stop,
            editors::detect_editors,
            editors::open_with,
            editors::open_with_custom,
            popout::open_popout,
            remote::commands::device_list,
            remote::commands::device_add,
            remote::commands::device_update,
            remote::commands::device_remove,
            remote::commands::device_connect,
            remote::commands::device_disconnect,
            remote::commands::device_status,
            remote::commands::device_test,
            remote::commands::device_hostkey_respond,
            remote::commands::device_auth_respond,
            remote::commands::device_forget_host_key,
            remote::commands::device_host_key,
            remote::commands::device_browse_dir,
            remote::commands::device_scan_repos,
            remote::commands::remote_repo_open,
            remote::commands::remote_poll_set_paused,
            remote::term::rshell_create,
            remote::term::rshell_write,
            remote::term::rshell_resize,
            remote::term::rshell_kill,
            updater::updater_check,
            updater::updater_download_and_install,
            updater::updater_relaunch,
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
        // registries' Drop impls never fire) — kill the managed opencode
        // serve tree and every remaining pty shell explicitly on the
        // graceful-exit path.
        .build(tauri::generate_context!())
        .expect("error while building mygitui")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(registry) = app.try_state::<pty::PtyRegistry>() {
                    registry.kill_all();
                }
                if let Some(registry) = app.try_state::<opencode::OpenCodeServerRegistry>() {
                    registry.stop();
                }
            }
        });
}
