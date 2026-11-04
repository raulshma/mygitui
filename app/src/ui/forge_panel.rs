//! Forge panel: GitHub pull requests and CI through the `gh`-backed service
//! layer (`crate::service::forge`). Self-contained like [`super::terminal`]:
//! own [`Msg`]/[`Action`] enums, a pure [`update`] state machine and a
//! [`view`] that renders from [`ForgeState`] plus a small [`ForgeCtx`] input
//! the shell rebuilds each frame (current branch, HEAD sha, branch list).
//!
//! The panel never talks to services itself. Every effect is an [`Action`]
//! intent the shell maps onto `service::forge` calls, whose results come
//! back as [`Msg`] constructors (`StatusLoaded`, `PrsLoaded`, `ChecksLoaded`,
//! `RunsLoaded`, `PrCreated`, `ActionDone`). Mergeability caveats from
//! `PrInfo.mergeable` are surfaced as a warning line; the actual rejection
//! (`ForgeError`) arrives through `ActionDone`/`PrCreated`.
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod forge_panel;` in `ui/mod.rs` (drop the temporary
//!    `#![allow(dead_code)]` at the top of this file in the same commit).
//! 2. Add `Forge(forge_panel::Msg)` to `app::Message` and hold one
//!    `forge_panel::ForgeState` per tab (`tab.forge: ForgeState::default()`).
//! 3. Route messages: `Message::Forge(msg) => {
//!    let action = forge_panel::update(&mut tab.forge, msg);
//!    Self::run_forge_action(repo_id, &mut tab.forge, action) }`.
//! 4. Execute actions via `crate::service::forge`:
//!    - `Action::Refresh { pr_state }`: `forge_status` → `Msg::StatusLoaded`,
//!      `forge_context` → `Msg::ContextLoaded`, `pr_list(repo_id, pr_state)`
//!      → `Msg::PrsLoaded`. When `state.selected` is set, also fetch details
//!      like `Action::LoadPrDetails` so checks/runs stay fresh.
//!    - `Action::LoadPrDetails { number, head_branch }`: `pr_checks(number)`
//!      → `Msg::ChecksLoaded { number, .. }` and
//!      `workflow_runs(repo_id, None, Some(head_branch), None, None)` (pass
//!      `None` for the branch when `head_branch` is empty) → `Msg::RunsLoaded`.
//!    - `Action::OpenUrl(url)`: `open::that(url)` (the `open` crate) wrapped
//!      in `Task::perform` or fired fire-and-forget.
//!    - `Action::CheckoutPr { number }`: `pr_checkout` → `Msg::ActionDone`;
//!      on success also refresh the repo status (the branch changed).
//!    - `Action::MergePr { number, method, delete_branch }`:
//!      `pr_merge(repo_id, number, method.as_str(), delete_branch, false)`
//!      → `Msg::ActionDone { label: "Merge", .. }`.
//!    - `Action::ClosePr` / `Action::ReopenPr` / `Action::MarkReady`:
//!      `pr_close` / `pr_reopen` / `pr_ready` (comment `None`) →
//!      `Msg::ActionDone`.
//!    - `Action::CreatePr { base, head, title, body }`: `pr_create(repo_id,
//!      base, title, body, false)` → `Msg::PrCreated`. `head` defaults to
//!      the current branch (`gh pr create` always uses the checked-out
//!      branch as head); if `head` differs from the current branch the shell
//!      should refuse with a notice instead of silently ignoring it.
//! 5. Mirror the per-frame inputs the pure update half needs:
//!    `state.current_branch = ctx.branch` (also set by `Msg::ContextLoaded`)
//!    and `state.base_branches = ctx.base_branches.clone()` whenever the
//!    branch list loads — `update` has no `ForgeCtx`, it decides from state.
//! 6. View: `forge_panel::view(&tab.forge, tokens, &forge_ctx)
//!    .map(Message::Forge)` where `ForgeCtx` is built from the repo state
//!    (current branch, HEAD sha, all local + remote branch shorthands).
//!
//! Errors that are not [`ForgeError`]s (task/parse failures from
//! `pr_list`/`pr_checks`/`workflow_runs`, which return `String`) arrive via
//! the same `*Loaded` messages' `Err` arm and render in their section.

use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_editor, text_input,
    Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::forge::{
    CheckInfo, ForgeContext, ForgeError, ForgeStatus, PrCreated, PrInfo, WorkflowRun,
    KIND_ALREADY_EXISTS, KIND_NOT_AUTHED, KIND_NO_GH,
};

use super::repo_view::short_sha;
use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// View inputs (what the shell knows that the panel state does not)
// ---------------------------------------------------------------------------

/// Per-frame inputs for [`view`]: what the shell reads off the open repo.
/// Kept small on purpose — everything the *state machine* needs to decide
/// (`current_branch`, `base_branches`) is mirrored into [`ForgeState`],
/// because `update` cannot see the tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgeCtx {
    /// Current branch shorthand of the open repo (empty when detached).
    pub branch: String,
    /// HEAD commit sha of the open repo (empty when unknown).
    pub head_sha: String,
    /// Branch shorthands offered as create-form base candidates
    /// (local + remote, in whatever order the shell lists them).
    pub base_branches: Vec<String>,
}

// ---------------------------------------------------------------------------
// Row models (pure mapping from service payloads)
// ---------------------------------------------------------------------------

/// PR state as a closed set for chips/buttons, derived from
/// `PrInfo.state`/`PrInfo.is_draft`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
    #[default]
    Unknown,
}

impl PrState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

/// Map a `PrInfo` to its display state (`OPEN` + draft → [`PrState::Draft`]).
pub fn pr_state_of(pr: &PrInfo) -> PrState {
    match pr.state.to_ascii_uppercase().as_str() {
        "OPEN" if pr.is_draft => PrState::Draft,
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Unknown,
    }
}

/// Counts of one PR's checks by canonical bucket (`CheckInfo.state`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckSummary {
    pub pass: usize,
    pub fail: usize,
    pub pending: usize,
    pub skipping: usize,
}

impl CheckSummary {
    pub fn total(&self) -> usize {
        self.pass + self.fail + self.pending + self.skipping
    }

    /// Compact chip label: `"2✓ 1✗ 1…"`, or `"no checks"` when empty.
    pub fn label(&self) -> String {
        if self.total() == 0 {
            return "no checks".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if self.pass > 0 {
            parts.push(format!("{}✓", self.pass));
        }
        if self.fail > 0 {
            parts.push(format!("{}✗", self.fail));
        }
        if self.pending > 0 {
            parts.push(format!("{}…", self.pending));
        }
        if self.skipping > 0 {
            parts.push(format!("{}–", self.skipping));
        }
        parts.join(" ")
    }
}

/// Aggregate [`CheckInfo`]s by canonical state; unknown buckets are ignored
/// (the parser canonicalizes to pass/fail/pending/skipping).
pub fn check_summary(checks: &[CheckInfo]) -> CheckSummary {
    let mut summary = CheckSummary::default();
    for check in checks {
        match check.state.as_str() {
            "pass" => summary.pass += 1,
            "fail" => summary.fail += 1,
            "pending" => summary.pending += 1,
            "skipping" => summary.skipping += 1,
            _ => {}
        }
    }
    summary
}

/// One PR row of the list (a view-shaped [`PrInfo`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrRow {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head: String,
    pub base: String,
    pub state: PrState,
    pub url: String,
    /// `MERGEABLE` | `CONFLICTING` | `UNKNOWN` | `""` (verbatim `PrInfo`).
    pub mergeable: String,
    /// `APPROVED` | `CHANGES_REQUESTED` | `REVIEW_REQUIRED` | `""`.
    pub review_decision: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// CI summary once this PR's checks were loaded (selection only).
    pub checks: Option<CheckSummary>,
}

/// Map `PrInfo`s to row models, in list order.
pub fn pr_rows(prs: &[PrInfo]) -> Vec<PrRow> {
    prs.iter()
        .map(|pr| PrRow {
            number: pr.number,
            title: pr.title.clone(),
            author: pr.author.clone(),
            head: pr.head_ref_name.clone(),
            base: pr.base_ref_name.clone(),
            state: pr_state_of(pr),
            url: pr.url.clone(),
            mergeable: pr.mergeable.clone(),
            review_decision: pr.review_decision.clone(),
            additions: pr.additions,
            deletions: pr.deletions,
            changed_files: pr.changed_files,
            checks: None,
        })
        .collect()
}

