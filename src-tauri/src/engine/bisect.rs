//! Bisect (lane M10): binary search over history for the first bad commit.
//!
//! State lives at `<gitdir>/mygitui/bisect.json` (its presence = an active
//! bisect), mirroring the rebase engine's persistence model. The candidate
//! set is `revwalk(bad, hide good + skipped)`; each `bisect_mark` checks out
//! the midpoint of the remaining candidates (`git bisect` semantics for a
//! linear first-bad search — parents-only, not the full DAG median, which
//! matches git's default approximation closely and is deterministic).
//!
//! Pure candidate/midpoint logic is separated for unit testing.

use serde::{Deserialize, Serialize};

use git2::build::CheckoutBuilder;
use git2::{Oid, Repository, Sort};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::BisectState;

/// Persisted bisect state (schema in the module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BisectFileState {
    /// Bad tip (search starts here).
    bad: String,
    /// Known-good commit (exclusive floor). Empty string = no good yet
    /// (candidates = whole history of `bad`).
    good: String,
    /// Skipped commit shas (excluded from candidates).
    skipped: Vec<String>,
    /// HEAD sha when the bisect started — the reset target.
    orig_head: String,
    /// Full branch ref HEAD pointed at when the bisect started.
    orig_branch: Option<String>,
}

fn state_path(repo: &Repository) -> std::path::PathBuf {
    repo.path().join("mygitui").join("bisect.json")
}

fn load_state(repo: &Repository) -> EngineResult<Option<BisectFileState>> {
    let path = state_path(repo);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| EngineError::Invalid(format!("corrupt mygitui/bisect.json: {e}")))
}

fn save_state(repo: &Repository, st: &BisectFileState) -> EngineResult<()> {
    std::fs::create_dir_all(repo.path().join("mygitui"))?;
    let json = serde_json::to_string_pretty(st)
        .map_err(|e| EngineError::Invalid(format!("cannot serialize bisect state: {e}")))?;
    std::fs::write(state_path(repo), json)?;
    Ok(())
}

fn remove_state(repo: &Repository) {
    let _ = std::fs::remove_file(state_path(repo));
}

/// Candidate oids: first-parent walk from `bad`, stopping at `good` (and at
/// any skipped commit's boundary), oldest-first. Pure (operates on oids).
fn candidates_between(
    repo: &Repository,
    bad: Oid,
    good: Option<Oid>,
    skipped: &[String],
) -> EngineResult<Vec<Oid>> {
    let skip_oids: Vec<Oid> = skipped
        .iter()
        .filter_map(|s| Oid::from_str(s).ok())
        .collect();
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::REVERSE | Sort::TOPOLOGICAL)?;
    walk.push(bad)?;
    if let Some(good) = good {
        walk.hide(good)?;
    }
    for s in &skip_oids {
        walk.hide(*s)?;
    }
    Ok(walk.filter_map(Result::ok).collect())
}

/// Midpoint commit of the remaining candidates (git bisect's probe).
fn midpoint<'r>(repo: &'r Repository, candidates: &[Oid]) -> EngineResult<git2::Commit<'r>> {
    let mid = candidates
        .get(candidates.len() / 2)
        .ok_or_else(|| EngineError::Invalid("no candidates left to probe".into()))?;
    Ok(repo.find_commit(*mid)?)
}

fn to_public(repo: &Repository, st: &BisectFileState) -> EngineResult<BisectState> {
    let bad = Oid::from_str(&st.bad)
        .map_err(|e| EngineError::Invalid(format!("corrupt bisect bad tip: {e}")))?;
    let good = if st.good.is_empty() {
        None
    } else {
        Some(
            Oid::from_str(&st.good)
                .map_err(|e| EngineError::Invalid(format!("corrupt bisect good commit: {e}")))?,
        )
    };
    // Untested probes: the walk from bad minus the (already known-bad) tip
    // itself. 0 ⇒ `bad` is the first bad commit.
    let remaining = candidates_between(repo, bad, good, &st.skipped)?
        .into_iter()
        .filter(|oid| *oid != bad)
        .count();
    Ok(BisectState {
        active: true,
        bad: st.bad.clone(),
        good: if st.good.is_empty() {
            None
        } else {
            Some(st.good.clone())
        },
        current: repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|o| o.to_string()),
        remaining,
        skipped: st.skipped.clone(),
        orig_head: st.orig_head.clone(),
        orig_branch: st.orig_branch.clone(),
        log: Vec::new(),
    })
}

/// Probe selection: midpoint of the walk MINUS the bad tip (the known-bad
/// commit is never re-probed). Empty slice = the search is over.
fn probe_candidates(
    repo: &Repository,
    bad: Oid,
    good: Option<Oid>,
    skipped: &[String],
) -> EngineResult<Vec<Oid>> {
    Ok(candidates_between(repo, bad, good, skipped)?
        .into_iter()
        .filter(|oid| *oid != bad)
        .collect())
}

/// Force the workdir+index to `commit` and detach HEAD (shared semantics
/// with the rebase engine's replay base move).
fn checkout_detached(repo: &Repository, commit: &git2::Commit<'_>) -> EngineResult<()> {
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_tree(commit.as_object(), Some(&mut checkout))?;
    repo.set_head_detached(commit.id())?;
    Ok(())
}

