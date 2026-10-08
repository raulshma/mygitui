pub mod branches;
pub mod git_engine;
pub mod libgit2;
pub mod mutations;
pub mod netops;
pub mod types;

#[cfg(test)]
mod tests;

pub mod checkpoints;
pub mod merge;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod net_tests;
pub mod rebase;
#[cfg(test)]
mod rebase_tests;
pub mod stash;
#[cfg(test)]
mod stash_tests;

#[cfg(test)]
mod checkpoint_tests;
pub mod stats;
#[cfg(test)]
mod stats_tests;
#[cfg(test)]
mod submodule_tests;
pub mod submodules;
