/**
 * GitHub footer indication state machine (plan §9.2, Workstream E1).
 *
 * PURE derivation from snapshot data → chip states: no stores, no DOM, no
 * timers. Unit tests drive every state without mounting anything — the
 * {@link ./chips.svelte.ts} singleton feeds it (forge store snapshot data +
 * poll-scheduler lane state) and the StatusBar renders the result.
 *
 * Grammar (§9.2): icon + color + text, never color alone; the chip's
 * aria-label carries the full state text. States: loading, none, ok,
 * running (spinner + elapsed), warning, failing (failure count),
 * merged/closed, unpushed; degraded: NoGh, NotAuthed, non-GitHub remote,
 * offline, rate-limited ("retry in Xm"). Worst-state collapse (§9.2,
 * <1024px): failing > degraded > running > warning > unpushed > ok >
 * merged/closed > none > loading — a failure never hides.
 */

import type {
  CheckInfo,
  ForgeFailureKind,
  ForgeSnapshot,
  ForgeStatus,
  PrInfo,
  WorkflowRun,
} from "$lib/ipc/client";

/** Named degraded states a chip can render (§9.1). */
export type ForgeDegradedKind =
  | "no-gh"
  | "not-authed"
  | "not-repo"
  | "offline"
  | "rate-limited";

/** One chip's state: kind drives icon + color, text carries the facts. */
export type ForgeChipState =
  | { kind: "loading"; text: string }
  | { kind: "none"; text: string }
  | { kind: "ok"; text: string }
  | { kind: "running"; text: string; startedAtMs: number | null }
  | { kind: "warning"; text: string }
  | { kind: "failing"; text: string; count: number }
  | { kind: "merged"; text: string }
  | { kind: "closed"; text: string }
  | { kind: "unpushed"; text: string }
  | {
      kind: "degraded";
      text: string;
      degraded: ForgeDegradedKind;
      /** Human guidance (copyable commands ride in the popover). */
      message: string;
      /** Scheduler backoff for rate-limited retries (`null` = no estimate). */
      retryInMs: number | null;
    };

/** Shared derivation inputs (what every chip needs beyond its object). */
export interface ForgeChipInput {
  /** True while the snapshot fetch is in flight with nothing to show yet. */
  loading: boolean;
  /** Degraded state for the whole repo (`null` = the normal states apply). */
  degraded: ForgeDegradedKind | null;
  /** Degraded guidance message (verbatim reason). */
  degradedMessage: string;
  /** Rate-limited retry estimate (scheduler backoff), `null` when unknown. */
  retryInMs: number | null;
}

const LOADING: ForgeChipState = { kind: "loading", text: "…" };
const NONE: ForgeChipState = { kind: "none", text: "" };

/** Degraded-state chip shared by all three chips (the repo's reality). */
function degradedState(input: ForgeChipInput): ForgeChipState {
  return {
    kind: "degraded",
    text: degradedLabel(input.degraded ?? "not-repo"),
    degraded: input.degraded ?? "not-repo",
    message: input.degradedMessage,
    retryInMs: input.retryInMs,
  };
}

/** Short chip text of a degraded state (the full reason rides in the popover). */
export function degradedLabel(degraded: ForgeDegradedKind): string {
  switch (degraded) {
    case "no-gh":
      return "no gh";
    case "not-authed":
      return "not authed";
    case "not-repo":
      return "off";
    case "offline":
      return "offline";
    case "rate-limited":
      return "rate limited";
  }
}

/** Maps a backend `ForgeError` kind onto the degraded state (`null` = none —
 * an `Other` failure leaves the chip's normal states in charge, with the
 * message surfaced in the popover instead). */
export function degradedFromFailureKind(
  kind: ForgeFailureKind | null | undefined,
): ForgeDegradedKind | null {
  switch (kind) {
    case "NoGh":
      return "no-gh";
    case "NotAuthed":
      return "not-authed";
    case "NotRepo":
      return "not-repo";
    case "RateLimited":
      return "rate-limited";
    case "Offline":
      return "offline";
    default:
      return null;
  }
}

