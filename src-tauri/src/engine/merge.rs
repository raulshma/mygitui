//! Merge, conflicts, cherry-pick, revert, reset — lane D1 owns this file.
//! Replace this placeholder impl with real implementations.

use git2::Repository;

use crate::engine::git_engine::{EngineResult, GitEngineM3};
use crate::engine::libgit2::Libgit2Engine;

// Temporary satisfaction of the trait bound; D1 replaces this block with
// real methods (the trait's defaults keep the app compiling until then).
impl GitEngineM3 for Libgit2Engine {
    fn placeholder(&self, _repo: &Repository) -> EngineResult<()> {
        unimplemented!()
    }
}