/// Human label for a `PrInfo.review_decision` value.
pub fn review_label(review_decision: &str) -> String {
    match review_decision {
        "APPROVED" => "approved".to_string(),
        "CHANGES_REQUESTED" => "changes requested".to_string(),
        "REVIEW_REQUIRED" => "review required".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Which PR-side action is running (`busy.number` locks that PR's buttons).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// Target PR, `None` for non-PR work.
    pub number: Option<u64>,
    pub label: String,
}

/// Forge panel state: gh status, PR list, selection with checks/runs and
/// the create-PR form. Owned by the shell's tab; `current_branch` and
/// `base_branches` are mirrored in from [`ForgeCtx`] (see wiring docs).
#[derive(Debug, Clone, Default)]
pub struct ForgeState {
    pub open: bool,

    // Mirrored from ForgeCtx (see wiring docs): `update` cannot see the tab,
    // so the head-default/base-seed decisions read these instead.
    pub current_branch: String,
    pub base_branches: Vec<String>,

    // Header: gh availability + repo context.
    pub status: Option<ForgeStatus>,
    pub status_loading: bool,
    pub context: Option<ForgeContext>,
    pub context_error: Option<String>,

    // PR list.
    pub loading: bool,
    /// `None` = service default ("open"); `Some("all")` shows every PR.
    pub prs_filter: Option<String>,
    pub prs: Option<Vec<PrRow>>,
    pub prs_error: Option<String>,

    // Selected PR: checks + workflow runs.
    pub selected: Option<u64>,
    pub checks: Option<Vec<CheckInfo>>,
    pub checks_loading: bool,
    pub checks_error: Option<String>,
    pub runs: Option<Vec<WorkflowRun>>,
    pub runs_loading: bool,
    pub runs_error: Option<String>,

    // Create-PR form.
    pub create_open: bool,
    pub create_base: String,
    /// Head override; empty = `current_branch` (`resolve_head`).
    pub create_head: String,
    pub create_title: String,
    pub create_body: text_editor::Content,
    pub submitting: bool,
    pub create_error: Option<String>,

    // Comment/review form (selected PR; one shared body field).
    pub comment: text_editor::Content,
    pub comment_error: Option<String>,

    // Merge options + action feedback.
    pub merge_delete_branch: bool,
    pub busy: Option<Busy>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

impl ForgeState {
    /// URL of a loaded PR row, if any.
    pub fn pr_url(&self, number: u64) -> Option<String> {
        let url = &self
            .prs
            .as_ref()?
            .iter()
            .find(|row| row.number == number)?
            .url;
        (!url.is_empty()).then(|| url.clone())
    }

    /// Row model of a loaded PR.
    pub fn pr_row(&self, number: u64) -> Option<&PrRow> {
        self.prs.as_ref()?.iter().find(|row| row.number == number)
    }
}

// ---------------------------------------------------------------------------
// Messages + actions
// ---------------------------------------------------------------------------

/// Panel messages: user input, shell-delivered service results.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Show/hide the panel (first open triggers a refresh).
    ToggleOpen,
    /// The header ⟳ button: reload status, context and the PR list.
    RefreshPressed,
    /// Cycle the PR state filter (open → all → open) and reload.
    CyclePrFilter,

    /// `forge_status` result.
    StatusLoaded(ForgeStatus),
    /// `forge_context` result (Ok also sets `state.current_branch`).
    ContextLoaded(Result<ForgeContext, String>),

    /// `pr_list` result (rows are mapped through [`pr_rows`]).
    PrsLoaded {
        result: Result<Vec<PrInfo>, String>,
    },
    /// A PR row was clicked: loads its checks + workflow runs.
    SelectPr(u64),
    /// `pr_checks` result for `number` (stale responses are ignored).
    ChecksLoaded {
        number: u64,
        result: Result<Vec<CheckInfo>, String>,
    },
    /// `workflow_runs` result for the selected PR's head branch.
    RunsLoaded {
        result: Result<Vec<WorkflowRun>, String>,
    },

    // Per-PR actions (the shell runs the matching service call).
    OpenInBrowser(u64),
    CheckoutPressed(u64),
    /// Merge with an explicit method (one button per method).
    MergePressed {
        number: u64,
        method: MergeMethod,
    },
    ClosePressed(u64),
    ReopenPressed(u64),
    /// `ready = true` marks ready, `false` converts back to draft.
    ReadyPressed {
        number: u64,
        ready: bool,
    },

    // Comment/review form (selected PR).
    /// Keystrokes into the shared comment/review body field.
    CommentChanged(text_editor::Action),
    /// Post the body as a top-level PR comment (`pr_comment`).
    CommentPressed(u64),
    /// Submit a review (`pr_review`): approve, request-changes, or a
    /// comment-only review — all carrying the shared body field.
    ReviewPressed {
        number: u64,
        action: ReviewAction,
    },

    // Create-PR form.
    ToggleCreate,
    CreateBaseChanged(String),
    CreateHeadChanged(String),
    CreateTitleChanged(String),
    CreateBodyChanged(text_editor::Action),
    /// Replace the whole body (AI prefill) — `CreateBodyChanged` handles
    /// keystrokes, this sets programmatic content.
    CreateBodySet(String),
    CreatePressed,
    /// `pr_create` result.
    PrCreated(Result<PrCreated, ForgeError>),

    // Shared action feedback + options.
    MergeDeleteBranchToggled(bool),
    /// Result of any [`Action::CheckoutPr`]/`MergePr`/`ClosePr`/`ReopenPr`/
    /// `MarkReady` run by the shell (`ForgeError` renders via
    /// [`forge_error_text`]).
    ActionDone {
        number: Option<u64>,
        label: String,
        result: Result<String, ForgeError>,
    },
}

/// Merge strategies accepted by `service::forge::pr_merge` (`--merge`,
/// `--squash`, `--rebase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// Value passed straight to `pr_merge`'s `method` argument.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Squash => "Squash",
            Self::Rebase => "Rebase",
        }
    }
}

/// Review kinds accepted by `service::forge::pr_review` (`gh pr review`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    Approve,
    RequestChanges,
    /// A review whose only content is the written body (distinct from a
    /// top-level [`Action::CommentPr`], which needs no review verdict).
    Comment,
}

impl ReviewAction {
    /// Value passed straight to `pr_review`'s `action` argument.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request-changes",
            Self::Comment => "comment",
        }
    }

    /// Button / busy label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::RequestChanges => "Request changes",
            Self::Comment => "Comment-only review",
        }
    }
}

/// Intents the shell executes against the service layer (see wiring docs
/// for the exact service mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Reload status + context + PR list. `pr_state` goes to `pr_list`
    /// (`None` = the service default, "open").
    Refresh {
        pr_state: Option<String>,
    },
    /// Load one PR's checks and workflow runs (`head_branch` filters the
    /// runs; empty string means "no branch filter").
    LoadPrDetails {
        number: u64,
        head_branch: String,
    },
    /// Open a URL with the OS default handler.
    OpenUrl(String),
    CheckoutPr {
        number: u64,
    },
    MergePr {
        number: u64,
        method: MergeMethod,
        delete_branch: bool,
    },
    ClosePr {
        number: u64,
    },
    ReopenPr {
        number: u64,
    },
    MarkReady {
        number: u64,
        ready: bool,
    },
    CommentPr {
        number: u64,
        body: String,
    },
    ReviewPr {
        number: u64,
        action: ReviewAction,
        body: String,
    },
    CreatePr {
        base: String,
        head: String,
        title: String,
        body: String,
    },
}

// ---------------------------------------------------------------------------
// Pure decision helpers (update has no ForgeCtx, so it decides from state)
// ---------------------------------------------------------------------------

/// Whether the create form can be submitted: not already submitting, base
/// and title non-empty. Head may stay empty (it defaults to the current
/// branch via [`resolve_head`]).
pub fn can_submit(state: &ForgeState) -> bool {
    !state.submitting
        && !state.create_base.trim().is_empty()
        && !state.create_title.trim().is_empty()
}

/// Head branch for the create action: the typed override when set, else the
/// mirrored current branch (possibly empty — the shell then knows best).
pub fn resolve_head(state: &ForgeState) -> String {
    let typed = state.create_head.trim();
    if typed.is_empty() {
        state.current_branch.trim().to_string()
    } else {
        typed.to_string()
    }
}

/// Default base branch: "main" when present (or when nothing is known),
/// else the first listed branch.
pub fn default_base(branches: &[String]) -> String {
    if branches.is_empty() {
        return "main".to_string();
    }
    if branches.iter().any(|branch| branch == "main") {
        "main".to_string()
    } else {
        branches[0].clone()
    }
}

/// Base branch the create form displays: the picked one when set, else
/// [`default_base`] of the context's candidates.
pub fn resolve_base(state: &ForgeState, ctx: &ForgeCtx) -> String {
    let picked = state.create_base.trim();
    if picked.is_empty() {
        default_base(&ctx.base_branches)
    } else {
        picked.to_string()
    }
}

/// Whether another action on `number` is still in flight.
pub fn is_busy(state: &ForgeState, number: u64) -> bool {
    state
        .busy
        .as_ref()
        .is_some_and(|busy| busy.number == Some(number))
}

/// Inline review validation: request-changes (and a comment-only review)
/// carry a verdict gh cannot submit without written feedback; approving is
/// fine without text. `None` = the review may be submitted.
pub fn review_body_error(action: ReviewAction, body: &str) -> Option<&'static str> {
    let empty = body.trim().is_empty();
    match action {
        ReviewAction::RequestChanges if empty => {
            Some("Request changes needs a comment body — write what to fix.")
        }
        ReviewAction::Comment if empty => {
            Some("A comment-only review needs a body — use Comment for a plain PR comment.")
        }
        _ => None,
    }
}

/// The trimmed comment/review body as the service wants it.
pub fn comment_body(state: &ForgeState) -> String {
    state.comment.text().trim().to_string()
}

/// Guidance text for a structured [`ForgeError`] (install/auth hints for
/// the environment kinds, `kind: message` otherwise).
pub fn forge_error_text(err: &ForgeError) -> String {
    match err.kind.as_str() {
        KIND_NO_GH => "gh not found — install the GitHub CLI (https://cli.github.com)".to_string(),
        KIND_NOT_AUTHED => "gh is not authenticated — run `gh auth login`".to_string(),
        _ => format!("{}: {}", err.kind, err.message),
    }
}