/**
 * Repo-level degraded state: the global `gh` status wins (nothing can work
 * without it), then the snapshot's failure kind (context/snapshot
 * resolution: NotRepo, 403, offline…), then the snapshot's own part error.
 * `null` = the normal chip states apply.
 */
export function repoDegraded(
  status: ForgeStatus | null,
  failureKind: ForgeFailureKind | null,
  snapshot: ForgeSnapshot | null,
): ForgeDegradedKind | null {
  if (status !== null) {
    if (!status.available) return "no-gh";
    if (!status.authed) return "not-authed";
  }
  return (
    degradedFromFailureKind(failureKind) ??
    degradedFromFailureKind(
      (snapshot?.error?.kind as ForgeFailureKind | undefined) ?? null,
    )
  );
}

/**
 * Guidance copy for a degraded state: what happened + the fix. The popover
 * renders it with copyable commands.
 */
export function degradedGuidance(
  degraded: ForgeDegradedKind,
  detail: string,
): { title: string; message: string; command: string | null; docsUrl: string | null } {
  switch (degraded) {
    case "no-gh":
      return {
        title: "GitHub CLI (gh) not found",
        message:
          "mygitui talks to GitHub through the gh CLI. Install it, then sign in once — no token is stored in the app.",
        command: "gh auth login",
        docsUrl: "https://cli.github.com/manual/installation",
      };
    case "not-authed":
      return {
        title: "gh is installed but not authenticated",
        message:
          "Sign in from a terminal; mygitui reuses gh's own authentication and never stores a token.",
        command: "gh auth login",
        docsUrl: null,
      };
    case "not-repo":
      return {
        title: "No GitHub remote",
        message:
          detail ||
          "This repository's remote is not a GitHub host known to gh (check `gh auth status` for Enterprise hosts, or pick another remote).",
        command: null,
        docsUrl: null,
      };
    case "offline":
      return {
        title: "GitHub is unreachable",
        message:
          "The network is down or GitHub cannot be reached — chips resume when the connection returns.",
        command: null,
        docsUrl: null,
      };
    case "rate-limited":
      return {
        title: "GitHub API rate limit reached",
        message:
          "gh hit the API quota. Polling backs off automatically and resumes with fresh data.",
        command: null,
        docsUrl: null,
      };
  }
}

/** "retry in 2m" / "retry in 45s" (`null` → ""). */
export function retryLabel(retryInMs: number | null): string {
  if (retryInMs === null || retryInMs <= 0) return "";
  const seconds = Math.round(retryInMs / 1000);
  if (seconds < 60) return `retry in ${seconds}s`;
  const minutes = Math.round(seconds / 60);
  return `retry in ${minutes}m`;
}

// ---------------------------------------------------------------------------
// PR chip — branch PR number + state ("#222 · draft")
// ---------------------------------------------------------------------------

/** Derives the PR chip state from the snapshot's branch PR. */
export function prChip(input: ForgeChipInput, pr: PrInfo | null): ForgeChipState {
  if (input.degraded !== null) return degradedState(input);
  if (input.loading) return LOADING;
  if (pr === null) return { kind: "none", text: "no PR" };
  const number = `#${pr.number}`;
  if (pr.state === "MERGED") return { kind: "merged", text: `${number} · merged` };
  if (pr.state === "CLOSED") return { kind: "closed", text: `${number} · closed` };
  if (pr.is_draft) return { kind: "warning", text: `${number} · draft` };
  if (pr.review_decision === "CHANGES_REQUESTED") {
    return { kind: "warning", text: `${number} · changes requested` };
  }
  return { kind: "ok", text: number };
}

// ---------------------------------------------------------------------------
// Checks chip — local HEAD check-runs ("3/4 · 1 failing"); unpushed wins
// ---------------------------------------------------------------------------

