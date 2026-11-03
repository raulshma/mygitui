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
mod diffcore;
mod editors;
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
mod projects;
#[cfg(test)]
mod prop_tests;
mod pty;
#[cfg(test)]
mod pty_tests;
pub mod remote;
mod repo;
mod rt;
pub mod service;
mod watcher;
mod workspace;
#[cfg(test)]
mod workspace_tests;

/// Arguments captured at first launch (`mygitui <path>`), consumed once by
/// the frontend via `cli_args_initial`. The single-instance plugin handles
/// the *second*-launch path by emitting `cli-args` directly.
#[derive(Default)]
pub struct CliArgs(pub std::sync::Mutex<Vec<String>>);

/// M0 bootstrap: single-instance with CLI arg forwarding, updater wiring.
/// The L1 lane adds `keyring_store` (secret storage for AI provider keys).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Tracing to stderr (`tauri dev` console; release builds keep it but it
    // is invisible without a console). RUST_LOG overrides, e.g.
    // RUST_LOG=mygitui_lib=debug to see device reconnect churn.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,mygitui_lib=info")),
        )
        .with_target(false)
        .try_init()
        .ok();
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
        // M2: the auth broker publishes `auth-request` events on the backend
        // event bus; must be registered before the first net op.
        .setup(move |app| {
            auth::init();
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
        // Remote (SSH) devices: live-session pool (`device_*` commands),
        // remote shell sessions (`rshell_*`), and remote action runs.
        .manage(remote::transport::DeviceRegistry::new())
        .manage(remote::term::RemoteShellRegistry::default())
        .manage(remote::actions::RemoteActionRegistry::default())
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