impl ForgeState {
    fn lock(&mut self, number: u64, label: &str) {
        self.busy = Some(Busy {
            number: Some(number),
            label: label.to_string(),
        });
    }
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Pure state machine: apply `msg`, return the intent (if any) for the shell
/// to execute. See the module docs for the update-shape decision.
pub fn update(state: &mut ForgeState, msg: Msg) -> Option<Action> {
    match msg {
        Msg::ToggleOpen => {
            state.open = !state.open;
            // First open (or a panel that never loaded) kicks off a full load.
            if state.open && state.prs.is_none() && state.prs_error.is_none() && !state.loading {
                state.loading = true;
                state.status_loading = true;
                return Some(Action::Refresh {
                    pr_state: state.prs_filter.clone(),
                });
            }
            None
        }
        Msg::RefreshPressed => {
            state.loading = true;
            state.status_loading = true;
            state.prs_error = None;
            state.notice = None;
            Some(Action::Refresh {
                pr_state: state.prs_filter.clone(),
            })
        }
        Msg::CyclePrFilter => {
            state.prs_filter = match state.prs_filter {
                None => Some("all".to_string()),
                Some(_) => None,
            };
            state.loading = true;
            Some(Action::Refresh {
                pr_state: state.prs_filter.clone(),
            })
        }

        Msg::StatusLoaded(status) => {
            state.status = Some(status);
            state.status_loading = false;
            None
        }
        Msg::ContextLoaded(result) => {
            match result {
                Ok(context) => {
                    state.current_branch = context.branch.clone();
                    state.context = Some(context);
                    state.context_error = None;
                }
                Err(error) => state.context_error = Some(error),
            }
            None
        }

        Msg::PrsLoaded { result } => {
            state.loading = false;
            match result {
                Ok(prs) => {
                    // Carry over CI summaries from the previous list.
                    let previous = std::mem::take(&mut state.prs).unwrap_or_default();
                    let mut rows = pr_rows(&prs);
                    for row in &mut rows {
                        row.checks = previous
                            .iter()
                            .find(|prev| prev.number == row.number)
                            .and_then(|prev| prev.checks.clone());
                    }
                    if let Some(selected) = state.selected {
                        if !rows.iter().any(|row| row.number == selected) {
                            state.selected = None;
                            state.checks = None;
                            state.runs = None;
                        }
                    }
                    state.prs = Some(rows);
                    state.prs_error = None;
                }
                Err(error) => {
                    state.prs = None;
                    state.prs_error = Some(error);
                    state.selected = None;
                    state.checks = None;
                    state.runs = None;
                }
            }
            None
        }
        Msg::SelectPr(number) => {
            // Only rows that exist can be selected (stale clicks ignored).
            let head = state.pr_row(number)?.head.clone();
            state.selected = Some(number);
            state.checks = None;
            state.checks_error = None;
            state.checks_loading = true;
            state.runs = None;
            state.runs_error = None;
            state.runs_loading = true;
            Some(Action::LoadPrDetails {
                number,
                head_branch: head,
            })
        }
        Msg::ChecksLoaded { number, result } => {
            if state.selected == Some(number) {
                state.checks_loading = false;
                match result {
                    Ok(checks) => {
                        let summary = check_summary(&checks);
                        if let Some(row) = state
                            .prs
                            .as_mut()
                            .and_then(|rows| rows.iter_mut().find(|row| row.number == number))
                        {
                            row.checks = Some(summary);
                        }
                        state.checks = Some(checks);
                        state.checks_error = None;
                    }
                    Err(error) => {
                        state.checks = None;
                        state.checks_error = Some(error);
                    }
                }
            }
            None
        }
        Msg::RunsLoaded { result } => {
            state.runs_loading = false;
            match result {
                Ok(runs) => {
                    state.runs = Some(runs);
                    state.runs_error = None;
                }
                Err(error) => {
                    state.runs = None;
                    state.runs_error = Some(error);
                }
            }
            None
        }

        Msg::OpenInBrowser(number) => state.pr_url(number).map(Action::OpenUrl),
        Msg::CheckoutPressed(number) => {
            if is_busy(state, number) {
                return None;
            }
            state.lock(number, "Checkout");
            Some(Action::CheckoutPr { number })
        }
        Msg::MergePressed { number, method } => {
            if is_busy(state, number) {
                return None;
            }
            state.lock(number, "Merge");
            Some(Action::MergePr {
                number,
                method,
                delete_branch: state.merge_delete_branch,
            })
        }
        Msg::ClosePressed(number) => {
            if is_busy(state, number) {
                return None;
            }
            state.lock(number, "Close");
            Some(Action::ClosePr { number })
        }
        Msg::ReopenPressed(number) => {
            if is_busy(state, number) {
                return None;
            }
            state.lock(number, "Reopen");
            Some(Action::ReopenPr { number })
        }
        Msg::ReadyPressed { number, ready } => {
            if is_busy(state, number) {
                return None;
            }
            state.lock(number, if ready { "Mark ready" } else { "Draft" });
            Some(Action::MarkReady { number, ready })
        }

        Msg::CommentChanged(action) => {
            state.comment.perform(action);
            state.comment_error = None;
            None
        }
        Msg::CommentPressed(number) => {
            let body = comment_body(state);
            if body.is_empty() || is_busy(state, number) {
                return None;
            }
            state.comment_error = None;
            state.lock(number, "Comment");
            Some(Action::CommentPr { number, body })
        }
        Msg::ReviewPressed { number, action } => {
            if is_busy(state, number) {
                return None;
            }
            let body = comment_body(state);
            if let Some(error) = review_body_error(action, &body) {
                state.comment_error = Some(error.to_string());
                return None;
            }
            state.comment_error = None;
            state.lock(number, action.label());
            Some(Action::ReviewPr {
                number,
                action,
                body,
            })
        }

        Msg::ToggleCreate => {
            state.create_open = !state.create_open;
            state.create_error = None;
            if state.create_open && state.create_base.trim().is_empty() {
                state.create_base = default_base(&state.base_branches);
            }
            None
        }
        Msg::CreateBaseChanged(base) => {
            state.create_base = base;
            None
        }
        Msg::CreateHeadChanged(head) => {
            state.create_head = head;
            None
        }
        Msg::CreateTitleChanged(title) => {
            state.create_title = title;
            None
        }
        Msg::CreateBodySet(text) => {
            state.create_body = text_editor::Content::with_text(&text);
            None
        }
        Msg::CreateBodyChanged(action) => {
            state.create_body.perform(action);
            None
        }
        Msg::CreatePressed => {
            if !can_submit(state) {
                return None;
            }
            state.submitting = true;
            state.create_error = None;
            Some(Action::CreatePr {
                base: state.create_base.trim().to_string(),
                head: resolve_head(state),
                title: state.create_title.trim().to_string(),
                body: state.create_body.text(),
            })
        }
        Msg::PrCreated(result) => {
            state.submitting = false;
            match result {
                Ok(created) => {
                    state.create_open = false;
                    state.create_title.clear();
                    state.create_head.clear();
                    state.create_body = text_editor::Content::new();
                    state.notice =
                        Some(format!("PR #{} created — {}", created.number, created.url));
                    state.loading = true;
                    Some(Action::Refresh {
                        pr_state: state.prs_filter.clone(),
                    })
                }
                Err(err) => {
                    let mut text = forge_error_text(&err);
                    if err.kind == KIND_ALREADY_EXISTS {
                        if let Some(url) = &err.url {
                            text = format!("{text} ({url})");
                        }
                    }
                    state.create_error = Some(text);
                    None
                }
            }
        }

        Msg::MergeDeleteBranchToggled(delete) => {
            state.merge_delete_branch = delete;
            None
        }
        Msg::ActionDone {
            number: _,
            label,
            result,
        } => {
            state.busy = None;
            match result {
                Ok(output) => {
                    state.notice = Some(if output.trim().is_empty() {
                        format!("{label} ok")
                    } else {
                        clip(&format!("{label}: {output}"), 140)
                    });
                    // Merges/closes/reopens/ready/checkouts all change what
                    // the list shows — reload it.
                    state.loading = true;
                    Some(Action::Refresh {
                        pr_state: state.prs_filter.clone(),
                    })
                }
                Err(err) => {
                    state.error = Some(format!("{label} failed: {}", forge_error_text(&err)));
                    None
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Render the panel. `tokens` styles everything; `ctx` carries the repo's
/// current branch/HEAD/branch list (see [`ForgeCtx`]).
pub fn view<'a>(state: &'a ForgeState, tokens: Tokens, ctx: &'a ForgeCtx) -> Element<'a, Msg> {
    let mut body = column![].spacing(6).width(Length::Fill);
    body = body.push(header(state, tokens));
    body = body.push(status_line(state, tokens, ctx));

    if let Some(error) = &state.error {
        body = body.push(banner(error, tokens.deleted));
    }
    if let Some(notice) = &state.notice {
        body = body.push(banner(notice, tokens.added));
    }

    // gh known-broken → guidance only. Status still unknown → render
    // optimistically; failures surface through the section errors.
    let gh_blocked = state
        .status
        .as_ref()
        .is_some_and(|status| !status.available || !status.authed);
    if !gh_blocked {
        body = body.push(create_section(state, tokens, ctx));
        body = body.push(pr_list(state, tokens));
        if let Some(detail) = detail_section(state, tokens) {
            body = body.push(detail);
        }
    }

    container(body)
        .width(Length::Fill)
        .padding([8, 10])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.card)),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// Title row: gh badge, PR count, loading marker, filter/refresh/close.
fn header<'a>(state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let mut header = row![text("Forge").size(12).font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
    header = header.push(status_badge(state, tokens));
    if let Some(prs) = &state.prs {
        header = header.push(
            text(format!(
                "{} PR{}",
                prs.len(),
                if prs.len() == 1 { "" } else { "s" }
            ))
            .size(10)
            .color(tokens.muted_foreground),
        );
    }
    if state.loading || state.status_loading {
        header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
    }
    header = header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text(prs_filter_label(&state.prs_filter)).size(9))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::CyclePrFilter),
        )
        .push(
            button(text("⟳").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::RefreshPressed),
        )
        .push(
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::ToggleOpen),
        );
    header.into()
}

