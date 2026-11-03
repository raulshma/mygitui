pub mod engine;
pub mod graph;

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
mod keyring_store;
mod maintenance;
mod mergetool;
mod opencode;
mod ops;
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
mod ui;
mod watcher;
mod workspace;
#[cfg(test)]
mod workspace_tests;

/// Arguments captured at first launch (`mygitui <path>`), consumed once by
/// the UI to open the requested repository. Second-launch arguments arrive as
/// [`service::events::BackendEvent::CliArgs`] once single-instance forwarding
/// is restored.
#[derive(Default)]
pub struct CliArgs(pub std::sync::Mutex<Vec<String>>);

pub fn run() -> iced::Result {
    // Tracing to stderr (release builds keep it but it is invisible without
    // a console). RUST_LOG overrides, e.g. RUST_LOG=mygitui_lib=debug to see
    // device reconnect churn.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,mygitui_lib=info")),
        )
        .with_target(false)
        .try_init()
        .ok();
    let cli_args: Vec<String> = std::env::args().skip(1).collect();
    ui::run(cli_args)
}
