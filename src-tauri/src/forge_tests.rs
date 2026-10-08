//! Tests for the forge module (`forge.rs`): pure parsing (GitHub remote
//! URLs, gh JSON/table output, PR-URL extraction, failure classification,
//! version strings) plus real process-runner behavior (timeout kill on a
//! long-lived `ping`/`sleep`, spawn failure on a missing binary). Nothing
//! here touches the network; the one test that shells out to a real `gh`
//! is `#[ignore]`d like the CLI-integration tests elsewhere.

use std::time::{Duration, Instant};

use crate::forge::{
    extract_pull_url, forge_failure, parse_github_remote, parse_pr_checks_json,
    parse_pr_checks_table, parse_pr_list_json, pr_number_of, run_program, version_from_output,
    ForgeError, KIND_ALREADY_EXISTS, KIND_NOT_AUTHED, KIND_NO_GH, KIND_OTHER,
};

// ---------------------------------------------------------------------------
// Remote URL parsing
// ---------------------------------------------------------------------------

#[test]
fn github_remote_https_forms() {
    assert_eq!(
        parse_github_remote("https://github.com/owner/repo"),
        Some(("owner".into(), "repo".into()))
    );
    assert_eq!(
        parse_github_remote("https://github.com/owner/repo.git"),
        Some(("owner".into(), "repo".into()))
    );
    assert_eq!(
        parse_github_remote("https://github.com/owner/repo/"),
        Some(("owner".into(), "repo".into()))
    );
    // Case of the host does not matter; owner/repo keep their case.
    assert_eq!(
        parse_github_remote("https://WWW.GitHub.com/Owner/Repo.GIT"),
        Some(("Owner".into(), "Repo".into()))
    );
}

#[test]
fn github_remote_ssh_and_scp_forms() {
    assert_eq!(
        parse_github_remote("git@github.com:owner/repo.git"),
        Some(("owner".into(), "repo".into()))
    );
    assert_eq!(
        parse_github_remote("git@github.com:owner/repo"),
        Some(("owner".into(), "repo".into()))
    );
    assert_eq!(
        parse_github_remote("ssh://git@github.com/owner/repo.git"),
        Some(("owner".into(), "repo".into()))
    );
    assert_eq!(
        parse_github_remote("git://github.com/owner/repo.git"),
        Some(("owner".into(), "repo".into()))
    );
    assert_eq!(
        parse_github_remote("git+ssh://git@github.com:22/owner/repo.git"),
        Some(("owner".into(), "repo".into()))
    );
}

#[test]
fn github_remote_rejects_non_github_and_malformed() {
    assert_eq!(
        parse_github_remote("https://gitlab.com/owner/repo.git"),
        None
    );
    assert_eq!(parse_github_remote("git@example.com:owner/repo.git"), None);
    assert_eq!(parse_github_remote("https://github.com/owner"), None);
    assert_eq!(
        parse_github_remote("https://github.com/owner/repo/extra"),
        None
    );
    assert_eq!(parse_github_remote("owner/repo"), None);
    assert_eq!(parse_github_remote(""), None);
    assert_eq!(parse_github_remote("   "), None);
}

// ---------------------------------------------------------------------------
// gh output parsing (fixture strings, no network)
// ---------------------------------------------------------------------------

const PR_LIST_FIXTURE: &str = r#"[
  {
    "number": 12,
    "title": "Add forge panel",
    "headRefName": "m6-forge",
    "baseRefName": "main",
    "state": "OPEN",
    "isDraft": true,
    "url": "https://github.com/acme/widget/pull/12",
    "createdAt": "2026-10-01T10:00:00Z"
  },
  {
    "number": 11,
    "title": "Old one",
    "headRefName": "old",
    "baseRefName": "main",
    "state": "MERGED",
    "isDraft": false,
    "url": "https://github.com/acme/widget/pull/11"
  },
  { "title": "no number — skipped" }
]"#;

#[test]
fn pr_list_json_parses_fixture_tolerantly() {
    let prs = parse_pr_list_json(PR_LIST_FIXTURE);
    assert_eq!(prs.len(), 2, "entries without a number are skipped");
    assert_eq!(prs[0].number, 12);
    assert_eq!(prs[0].title, "Add forge panel");
    assert_eq!(prs[0].head_ref_name, "m6-forge");
    assert_eq!(prs[0].base_ref_name, "main");
    assert_eq!(prs[0].state, "OPEN");
    assert!(prs[0].is_draft);
    assert_eq!(prs[0].url, "https://github.com/acme/widget/pull/12");
    assert_eq!(prs[0].created_at.as_deref(), Some("2026-10-01T10:00:00Z"));
    assert_eq!(prs[1].state, "MERGED");
    assert!(!prs[1].is_draft);
    assert_eq!(
        prs[1].created_at, None,
        "missing createdAt tolerates to None"
    );
}