fn status_badge<'a>(state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let (label, color) = match &state.status {
        None if state.status_loading => ("gh …".to_string(), tokens.muted_foreground),
        None => ("gh ?".to_string(), tokens.muted_foreground),
        Some(status) if !status.available => ("gh missing".to_string(), tokens.deleted),
        Some(status) if !status.authed => ("gh unauthenticated".to_string(), tokens.modified),
        Some(status) => {
            let version = if status.version.is_empty() {
                "?".to_string()
            } else {
                status.version.clone()
            };
            (format!("gh {version}"), tokens.added)
        }
    };
    text(label).size(10).color(color).into()
}

/// gh status line: install/auth guidance, or owner/repo + branch + sha.
fn status_line<'a>(state: &'a ForgeState, tokens: Tokens, ctx: &'a ForgeCtx) -> Element<'a, Msg> {
    let mut line = column![].spacing(2).width(Length::Fill);

    if let Some(status) = &state.status {
        if !status.available {
            line = line.push(
                text("gh not found — install the GitHub CLI (https://cli.github.com) and run `gh auth login`.")
                    .size(10)
                    .color(tokens.deleted),
            );
        } else if !status.authed {
            line = line.push(
                text("gh is installed but not authenticated — run `gh auth login`.")
                    .size(10)
                    .color(tokens.modified),
            );
        }
    } else if state.status_loading {
        line = line.push(text("Checking gh…").size(10).color(tokens.muted_foreground));
    }

    if let Some(error) = &state.context_error {
        line = line.push(
            text(clip(&format!("not a GitHub remote: {error}"), 120))
                .size(9)
                .color(tokens.muted_foreground),
        );
    } else if let Some(context) = &state.context {
        let sha = if ctx.head_sha.is_empty() {
            String::new()
        } else {
            format!(" · {}", short_sha(&ctx.head_sha))
        };
        let branch = if ctx.branch.is_empty() {
            context.branch.as_str()
        } else {
            ctx.branch.as_str()
        };
        line = line.push(
            text(format!(
                "{}/{} · branch {branch}{sha}",
                context.owner, context.repo
            ))
            .size(9)
            .color(tokens.muted_foreground),
        );
    }

    line.into()
}

fn create_section<'a>(
    state: &'a ForgeState,
    tokens: Tokens,
    ctx: &'a ForgeCtx,
) -> Element<'a, Msg> {
    let mut section = column![].spacing(4).width(Length::Fill);

    let mut head = row![].spacing(6).align_y(Alignment::Center);
    head = head.push(
        button(
            text(if state.create_open {
                "× Discard draft PR"
            } else {
                "+ New pull request"
            })
            .size(10),
        )
        .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
        .padding([2, 8])
        .on_press(Msg::ToggleCreate),
    );
    if state.create_open {
        let resolved_head = resolve_head(state);
        let head_label = if resolved_head.is_empty() {
            "(current branch)".to_string()
        } else {
            resolved_head
        };
        head = head.push(
            text(format!("head {head_label} → base"))
                .size(9)
                .color(tokens.muted_foreground),
        );
    }
    section = section.push(head);

    if state.create_open {
        let mut picks = row![].spacing(6).align_y(Alignment::Center);
        picks = picks.push(text("base").size(9).color(tokens.muted_foreground));
        picks = picks.push(
            pick_list(
                ctx.base_branches.as_slice(),
                Some(resolve_base(state, ctx)),
                Msg::CreateBaseChanged,
            )
            .text_size(11)
            .padding([3, 6])
            .width(Length::Fixed(150.0)),
        );
        picks = picks.push(text("head").size(9).color(tokens.muted_foreground));
        picks = picks.push(
            text_input("defaults to current branch", state.create_head.as_str())
                .on_input(Msg::CreateHeadChanged)
                .size(11)
                .padding([4, 6])
                .width(Length::Fixed(150.0)),
        );
        section = section.push(picks);

        section = section.push(
            text_input("Title", state.create_title.as_str())
                .on_input(Msg::CreateTitleChanged)
                .on_submit(Msg::CreatePressed)
                .size(11)
                .padding([4, 6])
                .width(Length::Fill),
        );

        section = section.push(
            text_editor(&state.create_body)
                .placeholder("Description (optional)")
                .on_action(Msg::CreateBodyChanged)
                .size(11)
                .padding(6)
                .height(Length::Fixed(72.0)),
        );

        let enabled = can_submit(state);
        section = section.push(
            row![
                Space::new().width(Length::Fill),
                button(
                    text(if state.submitting {
                        "Creating…"
                    } else {
                        "Create pull request"
                    })
                    .size(10)
                    .color(tokens.primary_foreground)
                )
                .style(theme::button_style(
                    tokens,
                    if enabled {
                        theme::ButtonKind::Primary
                    } else {
                        theme::ButtonKind::Secondary
                    },
                ))
                .padding([4, 12])
                .on_press_maybe(enabled.then_some(Msg::CreatePressed)),
            ]
            .align_y(Alignment::Center),
        );

        if let Some(error) = &state.create_error {
            section = section.push(text(error).size(10).color(tokens.deleted));
        }
    }

    section.into()
}

fn pr_list<'a>(state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let list: Element<'a, Msg> = match &state.prs {
        Some(prs) if !prs.is_empty() => {
            let mut rows = column![].spacing(2).width(Length::Fill);
            for pr in prs {
                rows = rows.push(pr_row(pr, state.selected == Some(pr.number), tokens));
            }
            let height = (prs.len() as f32 * 36.0).clamp(36.0, 220.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => note(tokens, "No pull requests match this filter."),
        None if state.prs_error.is_some() => note(
            tokens,
            &clip(state.prs_error.as_deref().unwrap_or_default(), 160),
        ),
        None if state.loading => note(tokens, "Loading pull requests…"),
        None => note(tokens, "No pull requests loaded — press ⟳."),
    };
    list
}

fn pr_row<'a>(pr: &'a PrRow, selected: bool, tokens: Tokens) -> Element<'a, Msg> {
    let mut title_line = row![].spacing(6).align_y(Alignment::Center);
    title_line = title_line.push(
        text(format!("#{}", pr.number))
            .size(11)
            .font(theme::mono())
            .color(tokens.muted_foreground),
    );
    title_line = title_line.push(text(clip(&pr.title, 60)).size(11).color(tokens.foreground));
    title_line = title_line.push(state_chip(pr.state, tokens));
    if let Some(summary) = &pr.checks {
        title_line = title_line.push(ci_chip(summary, tokens));
    }

    let line = column![
        title_line,
        text(pr_meta(pr)).size(9).color(tokens.muted_foreground),
    ]
    .spacing(1)
    .width(Length::Fill);

    container(
        button(line)
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 4])
            .width(Length::Fill)
            .on_press(Msg::SelectPr(pr.number)),
    )
    .padding([1, 2])
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: selected.then_some(Background::Color(theme::with_alpha(tokens.active, 0.5))),
        border: Border::default(),
        ..container::Style::default()
    })
    .into()
}

fn pr_meta(pr: &PrRow) -> String {
    let author = if pr.author.is_empty() {
        "unknown author".to_string()
    } else {
        clip(&pr.author, 24)
    };
    format!(
        "{author} · {} → {} · +{} −{} · {} files",
        pr.head, pr.base, pr.additions, pr.deletions, pr.changed_files
    )
}

/// Actions + checks + workflow runs for the selected PR (`None` when nothing
/// usable is selected).
fn detail_section<'a>(state: &'a ForgeState, tokens: Tokens) -> Option<Element<'a, Msg>> {
    let pr = state.pr_row(state.selected?)?;

    let mut detail = column![].spacing(4).width(Length::Fill);

    let mut title = row![].spacing(6).align_y(Alignment::Center);
    title = title.push(
        text(format!("#{} {}", pr.number, clip(&pr.title, 60)))
            .size(11)
            .font(theme::semibold())
            .color(tokens.foreground),
    );
    title = title.push(state_chip(pr.state, tokens));
    if !pr.review_decision.is_empty() {
        title = title.push(
            text(review_label(&pr.review_decision))
                .size(9)
                .color(tokens.muted_foreground),
        );
    }
    detail = detail.push(title);
    detail = detail.push(text(pr_meta(pr)).size(9).color(tokens.muted_foreground));

    if pr.mergeable == "CONFLICTING" {
        detail = detail.push(
            text("head conflicts with base — merging fails until the conflicts are resolved")
                .size(9)
                .color(tokens.deleted),
        );
    }

    detail = detail.push(pr_actions(pr, state, tokens));
    detail = detail.push(comment_section(state, tokens));
    detail = detail.push(checks_section(pr, state, tokens));
    detail = detail.push(runs_section(state, tokens));

    Some(
        container(detail)
            .padding([6, 8])
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.08))),
                border: Border {
                    color: theme::with_alpha(tokens.border, 0.6),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            })
            .into(),
    )
}

