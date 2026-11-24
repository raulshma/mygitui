import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { appSettings } from "$lib/state/appSettings.svelte";
import {
  GITHUB_FEATURES,
  GITHUB_FEATURE_LABELS,
  githubSnapshotParts,
  hasGithubSnapshotParts,
  isGithubAnyFeatureEnabled,
  isGithubEnabled,
  isGithubFeatureEnabled,
} from "$lib/state/githubGate";
import { githubSwitches } from "$lib/state/githubSync";

describe("githubGate", () => {
  beforeEach(() => {
    appSettings.update({
      githubEnabled: true,
      githubPullRequests: true,
      githubActions: true,
      githubIssues: true,
      githubNotifications: true,
      githubStatusChips: true,
    });
  });

  afterEach(() => {
    appSettings.update({
      githubEnabled: true,
      githubPullRequests: true,
      githubActions: true,
      githubIssues: true,
      githubNotifications: true,
      githubStatusChips: true,
    });
  });

  it("exposes expected feature list and human labels", () => {
    expect(GITHUB_FEATURES).toEqual([
      "pullRequests",
      "actions",
      "issues",
      "notifications",
      "statusChips",
    ]);
    for (const feature of GITHUB_FEATURES) {
      expect(typeof GITHUB_FEATURE_LABELS[feature]).toBe("string");
      expect(GITHUB_FEATURE_LABELS[feature].length).toBeGreaterThan(0);
    }
  });

  it("reflects master switch across all feature queries", () => {
    expect(isGithubEnabled()).toBe(true);
    expect(isGithubAnyFeatureEnabled()).toBe(true);
    for (const feature of GITHUB_FEATURES) {
      expect(isGithubFeatureEnabled(feature)).toBe(true);
    }

    appSettings.update({ githubEnabled: false });

    expect(isGithubEnabled()).toBe(false);
    expect(isGithubAnyFeatureEnabled()).toBe(false);
    for (const feature of GITHUB_FEATURES) {
      expect(isGithubFeatureEnabled(feature)).toBe(false);
    }
  });

  it("reflects individual feature switches when master switch is on", () => {
    appSettings.update({
      githubEnabled: true,
      githubPullRequests: false,
      githubActions: true,
      githubIssues: false,
      githubNotifications: true,
      githubStatusChips: false,
    });

    expect(isGithubFeatureEnabled("pullRequests")).toBe(false);
    expect(isGithubFeatureEnabled("actions")).toBe(true);
    expect(isGithubFeatureEnabled("issues")).toBe(false);
    expect(isGithubFeatureEnabled("notifications")).toBe(true);
    expect(isGithubFeatureEnabled("statusChips")).toBe(false);
    expect(isGithubAnyFeatureEnabled()).toBe(true);

    // Turn remaining on-features off
    appSettings.update({
      githubActions: false,
      githubNotifications: false,
    });
    expect(isGithubAnyFeatureEnabled()).toBe(false);
  });

  it("derives snapshot parts mask from pullRequests and actions", () => {
    expect(githubSnapshotParts()).toEqual({
      pr: true,
      checks: true,
      runs: true,
    });
    expect(hasGithubSnapshotParts(githubSnapshotParts())).toBe(true);

    appSettings.update({ githubPullRequests: false });
    expect(githubSnapshotParts()).toEqual({
      pr: false,
      checks: false,
      runs: true,
    });
    expect(hasGithubSnapshotParts(githubSnapshotParts())).toBe(true);

    appSettings.update({ githubActions: false });
    expect(githubSnapshotParts()).toEqual({
      pr: false,
      checks: false,
      runs: false,
    });
    expect(hasGithubSnapshotParts(githubSnapshotParts())).toBe(false);

    // Master off forces all parts false regardless of feature flags
    appSettings.update({
      githubEnabled: false,
      githubPullRequests: true,
      githubActions: true,
    });
    expect(githubSnapshotParts()).toEqual({
      pr: false,
      checks: false,
      runs: false,
    });
    expect(hasGithubSnapshotParts(githubSnapshotParts())).toBe(false);
  });

  it("githubSwitches returns full snapshot of settings", () => {
    appSettings.update({
      githubEnabled: true,
      githubPullRequests: false,
      githubActions: true,
      githubIssues: false,
      githubNotifications: true,
      githubStatusChips: false,
    });

    expect(githubSwitches()).toEqual({
      enabled: true,
      pullRequests: false,
      actions: true,
      issues: false,
      notifications: true,
      statusChips: false,
    });
  });
});
