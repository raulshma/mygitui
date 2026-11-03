//! Worktree commands: `worktrees` / `worktree_add` / `worktree_remove`.
//!
//! The worktree surface is small (three thin queue wrappers plus
//! `worktree_prune`, which lives next to the other history-rewriting
//! commands in [`crate::service::merge`]), so the handlers stay in
//! [`crate::service::stash`] instead of growing a module of their own. This
//! module is intentionally empty; it exists so a later lane can move the
//! worktree commands here without reshuffling [`crate::service`].