/** Counts a check list per canonical state (mirror of the panel's summary). */
export function countChecks(checks: CheckInfo[]): {
  pass: number;
  fail: number;
  pending: number;
  total: number;
} {
  const counts = { pass: 0, fail: 0, pending: 0, total: 0 };
  for (const check of checks) {
    counts.total += 1;
    if (check.state === "pass") counts.pass += 1;
    else if (check.state === "fail") counts.fail += 1;
    else if (check.state === "pending") counts.pending += 1;
  }
  return counts;
}

/**
 * Derives the Checks chip state: `unpushed` when HEAD is not on the remote
 * (§9.2); otherwise the HEAD check-runs' worst state — any failure → failing
 * (count), pending CI → running, all green → ok, no checks → none.
 */
export function checksChip(
  input: ForgeChipInput,
  checks: CheckInfo[] | null,
  pushed: boolean | null,
): ForgeChipState {
  if (input.degraded !== null) return degradedState(input);
  if (pushed === false) {
    return { kind: "unpushed", text: "unpushed" };
  }
  if (input.loading && (checks === null || checks.length === 0)) return LOADING;
  if (checks === null || checks.length === 0) {
    return { kind: "none", text: "no checks" };
  }
  const counts = countChecks(checks);
  if (counts.fail > 0) {
    return {
      kind: "failing",
      count: counts.fail,
      text: `${counts.pass}/${counts.total} · ${counts.fail} failing`,
    };
  }
  if (counts.pending > 0) {
    return {
      kind: "running",
      startedAtMs: null,
      text: `${counts.pass}/${counts.total} · running`,
    };
  }
  return { kind: "ok", text: `${counts.pass}/${counts.total}` };
}

/** One-line checks summary ("3/4 · 1 failing", "3/3", "no checks"). */
export function summarize(checks: CheckInfo[]): string {
  const counts = countChecks(checks);
  if (counts.total === 0) return "no checks";
  if (counts.fail > 0) {
    return `${counts.pass}/${counts.total} · ${counts.fail} failing`;
  }
  if (counts.pending > 0) {
    return `${counts.pass}/${counts.total} · ${counts.pending} running`;
  }
  return `${counts.pass}/${counts.total}`;
}

/** True when the PR head sha differs from the local HEAD sha (both known). */
export function prHeadDiverged(
  snapshot: Pick<ForgeSnapshot, "pr_head_sha" | "head_sha"> | null,
): boolean {
  if (snapshot === null) return false;
  const prSha = snapshot.pr_head_sha;
  const headSha = snapshot.head_sha;
  return (
    prSha !== null && prSha !== "" && headSha !== null && prSha !== headSha
  );
}

// ---------------------------------------------------------------------------
// Runs chip — latest run for the branch ("#1802 · 2m")
// ---------------------------------------------------------------------------

/**
 * Derives the Runs chip state from the latest branch run: in_progress →
 * running (spinner + elapsed from `started_at`), queued family → warning,
 * success → ok (duration), failure family → failing, cancelled/skipped
 * family → muted none (text still carries the run's outcome).
 */
export function runsChip(
  input: ForgeChipInput,
  run: WorkflowRun | null,
  nowMs: number,
): ForgeChipState {
  if (input.degraded !== null) return degradedState(input);
  if (input.loading) return LOADING;
  if (run === null) return { kind: "none", text: "no runs" };
  const number = `#${run.number}`;
  if (run.status !== "completed") {
    if (run.status === "in_progress") {
      const startedAtMs = run.started_at ? Date.parse(run.started_at) : NaN;
      return {
        kind: "running",
        startedAtMs: Number.isNaN(startedAtMs) ? null : startedAtMs,
        text: number,
      };
    }
    // queued / waiting / requested / pending — visible but not alarming.
    return { kind: "warning", text: `${number} · ${run.status.replaceAll("_", " ")}` };
  }
  const duration = runDurationLabel(run, nowMs);
  const suffix = duration === null ? "" : ` · ${duration}`;
  switch (run.conclusion) {
    case "success":
      return { kind: "ok", text: `${number}${suffix}` };
    case "failure":
    case "timed_out":
    case "action_required":
    case "startup_failure":
      return { kind: "failing", count: 1, text: `${number}${suffix}` };
    default:
      // cancelled / skipped / neutral / stale: settled, muted.
      return {
        kind: "none",
        text: `${number} · ${run.conclusion.replaceAll("_", " ") || "completed"}`,
      };
  }
}

