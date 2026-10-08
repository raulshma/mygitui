//! Contribution statistics — M7 lane I1.
//!
//! Two read-only rollups over the full commit graph, sharing one revwalk:
//!
//! - [`Libgit2Engine::commit_activity_impl`] buckets commits per local day
//!   ("YYYY-MM-DD") for the contribution heatmap; an optional `author`
//!   substring filters by author name OR email (case-insensitive).
//! - [`Libgit2Engine::contributor_stats_impl`] groups the same walk by
//!   author identity (exact name+email pair) into per-contributor counts
//!   and active ranges.
//!
//! Semantics shared by both:
//! - The walk covers every commit reachable from a branch, remote branch,
//!   tag, or HEAD. Internal namespaces (`refs/mygitui/*`, e.g. checkpoint
//!   snapshots) are deliberately NOT pushed — their synthetic commits must
//!   not count as user activity. The revwalk dedupes shared history, so a
//!   commit reachable from several refs counts once.
//! - Buckets use the COMMITTER timestamp plus its recorded UTC offset (the
//!   "local date" the commit was made in); identity/filtering uses the
//!   AUTHOR signature. This mirrors `git shortlog`/`git log --since`
//!   conventions closely enough for a heatmap while staying deterministic.
//! - The window is `[now - max_days, now]` against the committer time.
//! - The walk is capped at [`MAX_WALK_COMMITS`] commits (defensive bound
//!   for huge monorepos; the cap applies per call, before filtering).
//!
//! M2-lane convention: inherent `*_impl` methods on `Libgit2Engine`; the
//! IPC commands in `ipc_commands.rs` call them directly.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use git2::{ErrorCode, Repository, Sort};
use serde::Serialize;

use super::git_engine::EngineResult;
use super::libgit2::Libgit2Engine;

/// Hard cap on commits walked per stats call (contracts.md M7).
const MAX_WALK_COMMITS: usize = 100_000;

/// Seconds in a day.
const DAY_SECS: i64 = 86_400;

/// Days offset of the Unix epoch from the civil date 1970-01-01 (zero).
const EPOCH_SHIFT: i64 = 719_468;

/// Commits per local day (`commit_activity` reply; contracts.md M7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayCount {
    /// Committer local date, "YYYY-MM-DD".
    pub day: String,
    /// Commits bucketed on that day.
    pub count: u32,
}

/// One author identity aggregated over the window (`contributor_stats`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contributor {
    /// Author name (verbatim from the commit signature).
    pub name: String,
    /// Author email (verbatim from the commit signature).
    pub email: String,
    /// Commits by this identity inside the window.
    pub count: u32,
    /// First active day (committer local date, "YYYY-MM-DD").
    pub first_day: String,
    /// Last active day (committer local date, "YYYY-MM-DD").
    pub last_day: String,
}

impl Libgit2Engine {
    /// Commits per local day inside the window, day ascending; days with
    /// zero commits are omitted. `author` (when non-blank) is a
    /// case-insensitive substring matched against the author name OR email.
    pub(crate) fn commit_activity_impl(
        &self,
        repo: &Repository,
        max_days: u32,
        author: Option<&str>,
    ) -> EngineResult<Vec<DayCount>> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        for_each_commit_in_window(repo, max_days, author, |commit| {
            let day = local_day_string(commit);
            *counts.entry(day).or_default() += 1;
        })?;
        let mut out: Vec<DayCount> = counts
            .into_iter()
            .map(|(day, count)| DayCount { day, count })
            .collect();
        out.sort_by(|a, b| a.day.cmp(&b.day));
        Ok(out)
    }

    /// Per-author rollups inside the window, count descending (ties broken
    /// by name, then email, for deterministic output). Identity is the
    /// exact `(name, email)` pair — no case folding, no mailmap.
    pub(crate) fn contributor_stats_impl(
        &self,
        repo: &Repository,
        max_days: u32,
    ) -> EngineResult<Vec<Contributor>> {
        struct Rollup {
            count: u32,
            first: i64,
            last: i64,
        }
        let mut rollups: HashMap<(String, String), Rollup> = HashMap::new();
        for_each_commit_in_window(repo, max_days, None, |commit| {
            let author = commit.author();
            let key = (
                author.name().unwrap_or_default().to_owned(),
                author.email().unwrap_or_default().to_owned(),
            );
            let day = local_day_number(commit);
            let entry = rollups.entry(key).or_insert(Rollup {
                count: 0,
                first: day,
                last: day,
            });
            entry.count += 1;
            entry.first = entry.first.min(day);
            entry.last = entry.last.max(day);
        })?;
        let mut out: Vec<Contributor> = rollups
            .into_iter()
            .map(|((name, email), r)| Contributor {
                name,
                email,
                count: r.count,
                first_day: day_string(r.first),
                last_day: day_string(r.last),
            })
            .collect();
        out.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.email.cmp(&b.email))
        });
        Ok(out)
    }
}

