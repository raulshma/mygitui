//! Tests for the forge module (`forge.rs`): pure parsing (GitHub remote
//! URLs, gh JSON/table output, PR-URL extraction, failure classification,
//! version strings) plus real process-runner behavior (timeout kill on a
//! long-lived `ping`/`sleep`, spawn failure on a missing binary). Nothing
//! here touches the network; the one test that shells out to a real `gh`
//! is `#[ignore]`d like the CLI-integration tests elsewhere.

use std::time::{Duration, Instant};

use crate::forge::{
    cap_log, extract_pull_url, forge_failure, normalize_pr_state, normalize_run_status,
    parse_github_remote, parse_pr_checks_json, parse_pr_checks_table, parse_pr_list_json,
    parse_workflow_list_json, parse_workflow_run_detail_json, parse_workflow_runs_json,
    pr_list_args, pr_merge_args, pr_number_of, prefix_job_log, run_program, strip_log_bom,
    version_from_output, workflow_dispatch_args, workflow_runs_args, ForgeError, WorkflowInput,
    KIND_ALREADY_EXISTS, KIND_NOT_AUTHED, KIND_NO_GH, KIND_OTHER, MAX_LOG_CHARS,
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
    "createdAt": "2026-10-01T10:00:00Z",
    "updatedAt": "2026-10-03T09:30:00Z",
    "author": { "login": "octocat" },
    "body": "Implements the forge panel.",
    "additions": 240,
    "deletions": 12,
    "changedFiles": 8,
    "reviewDecision": "APPROVED",
    "mergeable": "MERGEABLE",
    "labels": [{ "name": "feature", "color": "aabbcc" }, { "name": "ui" }]
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
    assert_eq!(prs[0].updated_at.as_deref(), Some("2026-10-03T09:30:00Z"));
    assert_eq!(prs[0].author, "octocat");
    assert_eq!(prs[0].body, "Implements the forge panel.");
    assert_eq!(prs[0].additions, 240);
    assert_eq!(prs[0].deletions, 12);
    assert_eq!(prs[0].changed_files, 8);
    assert_eq!(prs[0].review_decision, "APPROVED");
    assert_eq!(prs[0].mergeable, "MERGEABLE");
    assert_eq!(prs[0].labels, vec!["feature", "ui"]);
    assert_eq!(prs[1].state, "MERGED");
    assert!(!prs[1].is_draft);
    assert_eq!(
        prs[1].created_at, None,
        "missing createdAt tolerates to None"
    );
    assert_eq!(prs[1].updated_at, None);
    assert_eq!(prs[1].author, "");
    assert_eq!(prs[1].additions, 0);
    assert_eq!(prs[1].labels, Vec::<String>::new());
}

#[test]
fn pr_state_filter_and_list_args_are_normalized() {
    assert_eq!(normalize_pr_state("").unwrap(), "open");
    assert_eq!(normalize_pr_state("OPEN").unwrap(), "open");
    assert_eq!(normalize_pr_state(" closed ").unwrap(), "closed");
    assert_eq!(normalize_pr_state("merged").unwrap(), "merged");
    assert_eq!(normalize_pr_state("all").unwrap(), "all");
    assert!(normalize_pr_state("draft").is_err());

    let args = pr_list_args("all").unwrap();
    assert_eq!(&args[..4], ["pr", "list", "--state", "all"]);
    assert!(args.iter().any(|arg| arg.contains("reviewDecision")));
}

#[test]
fn pr_merge_args_map_methods_and_flags() {
    let plain = pr_merge_args(7, "squash", false, false).unwrap();
    assert_eq!(&plain[..4], ["pr", "merge", "7", "--squash"]);
    assert!(!plain.iter().any(|arg| arg == "--delete-branch"));

    let full = pr_merge_args(9, "rebase", true, true).unwrap();
    assert_eq!(
        full,
        vec!["pr", "merge", "9", "--rebase", "--delete-branch", "--auto"]
    );

    let err = pr_merge_args(1, "fast-forward", false, false).unwrap_err();
    assert_eq!(err.kind, KIND_OTHER);
    assert!(err.message.contains("unknown merge method"));
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
// GitHub Actions (run/workflow JSON parsing + args)
// ---------------------------------------------------------------------------

const RUN_LIST_FIXTURE: &str = r#"[
  {
    "attempt": 2,
    "conclusion": "success",
    "createdAt": "2026-10-20T02:00:43Z",
    "databaseId": 39356574672,
    "displayTitle": "chore(release): bump version",
    "event": "push",
    "headBranch": "main",
    "headSha": "4c536ee0d05e83d0fbf3bfedd33f4cd6f6667313",
    "name": "CI",
    "number": 27,
    "startedAt": "2026-10-20T02:15:26Z",
    "status": "completed",
    "updatedAt": "2026-10-20T02:19:39Z",
    "url": "https://github.com/acme/widget/actions/runs/39356574672",
    "workflowDatabaseId": 377963873,
    "workflowName": "CI"
  },
  {
    "attempt": 1,
    "conclusion": null,
    "databaseId": 39390637358,
    "displayTitle": "Nightly",
    "event": "schedule",
    "headBranch": "main",
    "headSha": "abc",
    "name": "",
    "number": 13,
    "status": "in_progress",
    "url": "https://github.com/acme/widget/actions/runs/39390637358",
    "workflowName": "Nightly"
  },
  { "displayTitle": "no databaseId — skipped" }
]"#;

