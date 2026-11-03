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

mod app;
mod bus;
mod commit_bar;
mod commit_detail;
mod config;
mod diff_model;
mod diff_view;
mod graph_canvas;
mod history_model;
mod history_panel;
mod home;
mod repo_view;
mod status_model;
mod status_panel;
mod theme;

pub use app::run;
