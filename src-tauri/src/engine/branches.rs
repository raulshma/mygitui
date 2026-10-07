//! Branch and tag operations.
//! Lane C2 owns this file: replace the `*_impl` stubs with real git2 logic.

use git2::Repository;

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::BranchInfo;

impl Libgit2Engine {
    pub(crate) fn branches_impl(&self, _repo: &Repository) -> EngineResult<Vec<BranchInfo>> {
        Err(EngineError::Unsupported("branches".into()))
    }

    pub(crate) fn branch_create_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _from: Option<&str>,
        _checkout: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_create".into()))
    }

    pub(crate) fn branch_switch_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _force: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_switch".into()))
    }

    pub(crate) fn branch_is_merged_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _into: &str,
    ) -> EngineResult<bool> {
        Err(EngineError::Unsupported("branch_is_merged".into()))
    }

    pub(crate) fn branch_delete_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _force: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_delete".into()))
    }

    pub(crate) fn branch_rename_impl(
        &self,
        _repo: &Repository,
        _old: &str,
        _new: &str,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_rename".into()))
    }

    pub(crate) fn tag_create_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _target: Option<&str>,
        _message: Option<&str>,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("tag_create".into()))
    }

    pub(crate) fn tag_delete_impl(&self, _repo: &Repository, _name: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("tag_delete".into()))
    }
}