/// Runs `visit` for every commit reachable from a branch, remote branch,
/// tag, or HEAD (deduped by the revwalk), inside the `[now - max_days,
/// now]` committer-time window and matching the optional author substring.
/// The walk aborts at [`MAX_WALK_COMMITS`] commits.
fn for_each_commit_in_window(
    repo: &Repository,
    max_days: u32,
    author: Option<&str>,
    mut visit: impl FnMut(&git2::Commit<'_>),
) -> EngineResult<()> {
    let now = now_secs();
    let cutoff = now - i64::from(max_days) * DAY_SECS;
    // Case-insensitive substring over name or email; blank needle = no filter.
    let needle = author
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TIME)?;
    // An unborn HEAD simply contributes nothing (an empty repo yields empty
    // stats); libgit2 reports it with a generic error code, so probe the
    // ref instead of pattern-matching the push error.
    if repo.head().is_ok() {
        walk.push_head()?;
    }
    // Explicit namespace pushes: internal `refs/mygitui/*` snapshots (and
    // any other exotic ref) must not count as user activity.
    for glob in ["refs/heads/*", "refs/remotes/*", "refs/tags/*"] {
        if let Err(err) = walk.push_glob(glob) {
            if err.code() != ErrorCode::NotFound {
                return Err(err.into());
            }
        }
    }

    let mut seen = 0usize;
    for oid in walk {
        seen += 1;
        if seen > MAX_WALK_COMMITS {
            break;
        }
        let Ok(oid) = oid else {
            continue; // unreachable/corrupt ref entry — skip, keep walking
        };
        let Ok(commit) = repo.find_commit(oid) else {
            continue; // shallow/grafted gap — skip
        };
        if commit.time().seconds() < cutoff {
            // Time-sorted walk: everything after this is older than the
            // window. `continue` (not `break`) stays correct even when
            // libgit2 interleaves equal timestamps across refs.
            continue;
        }
        if let Some(needle) = &needle {
            let author = commit.author();
            let name = author.name().unwrap_or_default().to_lowercase();
            let email = author.email().unwrap_or_default().to_lowercase();
            if !name.contains(needle) && !email.contains(needle) {
                continue;
            }
        }
        visit(&commit);
    }
    Ok(())
}

/// Committer local day as days-since-epoch (committer time + UTC offset).
fn local_day_number(commit: &git2::Commit<'_>) -> i64 {
    let time = commit.time();
    (time.seconds() + i64::from(time.offset_minutes()) * 60).div_euclid(DAY_SECS)
}

/// Committer local day as "YYYY-MM-DD".
fn local_day_string(commit: &git2::Commit<'_>) -> String {
    day_string(local_day_number(commit))
}

/// Formats days-since-epoch as a civil ISO date (Howard Hinnant's
/// `civil_from_days`; no chrono dependency). `pub(crate)` so the stats
/// tests derive expected buckets with the exact same math.
pub(crate) fn day_string(days_since_epoch: i64) -> String {
    let z = days_since_epoch + EPOCH_SHIFT;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Whole seconds since the Unix epoch (UTC).
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::day_string;

    #[test]
    fn day_string_formats_iso_dates() {
        assert_eq!(day_string(0), "1970-01-01");
        assert_eq!(day_string(1), "1970-01-02");
        assert_eq!(day_string(31), "1970-02-01");
        assert_eq!(day_string(365), "1971-01-01");
        // 2024-01-01 (a leap year start) and 2025-01-01 after 366 days.
        assert_eq!(day_string(19_723), "2024-01-01");
        assert_eq!(day_string(19_723 + 366), "2025-01-01");
        assert_eq!(day_string(-1), "1969-12-31");
    }
}
