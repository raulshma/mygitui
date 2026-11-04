pub mod ai;
pub mod engine;
pub mod graph;

mod actions;
#[cfg(test)]
mod actions_tests;
mod auth;
mod cli;
mod credential_helper;
mod diffcore;
// Backend modules below are retained for panels that land in later
// milestones (terminal, file tree, open-in-editor, AI/opencode, secrets
// settings) and are dead until their iced wiring arrives. Drop each allow
// as the matching panel starts using its backend.
#[allow(dead_code)]
mod editors;
mod forge;
#[cfg(test)]
mod forge_tests;
mod instance;
#[allow(dead_code)]
mod keyring_store;
mod maintenance;
mod mergetool;
#[allow(dead_code)]
mod opencode;
mod ops;
mod process;
mod projects;
#[cfg(test)]
mod prop_tests;
#[allow(dead_code)]
mod pty;
#[cfg(test)]
mod pty_tests;
pub mod remote;
mod repo;
mod rt;
pub mod service;
mod ui;
mod update_check;
mod updater;
mod watcher;
#[allow(dead_code)]
mod workspace;
#[cfg(test)]
mod workspace_tests;

/// Arguments captured at first launch (`mygitui <path>`), consumed once by
/// the UI to open the requested repository. Second-launch arguments arrive
/// as [`service::events::BackendEvent::CliArgs`] via single-instance
/// forwarding (see [`instance`]).
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

    // Single-instance handoff: a live primary receives this launch's argv
    // and we exit before starting any UI. Decided on a throwaway runtime —
    // the accept loop (if primary) runs on iced's tokio executor.
    let role = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .map(|rt| rt.block_on(instance::bind_or_forward(&cli_args)));
    let listener: instance::SharedListener = match role {
        #[cfg(unix)]
        Some(instance::Role::Forwarded) => return Ok(()),
        Some(instance::Role::Primary(listener)) => {
            std::sync::Arc::new(std::sync::Mutex::new(listener))
        }
        None => std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    ui::run(cli_args, listener)
}
