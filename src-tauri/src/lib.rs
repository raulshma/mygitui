use tauri::{Emitter, Manager};

mod keyring_store;

/// M0 bootstrap: single-instance with CLI arg forwarding, updater wiring.
/// The L1 lane adds `keyring_store` (secret storage for AI provider keys).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
        .invoke_handler(tauri::generate_handler![
            keyring_store::secrets_get,
            keyring_store::secrets_set,
            keyring_store::secrets_delete,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mygitui");
}
