//! M1 IPC command surface (names/shapes fixed by docs/contracts.md) — facade.
//!
//! The handlers were split by domain into [`crate::service`] (repo, diff, log,
//! branches, network, merge, stash, worktree, maintenance, forge, files,
//! misc). This module only re-exports that surface, so `lib.rs`'s
//! `tauri::generate_handler![ipc_commands::...]` paths keep resolving, as do
//! the `#[tauri::command]` wrapper macros generated next to each handler
//! (glob re-exports carry the macro namespace too).

pub use crate::service::*;
