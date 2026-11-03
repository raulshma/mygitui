//! Native iced UI.
//!
//! Module map:
//! - [`app`] — shell state, message routing, header/footer, subscriptions
//! - [`theme`] — shadcn token port (light/dark) and widget styles
//! - [`config`] — JSON-persisted user settings (`config.json`)
//! - [`bus`] — iced subscription over the backend event bus
//! - [`home`] — repository switcher / recent list
//! - [`repo_view`] — tab strip, repo header and three-column workspace
//! - [`status_model`] — pure status sections, rows, selection and actions
//! - [`diff_model`] — pure diff row model, split pairing and virtualizer
//! - [`history_model`] — pure log accumulation, graph rows and filters
//! - [`status_panel`] — working-copy sections and file rows
//! - [`commit_bar`] — commit message, options and primary action
//! - [`diff_view`] — virtualized unified/split diff with line staging
//! - [`history_panel`] — commit list with the graph lane column
//! - [`graph_canvas`] — per-row lane canvas renderer
//! - [`commit_detail`] — selected commit metadata and file diff
//! - [`branch_panel`] — branch picker overlay
//! - [`stash_panel`] — stash stack and checkpoint snapshots overlay
//! - [`remote_panel`] — remotes, fetch/pull/push controls
//! - [`prompts`] — credential, host-key and device-secret modals
//! - [`dialog`] — modal confirmation overlay
//! - [`palette`] — command palette overlay (ranked command registry)
//! - [`context_menu`] — reusable right-click menu component
//! - [`quick_switcher`] — repo quick-switcher overlay (ctrl/cmd+K)

mod app;
mod branch_panel;
mod bus;
mod commit_bar;
mod commit_detail;
mod config;
mod context_menu;
mod dialog;
mod diff_model;
mod diff_view;
mod graph_canvas;
mod history_model;
mod history_panel;
mod home;
mod palette;
mod prompts;
mod quick_switcher;
mod remote_panel;
mod repo_view;
mod stash_panel;
mod status_model;
mod status_panel;
mod theme;

pub use app::run;