fn pr_actions<'a>(pr: &'a PrRow, state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let busy = is_busy(state, pr.number);
    let terminal = matches!(pr.state, PrState::Merged | PrState::Closed);

    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    actions = actions.push(action_button(
        "Checkout",
        theme::ButtonKind::Secondary,
        !busy,
        Msg::CheckoutPressed(pr.number),
        tokens,
    ));
    actions = actions.push(action_button(
        "Open",
        theme::ButtonKind::Ghost,
        !busy && !pr.url.is_empty(),
        Msg::OpenInBrowser(pr.number),
        tokens,
    ));
    if !terminal {
        for method in [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase] {
            actions = actions.push(action_button(
                method.label(),
                theme::ButtonKind::Secondary,
                !busy,
                Msg::MergePressed {
                    number: pr.number,
                    method,
                },
                tokens,
            ));
        }
    }
    if !terminal {
        actions = actions.push(action_button(
            "Close",
            theme::ButtonKind::Ghost,
            !busy,
            Msg::ClosePressed(pr.number),
            tokens,
        ));
    }
    if pr.state == PrState::Closed {
        actions = actions.push(action_button(
            "Reopen",
            theme::ButtonKind::Ghost,
            !busy,
            Msg::ReopenPressed(pr.number),
            tokens,
        ));
    }
    if pr.state == PrState::Draft {
        actions = actions.push(action_button(
            "Mark ready",
            theme::ButtonKind::Ghost,
            !busy,
            Msg::ReadyPressed {
                number: pr.number,
                ready: true,
            },
            tokens,
        ));
    } else if pr.state == PrState::Open {
        actions = actions.push(action_button(
            "Convert to draft",
            theme::ButtonKind::Ghost,
            !busy,
            Msg::ReadyPressed {
                number: pr.number,
                ready: false,
            },
            tokens,
        ));
    }
    actions = actions.push(
        checkbox(state.merge_delete_branch)
            .label("delete branch")
            .size(12)
            .text_size(9)
            .spacing(4)
            .on_toggle(Msg::MergeDeleteBranchToggled),
    );
    if busy {
        actions = actions.push(
            text(format!(
                "{}…",
                state
                    .busy
                    .as_ref()
                    .map(|busy| busy.label.as_str())
                    .unwrap_or("working")
            ))
            .size(9)
            .color(tokens.muted_foreground),
        );
    }
    actions.into()
}

/// Comment/review form for the selected PR: one shared body field with a
/// top-level `Comment` button (pr_comment) and the three `pr_review`
/// verdicts. Request-changes/comment-only require a written body (validated
/// inline, see [`review_body_error`]).
fn comment_section<'a>(state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let Some(number) = state.selected else {
        return column![].into();
    };
    let busy = is_busy(state, number);
    let body = comment_body(state);
    let body_written = !body.is_empty();

    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    actions = actions.push(action_button(
        "Comment",
        theme::ButtonKind::Secondary,
        !busy && body_written,
        Msg::CommentPressed(number),
        tokens,
    ));
    for action in [
        ReviewAction::Approve,
        ReviewAction::RequestChanges,
        ReviewAction::Comment,
    ] {
        // Approve may go out without text; the other verdicts need a body.
        let enabled = !busy
            && match action {
                ReviewAction::Approve => true,
                _ => body_written,
            };
        actions = actions.push(action_button(
            action.label(),
            theme::ButtonKind::Ghost,
            enabled,
            Msg::ReviewPressed { number, action },
            tokens,
        ));
    }

    let mut section = column![].spacing(2).width(Length::Fill);
    section = section.push(
        text("Comment / review")
            .size(10)
            .color(tokens.muted_foreground),
    );
    section = section.push(
        text_editor(&state.comment)
            .placeholder("Comment body (request changes needs one)…")
            .on_action(Msg::CommentChanged)
            .size(11)
            .padding(6)
            .height(Length::Fixed(56.0)),
    );
    section = section.push(actions);
    if let Some(error) = &state.comment_error {
        section = section.push(text(error.clone()).size(10).color(tokens.deleted));
    }

    section.into()
}

fn checks_section<'a>(pr: &'a PrRow, state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = column![].spacing(2).width(Length::Fill);

    let mut head = row![].spacing(6).align_y(Alignment::Center);
    head = head.push(text("Checks").size(10).color(tokens.muted_foreground));
    if let Some(summary) = &pr.checks {
        head = head.push(text(summary.label()).size(9).color(tokens.muted_foreground));
    }
    section = section.push(head);

    let body: Element<'a, Msg> = match (&state.checks, state.checks_loading, &state.checks_error) {
        (Some(checks), _, _) if checks.is_empty() => {
            note(tokens, "No checks reported for this PR.")
        }
        (Some(checks), _, _) => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            for check in checks {
                rows = rows.push(check_row(check, tokens));
            }
            let height = (checks.len() as f32 * 16.0).clamp(16.0, 120.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        (None, true, _) => note(tokens, "Loading checks…"),
        (None, false, Some(error)) => note(tokens, &clip(error, 140)),
        (None, false, None) => note(tokens, "…"),
    };
    section = section.push(body);

    container(section)
        .padding([4, 6])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.08))),
            border: Border {
                color: theme::with_alpha(tokens.border, 0.6),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn check_row<'a>(check: &'a CheckInfo, tokens: Tokens) -> Element<'a, Msg> {
    let (glyph, color) = check_glyph(&check.state, tokens);
    row![
        text(glyph).size(10).color(color),
        text(check.name.as_str())
            .size(10)
            .font(theme::mono())
            .color(tokens.foreground),
        text(check.state.as_str())
            .size(9)
            .color(tokens.muted_foreground),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn runs_section<'a>(state: &'a ForgeState, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = column![].spacing(2).width(Length::Fill);
    section = section.push(
        text("Workflow runs")
            .size(10)
            .color(tokens.muted_foreground),
    );

    let body: Element<'a, Msg> = match (&state.runs, state.runs_loading, &state.runs_error) {
        (Some(runs), _, _) if runs.is_empty() => note(tokens, "No workflow runs for this branch."),
        (Some(runs), _, _) => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            for run in runs {
                rows = rows.push(run_row(run, tokens));
            }
            let height = (runs.len() as f32 * 28.0).clamp(28.0, 150.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        (None, true, _) => note(tokens, "Loading workflow runs…"),
        (None, false, Some(error)) => note(tokens, &clip(error, 140)),
        (None, false, None) => note(tokens, "…"),
    };
    section = section.push(body);

    container(section)
        .padding([4, 6])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.08))),
            border: Border {
                color: theme::with_alpha(tokens.border, 0.6),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn run_row<'a>(run: &'a WorkflowRun, tokens: Tokens) -> Element<'a, Msg> {
    let (glyph, color) = run_glyph(run, tokens);
    row![
        text(glyph).size(10).color(color),
        column![
            text(format!("#{} {}", run.number, clip(&run.name, 32)))
                .size(10)
                .font(theme::mono())
                .color(tokens.foreground),
            text(format!(
                "{} · {} · {}",
                run.head_branch,
                run.event,
                clip(&run.display_title, 48)
            ))
            .size(9)
            .color(tokens.muted_foreground),
        ]
        .spacing(0)
        .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

// ---------------------------------------------------------------------------
// View atoms
// ---------------------------------------------------------------------------

/// Tone bucket for status glyphs, mapped to tokens once here.
#[derive(Debug, Clone, Copy)]
enum Tone {
    Good,
    Bad,
    Wait,
    Muted,
}

fn tone_color(tone: Tone, tokens: Tokens) -> Color {
    match tone {
        Tone::Good => tokens.added,
        Tone::Bad => tokens.deleted,
        Tone::Wait => tokens.modified,
        Tone::Muted => tokens.muted_foreground,
    }
}

fn check_glyph(state: &str, tokens: Tokens) -> (&'static str, Color) {
    match state {
        "pass" => ("✓", tone_color(Tone::Good, tokens)),
        "fail" => ("✗", tone_color(Tone::Bad, tokens)),
        "pending" => ("…", tone_color(Tone::Wait, tokens)),
        "skipping" => ("–", tone_color(Tone::Muted, tokens)),
        _ => ("?", tone_color(Tone::Muted, tokens)),
    }
}

fn run_glyph(run: &WorkflowRun, tokens: Tokens) -> (&'static str, Color) {
    if run.status != "completed" {
        return match run.status.as_str() {
            "in_progress" => ("●", tone_color(Tone::Wait, tokens)),
            _ => ("○", tone_color(Tone::Muted, tokens)),
        };
    }
    match run.conclusion.as_str() {
        "success" => ("✓", tone_color(Tone::Good, tokens)),
        "failure" | "timed_out" | "startup_failure" | "action_required" => {
            ("✗", tone_color(Tone::Bad, tokens))
        }
        "cancelled" => ("⊘", tone_color(Tone::Muted, tokens)),
        "skipped" | "neutral" => ("–", tone_color(Tone::Muted, tokens)),
        _ => ("?", tone_color(Tone::Muted, tokens)),
    }
}

fn state_chip<'a>(state: PrState, tokens: Tokens) -> Element<'a, Msg> {
    let (fg, bg) = match state {
        PrState::Open => (tokens.added, tokens.added_bg()),
        PrState::Draft => (
            tokens.muted_foreground,
            theme::with_alpha(tokens.muted, 0.6),
        ),
        PrState::Merged => (tokens.renamed, tokens.renamed_bg()),
        PrState::Closed => (tokens.deleted, tokens.deleted_bg()),
        PrState::Unknown => (
            tokens.muted_foreground,
            theme::with_alpha(tokens.muted, 0.6),
        ),
    };
    chip(state.label(), fg, bg)
}

fn ci_chip<'a>(summary: &'a CheckSummary, tokens: Tokens) -> Element<'a, Msg> {
    let (fg, bg) = if summary.fail > 0 {
        (tokens.deleted, tokens.deleted_bg())
    } else if summary.pending > 0 {
        (tokens.modified, tokens.modified_bg())
    } else if summary.total() > 0 {
        (tokens.added, tokens.added_bg())
    } else {
        (
            tokens.muted_foreground,
            theme::with_alpha(tokens.muted, 0.6),
        )
    };
    chip(&summary.label(), fg, bg)
}

