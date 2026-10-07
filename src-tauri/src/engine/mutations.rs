//! Staging, committing, signing detection, hook introspection.
//! Lane C1 owns this file: replace the `*_impl` stubs with real git2 logic.

use git2::Repository;

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{CommitOptions, HookInfo, SigningInfo, StageRequest};

impl Libgit2Engine {
    pub(crate) fn stage_impl(&self, _repo: &Repository, _req: &StageRequest) -> EngineResult<()> {
        Err(EngineError::Unsupported("stage".into()))
    }

    pub(crate) fn stage_all_impl(&self, _repo: &Repository, _unstage: bool) -> EngineResult<()> {
        Err(EngineError::Unsupported("stage_all".into()))
    }

    pub(crate) fn commit_impl(
        &self,
        _repo: &Repository,
        _opts: &CommitOptions,
    ) -> EngineResult<String> {
        Err(EngineError::Unsupported("commit".into()))
    }

    pub(crate) fn signing_info_impl(&self, _repo: &Repository) -> EngineResult<SigningInfo> {
        Err(EngineError::Unsupported("signing_info".into()))
    }

    pub(crate) fn hooks_impl(&self, _repo: &Repository) -> EngineResult<Vec<HookInfo>> {
        Err(EngineError::Unsupported("hooks".into()))
    }
}
