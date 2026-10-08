//! Contribution-statistics engine tests (M7 lane I1).
//!
//! Everything runs against throwaway repositories under
//! `std::env::temp_dir()` (mirrors `checkpoint_tests.rs`). Commits are
//! forged with explicit signatures (`Signature::new` + `Time::new`, UTC
//! offset 0) so buckets are deterministic relative to `now`. Expected day
//! strings are always derived from the same offsets the commits were
//! forged with (via the shared `day_string` helper), so midnight rollovers
//! during a test run cannot flake the assertions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use git2::{IndexAddOption, Repository, RepositoryInitOptions, Signature, Time};

use super::libgit2::Libgit2Engine;
use super::stats::day_string;

const ENGINE: Libgit2Engine = Libgit2Engine;
const DAY_SECS: i64 = 86_400;

// ---------------------------------------------------------------------------
// temp scaffolding
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("mygitui-stats-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best-effort delayed cleanup (Windows handle release, like the
        // checkpoint tests).
        let dir = self.0.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

fn init_repo(path: &Path) -> Repository {
    let mut opts = RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = Repository::init_opts(path, &opts).expect("init temp repo");
    repo.config()
        .and_then(|mut c| {
            c.set_str("user.name", "Stats Test")?;
            c.set_str("user.email", "stats@test.local")?;
            c.set_bool("core.autocrlf", false)
        })
        .expect("configure identity");
    repo
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Write + stage + commit on HEAD with an explicit author/committer
/// signature at `secs` (UTC, offset 0); returns the new sha.
fn commit_at(
    repo: &Repository,
    path: &str,
    content: &str,
    message: &str,
    name: &str,
    email: &str,
    secs: i64,
) -> String {
    let file = repo.workdir().unwrap().join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let sig = Signature::new(name, email, &Time::new(secs, 0)).expect("signature");
    let mut parents = Vec::new();
    if let Ok(head) = repo.head() {
        parents.push(head.peel_to_commit().expect("head commit"));
    }
    let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .expect("commit");
    oid.to_string()
}

/// Commit with an explicit parent WITHOUT moving HEAD (builds side refs).
fn commit_detached_at(
    repo: &Repository,
    parent_sha: &str,
    path: &str,
    content: &str,
    name: &str,
    email: &str,
    secs: i64,
) -> String {
    let file = repo.workdir().unwrap().join(path);
    std::fs::write(file, content).expect("write file");
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let sig = Signature::new(name, email, &Time::new(secs, 0)).expect("signature");
    let parent = repo
        .revparse_single(parent_sha)
        .expect("parent")
        .peel_to_commit()
        .expect("parent commit");
    repo.commit(None, &sig, &sig, "detached", &tree, &[&parent])
        .expect("commit")
        .to_string()
}

/// Activity buckets as a `day -> count` map for easy assertions.
fn activity_map(repo: &Repository, max_days: u32, author: Option<&str>) -> HashMap<String, u32> {
    ENGINE
        .commit_activity_impl(repo, max_days, author)
        .expect("commit_activity")
        .into_iter()
        .map(|dc| (dc.day, dc.count))
        .collect()
}

/// Expected day string for a commit forged `ago_secs` before the test's
/// `now` (identical math to the engine: UTC day of `now - ago_secs`).
fn expected_day(now: i64, ago_secs: i64) -> String {
    day_string((now - ago_secs).div_euclid(DAY_SECS))
}

// ---------------------------------------------------------------------------
// commit_activity
// ---------------------------------------------------------------------------

#[test]
fn activity_buckets_days_within_window_and_respects_max_days() {
    let dir = TempDir::new("activity-window");
    let repo = init_repo(dir.path());
    let now = now_secs();

    commit_at(
        &repo,
        "a.txt",
        "1\n",
        "today",
        "Alice",
        "alice@acme.dev",
        now,
    );
    commit_at(
        &repo,
        "b.txt",
        "2\n",
        "today+1h",
        "Alice",
        "alice@acme.dev",
        now - 3_600,
    );
    commit_at(
        &repo,
        "c.txt",
        "3\n",
        "yesterday-ish",
        "Bob",
        "bob@other.io",
        now - DAY_SECS,
    );
    commit_at(
        &repo,
        "d.txt",
        "4\n",
        "3 days ago",
        "Alice",
        "alice@acme.dev",
        now - 3 * DAY_SECS,
    );
    commit_at(
        &repo,
        "e.txt",
        "5\n",
        "40 days ago",
        "Alice",
        "alice@acme.dev",
        now - 40 * DAY_SECS,
    );

    // 30-day window: the 40-day-old commit is outside; the two "now-ish"
    // commits may share a day (midnight straddle) — expectations are built
    // dynamically from the same offsets.
    let mut expected: HashMap<String, u32> = HashMap::new();
    for ago in [0, 3_600, DAY_SECS, 3 * DAY_SECS] {
        *expected.entry(expected_day(now, ago)).or_default() += 1;
    }
    assert_eq!(activity_map(&repo, 30, None), expected, "30-day window");

    // 90-day window: everything comes in.
    let mut expected90 = expected.clone();
    *expected90
        .entry(expected_day(now, 40 * DAY_SECS))
        .or_default() += 1;
    assert_eq!(activity_map(&repo, 90, None), expected90, "90-day window");

    // The wire shape is day-ascending with counts > 0.
    let flat = ENGINE.commit_activity_impl(&repo, 30, None).expect("flat");
    assert!(
        flat.windows(2).all(|w| w[0].day < w[1].day),
        "sorted: {flat:?}"
    );
    assert!(flat.iter().all(|dc| dc.count > 0), "no empty buckets");
}

#[test]
fn activity_author_filter_matches_name_or_email_substring() {
    let dir = TempDir::new("activity-author");
    let repo = init_repo(dir.path());
    let now = now_secs();

    commit_at(
        &repo,
        "a.txt",
        "1\n",
        "a1",
        "Alice Author",
        "alice@acme.dev",
        now,
    );
    commit_at(
        &repo,
        "b.txt",
        "2\n",
        "a2",
        "Alice Author",
        "alice@acme.dev",
        now,
    );
    commit_at(&repo, "c.txt", "3\n", "a3", "Bob", "bob@other.io", now);

    let total = |map: &HashMap<String, u32>| -> u32 { map.values().sum() };
    assert_eq!(total(&activity_map(&repo, 30, None)), 3, "no filter");
    assert_eq!(
        total(&activity_map(&repo, 30, Some("   "))),
        3,
        "blank = no filter"
    );
    assert_eq!(
        total(&activity_map(&repo, 30, Some("alice"))),
        2,
        "name substring"
    );
    assert_eq!(
        total(&activity_map(&repo, 30, Some("ACME"))),
        2,
        "email substring, case-insensitive"
    );
    assert_eq!(
        total(&activity_map(&repo, 30, Some("bob@OTHER"))),
        1,
        "other author via email"
    );
    assert_eq!(total(&activity_map(&repo, 30, Some("zzz"))), 0, "no match");
}

#[test]
fn shared_history_counted_once_across_refs() {
    let dir = TempDir::new("activity-dedupe");
    let repo = init_repo(dir.path());
    let now = now_secs();

    let c1 = commit_at(
        &repo,
        "a.txt",
        "1\n",
        "base",
        "Alice",
        "alice@acme.dev",
        now - 8 * DAY_SECS,
    );
    let c2 = commit_at(
        &repo,
        "b.txt",
        "2\n",
        "tip",
        "Alice",
        "alice@acme.dev",
        now - 6 * DAY_SECS,
    );
    // Side branch + tag both reach the shared history.
    let c3 = commit_detached_at(
        &repo,
        &c2,
        "c.txt",
        "3\n",
        "Alice",
        "alice@acme.dev",
        now - 4 * DAY_SECS,
    );
    repo.reference("refs/heads/feature", c3.parse().unwrap(), true, "test")
        .expect("feature ref");
    repo.reference("refs/tags/v1", c1.parse().unwrap(), true, "test")
        .expect("tag ref");

    let total: u32 = activity_map(&repo, 90, None).values().sum();
    assert_eq!(total, 3, "c1/c2 shared by main+feature+tag count once");

    // Contributor rollups agree (same dedupe).
    let contributors = ENGINE
        .contributor_stats_impl(&repo, 90)
        .expect("contributor_stats");
    assert_eq!(contributors.len(), 1);
    assert_eq!(contributors[0].count, 3);
}

#[test]
fn internal_refs_are_excluded_from_activity() {
    let dir = TempDir::new("activity-internal-refs");
    let repo = init_repo(dir.path());
    let now = now_secs();

    let head = commit_at(
        &repo,
        "a.txt",
        "1\n",
        "real",
        "Alice",
        "alice@acme.dev",
        now,
    );
    // A mygitui-internal snapshot commit (checkpoint-style): reachable ONLY
    // from refs/mygitui/*, so it must not count as activity.
    let hidden = commit_detached_at(
        &repo,
        &head,
        "snapshot.txt",
        "snap\n",
        "Alice",
        "alice@acme.dev",
        now,
    );
    repo.reference(
        "refs/mygitui/checkpoints/1-test",
        hidden.parse().unwrap(),
        true,
        "test",
    )
    .expect("internal ref");

    let total: u32 = activity_map(&repo, 30, None).values().sum();
    assert_eq!(total, 1, "internal snapshot commit excluded");
}

#[test]
fn empty_repo_yields_empty_stats() {
    let dir = TempDir::new("activity-empty");
    let repo = init_repo(dir.path());
    assert!(ENGINE
        .commit_activity_impl(&repo, 365, None)
        .expect("activity")
        .is_empty());
    assert!(ENGINE
        .contributor_stats_impl(&repo, 365)
        .expect("contributors")
        .is_empty());
}

// ---------------------------------------------------------------------------
// contributor_stats
// ---------------------------------------------------------------------------

#[test]
fn contributors_group_by_identity_count_and_range() {
    let dir = TempDir::new("contributors");
    let repo = init_repo(dir.path());
    let now = now_secs();

    // Alice: 3 commits over three distinct forged offsets (days 0/2/4 back).
    commit_at(&repo, "a.txt", "1\n", "a1", "Alice", "alice@acme.dev", now);
    commit_at(
        &repo,
        "b.txt",
        "2\n",
        "a2",
        "Alice",
        "alice@acme.dev",
        now - 2 * DAY_SECS,
    );
    commit_at(
        &repo,
        "c.txt",
        "3\n",
        "a3",
        "Alice",
        "alice@acme.dev",
        now - 4 * DAY_SECS,
    );
    // Bob: 1 commit. Same name as another identity would merge only on the
    // exact (name, email) pair — here the pair is distinct.
    commit_at(
        &repo,
        "d.txt",
        "4\n",
        "b1",
        "Bob",
        "bob@other.io",
        now - DAY_SECS,
    );

    let out = ENGINE
        .contributor_stats_impl(&repo, 90)
        .expect("contributor_stats");
    assert_eq!(out.len(), 2, "{out:?}");

    // Count-descending order puts Alice first.
    let alice = &out[0];
    assert_eq!(alice.name, "Alice");
    assert_eq!(alice.email, "alice@acme.dev");
    assert_eq!(alice.count, 3);
    assert_eq!(alice.first_day, expected_day(now, 4 * DAY_SECS));
    assert_eq!(alice.last_day, expected_day(now, 0));

    let bob = &out[1];
    assert_eq!(bob.name, "Bob");
    assert_eq!(bob.email, "bob@other.io");
    assert_eq!(bob.count, 1);
    assert_eq!(bob.first_day, bob.last_day, "single-day range");
    assert_eq!(bob.first_day, expected_day(now, DAY_SECS));
}

#[test]
fn same_name_different_email_is_a_distinct_contributor() {
    let dir = TempDir::new("contributors-identity");
    let repo = init_repo(dir.path());
    let now = now_secs();

    commit_at(&repo, "a.txt", "1\n", "a1", "Alice", "alice@acme.dev", now);
    commit_at(
        &repo,
        "b.txt",
        "2\n",
        "a2",
        "Alice",
        "alice@personal.io",
        now,
    );

    let out = ENGINE
        .contributor_stats_impl(&repo, 90)
        .expect("contributor_stats");
    assert_eq!(out.len(), 2, "identity is the (name, email) pair: {out:?}");
    // Tie on count (1..=1 broken by name, then email — but both are "Alice",
    // so the email breaks the tie: acme < personal).
    assert_eq!(out[0].email, "alice@acme.dev");
    assert_eq!(out[1].email, "alice@personal.io");
    assert_eq!(out[0].count, 1);
    assert_eq!(out[1].count, 1);
}

// ---------------------------------------------------------------------------
// wire shape
// ---------------------------------------------------------------------------

#[test]
fn stats_payloads_serialize_with_documented_field_names() {
    let activity = super::stats::DayCount {
        day: "2026-10-07".to_string(),
        count: 7,
    };
    let json = serde_json::to_value(&activity).unwrap();
    assert_eq!(json["day"], "2026-10-07");
    assert_eq!(json["count"], 7);

    let contributor = super::stats::Contributor {
        name: "Alice".to_string(),
        email: "alice@acme.dev".to_string(),
        count: 3,
        first_day: "2026-10-01".to_string(),
        last_day: "2026-10-07".to_string(),
    };
    let json = serde_json::to_value(&contributor).unwrap();
    assert_eq!(json["name"], "Alice");
    assert_eq!(json["email"], "alice@acme.dev");
    assert_eq!(json["count"], 3);
    assert_eq!(json["first_day"], "2026-10-01");
    assert_eq!(json["last_day"], "2026-10-07");
}
