/**
 * Pure model for the M6 forge UI (no Svelte, no IPC — fully tested).
 *
 * Everything ForgePanel and CreatePrDialog render is computed here:
 * branch→PR matching, CI-check summarization (counts + worst state → chip
 * class/icon), PR age formatting, gh-status → banner mapping and PR URL
 * number extraction (for the already-exists flow).
 */

import type { CheckInfo, CheckState, ForgeStatus, PrInfo } from "$lib/ipc/client";

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
