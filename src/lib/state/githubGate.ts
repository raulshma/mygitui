/**
 * One place that answers "is this GitHub feature on?".
 *
 * The persisted switches live in `appSettings` (Settings → GitHub). This
 * module keeps the feature taxonomy and the derived snapshot-part mask in
 * one place so the IPC client, stores, pollers and components cannot drift:
 *
 * - {@link isGithubEnabled} — the master switch.
 * - {@link isGithubFeatureEnabled} — master AND one feature toggle.
 * - {@link isGithubAnyFeatureEnabled} — master AND at least one feature.
 * - {@link githubSnapshotParts} — which snapshot reads are allowed (PR,
 *   checks, runs).
 *
 * When a feature is off, the IPC client rejects before `invoke()` and the
 * pollers unregister their lanes, so no `gh` process is spawned. The Rust
 * runner also has a process-global gate as a second line of defense.
 */

import { appSettings } from "$lib/state/appSettings.svelte";

/** The independently configurable GitHub surfaces. */
export type GithubFeature =
  | "pullRequests"
  | "actions"
  | "issues"
  | "notifications"
  | "statusChips";

/** Every feature in display order (Settings renders one toggle each). */
export const GITHUB_FEATURES: readonly GithubFeature[] = [
  "pullRequests",
  "actions",
  "issues",
  "notifications",
  "statusChips",
];

/** Human labels for settings/notices (kept beside the taxonomy). */
export const GITHUB_FEATURE_LABELS: Record<GithubFeature, string> = {
  pullRequests: "Pull requests & checks",
  actions: "Actions runs",
  issues: "Issues",
  notifications: "Notifications",
  statusChips: "Status-bar chips",
};

/** Master switch: off means no GitHub request may leave the app. */
export function isGithubEnabled(): boolean {
  return appSettings.githubEnabled;
}

/** True when the master switch and `feature`'s own toggle are both on. */
export function isGithubFeatureEnabled(feature: GithubFeature): boolean {
  if (!appSettings.githubEnabled) return false;
  switch (feature) {
    case "pullRequests":
      return appSettings.githubPullRequests;
    case "actions":
      return appSettings.githubActions;
    case "issues":
      return appSettings.githubIssues;
    case "notifications":
      return appSettings.githubNotifications;
    case "statusChips":
      return appSettings.githubStatusChips;
  }
}

/** True when at least one GitHub surface is enabled (status probes allowed). */
export function isGithubAnyFeatureEnabled(): boolean {
  return GITHUB_FEATURES.some(isGithubFeatureEnabled);
}

/** Snapshot parts allowed by the feature toggles. */
export interface GithubSnapshotParts {
  /** Branch PR lookup (`pullRequests`). */
  pr: boolean;
  /** Commit check-runs (`pullRequests`). */
  checks: boolean;
  /** Latest Actions run (`actions`). */
  runs: boolean;
}

/**
 * Snapshot part mask for the footer/overview snapshot. The snapshot is an
 * aggregate read, so each part follows its own toggle: turning pull
 * requests off stops the PR/checks gh calls while Actions runs keep
 * polling (and vice versa). `statusChips` only controls whether the footer
 * lanes poll at all — the Overview still reads the snapshot on demand.
 */
export function githubSnapshotParts(): GithubSnapshotParts {
  const pullRequests = isGithubFeatureEnabled("pullRequests");
  return {
    pr: pullRequests,
    checks: pullRequests,
    runs: isGithubFeatureEnabled("actions"),
  };
}

/** True when the snapshot would read anything at all. */
export function hasGithubSnapshotParts(parts: GithubSnapshotParts): boolean {
  return parts.pr || parts.checks || parts.runs;
}
