//! Remote management + fetch/pull/push with progress and credential plumbing.
//! Lane C2 owns this file: replace the `*_impl` stubs with real git2 logic.
//! Credentials: run network ops with a `git2::Credentials` source supplied by
//! the platform layer (auth module) — see `with_credentials` wiring in repo/.

use git2::Repository;

use super::git_engine::{EngineError, EngineResult, FetchProgress, PushProgress};
use super::libgit2::Libgit2Engine;
use super::types::{FetchOptions, NetStats, PullOptions, PushOptions, RemoteInfo};

impl Libgit2Engine {
    pub(crate) fn remotes_impl(&self, _repo: &Repository) -> EngineResult<Vec<RemoteInfo>> {
        Err(EngineError::Unsupported("remotes".into()))
    }

    pub(crate) fn remote_add_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _url: &str,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("remote_add".into()))
    }

    pub(crate) fn remote_remove_impl(&self, _repo: &Repository, _name: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("remote_remove".into()))
    }

    pub(crate) fn remote_set_url_impl(
        &self,
        _repo: &Repository,
        _name: &str,
        _url: &str,
        _push: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("remote_set_url".into()))
    }

    pub(crate) fn fetch_impl(
        &self,
        _repo: &Repository,
        _opts: &FetchOptions,
        _progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        Err(EngineError::Unsupported("fetch".into()))
    }

    pub(crate) fn pull_impl(
        &self,
        _repo: &Repository,
        _opts: &PullOptions,
        _progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        Err(EngineError::Unsupported("pull".into()))
    }

    pub(crate) fn push_impl(
        &self,
        _repo: &Repository,
        _opts: &PushOptions,
        _progress: &mut dyn FnMut(PushProgress),
    ) -> EngineResult<NetStats> {
        Err(EngineError::Unsupported("push".into()))
    }
}
