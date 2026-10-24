/**
 * Pure model for the M6 forge UI (no Svelte, no IPC — fully tested).
 *
 * Everything ForgePanel and CreatePrDialog render is computed here:
 * branch→PR matching, CI-check summarization (counts + worst state → chip
 * class/icon), PR age formatting, gh-status → banner mapping and PR URL
 * number extraction (for the already-exists flow).
 */

import type {
  CheckInfo,
  CheckState,
  ForgeStatus,
  PrInfo,
  WorkflowInfo,
  WorkflowInput,
  WorkflowRun,
} from "$lib/ipc/client";


/** The worst overall state of a check set (`none` = no checks at all). */
export type CheckWorst = CheckState | "none";

/** Counts + worst state of one PR's checks. */
export interface CheckSummary {
  pass: number;
  fail: number;
  pending: number;
  skipping: number;
  total: number;
  worst: CheckWorst;
}

/** Ordering used for `worst`: fail beats pending beats skipping beats pass. */
const WORST_RANK: Record<CheckWorst, number> = {
  none: -1,
  pass: 0,
  skipping: 1,
  pending: 2,
  fail: 3,
};

/**
 * Summarizes a check list: per-state counts and the overall worst state
 * (any fail → fail; else any pending → pending; else skipping; else pass;
 * empty → none). Pure.
 */
export function summarizeChecks(checks: CheckInfo[]): CheckSummary {
  const summary: CheckSummary = {
    pass: 0,
    fail: 0,
    pending: 0,
    skipping: 0,
    total: checks.length,
    worst: "none",
  };
  let worst: CheckWorst = "none";
  for (const check of checks) {
    summary[check.state] += 1;
    if (WORST_RANK[check.state] > WORST_RANK[worst]) worst = check.state;
  }
  summary.worst = worst;
  return summary;
}

/** One-line chip text for a summary: "3/5 checks", "checks pending", … */
export function checkSummaryLabel(summary: CheckSummary): string {
  if (summary.total === 0) return "no checks";
  if (summary.worst === "pass") return `${summary.total}/${summary.total} checks`;
  if (summary.worst === "fail") return `${summary.pass}/${summary.total} checks`;
  return `${summary.pass}/${summary.total} checks · ${summary.worst}`;
}

/** CSS class for a worst-state chip (panel stylesheet defines the colors). */
export function worstChipClass(worst: CheckWorst): string {
  return `chip-${worst}`;
}

/** Status icon for a single check (aria-hidden decoration next to the name). */
export function checkIcon(state: CheckState): string {
  switch (state) {
    case "pass":
      return "✓";
    case "fail":
      return "✗";
    case "skipping":
      return "–";
    case "pending":
      return "•";
  }
}

/** CSS class for a single check state (panel stylesheet defines colors). */
export function checkStateClass(state: CheckState): string {
  return `check-${state}`;
}

/**
 * The PR of `branch`: the first list entry whose `head_ref_name` matches,
 * preferring `OPEN` over MERGED/CLOSED (a merged PR of the same branch name
 * must not shadow a fresh open one). Pure.
 */
export function prForBranch(prs: PrInfo[], branch: string): PrInfo | null {
  if (!branch) return null;
  let open: PrInfo | null = null;
  let other: PrInfo | null = null;
  for (const pr of prs) {
    if (pr.head_ref_name !== branch) continue;
    if (pr.state === "OPEN") open ??= pr;
    other ??= pr;
  }
  return open ?? other;
}

/** True when a gh PR state means the PR can still receive checks. */
export function isPrOpen(state: string): boolean {
  return state === "OPEN";
}

/** Chip CSS class for a PR state (open/merged/closed). */
export function prStateClass(state: string): string {
  switch (state) {
    case "OPEN":
      return "chip-pr-open";
    case "MERGED":
      return "chip-pr-merged";
    default:
      return "chip-pr-closed";
  }
}

/** Why a PR cannot be merged from the panel (`null` = mergeable now). */
export function prMergeBlockedReason(pr: {
  state: string;
  is_draft: boolean;
  mergeable: string;
}): string | null {
  if (pr.state !== "OPEN") return "This pull request is not open";
  if (pr.is_draft) return "Mark the pull request ready before merging";
  if (pr.mergeable === "CONFLICTING") return "Resolve merge conflicts first";
  return null;
}

/** Human review-decision label (`""` when no decision is known). */
export function reviewDecisionLabel(decision: string): string {
  switch (decision) {
    case "APPROVED":
      return "Approved";
    case "CHANGES_REQUESTED":
      return "Changes requested";
    case "REVIEW_REQUIRED":
      return "Review required";
    default:
      return "";
  }
}

/** Display label for a PR state. */
export function prStateLabel(state: string): string {
  switch (state) {
    case "OPEN":
      return "Open";
    case "MERGED":
      return "Merged";
    case "CLOSED":
      return "Closed";
    default:
      return state || "Unknown";
  }
}

/**
 * Relative age of an ISO-8601 timestamp ("just now", "5m", "3h", "2d",
 * "5w", "8mo"); `null` when the timestamp is missing or unparsable. Pure
 * (`now` injectable).
 */
