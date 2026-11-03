//! Native iced UI.
//!
//! Module map:
//! - [`app`] — shell state, message routing, header/footer, subscriptions
//! - [`theme`] — shadcn token port (light/dark) and widget styles
//! - [`config`] — JSON-persisted user settings (`config.json`)
//! - [`bus`] — iced subscription over the backend event bus
//! - [`home`] — repository switcher / recent list
//! - [`repo_view`] — tab strip, repo header and working-copy status

mod app;
mod bus;
mod config;
mod home;
mod repo_view;
mod theme;

pub use app::run;