#[test]
fn pr_list_json_garbage_and_empty_yield_empty() {
    assert!(parse_pr_list_json("").is_empty());
    assert!(parse_pr_list_json("not json at all").is_empty());
    assert!(parse_pr_list_json("{}").is_empty());
    assert!(parse_pr_list_json("[]").is_empty());
}

const CHECKS_JSON_FIXTURE: &str = r#"[
  { "name": "build", "state": "SUCCESS", "bucket": "pass" },
  { "name": "lint", "state": "FAILURE", "bucket": "fail" },
  { "name": "deploy", "state": "PENDING", "bucket": "pending" },
  { "name": "docs", "state": "SKIPPING", "bucket": "skipping" },
  { "name": "legacy", "state": "SUCCESS" },
  { "name": "", "state": "SUCCESS", "bucket": "pass" }
]"#;

#[test]
fn pr_checks_json_parses_and_maps_states() {
    let checks = parse_pr_checks_json(CHECKS_JSON_FIXTURE);
    assert_eq!(checks.len(), 5, "nameless entries are skipped");
    assert_eq!(checks[0].state, "pass");
    assert_eq!(checks[1].state, "fail");
    assert_eq!(checks[2].state, "pending");
    assert_eq!(checks[3].state, "skipping");
    // No bucket → the state field maps.
    assert_eq!(checks[4].name, "legacy");
    assert_eq!(checks[4].state, "pass");
}

#[test]
fn pr_checks_json_garbage_yields_empty() {
    assert!(parse_pr_checks_json("").is_empty());
    assert!(parse_pr_checks_json("{\"nope\": 1}").is_empty());
}

#[test]
fn pr_checks_table_parses_word_and_icon_rows() {
    let table = "\
build\tci\t*\tfail\t3m\turl
lint\tci\tskipping
plain word row pending
✓ unit tests
✗ e2e
- changelog check
* waiting on review
";
    let checks = parse_pr_checks_table(table);
    let find = |name: &str| {
        checks
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.state.clone())
    };
    assert_eq!(find("build").as_deref(), Some("fail"));
    assert_eq!(find("lint").as_deref(), Some("skipping"));
    assert_eq!(find("plain").as_deref(), Some("pending"));
    assert_eq!(find("unit").as_deref(), Some("pass"));
    assert_eq!(find("e2e").as_deref(), Some("fail"));
    assert_eq!(find("changelog").as_deref(), Some("skipping"));
    assert_eq!(find("waiting").as_deref(), Some("pending"));
}

// ---------------------------------------------------------------------------
// PR URL extraction + numbers
// ---------------------------------------------------------------------------

#[test]
fn extract_pull_url_finds_urls_in_output() {
    assert_eq!(
        extract_pull_url(
            "Creating pull request for feat:\nhttps://github.com/acme/widget/pull/42\n"
        )
        .as_deref(),
        Some("https://github.com/acme/widget/pull/42")
    );
    // Sentence-punctuation after the URL must not break parsing.
    assert_eq!(
        extract_pull_url(
            "a pull request for branch \"feat\" already exists:\nhttps://github.com/acme/widget/pull/7."
        )
        .as_deref(),
        Some("https://github.com/acme/widget/pull/7")
    );
    // www. host forms normalize onto github.com.
    assert_eq!(
        extract_pull_url("see https://www.github.com/acme/widget/pull/3 for details").as_deref(),
        Some("https://github.com/acme/widget/pull/3")
    );
    assert_eq!(extract_pull_url("no pull url here"), None);
    assert_eq!(
        extract_pull_url("github.com/acme/widget/pull/ (no digits)"),
        None
    );
}

#[test]
fn pr_number_parses_from_url() {
    assert_eq!(pr_number_of("https://github.com/a/b/pull/123"), Some(123));
    assert_eq!(
        pr_number_of("https://github.com/a/b/pull/123#issuecomment-1"),
        Some(123)
    );
    assert_eq!(pr_number_of("https://github.com/a/b/pull/"), None);
    assert_eq!(pr_number_of("https://github.com/a/b/tree/main"), None);
}

// ---------------------------------------------------------------------------
// Failure classification
// ---------------------------------------------------------------------------