fn chip<'a>(label: &str, fg: Color, bg: Color) -> Element<'a, Msg> {
    container(text(label.to_string()).size(9).color(fg))
        .padding([1, 6])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: bg,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn action_button<'a>(
    label: &str,
    kind: theme::ButtonKind,
    enabled: bool,
    msg: Msg,
    tokens: Tokens,
) -> Element<'a, Msg> {
    button(text(label.to_string()).size(9))
        .style(theme::button_style(tokens, kind))
        .padding([2, 6])
        .on_press_maybe(enabled.then_some(msg))
        .into()
}

fn banner<'a>(label: &str, color: Color) -> Element<'a, Msg> {
    container(
        text(clip(label, 200))
            .size(10)
            .font(theme::semibold())
            .color(color),
    )
    .padding([3, 6])
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(theme::with_alpha(color, 0.10))),
        border: Border {
            color: theme::with_alpha(color, 0.35),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn note<'a>(tokens: Tokens, label: &str) -> Element<'a, Msg> {
    container(
        text(label.to_string())
            .size(10)
            .color(tokens.muted_foreground),
    )
    .padding([4, 2])
    .width(Length::Fill)
    .into()
}

fn prs_filter_label(filter: &Option<String>) -> String {
    match filter {
        None => "open".to_string(),
        Some(state) => state.clone(),
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// Tests (pure logic; fixtures mirror the gh JSON shapes the parsers in
// `forge.rs` produce — see `forge_tests.rs` for those)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A PrInfo shaped like `parse_pr_list_json` output for the repo
    /// `acme/widget` (same fixture values as `forge_tests.rs`).
    fn pr_fixture(number: u64, state: &str, draft: bool) -> PrInfo {
        PrInfo {
            number,
            title: format!("PR {number}"),
            head_ref_name: format!("pr-{number}"),
            base_ref_name: "main".into(),
            state: state.into(),
            is_draft: draft,
            url: format!("https://github.com/acme/widget/pull/{number}"),
            created_at: Some("2026-10-01T10:00:00Z".into()),
            updated_at: Some("2026-10-03T09:30:00Z".into()),
            merged_at: None,
            closed_at: None,
            author: "octocat".into(),
            body: "Implements the forge panel.".into(),
            additions: 240,
            deletions: 12,
            changed_files: 8,
            review_decision: "APPROVED".into(),
            mergeable: "MERGEABLE".into(),
            labels: vec!["feature".into(), "ui".into()],
        }
    }

    fn checks_fixture() -> Vec<CheckInfo> {
        vec![
            CheckInfo {
                name: "build".into(),
                state: "pass".into(),
            },
            CheckInfo {
                name: "lint".into(),
                state: "fail".into(),
            },
            CheckInfo {
                name: "deploy".into(),
                state: "pending".into(),
            },
            CheckInfo {
                name: "docs".into(),
                state: "skipping".into(),
            },
        ]
    }

    /// A default state with `fields` applied (keeps clippy's
    /// `field_reassign_with_default` away from the test setups).
    fn state_with(fields: impl FnOnce(&mut ForgeState)) -> ForgeState {
        let mut state = ForgeState::default();
        fields(&mut state);
        state
    }

    fn state_with_prs(prs: Vec<PrInfo>) -> ForgeState {
        let mut state = ForgeState::default();
        update(
            &mut state,
            Msg::PrsLoaded {
                result: Ok(prs.clone()),
            },
        );
        assert!(state.prs.is_some());
        assert_eq!(state.prs.as_ref().unwrap().len(), prs.len());
        state
    }

    // -- mapping: PrInfo → row models ---------------------------------------

    #[test]
    fn pr_state_of_maps_gh_states() {
        assert_eq!(pr_state_of(&pr_fixture(1, "OPEN", false)), PrState::Open);
        assert_eq!(pr_state_of(&pr_fixture(2, "OPEN", true)), PrState::Draft);
        assert_eq!(
            pr_state_of(&pr_fixture(3, "MERGED", false)),
            PrState::Merged
        );
        assert_eq!(
            pr_state_of(&pr_fixture(4, "CLOSED", false)),
            PrState::Closed
        );
        // Case-insensitive like gh's verbatim values; unknown → fallback.
        assert_eq!(pr_state_of(&pr_fixture(5, "open", false)), PrState::Open);
        assert_eq!(
            pr_state_of(&pr_fixture(6, "WEIRD", false)),
            PrState::Unknown
        );
    }

    #[test]
    fn pr_rows_map_fields_from_fixture() {
        let rows = pr_rows(&[
            pr_fixture(12, "OPEN", true),
            pr_fixture(11, "MERGED", false),
        ]);
        assert_eq!(rows.len(), 2);
        let row = &rows[0];
        assert_eq!(row.number, 12);
        assert_eq!(row.title, "PR 12");
        assert_eq!(row.author, "octocat");
        assert_eq!(row.head, "pr-12");
        assert_eq!(row.base, "main");
        assert_eq!(row.state, PrState::Draft);
        assert_eq!(row.url, "https://github.com/acme/widget/pull/12");
        assert_eq!(row.mergeable, "MERGEABLE");
        assert_eq!(row.review_decision, "APPROVED");
        assert_eq!(row.additions, 240);
        assert_eq!(row.deletions, 12);
        assert_eq!(row.changed_files, 8);
        assert_eq!(row.checks, None);
        assert_eq!(rows[1].state, PrState::Merged);
    }

    // -- check summary aggregation -------------------------------------------

    #[test]
    fn check_summary_counts_by_status() {
        let summary = check_summary(&checks_fixture());
        assert_eq!(summary.pass, 1);
        assert_eq!(summary.fail, 1);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.skipping, 1);
        assert_eq!(summary.total(), 4);
        assert_eq!(summary.label(), "1✓ 1✗ 1… 1–");
    }

    #[test]
    fn check_summary_empty_and_unknown_buckets() {
        assert_eq!(check_summary(&[]).label(), "no checks");
        let summary = check_summary(&[CheckInfo {
            name: "weird".into(),
            state: "something-else".into(),
        }]);
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.label(), "no checks");
        let pass_only = check_summary(&[
            CheckInfo {
                name: "a".into(),
                state: "pass".into(),
            },
            CheckInfo {
                name: "b".into(),
                state: "pass".into(),
            },
        ]);
        assert_eq!(pass_only.label(), "2✓");
    }

    // -- panel open/close + refresh -------------------------------------------

    #[test]
    fn toggle_open_requests_refresh_once() {
        let mut state = ForgeState::default();
        assert_eq!(
            update(&mut state, Msg::ToggleOpen),
            Some(Action::Refresh { pr_state: None })
        );
        assert!(state.open);
        assert!(state.loading);
        assert!(state.status_loading);

        // Closing never triggers loads.
        assert_eq!(update(&mut state, Msg::ToggleOpen), None);
        assert!(!state.open);

        // Reopening with a load already landing does not double-fire.
        state.prs = Some(Vec::new());
        assert_eq!(update(&mut state, Msg::ToggleOpen), None);

        // A failed load counts as "tried" too.
        state.prs = None;
        state.prs_error = Some("boom".into());
        assert_eq!(update(&mut state, Msg::ToggleOpen), None);
    }

    #[test]
    fn refresh_pressed_sets_loading_and_keeps_filter() {
        let mut state = state_with(|s| {
            s.prs_filter = Some("all".into());
            s.notice = Some("stale".into());
        });
        assert_eq!(
            update(&mut state, Msg::RefreshPressed),
            Some(Action::Refresh {
                pr_state: Some("all".into())
            })
        );
        assert!(state.loading && state.status_loading);
        assert_eq!(state.notice, None);
        assert_eq!(state.prs_filter, Some("all".into()));
    }

    #[test]
    fn cycle_filter_toggles_open_and_all() {
        let mut state = ForgeState::default();
        assert_eq!(
            update(&mut state, Msg::CyclePrFilter),
            Some(Action::Refresh {
                pr_state: Some("all".into())
            })
        );
        assert_eq!(state.prs_filter, Some("all".into()));
        assert_eq!(
            update(&mut state, Msg::CyclePrFilter),
            Some(Action::Refresh { pr_state: None })
        );
        assert_eq!(state.prs_filter, None);
        assert_eq!(prs_filter_label(&state.prs_filter), "open");
    }

    // -- PR list loading -------------------------------------------------------

    #[test]
    fn prs_loaded_maps_rows_and_carries_check_summaries() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        state.prs.as_mut().unwrap()[0].checks = Some(CheckSummary {
            pass: 2,
            ..CheckSummary::default()
        });

        update(
            &mut state,
            Msg::PrsLoaded {
                result: Ok(vec![
                    pr_fixture(12, "OPEN", false),
                    pr_fixture(13, "OPEN", true),
                ]),
            },
        );
        let rows = state.prs.as_ref().unwrap();
        assert_eq!(rows.len(), 2);
        // Same PR keeps its summary across reloads; the new one starts empty.
        assert_eq!(
            rows[0].checks,
            Some(CheckSummary {
                pass: 2,
                ..CheckSummary::default()
            })
        );
        assert_eq!(rows[1].checks, None);
        assert!(!state.loading);
        assert_eq!(state.prs_error, None);
    }

    #[test]
    fn prs_loaded_drops_selection_when_pr_vanishes() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert!(update(&mut state, Msg::SelectPr(12)).is_some());
        assert_eq!(state.selected, Some(12));

        update(
            &mut state,
            Msg::PrsLoaded {
                result: Ok(vec![pr_fixture(13, "OPEN", false)]),
            },
        );
        assert_eq!(state.selected, None);
    }

    #[test]
    fn prs_loaded_error_clears_list_and_selection() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert!(update(&mut state, Msg::SelectPr(12)).is_some());

        update(
            &mut state,
            Msg::PrsLoaded {
                result: Err("pr_list task failed: gh died".into()),
            },
        );
        assert_eq!(state.prs, None);
        assert_eq!(
            state.prs_error.as_deref(),
            Some("pr_list task failed: gh died")
        );
        assert_eq!(state.selected, None);
        assert!(!state.loading);
    }

    // -- selection + checks -----------------------------------------------------

    #[test]
    fn select_pr_loads_details_for_head_branch() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        let action = update(&mut state, Msg::SelectPr(12));
        assert_eq!(
            action,
            Some(Action::LoadPrDetails {
                number: 12,
                head_branch: "pr-12".into(),
            })
        );
        assert_eq!(state.selected, Some(12));
        assert!(state.checks_loading && state.runs_loading);
        assert_eq!(state.checks, None);
        assert_eq!(state.runs, None);
    }

    #[test]
    fn select_unknown_pr_is_ignored() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert_eq!(update(&mut state, Msg::SelectPr(999)), None);
        assert_eq!(state.selected, None);
    }

    #[test]
    fn checks_loaded_updates_row_summary() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert!(update(&mut state, Msg::SelectPr(12)).is_some());

        update(
            &mut state,
            Msg::ChecksLoaded {
                number: 12,
                result: Ok(checks_fixture()),
            },
        );
        assert!(!state.checks_loading);
        assert_eq!(state.checks_error, None);
        assert_eq!(state.checks.as_ref().unwrap().len(), 4);
        assert_eq!(
            state.pr_row(12).unwrap().checks,
            Some(CheckSummary {
                pass: 1,
                fail: 1,
                pending: 1,
                skipping: 1,
            })
        );
    }

    #[test]
    fn stale_checks_response_is_ignored() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert!(update(&mut state, Msg::SelectPr(12)).is_some());
        // The user re-selects another PR before the old response lands.
        update(
            &mut state,
            Msg::PrsLoaded {
                result: Ok(vec![
                    pr_fixture(12, "OPEN", false),
                    pr_fixture(13, "OPEN", false),
                ]),
            },
        );
        assert!(update(&mut state, Msg::SelectPr(13)).is_some());

        update(
            &mut state,
            Msg::ChecksLoaded {
                number: 12, // stale: no longer selected
                result: Err("boom".into()),
            },
        );
        assert!(state.checks_loading, "stale failure must not clear loading");
        assert_eq!(state.checks_error, None);
        assert_eq!(state.checks, None);
    }

    #[test]
    fn checks_error_is_surfaced() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert!(update(&mut state, Msg::SelectPr(12)).is_some());
        update(
            &mut state,
            Msg::ChecksLoaded {
                number: 12,
                result: Err("pr_checks task failed: timeout".into()),
            },
        );
        assert!(!state.checks_loading);
        assert_eq!(state.checks, None);
        assert_eq!(
            state.checks_error.as_deref(),
            Some("pr_checks task failed: timeout")
        );
    }

    #[test]
    fn runs_loaded_replaces_state() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert!(update(&mut state, Msg::SelectPr(12)).is_some());
        update(
            &mut state,
            Msg::RunsLoaded {
                result: Ok(vec![WorkflowRun {
                    database_id: 39356574672,
                    number: 27,
                    name: "CI".into(),
                    display_title: "chore(release): bump version".into(),
                    event: "push".into(),
                    head_branch: "pr-12".into(),
                    head_sha: "abcd1234".into(),
                    status: "completed".into(),
                    conclusion: "success".into(),
                    url: "https://github.com/acme/widget/actions/runs/39356574672".into(),
                    created_at: Some("2026-10-20T02:00:43Z".into()),
                    updated_at: None,
                    started_at: None,
                    attempt: 2,
                    workflow_database_id: 1,
                }]),
            },
        );
        assert_eq!(state.runs.as_ref().unwrap().len(), 1);
        assert_eq!(state.runs_error, None);

        update(
            &mut state,
            Msg::RunsLoaded {
                result: Err("workflow_runs task failed".into()),
            },
        );
        assert_eq!(state.runs, None);
        assert_eq!(
            state.runs_error.as_deref(),
            Some("workflow_runs task failed")
        );
    }

    // -- create form -------------------------------------------------------------

    #[test]
    fn create_validation_requires_base_and_title() {
        let mut state = ForgeState::default();
        assert!(!can_submit(&state));
        assert_eq!(update(&mut state, Msg::CreatePressed), None);

        state.create_base = "main".into();
        assert!(!can_submit(&state));
        assert_eq!(update(&mut state, Msg::CreatePressed), None);

        state.create_title = "Add forge panel".into();
        assert!(can_submit(&state));
    }

    #[test]
    fn create_submit_trims_fields_and_defaults_head() {
        let mut state = state_with(|s| {
            s.current_branch = "m6-forge".into();
            s.create_base = " main ".into();
            s.create_title = "  Add forge panel  ".into();
        });

        let action = update(&mut state, Msg::CreatePressed);
        assert_eq!(
            action,
            Some(Action::CreatePr {
                base: "main".into(),
                head: "m6-forge".into(),
                title: "Add forge panel".into(),
                body: String::new(),
            })
        );
        assert!(state.submitting);
        // Submitting again while in flight is rejected.
        assert_eq!(update(&mut state, Msg::CreatePressed), None);
    }

    #[test]
    fn create_head_override_wins_over_current_branch() {
        let mut state = state_with(|s| {
            s.current_branch = "current".into();
            s.create_base = "main".into();
            s.create_title = "t".into();
            s.create_head = " override ".into();
        });
        assert_eq!(resolve_head(&state), "override");

        state.create_head.clear();
        assert_eq!(resolve_head(&state), "current");
    }

    #[test]
    fn toggle_create_seeds_default_base() {
        let mut state = state_with(|s| s.base_branches = vec!["dev".into(), "main".into()]);
        update(&mut state, Msg::ToggleCreate);
        assert!(state.create_open);
        assert_eq!(state.create_base, "main");

        state.create_base.clear();
        state.create_open = false;
        state.base_branches = vec!["dev".into(), "next".into()];
        update(&mut state, Msg::ToggleCreate);
        assert_eq!(state.create_base, "dev");

        state.create_base.clear();
        state.create_open = false;
        state.base_branches.clear();
        update(&mut state, Msg::ToggleCreate);
        assert_eq!(
            state.create_base, "main",
            "nothing known → conventional main"
        );
        assert_eq!(default_base(&[]), "main");
    }

    #[test]
    fn resolve_base_prefers_picked_then_default() {
        let mut state = ForgeState::default();
        let ctx = ForgeCtx {
            branch: "feature".into(),
            head_sha: String::new(),
            base_branches: vec!["dev".into(), "main".into()],
        };
        assert_eq!(resolve_base(&state, &ctx), "main");
        state.create_base = "dev".into();
        assert_eq!(resolve_base(&state, &ctx), "dev");
    }

    #[test]
    fn pr_created_success_resets_form_and_refreshes() {
        let mut state = state_with(|s| {
            s.submitting = true;
            s.create_open = true;
            s.create_title = "Add forge panel".into();
            s.create_head = "m6-forge".into();
        });

        update(
            &mut state,
            Msg::PrCreated(Ok(PrCreated {
                number: 14,
                url: "https://github.com/acme/widget/pull/14".into(),
            })),
        );
        assert!(!state.submitting);
        assert!(!state.create_open);
        assert_eq!(state.create_title, "");
        assert_eq!(state.create_head, "");
        assert_eq!(
            state.notice.as_deref(),
            Some("PR #14 created — https://github.com/acme/widget/pull/14")
        );
        assert!(state.loading, "list reloads to include the new PR");
    }

    #[test]
    fn pr_created_already_exists_surfaces_url() {
        let mut state = state_with(|s| {
            s.submitting = true;
            s.create_open = true; // the form was open when the user submitted
        });
        update(
            &mut state,
            Msg::PrCreated(Err(ForgeError {
                kind: KIND_ALREADY_EXISTS.into(),
                message: "a pull request for branch already exists".into(),
                url: Some("https://github.com/acme/widget/pull/12".into()),
            })),
        );
        assert!(!state.submitting);
        assert_eq!(
            state.create_error.as_deref(),
            Some("AlreadyExists: a pull request for branch already exists (https://github.com/acme/widget/pull/12)")
        );
        assert!(state.create_open, "form stays open so the user can retry");
    }

    // -- per-PR actions ------------------------------------------------------------

    #[test]
    fn open_in_browser_maps_row_url() {
        let mut state = state_with_prs(vec![pr_fixture(12, "OPEN", false)]);
        assert_eq!(
            update(&mut state, Msg::OpenInBrowser(12)),
            Some(Action::OpenUrl(
                "https://github.com/acme/widget/pull/12".into()
            ))
        );
        assert_eq!(update(&mut state, Msg::OpenInBrowser(999)), None);
    }

    #[test]
    fn per_pr_actions_lock_and_map() {
        let mut state = state_with(|s| s.merge_delete_branch = true);

        assert_eq!(
            update(&mut state, Msg::CheckoutPressed(7)),
            Some(Action::CheckoutPr { number: 7 })
        );
        assert_eq!(state.busy.as_ref().unwrap().number, Some(7));

        // Same PR locked while busy…
        assert_eq!(
            update(
                &mut state,
                Msg::MergePressed {
                    number: 7,
                    method: MergeMethod::Squash
                }
            ),
            None
        );
        assert_eq!(update(&mut state, Msg::ClosePressed(7)), None);
        assert_eq!(update(&mut state, Msg::ReopenPressed(7)), None);
        assert_eq!(
            update(
                &mut state,
                Msg::ReadyPressed {
                    number: 7,
                    ready: true
                }
            ),
            None
        );

        // …but unlocked once the action reports back.
        update(
            &mut state,
            Msg::ActionDone {
                number: Some(7),
                label: "Checkout".into(),
                result: Ok("Switched to 'pr-7'".into()),
            },
        );
        assert_eq!(state.busy, None);
        assert_eq!(
            update(
                &mut state,
                Msg::MergePressed {
                    number: 7,
                    method: MergeMethod::Squash
                }
            ),
            Some(Action::MergePr {
                number: 7,
                method: MergeMethod::Squash,
                delete_branch: true,
            })
        );
    }

    #[test]
    fn action_done_success_sets_notice_and_refreshes() {
        let mut state = state_with(|s| {
            s.busy = Some(Busy {
                number: Some(7),
                label: "Merge".into(),
            })
        });
        assert_eq!(
            update(
                &mut state,
                Msg::ActionDone {
                    number: Some(7),
                    label: "Merge".into(),
                    result: Ok("Squashed and merged pull request #7".into()),
                },
            ),
            Some(Action::Refresh { pr_state: None })
        );
        assert_eq!(state.busy, None);
        assert_eq!(
            state.notice.as_deref(),
            Some("Merge: Squashed and merged pull request #7")
        );
        assert_eq!(state.error, None);
    }

    #[test]
    fn action_done_failure_sets_error_and_unlocks() {
        let mut state = state_with(|s| {
            s.busy = Some(Busy {
                number: Some(7),
                label: "Merge".into(),
            })
        });
        assert_eq!(
            update(
                &mut state,
                Msg::ActionDone {
                    number: Some(7),
                    label: "Merge".into(),
                    result: Err(ForgeError {
                        kind: KIND_NOT_AUTHED.into(),
                        message: "not logged in".into(),
                        url: None,
                    }),
                },
            ),
            None
        );
        assert_eq!(state.busy, None);
        assert_eq!(
            state.error.as_deref(),
            Some("Merge failed: gh is not authenticated — run `gh auth login`")
        );
    }

    // -- comment / review form ------------------------------------------------------

    #[test]
    fn review_body_error_requires_written_verdicts() {
        assert!(review_body_error(ReviewAction::Approve, "").is_none());
        assert!(review_body_error(ReviewAction::Approve, "  ").is_none());
        assert!(review_body_error(ReviewAction::RequestChanges, "fix the leak").is_none());
        assert!(review_body_error(ReviewAction::RequestChanges, "").is_some());
        assert!(review_body_error(ReviewAction::RequestChanges, "   ").is_some());
        assert!(review_body_error(ReviewAction::Comment, "").is_some());
        assert!(review_body_error(ReviewAction::Comment, "nit: typo").is_none());
    }

    #[test]
    fn review_action_strings_match_the_service() {
        assert_eq!(ReviewAction::Approve.as_str(), "approve");
        assert_eq!(ReviewAction::RequestChanges.as_str(), "request-changes");
        assert_eq!(ReviewAction::Comment.as_str(), "comment");
    }

    #[test]
    fn comment_press_maps_trimmed_body_and_locks() {
        let mut state = state_with(|s| {
            s.comment = text_editor::Content::with_text("  looks good to me  ");
        });
        assert_eq!(
            update(&mut state, Msg::CommentPressed(7)),
            Some(Action::CommentPr {
                number: 7,
                body: "looks good to me".into(),
            })
        );
        assert_eq!(state.busy.as_ref().unwrap().number, Some(7));

        // Empty body never submits.
        let mut state = ForgeState::default();
        assert_eq!(update(&mut state, Msg::CommentPressed(7)), None);
        // While busy, the press is ignored.
        let mut state = state_with(|s| {
            s.comment = text_editor::Content::with_text("hi");
            s.busy = Some(Busy {
                number: Some(7),
                label: "Merge".into(),
            });
        });
        assert_eq!(update(&mut state, Msg::CommentPressed(7)), None);
    }

    #[test]
    fn review_press_validates_inline_before_submitting() {
        let mut state = ForgeState::default();
        assert_eq!(
            update(
                &mut state,
                Msg::ReviewPressed {
                    number: 7,
                    action: ReviewAction::RequestChanges,
                }
            ),
            None,
            "request-changes without a body is rejected inline"
        );
        assert!(state.comment_error.is_some());
        assert_eq!(state.busy, None, "nothing locked on a rejected submit");

        state.comment = text_editor::Content::with_text(" the error path leaks the fd ");
        assert_eq!(
            update(
                &mut state,
                Msg::ReviewPressed {
                    number: 7,
                    action: ReviewAction::RequestChanges,
                }
            ),
            Some(Action::ReviewPr {
                number: 7,
                action: ReviewAction::RequestChanges,
                body: "the error path leaks the fd".into(),
            })
        );
        assert_eq!(state.comment_error, None, "a valid submit clears the error");
        assert_eq!(state.busy.as_ref().unwrap().label, "Request changes");
    }

    #[test]
    fn approve_may_go_out_without_a_body() {
        let mut state = ForgeState::default();
        assert_eq!(
            update(
                &mut state,
                Msg::ReviewPressed {
                    number: 7,
                    action: ReviewAction::Approve,
                }
            ),
            Some(Action::ReviewPr {
                number: 7,
                action: ReviewAction::Approve,
                body: String::new(),
            })
        );
    }

    #[test]
    fn comment_typing_clears_the_inline_error() {
        let mut state = state_with(|s| s.comment_error = Some("stale".into()));
        update(
            &mut state,
            Msg::CommentChanged(text_editor::Action::Edit(text_editor::Edit::Insert('x'))),
        );
        assert_eq!(state.comment_error, None);
    }

    // -- status / context ------------------------------------------------------------

    #[test]
    fn status_and_context_loaded() {
        let mut state = state_with(|s| s.status_loading = true);
        update(
            &mut state,
            Msg::StatusLoaded(ForgeStatus {
                available: true,
                version: "2.63.3".into(),
                authed: true,
            }),
        );
        assert!(!state.status_loading);
        assert_eq!(state.status.as_ref().unwrap().version, "2.63.3");

        update(
            &mut state,
            Msg::ContextLoaded(Ok(ForgeContext {
                owner: "acme".into(),
                repo: "widget".into(),
                branch: "m6-forge".into(),
                remote_url: "https://github.com/acme/widget".into(),
            })),
        );
        assert_eq!(state.context_error, None);
        assert_eq!(state.current_branch, "m6-forge");

        update(
            &mut state,
            Msg::ContextLoaded(Err("no origin remote".into())),
        );
        assert_eq!(state.context_error.as_deref(), Some("no origin remote"));
        assert_eq!(
            state.current_branch, "m6-forge",
            "kept from the last good context"
        );
    }

    #[test]
    fn forge_error_text_maps_guidance() {
        let guidance = |kind: &str| {
            forge_error_text(&ForgeError {
                kind: kind.into(),
                message: "whatever".into(),
                url: None,
            })
        };
        assert_eq!(
            guidance(KIND_NO_GH),
            "gh not found — install the GitHub CLI (https://cli.github.com)"
        );
        assert_eq!(
            guidance(KIND_NOT_AUTHED),
            "gh is not authenticated — run `gh auth login`"
        );
        assert_eq!(guidance("Other"), "Other: whatever");
    }

    // -- misc helpers -----------------------------------------------------------------

    #[test]
    fn merge_method_strings() {
        assert_eq!(MergeMethod::Merge.as_str(), "merge");
        assert_eq!(MergeMethod::Squash.as_str(), "squash");
        assert_eq!(MergeMethod::Rebase.as_str(), "rebase");
        assert_eq!(MergeMethod::default(), MergeMethod::Merge);
    }

    #[test]
    fn clip_truncates_long_text() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("0123456789", 5), "0123…");
    }

    #[test]
    fn review_label_maps_known_decisions() {
        assert_eq!(review_label("APPROVED"), "approved");
        assert_eq!(review_label("CHANGES_REQUESTED"), "changes requested");
        assert_eq!(review_label("REVIEW_REQUIRED"), "review required");
        assert_eq!(review_label(""), "");
    }
}