export function prAgeLabel(createdAt: string | null, now = Date.now()): string | null {
  if (!createdAt) return null;
  const created = Date.parse(createdAt);
  if (Number.isNaN(created)) return null;
  const minutes = Math.floor((now - created) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  const weeks = Math.floor(days / 7);
  if (weeks < 5) return `${weeks}w`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo`;
  return `${Math.floor(days / 365)}y`;
}

/** Kind of guidance the panel banner should show. */
export type BannerKind = "loading" | "missing" | "unauthed" | "ready";

/** Banner state derived from the (possibly not-yet-loaded) forge status. */
export interface Banner {
  kind: BannerKind;
  /** Primary banner text ("" when ready). */
  text: string;
}

/**
 * Maps `forge_status` onto the banner: loading → missing → unauthed →
 * ready. Pure.
 */
export function statusBanner(status: ForgeStatus | null): Banner {
  if (!status) return { kind: "loading", text: "Checking for the GitHub CLI (gh)…" };
  if (!status.available) {
    return {
      kind: "missing",
      text: "GitHub CLI (gh) not found on PATH — install it to work with pull requests.",
    };
  }
  if (!status.authed) {
    return {
      kind: "unauthed",
      text: `gh ${status.version} is installed but not authenticated.`,
    };
  }
  return { kind: "ready", text: "" };
}

/** Docs link for the install banner (opened via the opener plugin). */
export const GH_INSTALL_DOCS_URL = "https://cli.github.com/manual/installation";

/** The copyable login command shown in the unauthed banner. */
export const GH_AUTH_COMMAND = "gh auth login";

/**
 * Parses the PR number out of a `/pull/<digits>` URL (`null` otherwise).
 * Mirrors the backend's `pr_number_of` (used for the already-exists flow
 * where only the URL survives).
 */
export function prNumberFromUrl(url: string | null): number | null {
  if (!url) return null;
  const marker = url.indexOf("/pull/");
  if (marker === -1) return null;
  const after = url.slice(marker + "/pull/".length);
  const match = /^\d+/.exec(after);
  return match ? Number(match[0]) : null;
}

// ---------------------------------------------------------------------------
// GitHub Actions
// ---------------------------------------------------------------------------

/** Visual state bucket of a workflow run (drives chips/icons/colors). */
export type RunStateKind =
  | "success"
  | "failure"
  | "cancelled"
  | "skipped"
  | "running"
  | "queued"
  | "unknown";

/** Maps a run's status/conclusion pair onto a visual state bucket. Pure. */
export function runStateKind(
  run: Pick<WorkflowRun, "status" | "conclusion">,
): RunStateKind {
  if (run.status !== "completed") {
    switch (run.status) {
      case "queued":
      case "requested":
      case "waiting":
      case "pending":
        return "queued";
      case "":
        return "unknown";
      default:
        return "running";
    }
  }
  switch (run.conclusion) {
    case "success":
      return "success";
    case "failure":
    case "timed_out":
    case "action_required":
    case "startup_failure":
      return "failure";
    case "cancelled":
      return "cancelled";
    case "skipped":
    case "neutral":
    case "stale":
      return "skipped";
    case "":
      return "unknown";
    default:
      return "unknown";
  }
}

/** Icon for a run state bucket (decoration; labels carry the text). */
export function runStateIcon(kind: RunStateKind): string {
  switch (kind) {
    case "success":
      return "✓";
    case "failure":
      return "✗";
    case "cancelled":
      return "⊘";
    case "skipped":
      return "–";
    case "running":
      return "●";
    case "queued":
      return "◷";
    case "unknown":
      return "•";
  }
}

/** CSS class for a run state bucket (panel stylesheet defines the colors). */
export function runStateClass(kind: RunStateKind): string {
  return `run-${kind}`;
}

/** Human label for a run: "Success", "Running", "Queued", "Timed out", … */
export function runStateLabel(
  run: Pick<WorkflowRun, "status" | "conclusion">,
): string {
  if (run.status !== "completed") {
    switch (run.status) {
      case "queued":
        return "Queued";
      case "in_progress":
        return "Running";
      case "requested":
        return "Requested";
      case "waiting":
        return "Waiting";
      case "pending":
        return "Pending";
      case "":
        return "Unknown";
      default:
        return run.status;
    }
  }
  switch (run.conclusion) {
    case "success":
      return "Success";
    case "failure":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    case "skipped":
      return "Skipped";
    case "timed_out":
      return "Timed out";
    case "action_required":
      return "Action required";
    case "neutral":
      return "Neutral";
    case "stale":
      return "Stale";
    case "startup_failure":
      return "Startup failure";
    case "":
      return "Completed";
    default:
      return run.conclusion;
  }
}

/** True while the run is queued or running (cancel is meaningful). */
export function runIsActive(
  run: Pick<WorkflowRun, "status">,
): boolean {
  return run.status !== "completed" && run.status !== "";
}

/** True once a run finished (rerun is meaningful). */
export function runCanRerun(
  run: Pick<WorkflowRun, "status">,
): boolean {
  return run.status === "completed";
}

/** True when the run has failed jobs worth a failed-only rerun/log. */
export function runHasFailures(
  run: Pick<WorkflowRun, "conclusion">,
): boolean {
  return (
    run.conclusion === "failure" ||
    run.conclusion === "timed_out" ||
    run.conclusion === "startup_failure" ||
    run.conclusion === "action_required"
  );
}

/**
 * Compact duration between two ISO timestamps ("45s", "2m 10s", "3h 5m");
 * `null` when a timestamp is missing/unparsable. Pure.
 */
export function runDurationLabel(
  startedAt: string | null,
  finishedAt: string | null,
): string | null {
  if (!startedAt || !finishedAt) return null;
  const start = Date.parse(startedAt);
  const end = Date.parse(finishedAt);
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return null;
  const seconds = Math.round((end - start) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

/**
 * Live duration of a run: finished runs use `updated_at`, active runs tick
 * against `now`. Pure (`now` injectable).
 */
export function runElapsedLabel(
  run: Pick<WorkflowRun, "started_at" | "status" | "updated_at">,
  now = Date.now(),
): string | null {
  if (!run.started_at) return null;
  const finishedAt =
    run.status === "completed"
      ? run.updated_at
      : new Date(now).toISOString();
  return runDurationLabel(run.started_at, finishedAt);
}

/**
 * Live duration of a job/step: completed records use `completed_at`, active
 * ones tick against `now`. Pure (`now` injectable).
 */
export function activityDurationLabel(
  startedAt: string | null,
  completedAt: string | null,
  status: string,
  now = Date.now(),
): string | null {
  if (!startedAt) return null;
  const finishedAt =
    completedAt ?? (status === "completed" ? null : new Date(now).toISOString());
  return runDurationLabel(startedAt, finishedAt);
}

/** Friendly trigger-event label ("push" → "Push"). */
export function workflowEventLabel(event: string): string {
  switch (event) {
    case "push":
      return "Push";
    case "pull_request":
      return "Pull request";
    case "pull_request_target":
      return "Pull request target";
    case "workflow_dispatch":
      return "Manual";
    case "workflow_call":
      return "Reusable workflow";
    case "schedule":
      return "Schedule";
    case "release":
      return "Release";
    case "merge_group":
      return "Merge queue";
    case "issue_comment":
      return "Issue comment";
    case "pull_request_review":
      return "Pull request review";
    case "pull_request_review_comment":
      return "Review comment";
    case "repository_dispatch":
      return "Repository dispatch";
    case "workflow_run":
      return "Workflow run";
    default:
      return event || "Unknown";
  }
}

/** Label of one job/step conclusion ("Success", "Failed", "Skipped", …). */
export function stepStateLabel(conclusion: string, status = ""): string {
  if (conclusion === "failure") return "Failed";
  if (conclusion === "success") return "Success";
  if (conclusion === "cancelled") return "Cancelled";
  if (conclusion === "skipped") return "Skipped";
  if (conclusion === "timed_out") return "Timed out";
  if (conclusion === "action_required") return "Action required";
  if (conclusion) return conclusion;
  return status === "in_progress" ? "Running" : "Pending";
}

/** CSS class for a job/step conclusion (reuses the run state palette). */
export function stepStateClass(conclusion: string, status = ""): string {
  const kind = runStateKind({
    status: status || (conclusion ? "completed" : "in_progress"),
    conclusion,
  });
  return runStateClass(kind);
}

/** Display label for a workflow file state. */
export function workflowStateLabel(state: string): string {
  switch (state) {
    case "active":
      return "Active";
    case "disabled_manually":
      return "Disabled";
    case "disabled_inactivity":
      return "Disabled (inactive)";
    default:
      return state || "Unknown";
  }
}

/** Active (dispatchable) workflows of a list. */
export function activeWorkflows(workflows: WorkflowInfo[]): WorkflowInfo[] {
  return workflows.filter((workflow) => workflow.state === "active");
}

/** Parsed `key=value` dispatch inputs. */
export interface ParsedWorkflowInputs {
  inputs: WorkflowInput[];
  /** Human error (`null` when the text parsed cleanly). */
  error: string | null;
}

/**
 * Parses the dispatch dialog's input textarea: one `key=value` per line,
 * blank lines and `#` comments skipped. Duplicate keys and lines without
 * `=` are errors. Pure.
 */
export function parseWorkflowInputs(text: string): ParsedWorkflowInputs {
  const inputs: WorkflowInput[] = [];
  const seen = new Set<string>();
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].trim();
    if (!line || line.startsWith("#")) continue;
    const eq = line.indexOf("=");
    if (eq === -1) {
      return {
        inputs: [],
        error: `Line ${i + 1}: expected key=value`,
      };
    }
    const key = line.slice(0, eq).trim();
    const value = line.slice(eq + 1).trim();
    if (!key) {
      return { inputs: [], error: `Line ${i + 1}: input name is empty` };
    }
    if (seen.has(key)) {
      return { inputs: [], error: `Duplicate input "${key}"` };
    }
    seen.add(key);
    inputs.push({ key, value });
  }
  return { inputs, error: null };
}
