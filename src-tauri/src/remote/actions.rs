//! Remote action runs: execute project scripts on the device over SSH exec,
//! streaming output through the same `action-output` events the local
//! Actions panel consumes.
//!
//! Cancellation: flipping the run's flag makes the streaming loop drop the
//! exec channel; the remote process gets the connection-closed SIGHUP the
//! same way an OpenSSH client's Ctrl-C would.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use ts_rs::TS;

use crate::actions::{ActionOutput, ActionSink};
use crate::remote::engine::RemoteGitEngine;
use crate::remote::parsers::redact_prompt;
use crate::remote::quoting::shell_quote;

/// Process-global registry of remote action runs (tauri-managed state).
#[derive(Default)]
pub struct RemoteActionRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    runs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    counter: AtomicU64,
}

impl RemoteActionRegistry {
    /// Launch `command` in the repo's workdir on the device. Returns the
    /// run id; output flows through `sink` as `action-output` events.
    pub fn run(
        &self,
        repo_id: &str,
        name: &str,
        command: &str,
        git: Arc<RemoteGitEngine>,
        sink: Arc<dyn ActionSink>,
    ) -> Result<String, String> {
        let run_id = format!(
            "ract-{:x}",
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        self.inner
            .runs
            .lock()
            .unwrap()
            .insert(run_id.clone(), cancelled.clone());

        let run_id_thread = run_id.clone();
        let run_id_cleanup = run_id.clone();
        let registry = self.inner.clone();
        let repo_id = repo_id.to_string();
        let name = name.to_string();
        let command = command.to_string();
        std::thread::Builder::new()
            .name(format!("remote-action-{run_id}"))
            .spawn(move || {
                // Same trust model as local actions: the command comes from
                // the user's own action config; the workdir is quoted.
                let cmd = format!("cd {} && ({})", shell_quote(&git.root), command);
                let result = tauri::async_runtime::block_on(git.session.exec_stream(
                    &cmd,
                    std::time::Duration::from_secs(60 * 60),
                    |_is_stderr, line| {
                        // Cancel aborts the exec itself: returning false
                        // makes the transport drop the channel, so the
                        // remote process gets the connection-closed SIGHUP
                        // the same way an OpenSSH client's Ctrl-C would.
                        if cancelled.load(Ordering::SeqCst) {
                            return false;
                        }
                        sink.emit(&ActionOutput {
                            run_id: run_id_thread.clone(),
                            repo_id: repo_id.clone(),
                            name: name.clone(),
                            line: redact_prompt(line),
                            done: false,
                            exit_code: None,
                        });
                        true
                    },
                ));
                let exit_code = match result {
                    Ok(out) => i32::try_from(out.exit_code).unwrap_or(-1),
                    Err(_) => -1,
                };
                sink.emit(&ActionOutput {
                    run_id: run_id_thread.clone(),
                    repo_id,
                    name,
                    line: String::new(),
                    done: true,
                    exit_code: Some(exit_code),
                });
                // Finished runs leave the registry so a later `cancel`
                // reports false (mirrors the local registry's done flag).
                registry.runs.lock().unwrap().remove(&run_id_cleanup);
                let _ = &git;
            })
            .map_err(|e| format!("spawn remote action: {e}"))?;
        Ok(run_id)
    }

    /// Cancel a run (`false` when unknown/finished).
    pub fn cancel(&self, run_id: &str) -> bool {
        match self.inner.runs.lock().unwrap().get(run_id) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}

/// `remote action` payload re-export for FE discoverability.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RemoteActionRunInfo {
    pub run_id: String,
}