impl Libgit2Engine {
    pub(crate) fn bisect_start_impl(
        &self,
        repo: &Repository,
        bad: Option<&str>,
        good: Option<&str>,
    ) -> EngineResult<BisectState> {
        if load_state(repo)?.is_some() {
            return Err(EngineError::Invalid(
                "a bisect is already in progress; reset it first".into(),
            ));
        }
        let bad_commit = match bad {
            Some(spec) => resolve_commit(repo, spec)?,
            None => head_commit(repo)?,
        };
        let good_commit = match good {
            Some(spec) => Some(resolve_commit(repo, spec)?),
            None => None,
        };
        if let (Some(good), bad) = (&good_commit, bad_commit.id()) {
            if good.id() == bad {
                return Err(EngineError::Invalid(
                    "good and bad are the same commit".into(),
                ));
            }
        }

        let st = BisectFileState {
            bad: bad_commit.id().to_string(),
            good: good_commit
                .as_ref()
                .map(|c| c.id().to_string())
                .unwrap_or_default(),
            skipped: Vec::new(),
            orig_head: head_commit(repo)?.id().to_string(),
            orig_branch: repo
                .find_reference("HEAD")
                .ok()
                .and_then(|head| head.symbolic_target().ok().flatten().map(str::to_owned)),
        };
        save_state(repo, &st)?;

        // Probe the midpoint right away (no candidates without a good bound
        // → probe the bad tip itself).
        let candidates = probe_candidates(repo, bad_commit.id(), good_commit.map(|c| c.id()), &[])?;
        let probe = if candidates.is_empty() {
            bad_commit
        } else {
            midpoint(repo, &candidates)?
        };
        checkout_detached(repo, &probe)?;
        to_public(repo, &st)
    }

    pub(crate) fn bisect_state_impl(&self, repo: &Repository) -> EngineResult<BisectState> {
        match load_state(repo)? {
            Some(st) => to_public(repo, &st),
            None => Ok(BisectState {
                active: false,
                bad: String::new(),
                good: None,
                current: None,
                remaining: 0,
                skipped: Vec::new(),
                orig_head: String::new(),
                orig_branch: None,
                log: Vec::new(),
            }),
        }
    }

    /// Marks the checked-out probe `good` / `bad` / `skip` and checks out
    /// the next candidate. `first_bad` is returned when exactly one
    /// candidate remains (the caller toasts it; the bisect stays active
    /// until `bisect_reset` so the user can inspect).
    pub(crate) fn bisect_mark_impl(
        &self,
        repo: &Repository,
        mark: super::types::BisectMark,
    ) -> EngineResult<BisectState> {
        let mut st = load_state(repo)?.ok_or_else(|| {
            EngineError::Invalid("no bisect in progress (mygitui/bisect.json missing)".into())
        })?;
        let current = head_commit(repo)?;

        match mark {
            super::types::BisectMark::Good => st.good = current.id().to_string(),
            super::types::BisectMark::Bad => {
                st.bad = current.id().to_string();
            }
            super::types::BisectMark::Skip => {
                if !st.skipped.contains(&current.id().to_string()) {
                    st.skipped.push(current.id().to_string());
                }
            }
        }
        save_state(repo, &st)?;

        // Next probe: midpoint of what is left.
        let bad = Oid::from_str(&st.bad)
            .map_err(|e| EngineError::Invalid(format!("corrupt bisect bad tip: {e}")))?;
        let good = if st.good.is_empty() {
            None
        } else {
            Some(
                Oid::from_str(&st.good)
                    .map_err(|e| EngineError::Invalid(format!("corrupt bisect good: {e}")))?,
            )
        };
        let candidates = probe_candidates(repo, bad, good, &st.skipped)?;
        if candidates.is_empty() {
            return to_public(repo, &st); // done — `bad` is the first bad commit
        }
        let probe = midpoint(repo, &candidates)?;
        if probe.id() != current.id() {
            checkout_detached(repo, &probe)?;
        }
        to_public(repo, &st)
    }

    pub(crate) fn bisect_reset_impl(&self, repo: &Repository) -> EngineResult<()> {
        let st = load_state(repo)?.ok_or_else(|| {
            EngineError::Invalid("no bisect in progress (mygitui/bisect.json missing)".into())
        })?;
        let orig = repo.find_commit(
            Oid::from_str(&st.orig_head)
                .map_err(|e| EngineError::Invalid(format!("corrupt bisect orig_head: {e}")))?,
        )?;
        if let Some(branch) = st.orig_branch.as_deref() {
            repo.set_head(branch)?;
        } else {
            repo.set_head_detached(orig.id())?;
        }
        checkout_detached(repo, &orig)?;
        if let Some(branch) = st.orig_branch.as_deref() {
            // checkout_detached detached HEAD; re-attach to the branch.
            repo.set_head(branch)?;
        }
        remove_state(repo);
        Ok(())
    }
}

fn resolve_commit<'r>(repo: &'r Repository, spec: &str) -> EngineResult<git2::Commit<'r>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?;
    obj.peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("`{spec}` is not a commit: {e}")))
}

fn head_commit(repo: &Repository) -> EngineResult<git2::Commit<'_>> {
    repo.head()
        .map_err(|_| EngineError::Invalid("HEAD is unborn".into()))?
        .peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("HEAD is not a commit: {e}")))
}
