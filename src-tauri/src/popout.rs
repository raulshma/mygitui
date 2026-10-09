//! Popout windows (M4 F1, M9): runtime-created windows rendering a single
//! panel (`diff` / `history` / `filehistory` / `commitdetail`). The frontend
//! (`src/lib/layout/popout.ts`) passes the label, query string and title; the
//! window itself is built here instead of through the core
//! `create-webview-window` command for one Windows-specific reason:
//!
//! WebView2 refuses a controller whose environment options differ from the
//! environment already running on the same user data folder
//! (0x8007139F ERROR_INVALID_STATE, "The group or resource is not in the
//! correct state"). The main window is configured with
//! `additionalBrowserArgs` in tauri.conf.json, and the JS API cannot set
//! browser args — so a JS-created popout would build a second environment
//! with wry's default args and fail. Copying the configured args here keeps
//! every webview on one shared environment (and one browser process).

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// Opens the popout `label`, or focuses it when it already exists.
///
/// `query` is the window's own URL query (`?panel=...&repo=...`), joined
/// onto the app origin by `WebviewUrl::App`.
#[tauri::command]
pub async fn open_popout(
    app: tauri::AppHandle,
    label: String,
    query: String,
    title: String,
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let browser_args = app
        .config()
        .app
        .windows
        .iter()
        .find_map(|w| w.additional_browser_args.clone());
    let mut builder =
        WebviewWindowBuilder::new(&app, label, WebviewUrl::App(query.into()))
            .title(title)
            .inner_size(960.0, 680.0)
            .min_inner_size(420.0, 300.0);
    if let Some(args) = browser_args {
        builder = builder.additional_browser_args(&args);
    }
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}
