//! Native iced user interface.
//!
//! `app` owns the application state, [`Message`](app::Message) enum, and the
//! subscription wiring to the backend event bus. Wave 3 expands this module
//! into the full shell (tabs, panes, theme); the entry point already owns the
//! single [`AppServices`](crate::service::AppServices) bag.

mod app;

pub use app::run;
