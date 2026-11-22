//! Tests for the forge module (`forge.rs`): pure parsing (GitHub remote
//! URLs, gh JSON/table output, PR-URL extraction, failure classification,
//! version strings) plus real process-runner behavior (timeout kill on a
//! long-lived `ping`/`sleep`, spawn failure on a missing binary). Nothing
//! here touches the network; the one test that shells out to a real `gh`
//! is `#[ignore]`d like the CLI-integration tests elsewhere.

use std::time::{Duration, Instant};

use crate::forge::{
    cap_log, extract_issue_url, extract_pull_url, forge_failure, issue_list_args,
    normalize_issue_state, normalize_pr_state, normalize_run_status, notification_kind,
    parse_github_remote, parse_issue_list_json, parse_issue_view_json, parse_notifications_json,
    parse_pr_checks_json, parse_pr_checks_table, parse_pr_list_json, parse_workflow_list_json,
    parse_workflow_run_detail_json, parse_workflow_runs_json, pick_pr_template, pr_edit_args,
    pr_list_args, pr_merge_args, pr_number_of, prefix_job_log, run_program, strip_log_bom,
    version_from_output, workflow_dispatch_args, workflow_runs_args, ForgeError, WorkflowInput,
    KIND_ALREADY_EXISTS, KIND_NOT_AUTHED, KIND_NO_GH, KIND_OTHER, MAX_LOG_CHARS, PR_TEMPLATE_PATHS,
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
fn pr_edit_args_build_title_and_body_flags() {
    // Title only.
    let title_only = pr_edit_args(7, Some("New title"), &[]).unwrap();
    assert_eq!(title_only, vec!["pr", "edit", "7", "--title", "New title"]);

    // Body only: the `--body-file` pair from [`body_file_args`] (the local
    // temp-file path travels as argv; a device run passes `-` + stdin).
    let body_only = pr_edit_args(
        7,
        None,
        &["--body-file".to_string(), "/tmp/b.md".to_string()],
    )
    .unwrap();
    assert_eq!(
        body_only,
        vec!["pr", "edit", "7", "--body-file", "/tmp/b.md"]
    );

    // Both, with surrounding whitespace trimmed off the title.
    let both = pr_edit_args(
        12,
        Some("  Both  "),
        &["--body-file".to_string(), "/tmp/b2.md".to_string()],
    )
    .unwrap();
    assert_eq!(
        both,
        vec![
            "pr",
            "edit",
            "12",
            "--title",
            "Both",
            "--body-file",
            "/tmp/b2.md"
        ]
    );

    // Blank title counts as absent.
    let blank = pr_edit_args(1, Some("   "), &[]).unwrap_err();
    assert_eq!(blank.kind, KIND_OTHER);
    assert!(blank.message.contains("nothing to edit"));
}

#[test]
fn pr_edit_sync_maps_gh_failures_onto_forgerror_kinds() {
    // pr_edit_sync wraps `run_gh_mutating`, so a failed gh run classifies
    // through `forge_failure` exactly like the other mutating commands —
    // pinned here at the classification seam (spawn failure → NoGh, auth
    // markers → NotAuthed, exit failures → Other).
    let err = forge_failure(Some("gh: program not found"), "");
    assert_eq!(err.kind, KIND_NO_GH);
    let err = forge_failure(None, "gh: Not logged into github.com");
    assert_eq!(err.kind, KIND_NOT_AUTHED);
    let err = forge_failure(None, "some pr edit failure");
    assert_eq!(err.kind, KIND_OTHER);
}

// ---------------------------------------------------------------------------
// PR template discovery
// ---------------------------------------------------------------------------

#[test]
fn pick_pr_template_prefers_the_directory_form_over_flat_files() {
    // Discovery order: dir form → flat `.github/PULL_REQUEST_TEMPLATE.md` →
    // lowercase flat variant. `read` returns Some for "present" paths.
    let present =
        |wanted: &str, rel: &str| -> Option<String> { (rel == wanted).then(|| rel.to_string()) };
    assert_eq!(
        pick_pr_template(|rel| present(".github/PULL_REQUEST_TEMPLATE.md", rel)),
        Some(".github/PULL_REQUEST_TEMPLATE.md".to_string())
    );
    assert_eq!(
        pick_pr_template(|rel| present(".github/pull_request_template.md", rel)),
        Some(".github/pull_request_template.md".to_string())
    );
    assert_eq!(
        pick_pr_template(|rel| present(PR_TEMPLATE_PATHS[0], rel)),
        Some(PR_TEMPLATE_PATHS[0].to_string())
    );
    assert_eq!(pick_pr_template(|_| None), None);
}

#[test]
fn pr_template_sync_reads_the_first_existing_file() {
    let dir = std::env::temp_dir().join(format!(
        "mygitui-forge-template-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let github = dir.join(".github");
    std::fs::create_dir_all(github.join("PULL_REQUEST_TEMPLATE")).unwrap();
    std::fs::write(
        github.join("PULL_REQUEST_TEMPLATE/pull_request_template.md"),
        "dir form",
    )
    .unwrap();

    // The two flat variants differ only in case. macOS (APFS) and Windows
    // (NTFS) default to case-insensitive filesystems, where writing both
    // would overwrite a single file — the "uppercase wins" assertion cannot
    // hold there. Probe with a throwaway pair and only exercise the
    // case-ordering when the filesystem keeps them distinct (cf. the
    // README's probe-and-skip convention for platform facilities).
    let case_sensitive = {
        let upper = github.join("mygitui-case-probe-ABC");
        let lower = github.join("mygitui-case-probe-abc");
        let _ = std::fs::remove_file(&upper);
        std::fs::write(&upper, b"probe").unwrap();
        let distinct = std::fs::read(&lower).is_err();
        let _ = std::fs::remove_file(&upper);
        let _ = std::fs::remove_file(&lower);
        distinct
    };
    if case_sensitive {
        std::fs::write(github.join("PULL_REQUEST_TEMPLATE.md"), "upper flat").unwrap();
    }
    std::fs::write(github.join("pull_request_template.md"), "lower flat").unwrap();
    assert_eq!(
        crate::forge::pr_template_sync(&dir).as_deref(),
        Some("dir form")
    );

    // Without the dir form, the uppercase flat file wins on case-sensitive
    // filesystems; elsewhere the lowercase file is the only one that exists.
    std::fs::remove_dir_all(github.join("PULL_REQUEST_TEMPLATE")).unwrap();
    assert_eq!(
        crate::forge::pr_template_sync(&dir).as_deref(),
        Some(if case_sensitive {
            "upper flat"
        } else {
            "lower flat"
        })
    );

    // Only the lowercase variant left.
    if case_sensitive {
        std::fs::remove_file(github.join("PULL_REQUEST_TEMPLATE.md")).unwrap();
    }
    assert_eq!(
        crate::forge::pr_template_sync(&dir).as_deref(),
        Some("lower flat")
    );

    // No template anywhere → None (missing repos count too).
    std::fs::remove_file(github.join("pull_request_template.md")).unwrap();
    assert_eq!(crate::forge::pr_template_sync(&dir), None);
    assert_eq!(crate::forge::pr_template_sync(&dir.join("nope")), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Issues (list/view parsing + args)
// ---------------------------------------------------------------------------

const ISSUE_LIST_FIXTURE: &str = r#"[
  {
    "number": 42,
    "title": "Panel flickers on open",
    "state": "open",
    "author": { "login": "octocat" },
    "updatedAt": "2026-11-01T12:00:00Z",
    "labels": [{ "name": "bug", "color": "ff0000" }, { "name": "ui" }]
  },
  {
    "number": 41,
    "title": "Old one",
    "state": "closed"
  },
  { "title": "no number — skipped" }
]"#;

#[test]
fn issue_list_json_parses_fixture_tolerantly() {
    let issues = parse_issue_list_json(ISSUE_LIST_FIXTURE);
    assert_eq!(issues.len(), 2, "entries without a number are skipped");
    assert_eq!(issues[0].number, 42);
    assert_eq!(issues[0].title, "Panel flickers on open");
    assert_eq!(issues[0].state, "open");
    assert_eq!(issues[0].author, "octocat");
    assert_eq!(
        issues[0].updated_at.as_deref(),
        Some("2026-11-01T12:00:00Z")
    );
    assert_eq!(issues[0].labels, vec!["bug", "ui"]);
    // Missing fields default instead of failing the entry.
    assert_eq!(issues[1].author, "");
    assert_eq!(issues[1].updated_at, None);
    assert_eq!(issues[1].labels, Vec::<String>::new());
    assert_eq!(issues[1].state, "closed");
}

#[test]
fn issue_list_json_garbage_and_empty_yield_empty() {
    assert!(parse_issue_list_json("").is_empty());
    assert!(parse_issue_list_json("not json at all").is_empty());
    assert!(parse_issue_list_json("{}").is_empty());
    assert!(parse_issue_list_json("[]").is_empty());
}

#[test]
fn issue_state_filter_and_list_args_are_normalized() {
    assert_eq!(normalize_issue_state("").unwrap(), "open");
    assert_eq!(normalize_issue_state(" OPEN ").unwrap(), "open");
    assert_eq!(normalize_issue_state("closed").unwrap(), "closed");
    assert_eq!(normalize_issue_state("all").unwrap(), "all");
    assert!(
        normalize_issue_state("merged").is_err(),
        "issues have no merged"
    );

    let args = issue_list_args("all").unwrap();
    assert_eq!(&args[..4], ["issue", "list", "--state", "all"]);
    assert!(args.iter().any(|arg| arg.contains("updatedAt")));
    let limit = args.iter().position(|arg| arg == "--limit").unwrap();
    assert_eq!(args[limit + 1], "50");
}

const ISSUE_VIEW_FIXTURE: &str = r#"{
  "number": 42,
  "title": "Panel flickers on open",
  "state": "open",
  "author": { "login": "octocat" },
  "body": "Steps to reproduce:\n1. open the panel\n2. watch it flicker",
  "labels": [{ "name": "bug" }],
  "url": "https://github.com/acme/widget/issues/42",
  "updatedAt": "2026-11-01T12:00:00Z",
  "comments": [
    {
      "author": { "login": "helga" },
      "body": "Reproduced on Linux.",
      "createdAt": "2026-11-02T09:00:00Z"
    },
    { "body": "Comment without an author." },
    "not an object — skipped"
  ]
}"#;

#[test]
fn issue_view_json_parses_fixture_tolerantly() {
    let detail = parse_issue_view_json(ISSUE_VIEW_FIXTURE).expect("valid detail");
    assert_eq!(detail.number, 42);
    assert_eq!(detail.title, "Panel flickers on open");
    assert_eq!(detail.state, "open");
    assert_eq!(detail.author, "octocat");
    assert!(detail.body.contains("Steps to reproduce"));
    assert_eq!(detail.labels, vec!["bug"]);
    assert_eq!(detail.url, "https://github.com/acme/widget/issues/42");
    assert_eq!(detail.updated_at.as_deref(), Some("2026-11-01T12:00:00Z"));
    assert_eq!(detail.comments.len(), 2, "non-object comments are skipped");
    assert_eq!(detail.comments[0].author, "helga");
    assert_eq!(detail.comments[0].body, "Reproduced on Linux.");
    assert_eq!(
        detail.comments[0].created_at.as_deref(),
        Some("2026-11-02T09:00:00Z")
    );
    assert_eq!(detail.comments[1].author, "", "missing author tolerates");
    assert_eq!(detail.comments[1].created_at, None);
}

#[test]
fn issue_view_json_missing_fields_and_garbage() {
    // Missing body/comments/labels tolerate to defaults.
    let detail = parse_issue_view_json(r#"{"number": 7}"#).expect("minimal object");
    assert_eq!(detail.number, 7);
    assert_eq!(detail.title, "");
    assert_eq!(detail.body, "");
    assert!(detail.comments.is_empty());
    assert!(detail.labels.is_empty());
    // Non-object payloads reject.
    assert!(parse_issue_view_json("").is_none());
    assert!(parse_issue_view_json("[]").is_none());
    assert!(parse_issue_view_json("garbage").is_none());
}

// ---------------------------------------------------------------------------
// Notifications (gh api output parsing + URL→kind derivation)
// ---------------------------------------------------------------------------

const NOTIFICATIONS_FIXTURE: &str = r#"[
  {
    "id": 1001,
    "unread": true,
    "reason": "review_requested",
    "updated_at": "2026-11-10T08:00:00Z",
    "subject": {
      "title": "Add issues panel",
      "url": "https://api.github.com/repos/acme/widget/pulls/55"
    },
    "repository": { "id": 1, "name": "widget", "full_name": "acme/widget" }
  },
  {
    "id": 1002,
    "reason": "mention",
    "subject": {
      "title": "Crash on empty diff",
      "url": "https://api.github.com/repos/acme/widget/issues/9"
    },
    "repository": { "name": "widget" }
  },
  {
    "id": 1003,
    "reason": "ci_activity",
    "subject": {
      "title": "A commit you follow broke CI",
      "url": "https://api.github.com/repos/acme/widget/commits/abc123"
    },
    "repository": { "name": "widget" }
  },
  {
    "id": 1004,
    "reason": "release_published",
    "subject": {
      "title": "v2.0",
      "url": "https://api.github.com/repos/acme/widget/releases/77"
    },
    "repository": { "name": "widget" }
  },
  { "reason": "no id — skipped" }
]"#;

#[test]
fn notifications_json_parses_and_derives_kinds() {
    let items = parse_notifications_json(NOTIFICATIONS_FIXTURE);
    assert_eq!(items.len(), 4, "entries without an id are skipped");
    assert_eq!(items[0].id, 1001);
    assert_eq!(items[0].kind, "pull");
    assert_eq!(items[0].subject_title, "Add issues panel");
    assert_eq!(
        items[0].subject_url,
        "https://api.github.com/repos/acme/widget/pulls/55"
    );
    assert_eq!(items[0].repository, "widget");
    assert_eq!(items[0].reason, "review_requested");
    assert_eq!(items[0].updated_at.as_deref(), Some("2026-11-10T08:00:00Z"));
    assert_eq!(items[1].kind, "issue");
    assert_eq!(items[1].updated_at, None);
    assert_eq!(items[2].kind, "commit");
    assert_eq!(items[3].kind, "other");
}

#[test]
fn notifications_json_garbage_and_empty_yield_empty() {
    assert!(parse_notifications_json("").is_empty());
    assert!(parse_notifications_json("not json").is_empty());
    assert!(parse_notifications_json("{}").is_empty());
    assert!(parse_notifications_json("[]").is_empty());
}

#[test]
fn notification_kind_derives_from_the_subject_url() {
    assert_eq!(
        notification_kind("https://api.github.com/repos/o/r/pulls/5"),
        "pull"
    );
    assert_eq!(notification_kind("https://github.com/o/r/pull/12"), "pull");
    assert_eq!(
        notification_kind("https://api.github.com/repos/o/r/issues/9"),
        "issue"
    );
    assert_eq!(
        notification_kind("https://github.com/o/r/issues/3"),
        "issue"
    );
    assert_eq!(
        notification_kind("https://api.github.com/repos/o/r/commits/abc123"),
        "commit"
    );
    assert_eq!(
        notification_kind("https://api.github.com/repos/o/r/releases/4"),
        "other"
    );
    assert_eq!(notification_kind(""), "other");
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

#[test]
fn extract_issue_url_and_issue_number_mirror_the_pull_helpers() {
    assert_eq!(
        extract_issue_url("Creating issue:\nhttps://github.com/acme/widget/issues/42\n").as_deref(),
        Some("https://github.com/acme/widget/issues/42")
    );
    // Pull URLs must not leak into the issue extractor (and vice versa).
    assert_eq!(
        extract_issue_url("https://github.com/acme/widget/pull/42"),
        None
    );
    assert_eq!(extract_issue_url("no issue url here"), None);
    assert_eq!(
        crate::forge::issue_number_of("https://github.com/a/b/issues/9"),
        Some(9)
    );
    assert_eq!(
        crate::forge::issue_number_of("https://github.com/a/b/issues/"),
        None
    );
    // An issue number must not parse out of a pull URL.
    assert_eq!(
        crate::forge::issue_number_of("https://github.com/a/b/pull/9"),
        None
    );
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

    // Device runs: gh missing on the remote shell is a 127 with "<shell>:
    // gh: command not found" on stderr — the SSH transport succeeded, so
    // there is no spawn_error; it must still classify as NoGh (an Other here
    // would leave the chips in their empty states with no guidance).
    let err = forge_failure(None, "bash: gh: command not found");
    assert_eq!(err.kind, KIND_NO_GH);
    assert!(
        err.message.contains("repository's machine"),
        "got: {}",
        err.message
    );
    // …while a non-gh "command not found" (cd/git failures) stays Other.
    let err = forge_failure(None, "git: command not found");
    assert_eq!(err.kind, KIND_OTHER);

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

// ---------------------------------------------------------------------------
// E1: authed hosts + remote resolution
// ---------------------------------------------------------------------------

#[test]
fn gh_auth_hosts_parses_status_output() {
    let output = "github.com\n  ✓ Logged in to github.com account octocat (keyring)\n  \
                  - Active account: true\n  - Git operations protocol: https\nghe.example.com\n  \
                  ✓ Logged in to ghe.example.com account work (keyring)\n";
    let hosts = crate::forge::gh_auth_hosts(output);
    assert!(hosts.contains(&"github.com".to_string()), "{hosts:?}");
    assert!(hosts.contains(&"ghe.example.com".to_string()), "{hosts:?}");
}

#[test]
fn gh_auth_hosts_dedupes_and_cleans_tokens() {
    let output = "  ✓ Logged in to GITHUB.COM account a (keyring)\n  \
                  ✓ Logged in to www.GitHub.COM account b (keyring)\n  \
                  ✓ Logged in to ghe.corp.:22 account c\nno accounts\n";
    let hosts = crate::forge::gh_auth_hosts(output);
    assert_eq!(
        hosts,
        vec!["github.com".to_string(), "ghe.corp".to_string()]
    );
}

#[test]
fn gh_auth_hosts_without_logins_is_empty() {
    assert!(crate::forge::gh_auth_hosts("").is_empty());
    assert!(crate::forge::gh_auth_hosts("no authenticated accounts\n").is_empty());
}

#[test]
fn github_remote_accepts_gh_authed_hosts() {
    use crate::forge::parse_github_remote_with_hosts;
    let hosts = vec!["ghe.example.com".to_string()];
    assert_eq!(
        parse_github_remote_with_hosts("git@ghe.example.com:acme/widget.git", &hosts),
        Some(("acme".into(), "widget".into()))
    );
    assert_eq!(
        parse_github_remote_with_hosts("https://GHE.example.com/acme/widget.git", &hosts),
        Some(("acme".into(), "widget".into()))
    );
    // github.com always matches, hosts or not.
    assert_eq!(
        parse_github_remote_with_hosts("https://github.com/a/b", &[]),
        Some(("a".into(), "b".into()))
    );
    // Unknown dev-machine hosts stay unresolvable (the muted "off" chip).
    assert_eq!(
        parse_github_remote_with_hosts("git@devbox.local:acme/widget.git", &hosts),
        None
    );
    assert_eq!(parse_github_remote_with_hosts("", &hosts), None);
}

/// The host cache: `github.com` always present; a non-empty refresh replaces;
/// an empty refresh (unauthed `gh auth status`) must not wipe learned hosts.
#[test]
fn authed_host_cache_replaces_only_on_new_hosts() {
    use crate::forge::{cache_authed_hosts, cached_hosts};
    cache_authed_hosts(vec![
        "ghe.example.com".to_string(),
        "github.com".to_string(),
    ]);
    let hosts = cached_hosts();
    assert_eq!(hosts[0], "github.com");
    assert!(hosts.contains(&"ghe.example.com".to_string()));
    // Empty report keeps the previous cache.
    cache_authed_hosts(Vec::new());
    assert!(cached_hosts().contains(&"ghe.example.com".to_string()));
    // Restore the default cache so other tests see github.com only.
    cache_authed_hosts(vec!["github.com".to_string()]);
}

// ---------------------------------------------------------------------------
// E1: snapshot shaping, hashing and part error priority
// ---------------------------------------------------------------------------

#[test]
fn snapshot_hashes_are_stable_and_part_sensitive() {
    use crate::forge::hash_serialized;
    let pr = pr_list_json_parses_fixture_tolerantly_pr();
    let first = hash_serialized(&pr);
    assert_eq!(first, hash_serialized(&pr), "same content → same hash");
    let mut other = pr.clone();
    other.title = "Different".to_string();
    assert_ne!(first, hash_serialized(&other), "content change → new hash");
    // FNV-1a is order-sensitive over the bytes.
    use crate::forge::fnv1a64;
    assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
    assert_ne!(fnv1a64(b"a"), fnv1a64(b"b"));
}

/// The M6 PR-list fixture as a `PrInfo` (shared by snapshot-shaping tests).
fn pr_list_json_parses_fixture_tolerantly_pr() -> crate::forge::PrInfo {
    parse_pr_list_json(PR_LIST_FIXTURE).remove(0)
}

#[test]
fn branch_pr_of_prefers_open_over_closed() {
    use crate::forge::branch_pr_of;
    let mut prs = parse_pr_list_json(PR_LIST_FIXTURE);
    // The fixture's first entry is OPEN with head "m6-forge".
    let open = branch_pr_of(&prs, "m6-forge");
    assert_eq!(open.map(|pr| pr.state), Some("OPEN".to_string()));
    // A merged PR wins when there is no open one for the branch.
    prs.truncate(2);
    prs.remove(0);
    let merged = branch_pr_of(&prs, "old");
    assert_eq!(merged.map(|pr| pr.state), Some("MERGED".to_string()));
    assert_eq!(branch_pr_of(&prs, "missing"), None);
    assert_eq!(branch_pr_of(&prs, ""), None);
}

#[test]
fn pr_list_json_fills_head_ref_oid() {
    let mut payload = String::from("[");
    payload.push_str(
        r#"{"number": 7, "headRefName": "e1", "headRefOid": "abc123def456", "state": "OPEN"}"#,
    );
    payload.push_str(r#", {"number": 8, "headRefName": "no-oid", "state": "OPEN"}]"#);
    let prs = parse_pr_list_json(&payload);
    assert_eq!(prs[0].head_ref_oid, "abc123def456");
    // Older gh omits headRefOid: "" (the FE treats it as unknown).
    assert_eq!(prs[1].head_ref_oid, "");
}

#[test]
fn commit_check_runs_json_parses_states() {
    use crate::forge::parse_commit_check_runs_json;
    let payload = r#"{"total_count": 3, "check_runs": [
        {"name": "build", "status": "completed", "conclusion": "success"},
        {"name": "test", "status": "completed", "conclusion": "failure"},
        {"name": "lint", "status": "in_progress", "conclusion": null},
        {"name": "skip-me", "status": "completed", "conclusion": "skipped"},
        {"name": "", "status": "completed", "conclusion": "success"}
    ]}"#;
    let checks = parse_commit_check_runs_json(payload);
    assert_eq!(checks.len(), 4, "nameless entries are skipped");
    assert_eq!(checks[0].state, "pass");
    assert_eq!(checks[1].state, "fail");
    assert_eq!(checks[2].state, "pending");
    assert_eq!(checks[3].state, "skipping");
    // Tolerant: a bare array also parses; garbage yields empty.
    assert_eq!(
        parse_commit_check_runs_json(
            r#"[{"name": "solo", "status": "completed", "conclusion": "success"}]"#
        )[0]
        .name,
        "solo"
    );
    assert!(parse_commit_check_runs_json("not json").is_empty());
}

#[test]
fn check_run_state_maps_rest_conclusions() {
    use crate::forge::check_run_state;
    assert_eq!(check_run_state("completed", "success"), "pass");
    assert_eq!(check_run_state("completed", "failure"), "fail");
    assert_eq!(check_run_state("completed", "timed_out"), "fail");
    assert_eq!(check_run_state("completed", "neutral"), "skipping");
    assert_eq!(check_run_state("in_progress", "null"), "pending");
    assert_eq!(check_run_state("queued", ""), "pending");
}

#[test]
fn pr_files_json_parses_tolerantly() {
    use crate::forge::parse_pr_files_json;
    let payload = r#"[
        {"filename": "src/app.ts", "status": "modified", "additions": 12, "deletions": 3, "patch": "@@ -1 +1 @@"},
        {"filename": "bin/asset.png", "status": "added", "additions": 0, "deletions": 0, "patch": null},
        {"no filename": true}
    ]"#;
    let files = parse_pr_files_json(payload);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "src/app.ts");
    assert_eq!(files[0].status, "modified");
    assert_eq!(files[0].additions, 12);
    assert_eq!(files[0].patch.as_deref(), Some("@@ -1 +1 @@"));
    assert!(files[1].patch.is_none());
    assert!(parse_pr_files_json("garbage").is_empty());
}

#[test]
fn review_comments_group_into_threads() {
    use crate::forge::{group_review_threads, parse_review_comments_json};
    let payload = r#"[
        {"id": 1, "path": "src/app.ts", "line": 12, "outdated": false,
         "user": {"login": "ada"}, "body": "Consider a map here.", "created_at": "2026-11-01T10:00:00Z"},
        {"id": 2, "in_reply_to_id": 1, "path": "src/app.ts", "line": 12,
         "user": {"login": "linus"}, "body": "Done.", "created_at": "2026-11-01T11:00:00Z"},
        {"id": 3, "in_reply_to_id": 99, "path": "src/other.rs", "line": 1,
         "user": {"login": "orphan"}, "body": "Parent paginated away."}
    ]"#;
    let threads = group_review_threads(parse_review_comments_json(payload));
    assert_eq!(threads.len(), 2, "1 real thread + 1 orphan-kept thread");
    assert_eq!(threads[0].id, 1);
    assert_eq!(threads[0].author, "ada");
    assert_eq!(threads[0].line, Some(12));
    assert!(!threads[0].outdated);
    assert_eq!(threads[0].replies.len(), 1);
    assert_eq!(threads[0].replies[0].author, "linus");
    assert_eq!(threads[1].id, 3, "orphan reply survives as its own thread");
    assert!(threads[1].replies.is_empty());
    assert!(parse_review_comments_json("garbage").is_empty());
}

#[test]
fn worst_part_error_picks_the_most_fundamental_failure() {
    use crate::forge::{
        worst_part_error, ForgeError, KIND_NOT_AUTHED, KIND_NO_GH, KIND_OFFLINE, KIND_OTHER,
        KIND_RATE_LIMITED,
    };
    assert_eq!(worst_part_error(Vec::new()), None);
    let gh_missing = ForgeError {
        kind: KIND_NO_GH.to_string(),
        message: "gh missing".to_string(),
        url: None,
    };
    let unauthed = ForgeError {
        kind: KIND_NOT_AUTHED.to_string(),
        message: "unauthed".to_string(),
        url: None,
    };
    let rate_limited = ForgeError {
        kind: KIND_RATE_LIMITED.to_string(),
        message: "rate limited".to_string(),
        url: None,
    };
    let offline = ForgeError {
        kind: KIND_OFFLINE.to_string(),
        message: "offline".to_string(),
        url: None,
    };
    let other = ForgeError {
        kind: KIND_OTHER.to_string(),
        message: "other".to_string(),
        url: None,
    };
    // NoGh beats everything; the order in the input list never matters.
    assert_eq!(
        worst_part_error(vec![
            other.clone(),
            offline,
            unauthed.clone(),
            gh_missing.clone()
        ])
        .map(|e| e.kind),
        Some(KIND_NO_GH.to_string())
    );
    assert_eq!(
        worst_part_error(vec![other.clone(), rate_limited.clone(), unauthed]).map(|e| e.kind),
        Some(KIND_NOT_AUTHED.to_string())
    );
    assert_eq!(
        worst_part_error(vec![other, rate_limited]).map(|e| e.kind),
        Some(KIND_RATE_LIMITED.to_string())
    );
}

#[test]
fn snapshot_gh_args_are_argv_only_and_slug_scoped() {
    use crate::forge::{
        commit_check_runs_args, snapshot_pr_args, snapshot_pr_list_args, snapshot_run_args,
    };
    let pr = snapshot_pr_args("acme/widget", "e1-snap");
    assert_eq!(
        pr,
        vec![
            "pr".to_string(),
            "list".to_string(),
            "--repo".to_string(),
            "acme/widget".to_string(),
            "--state".to_string(),
            "all".to_string(),
            "--head".to_string(),
            "e1-snap".to_string(),
            "--json".to_string(),
            crate::forge::PR_LIST_JSON_FIELDS.to_string(),
            "--limit".to_string(),
            "10".to_string(),
        ]
    );
    assert_eq!(
        snapshot_pr_list_args("acme/widget")[..4],
        vec!["pr", "list", "--repo", "acme/widget"]
    );
    let run = snapshot_run_args("acme/widget", "main");
    assert!(run.contains(&"--branch".to_string()));
    assert!(run.contains(&"--limit".to_string()));
    assert!(run.contains(&"1".to_string()));
    let checks = commit_check_runs_args("acme/widget", "abc123");
    assert_eq!(
        checks,
        vec![
            "api".to_string(),
            "repos/acme/widget/commits/abc123/check-runs".to_string()
        ]
    );
}

// ---------------------------------------------------------------------------
// E1: degraded-kind classification
// ---------------------------------------------------------------------------

#[test]
fn forge_failure_maps_rate_limit_and_offline() {
    use crate::forge::{forge_failure, KIND_OFFLINE, KIND_RATE_LIMITED};
    let rate_limited = forge_failure(
        None,
        "gh: HTTP 403: Forbidden (https://api.github.com/repos/a/b) \n API rate limit exceeded for 1.2.3.4",
    );
    assert_eq!(rate_limited.kind, KIND_RATE_LIMITED);
    let offline = forge_failure(
        None,
        "gh: Get \"https://api.github.com/notifications\": dial tcp: lookup api.github.com: could not resolve host",
    );
    assert_eq!(offline.kind, KIND_OFFLINE);
    // A plain 403 refusal still classifies by its markers (Other here).
    let refusal = forge_failure(None, "gh: Resource not accessible by integration");
    assert_eq!(refusal.kind, KIND_OTHER);
}

// ---------------------------------------------------------------------------
// Remote repos: gh and git run on the device over SSH
// ---------------------------------------------------------------------------

#[test]
fn remote_gh_command_quotes_root_and_args() {
    use crate::forge::remote_gh_command;
    let cmd = remote_gh_command(
        "/home/u/my repo",
        &[
            "pr".to_string(),
            "list".to_string(),
            "--state".to_string(),
            "it's".to_string(),
        ],
    );
    // The device shell parses one string: every argv element stays inside
    // its own single-quoted region (injection-inert, same as remote/quoting).
    assert_eq!(
        cmd,
        "cd '/home/u/my repo' && 'pr' 'list' '--state' 'it'\\''s'"
    );
}

#[test]
fn body_file_args_local_uses_temp_file() {
    use crate::forge::{body_file_args, ForgeRepo};
    let local = ForgeRepo::local(std::path::Path::new("/r"));
    let (args, stdin, temp) = body_file_args(&local, Some("body text")).unwrap();
    assert_eq!(args[0], "--body-file");
    assert!(stdin.is_none(), "local bodies travel as argv, not stdin");
    let path = temp.expect("local bodies write the temp file");
    assert_eq!(args[1], path.to_string_lossy());
    assert!(path.is_file());
    let _ = std::fs::remove_file(&path);

    // Blank/absent bodies produce no argument, no stdin, no file.
    let (args, stdin, temp) = body_file_args(&local, Some("   ")).unwrap();
    assert!(args.is_empty() && stdin.is_none() && temp.is_none());
}

#[test]
fn exec_outcome_maps_transport_failures_without_no_gh() {
    use crate::forge::exec_outcome;
    use crate::remote::transport::ExecOutput;

    // A normal (failing) remote run keeps its streams and exit code.
    let out = exec_outcome(Ok(ExecOutput {
        exit_code: 1,
        stdout: b"o".to_vec(),
        stderr: b"e".to_vec(),
    }));
    assert_eq!(out.code, Some(1));
    assert!(!out.timed_out);
    assert!(out.spawn_error.is_none());

    // A cancelled/timed-out exec maps onto the same timeout kill the local
    // runner reports.
    let out = exec_outcome(Err("remote command timed out".into()));
    assert!(out.timed_out);

    // Transport failures must NOT read as "gh missing" (that classification
    // rides spawn_error); they surface through stderr + a non-zero code.
    let out = exec_outcome(Err("channel open failed: disconnect".into()));
    assert!(out.spawn_error.is_none());
    assert!(out.code != Some(0));
    assert!(out.stderr.contains("device:"));
}