#[test]
fn workflow_runs_json_parses_tolerantly() {
    let runs = parse_workflow_runs_json(RUN_LIST_FIXTURE);
    assert_eq!(runs.len(), 2, "entries without databaseId are skipped");
    assert_eq!(runs[0].database_id, 39356574672);
    assert_eq!(runs[0].number, 27);
    assert_eq!(runs[0].name, "CI");
    assert_eq!(runs[0].display_title, "chore(release): bump version");
    assert_eq!(runs[0].event, "push");
    assert_eq!(runs[0].head_branch, "main");
    assert_eq!(runs[0].status, "completed");
    assert_eq!(runs[0].conclusion, "success");
    assert_eq!(runs[0].attempt, 2);
    assert_eq!(runs[0].workflow_database_id, 377963873);
    assert_eq!(runs[0].started_at.as_deref(), Some("2026-10-20T02:15:26Z"));
    // Null conclusion + blank name fall back to the workflow name.
    assert_eq!(runs[1].conclusion, "");
    assert_eq!(runs[1].name, "Nightly");
    assert_eq!(runs[1].status, "in_progress");
    assert_eq!(runs[1].created_at, None);
    assert_eq!(runs[1].attempt, 1);
}

#[test]
fn workflow_runs_json_garbage_yields_empty() {
    assert!(parse_workflow_runs_json("").is_empty());
    assert!(parse_workflow_runs_json("{}").is_empty());
    assert!(parse_workflow_runs_json("not json").is_empty());
}

const RUN_VIEW_FIXTURE: &str = r#"{
  "attempt": 2,
  "conclusion": "failure",
  "databaseId": 101,
  "displayTitle": "fix the build",
  "event": "pull_request",
  "headBranch": "feat",
  "headSha": "abc",
  "name": "CI",
  "number": 7,
  "status": "completed",
  "url": "https://github.com/acme/widget/actions/runs/101",
  "workflowDatabaseId": 9,
  "jobs": [
    {
      "completedAt": "2026-10-20T02:10:00Z",
      "conclusion": "failure",
      "databaseId": 900,
      "name": "build",
      "startedAt": "2026-10-20T02:00:00Z",
      "status": "completed",
      "url": "https://github.com/acme/widget/actions/runs/101/job/900",
      "steps": [
        {
          "completedAt": "2026-10-20T02:01:00Z",
          "conclusion": "success",
          "name": "Set up job",
          "number": 1,
          "startedAt": "2026-10-20T02:00:00Z",
          "status": "completed"
        },
        {
          "conclusion": "failure",
          "name": "Tests",
          "number": 2,
          "status": "completed"
        }
      ]
    },
    { "name": "step-less / id-less job — skipped" }
  ]
}"#;

#[test]
fn workflow_run_detail_parses_jobs_and_steps() {
    let detail = parse_workflow_run_detail_json(RUN_VIEW_FIXTURE).expect("valid detail");
    assert_eq!(detail.run.database_id, 101);
    assert_eq!(detail.run.conclusion, "failure");
    assert_eq!(detail.jobs.len(), 1, "jobs without databaseId are skipped");
    let job = &detail.jobs[0];
    assert_eq!(job.database_id, 900);
    assert_eq!(job.name, "build");
    assert_eq!(job.conclusion, "failure");
    assert_eq!(job.steps.len(), 2);
    assert_eq!(job.steps[1].name, "Tests");
    assert_eq!(job.steps[1].number, 2);
    assert_eq!(job.steps[1].conclusion, "failure");
    assert_eq!(job.steps[1].started_at, None);
}