#[test]
fn forge_failure_kinds_classify() {
    // Spawn error → NoGh.
    let err = forge_failure(Some("gh: program not found"), "");
    assert_eq!(err.kind, KIND_NO_GH);
    assert!(err.message.contains("install it"), "got: {}", err.message);

    // Already exists keeps the existing URL when parseable.
    let err = forge_failure(
        None,
        "a pull request for branch \"feat\" already exists:\nhttps://github.com/acme/widget/pull/9",
    );
    assert_eq!(err.kind, KIND_ALREADY_EXISTS);
    assert_eq!(
        err.url.as_deref(),
        Some("https://github.com/acme/widget/pull/9")
    );
    // …and degrades to Other without "already exists" wording matching only
    // by accident is impossible here; plain other-error keeps url None.
    let err = forge_failure(None, "some other gh failure");
    assert_eq!(err.kind, KIND_OTHER);
    assert_eq!(err.url, None);

    // Auth markers → NotAuthed.
    for marker in [
        "To get started with GitHub CLI, please run: gh auth login",
        "gh: Not logged into github.com",
        "HTTP 401: Unauthorized",
        "remote: Bad credentials",
    ] {
        let err = forge_failure(None, marker);
        assert_eq!(err.kind, KIND_NOT_AUTHED, "marker: {marker}");
    }

    // Kind constants wire up to the documented strings.
    assert_eq!(KIND_NO_GH, "NoGh");
    assert_eq!(KIND_NOT_AUTHED, "NotAuthed");
    assert_eq!(KIND_ALREADY_EXISTS, "AlreadyExists");
    assert_eq!(KIND_OTHER, "Other");
    let other = ForgeError::other("boom");
    assert_eq!(other.kind, "Other");
    assert_eq!(other.to_string(), "Other: boom");
}

#[test]
fn forge_failure_message_is_bounded() {
    let wall = "x".repeat(10_000);
    let err = forge_failure(None, &wall);
    assert!(
        err.message.len() < 500,
        "message bounded: {}",
        err.message.len()
    );
    // Multibyte truncation must not panic (char-boundary safe).
    let unicode = "héllo wörld ".repeat(500);
    let err = forge_failure(None, &unicode);
    assert!(err.message.len() < 500);
}

// ---------------------------------------------------------------------------
// Version parsing
// ---------------------------------------------------------------------------

#[test]
fn version_from_output_parses_probe_line() {
    assert_eq!(
        version_from_output("gh version 2.63.3 (2024-12-11)\nhttps://github.com/cli/cli/releases"),
        "2.63.3"
    );
    assert_eq!(version_from_output("gh version 2.0.0"), "2.0.0");
    assert_eq!(version_from_output("garbage"), "");
    assert_eq!(version_from_output(""), "");
}

// ---------------------------------------------------------------------------
// Process runner (real child processes, no network)
// ---------------------------------------------------------------------------

/// A command that outlives the runner timeout (~30 s when not killed).
fn long_command() -> Vec<String> {
    if cfg!(windows) {
        vec!["-n".into(), "30".into(), "127.0.0.1".into()]
    } else {
        vec!["30".into()]
    }
}

fn long_program() -> &'static str {
    if cfg!(windows) {
        "ping"
    } else {
        "sleep"
    }
}

#[test]
fn run_program_kills_at_deadline() {
    let started = Instant::now();
    let outcome = run_program(
        long_program(),
        None,
        &long_command(),
        Duration::from_millis(1500),
    );
    let elapsed = started.elapsed();
    assert!(outcome.spawn_error.is_none(), "{:?}", outcome.spawn_error);
    assert!(outcome.timed_out, "the long child must hit the deadline");
    assert!(
        elapsed < Duration::from_secs(10),
        "kill must be prompt: {elapsed:?}"
    );
}

#[test]
fn run_program_reports_spawn_failure() {
    let outcome = run_program(
        "mygitui-definitely-not-a-binary-xyz",
        None,
        &[],
        Duration::from_secs(3),
    );
    assert!(
        outcome.spawn_error.is_some(),
        "missing binary → spawn_error"
    );
    assert!(!outcome.timed_out);
}

/// Real `gh` probe — env-dependent (needs gh installed), hence ignored;
/// run with `cargo test -- --ignored` on a provisioned machine.
#[test]
#[ignore = "shells out to real gh (PATH-dependent, may reach the network for auth status)"]
fn status_sync_against_real_gh() {
    let status = crate::forge::status_sync();
    if status.available {
        assert!(!status.version.is_empty(), "version parsed: {status:?}");
    }
}
