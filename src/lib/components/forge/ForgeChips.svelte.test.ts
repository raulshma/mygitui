/**
 * ForgeChips render tests (E1 §9.2): the three chip buttons render in
 * PR → Checks → Runs order with full-state aria labels, the popovers open
 * one at a time (PR details / guidance), degraded states open the guidance
 * popover with the copyable `gh auth login` command, and the <1024px
 * collapse renders ONE worst-state chip + attention count.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, within, cleanup } from "@testing-library/svelte";
import { tick } from "svelte";
import {
  forgeSnapshot,
  forgeStatus,
  forgeContext,
  remotes,
  repoStatus,
  openRepo,
  closeRepo,
  remoteRepoOpen,
} from "$lib/ipc/client";
import type { ForgeSnapshot, PrInfo } from "$lib/ipc/types";
import { forgeStore } from "$lib/stores/forge.svelte";
import { tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { appSettings } from "$lib/state/appSettings.svelte";
import { startWidthMode, stopWidthMode } from "$lib/layout/widthMode.svelte";
import { onUiEvent } from "$lib/palette/events";
import ForgeChips from "./ForgeChips.svelte";

vi.mock("$lib/ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc/client")>()),
  forgeStatus: vi.fn(),
  forgeContext: vi.fn(),
  forgeSnapshot: vi.fn(),
  remotes: vi.fn(),
  repoStatus: vi.fn(),
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  remoteRepoOpen: vi.fn(),
}));

const mockForgeStatus = vi.mocked(forgeStatus);
const mockForgeContext = vi.mocked(forgeContext);
const mockForgeSnapshot = vi.mocked(forgeSnapshot);

function tab(patch: Partial<Tab> & { id: string }): Tab {
  return {
    name: patch.id,
    root: `/repos/${patch.id}`,
    status: null,
    statusError: null,
    generation: 0,
    deviceId: null,
    openError: null,
    ...patch,
  };
}

const PR: PrInfo = {
  number: 222,
  title: "Add the GitHub footer",
  head_ref_name: "feat",
  head_ref_oid: "",
  base_ref_name: "main",
  state: "OPEN",
  is_draft: false,
  url: "https://github.com/acme/widget/pull/222",
  created_at: null,
  updated_at: null,
  merged_at: null,
  closed_at: null,
  author: "octocat",
  body: "",
  additions: 0,
  deletions: 0,
  changed_files: 0,
  review_decision: "APPROVED",
  mergeable: "MERGEABLE",
  labels: [],
};

function snapshot(patch: Partial<ForgeSnapshot> = {}): ForgeSnapshot {
  return {
    branch: "feat",
    head_sha: "abc123",
    pushed: true,
    pr: PR,
    head_checks: [
      { name: "ci", state: "pass" },
      { name: "lint", state: "pass" },
    ],
    pr_head_sha: null,
    pr_head_checks: null,
    run: {
      database_id: 1802,
      number: 77,
      name: "CI",
      display_title: "CI",
      event: "push",
      head_branch: "feat",
      head_sha: "abc123",
      status: "completed",
      conclusion: "success",
      url: "",
      created_at: null,
      updated_at: null,
      started_at: "2026-11-21T10:00:00Z",
      attempt: 1,
      workflow_database_id: 1,
    },
    hashes: { pr: 1, head_checks: 2, pr_head_checks: 0, run: 3 },
    error: null,
    ...patch,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockForgeStatus.mockResolvedValue({ available: true, version: "2.63.3", authed: true });
  mockForgeContext.mockRejectedValue(new Error("unused") as never);
  appSettings.update({ forgeSnapshotSeconds: 60 });
  tabStore.tabs = [tab({ id: "r1" })];
  tabStore.activeId = "r1";
});

afterEach(() => {
  cleanup();
  stopWidthMode();
  tabStore.tabs = [];
  tabStore.activeId = null;
  forgeStore.dropSnapshot("r1");
});

describe("ForgeChips", () => {
  it("renders the three chips with full-state aria labels", async () => {
    mockForgeSnapshot.mockResolvedValue(snapshot());
    await forgeStore.refreshStatus();
    await forgeStore.refreshSnapshot("r1", { silent: false });

    render(ForgeChips);
    await tick();
    await tick();

    const prChip = screen.getByRole("button", { name: /^Pull request: #222/ });
    const checksChip = screen.getByRole("button", { name: /^Checks: 2\/2/ });
    const runsChip = screen.getByRole("button", { name: /^Run: #77/ });
    expect(
      [prChip, checksChip, runsChip].map((chip) => chip.dataset.chip),
    ).toEqual(["pr", "checks", "runs"]);
  });

  it("the failing state carries the failure count in the label", async () => {
    mockForgeSnapshot.mockResolvedValue(
      snapshot({
        head_checks: [
          { name: "ci", state: "fail" },
          { name: "lint", state: "pass" },
          { name: "unit", state: "pass" },
        ],
        run: snapshot().run === null ? null : {
          ...snapshot().run!,
          status: "completed",
          conclusion: "failure",
        },
      }),
    );
    await forgeStore.refreshStatus();
    await forgeStore.refreshSnapshot("r1", { silent: false });

    render(ForgeChips);
    await tick();
    await tick();

    expect(screen.getByRole("button", { name: /^Checks: 2\/3 · 1 failing/ })).not.toBeNull();
    expect(screen.getByRole("button", { name: /^Run: #77/ })).not.toBeNull();
  });

  it("the PR popover shows the title and open-in-panel focuses the panel", async () => {
    mockForgeSnapshot.mockResolvedValue(snapshot());
    await forgeStore.refreshStatus();
    await forgeStore.refreshSnapshot("r1", { silent: false });

    render(ForgeChips);
    await tick();
    await tick();

    await fireEvent.click(
      screen.getByRole("button", { name: /^Pull request: #222/ }),
    );
    const popover = await screen.findByRole("dialog", { name: "GitHub status" });
    expect(within(popover).getByText(/Add the GitHub footer/)).not.toBeNull();

    const events: CustomEvent[] = [];
    const spy = (event: Event): void => {
      events.push(event as CustomEvent);
    };
    document.addEventListener("focus-panel", spy);
    try {
      await fireEvent.click(within(popover).getByRole("button", { name: "Open in panel" }));
    } finally {
      document.removeEventListener("focus-panel", spy);
    }
    expect(events[0]?.detail).toEqual({ panel: "forge", repoId: "r1" });
    await tick();
    expect(screen.queryByRole("dialog", { name: "GitHub status" })).toBeNull();
  });

  it("degraded states open the guidance popover with copyable commands", async () => {
    mockForgeStatus.mockResolvedValue({ available: true, version: "2.63.3", authed: false });
    await forgeStore.refreshStatus();

    render(ForgeChips);
    await tick();
    await tick();

    await fireEvent.click(
      screen.getByRole("button", { name: /^Pull request: not authed/ }),
    );
    const popover = await screen.findByRole("dialog", { name: "GitHub status" });
    expect(within(popover).getByText(/not authenticated/)).not.toBeNull();
    expect(within(popover).getByText("gh auth login")).not.toBeNull();
    expect(within(popover).getByRole("button", { name: "Copy" })).not.toBeNull();
  });

  it("gh-missing degrades every chip to the install guidance", async () => {
    mockForgeStatus.mockResolvedValue({ available: false, version: "", authed: false });
    await forgeStore.refreshStatus();

    render(ForgeChips);
    await tick();
    await tick();

    expect(
      screen.getByRole("button", { name: /^Pull request: no gh/ }),
    ).not.toBeNull();
    expect(screen.getByRole("button", { name: /^Checks: no gh/ })).not.toBeNull();
    expect(screen.getByRole("button", { name: /^Run: no gh/ })).not.toBeNull();
  });

  it("<1024px collapses to one worst-state chip with the attention count", async () => {
    startWidthMode({
      readWidth: () => 900,
      onResize: () => () => {},
      reflect: () => {},
    });
    mockForgeSnapshot.mockResolvedValue(
      snapshot({
        head_checks: [
          { name: "ci", state: "fail" },
          { name: "lint", state: "pass" },
        ],
        run: snapshot().run === null ? null : { ...snapshot().run!, conclusion: "failure" },
      }),
    );
    await forgeStore.refreshStatus();
    await forgeStore.refreshSnapshot("r1", { silent: false });

    render(ForgeChips);
    await tick();
    await tick();

    // One chip only, worst state (failing), with the attention count.
    expect(screen.queryByRole("button", { name: /^Pull request/ })).toBeNull();
    const chip = screen.getByRole("button", { name: /^Checks: 1\/2 · 1 failing/ });
    expect(within(chip).getByText("2")).not.toBeNull();

    await fireEvent.click(chip);
    const popover = await screen.findByRole("dialog", { name: "GitHub status" });
    expect(within(popover).getByRole("button", { name: "Open in panel" })).not.toBeNull();
  });
});