#[test]
fn workflow_run_detail_rejects_garbage() {
    assert!(parse_workflow_run_detail_json("").is_none());
    assert!(parse_workflow_run_detail_json("[]").is_none());
    assert!(parse_workflow_run_detail_json("{}").is_none());
    assert!(parse_workflow_run_detail_json(r#"{"databaseId": 5}"#).is_some());
}

#[test]
fn workflow_list_json_parses() {
    let list = parse_workflow_list_json(
        r#"[
          {"id": 1, "name": "CI", "path": ".github/workflows/ci.yml", "state": "active"},
          {"id": 2, "name": "Release", "path": ".github/workflows/release.yml", "state": "disabled_manually"},
          {"name": "no id — skipped"}
        ]"#,
    );
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, 1);
    assert_eq!(list[0].name, "CI");
    assert_eq!(list[0].path, ".github/workflows/ci.yml");
    assert_eq!(list[0].state, "active");
    assert_eq!(list[1].state, "disabled_manually");
    assert!(parse_workflow_list_json("{}").is_empty());
}

#[test]
fn run_status_and_list_args_normalize() {
    assert_eq!(normalize_run_status("").unwrap(), None);
    assert_eq!(normalize_run_status("  ").unwrap(), None);
    assert_eq!(normalize_run_status("FAILURE").unwrap(), Some("failure"));
    assert_eq!(
        normalize_run_status("in_progress").unwrap(),
        Some("in_progress")
    );
    assert!(normalize_run_status("exploded").is_err());

    let args = workflow_runs_args(500, " main ", "failure", " CI ").unwrap();
    assert_eq!(&args[..2], ["run", "list"]);
    assert!(args.iter().any(|arg| arg == "--json"));
    let limit = args.iter().position(|arg| arg == "--limit").unwrap();
    assert_eq!(args[limit + 1], "100", "limit is clamped");
    assert_eq!(
        &args[limit + 2..],
        [
            "--branch",
            "main",
            "--status",
            "failure",
            "--workflow",
            "CI"
        ]
    );

    let plain = workflow_runs_args(0, "", "", "").unwrap();
    let limit = plain.iter().position(|arg| arg == "--limit").unwrap();
    assert_eq!(plain[limit + 1], "1", "limit is clamped from below");
    assert!(!plain.iter().any(|arg| arg == "--branch"));
    assert!(!plain.iter().any(|arg| arg == "--status"));
    assert!(!plain.iter().any(|arg| arg == "--workflow"));
}

#[test]
fn workflow_dispatch_args_validate_and_format_inputs() {
    let inputs = vec![
        WorkflowInput {
            key: "version".into(),
            value: "1.2.3".into(),
        },
        WorkflowInput {
            key: "dry_run".into(),
            value: "true".into(),
        },
    ];
    let args = workflow_dispatch_args("release.yml", "main", &inputs).unwrap();
    assert_eq!(
        args,
        vec![
            "workflow",
            "run",
            "release.yml",
            "--ref",
            "main",
            "--raw-field",
            "version=1.2.3",
            "--raw-field",
            "dry_run=true",
        ]
    );
    // An empty ref lets gh use the repository default branch.
    let args = workflow_dispatch_args("CI", "", &[]).unwrap();
    assert_eq!(args, vec!["workflow", "run", "CI"]);

    let err = workflow_dispatch_args("  ", "main", &[]).unwrap_err();
    assert_eq!(err.kind, KIND_OTHER);
    assert!(err.message.contains("workflow"));
    let err = workflow_dispatch_args(
        "CI",
        "main",
        &[WorkflowInput {
            key: " ".into(),
            value: "x".into(),
        }],
    )
    .unwrap_err();
    assert!(err.message.contains("input names"));
}

#[test]
fn cap_log_bounds_large_output_without_panicking() {
    let small = "hello\n".to_string();
    assert_eq!(cap_log(small.clone()), small);
    let huge = "é".repeat(MAX_LOG_CHARS + 10);
    let capped = cap_log(huge);
    assert!(capped.chars().count() < MAX_LOG_CHARS + 100);
    assert!(capped.contains("log truncated"));
}

#[test]
fn job_log_fallback_strips_bom_and_prefixes_the_job_name() {
    // `gh api …/jobs/<id>/logs` starts with a BOM and has no job column.
    let payload = "\u{feff}2026-10-25T18:05:47.100Z hello\nsecond line\n";
    assert_eq!(
        strip_log_bom(payload),
        "2026-10-25T18:05:47.100Z hello\nsecond line\n"
    );
    assert_eq!(
        prefix_job_log("Frontend (test)", "first\nsecond\n"),
        "Frontend (test)\t\tfirst\nFrontend (test)\t\tsecond\n"
    );
    // Empty payloads stay empty instead of producing a bare job prefix.
    assert_eq!(prefix_job_log("job", ""), "");
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
