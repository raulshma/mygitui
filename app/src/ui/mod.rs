//! Native iced UI.
//!
//! Module map:
//! - [`app`] — shell state, message routing, header/footer, subscriptions
//! - [`theme`] — shadcn token port (light/dark) and widget styles
//! - [`config`] — JSON-persisted user settings (`config.json`)
//! - [`bus`] — iced subscription over the backend event bus
//! - [`home`] — repository switcher / recent list + SSH devices section
//! - [`devices`] — SSH device management (Home section, add/edit dialog,
//!   device file browser / repo scanner)
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
//! - [`clone_dialog`] — clone-repository dialog (URL + destination +
//!   options, streaming clone progress)
//! - [`palette`] — command palette overlay (ranked command registry)
//! - [`context_menu`] — reusable right-click menu component
//! - [`quick_switcher`] — repo quick-switcher overlay (ctrl/cmd+K)
//! - [`quick_open`] — quick-open overlay (ctrl/cmd+P): fuzzy file search
//!   over the workspace file index, opens in the in-app editor
//! - [`terminal`] — embedded PTY terminal pane (monitor with input)
//! - [`forge_panel`] — GitHub PRs, CI checks and workflow runs (gh CLI)
//! - [`stats_panel`] — contribution heatmap and contributor leaderboard
//! - [`actions_panel`] / [`output_panel`] / [`editor_view`] — custom
//!   shell actions with live output, and the in-app file editor
//! - [`blame_view`] / [`file_history_view`] / [`compare_view`] —
//!   center-pane read views (blame rows, per-file history, two-ref diff)
//! - [`files_panel`] — files panel (lazy working-tree browser as a center
//!   mode, git-state decorations, context menu, create/rename/delete)
//! - [`worktree_panel`] / [`submodule_panel`] / [`tags_panel`] —
//!   worktree management, submodule sync and tag management dropdowns
//! - [`health_panel`] — repo health census, maintenance ops, archive,
//!   sparse checkout and LFS
//! - [`bisect_panel`] — bisect stepper banner (start/mark/log/reset)
//! - [`reflog_panel`] — reflog browser with reset/branch-from intents

mod actions_panel;
mod app;
mod bisect_panel;
mod blame_view;
mod branch_panel;
mod bus;
mod clone_dialog;
mod commit_bar;
mod commit_detail;
mod compare_view;
mod config;
mod context_menu;
mod devices;
mod dialog;
mod diff_model;
mod diff_view;
mod editor_view;
mod file_history_view;
mod files_panel;
mod forge_panel;
mod graph_canvas;
mod health_panel;
mod history_model;
mod history_panel;
mod home;
mod output_panel;
mod palette;
mod prompts;
mod quick_open;
mod quick_switcher;
mod reflog_panel;
mod remote_panel;
mod repo_view;
mod stash_panel;
mod stats_panel;
mod status_model;
mod status_panel;
mod submodule_panel;
mod tags_panel;
mod terminal;
mod theme;
mod worktree_panel;

pub use app::run;