/** Compact duration of a run ("45s", "2m", "1h 5m"); live runs tick vs now. */
export function runDurationLabel(run: WorkflowRun, nowMs: number): string | null {
  if (!run.started_at) return null;
  const start = Date.parse(run.started_at);
  if (Number.isNaN(start)) return null;
  const end =
    run.status === "completed" && run.updated_at
      ? Date.parse(run.updated_at)
      : nowMs;
  if (Number.isNaN(end) || end < start) return null;
  const seconds = Math.max(0, Math.round((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

// ---------------------------------------------------------------------------
// Worst-state collapse (§9.2, <1024px)
// ---------------------------------------------------------------------------

/**
 * Collapse rank (higher = more urgent). The plan's ladder (failing >
 * running > warning > ok > none) extended with the other states: degraded
 * outranks running (nothing works), unpushed outranks ok (actionable),
 * merged/closed sit between ok and none (settled).
 */
const CHIP_RANK: Record<ForgeChipState["kind"], number> = {
  failing: 8,
  degraded: 7,
  running: 6,
  warning: 5,
  unpushed: 4,
  ok: 3,
  merged: 2,
  closed: 2,
  none: 1,
  loading: 0,
};

/** Which chip the worst-state collapse picks ("pr" | "checks" | "runs"). */
export type ChipId = "pr" | "checks" | "runs";

/**
 * Picks the worst of `states` (`null` for an empty list). Ties keep the
 * earlier chip — with equal ranks the PR chip wins, then checks, then runs,
 * so the collapsed chip is stable.
 */
export function worstChip(
  states: [ChipId, ForgeChipState][],
): { id: ChipId; state: ForgeChipState } | null {
  let best: { id: ChipId; state: ForgeChipState } | null = null;
  for (const [id, state] of states) {
    if (best === null || CHIP_RANK[state.kind] > CHIP_RANK[best.state.kind]) {
      best = { id, state };
    }
  }
  return best;
}

/**
 * Attention states (§9.2): failing, degraded, warning, unpushed. These drive
 * the repo-tab attention badges and the collapsed chip's attention count —
 * running/loading/ok/none never beg for attention.
 */
export function isAttention(state: ForgeChipState): boolean {
  return (
    state.kind === "failing" ||
    state.kind === "degraded" ||
    state.kind === "warning" ||
    state.kind === "unpushed"
  );
}

/** Failure states — the toast-on-transition set (failures only, §9.2). */
export function isFailure(state: ForgeChipState): boolean {
  return state.kind === "failing";
}

/** The three chips derived from one snapshot (the chips store's output). */
export interface ForgeChips {
  pr: ForgeChipState;
  checks: ForgeChipState;
  runs: ForgeChipState;
  /** Worst-state collapse (the <1024px single chip). */
  worst: { id: ChipId; state: ForgeChipState } | null;
  /** Attention-worthy chip count (collapsed chip's badge). */
  attentionCount: number;
}

/** Derives all three chips + the collapse in one call. */
export function deriveChips(
  input: ForgeChipInput,
  snapshot: ForgeSnapshot | null,
  nowMs: number,
): ForgeChips {
  const pr = prChip(input, snapshot?.pr ?? null);
  const checks = checksChip(
    input,
    snapshot?.head_checks ?? null,
    snapshot?.pushed ?? null,
  );
  const runs = runsChip(input, snapshot?.run ?? null, nowMs);
  const states: [ChipId, ForgeChipState][] = [
    ["pr", pr],
    ["checks", checks],
    ["runs", runs],
  ];
  return {
    pr,
    checks,
    runs,
    worst: worstChip(states),
    attentionCount: states.filter(([, state]) => isAttention(state)).length,
  };
}
