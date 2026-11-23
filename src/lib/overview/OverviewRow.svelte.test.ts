import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import type { PrInfo, RepoSummary, StatusEntry } from "$lib/ipc/types";
import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
import OverviewRow from "./OverviewRow.svelte";
import type { OverviewRowData } from "./overviewModel";

// jsdom has no Element.scrollTo (the diff viewer resets scroll on new
// diffs) and no ResizeObserver (the viewer's viewport tracking).
class ResizeObserverStub implements ResizeObserver {
  static instances: ResizeObserverStub[] = [];
  constructor(readonly callback: ResizeObserverCallback) {
    ResizeObserverStub.instances.push(this);
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

beforeAll(() => {
  if (typeof Element.prototype.scrollTo !== "function") {
    Element.prototype.scrollTo = () => {};
  }
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
});
afterAll(() => {
  vi.unstubAllGlobals();
});


// The expanded row streams one file diff via `streamDiff` (the same seam
// the workspace uses); pages land through the callback. The page is
// delivered on a MICROTASK like the real backend's channel (a synchronous
// call would re-enter the streaming effect mid-run).
const streamDiff = vi.fn(
  (
    _repoId: string,
    _old: string,
    _new: string,
    onPage: (files: unknown[]) => void,
    _paths?: string[],
    _consumerId?: string,
  ) => {
    queueMicrotask(() => {
      onPage([
        {
          path: "a.ts",
          old_path: null,
          binary: false,
          is_image: false,
          additions: 1,
          deletions: 0,
          hunks: [],
        },
      ]);
    });
    return Promise.resolve();
  },
);

vi.mock("$lib/ipc/client", () => ({
  streamDiff: (...args: Parameters<typeof streamDiff>) => streamDiff(...args),
}));

// DiffViewer's viewport machinery (canvas highlighting, scroll capture)
// loops in jsdom; stubbed so the row test exercises the stream invocation
// + pane wiring only (the viewer itself has its own suite).
vi.mock("$lib/components/diff/DiffViewer.svelte", () => ({
  default: () => ``,
}));

vi.mock("$lib/stores/forge.svelte", () => ({
  openExternal: vi.fn(async () => true),
}));

function entry(
  path: string,
  index: StatusEntry["index"],
  worktree: StatusEntry["worktree"],
): StatusEntry {
  return { path, old_path: null, index, worktree };
}

function pr(number: number, patch: Partial<PrInfo> = {}): PrInfo {
  return {
    number,
    title: `Fix the thing ${number}`,
    head_ref_name: "feature",
    head_ref_oid: "",
    base_ref_name: "main",
    state: "OPEN",
    is_draft: false,
    url: `https://example.test/${number}`,
    created_at: null,
    updated_at: null,
    merged_at: null,
    closed_at: null,
    author: "someone",
    body: "",
    additions: 1,
    deletions: 0,
    changed_files: 1,
    review_decision: "",
    mergeable: "",
    labels: [],
    ...patch,
  };
}

function row(patch: Partial<OverviewRowData> = {}): OverviewRowData {
  return {
    root: "/repos/alpha",
    name: "alpha",
    pinned: false,
    favorite: false,
    open: false,
    repoId: "warm-1",
    missing: false,
    refreshing: false,
    loaded: true,
    prs: null,
    failingChecks: null,
    branch: "main",
    detached: false,
    ahead: 0,
    behind: 0,
    counts: { staged: 0, unstaged: 0, untracked: 0, conflicts: 0 },
    entries: [],
    lastCommit: null,
    refreshedAt: null,
    lastOpened: 0,
    hasRemote: true,
    remoteUrl: "https://github.com/acme/alpha.git",
    isRemoteDevice: false,
    ...patch,
  };
}

afterEach(() => {
  cleanup();
  streamDiff.mockClear();
});

const NOW = 1_000_000;

describe("OverviewRow header", () => {
  it("renders name, branch, sync + change counts and stamps", () => {
    render(OverviewRow, {
      props: {
        row: row({
          ahead: 2,
          behind: 1,
          counts: { staged: 1, unstaged: 2, untracked: 3, conflicts: 0 },
          prs: [pr(7)],
          failingChecks: 1,
          refreshedAt: NOW - 30_000,
        }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("alpha")).toBeTruthy();
    expect(screen.getByText("main")).toBeTruthy();
    expect(screen.getByTitle("2 ahead").textContent).toContain("2");
    expect(screen.getByTitle("1 behind")).toBeTruthy();
    expect(screen.getByTitle("1 staged")).toBeTruthy();
    expect(screen.getByTitle("2 unstaged")).toBeTruthy();
    expect(screen.getByTitle("3 untracked")).toBeTruthy();
    // The header chip + the always-visible PR row both carry the number.
    expect(screen.getAllByText("#7").length).toBe(2);
    expect(screen.getByTitle("Failing CI checks on the branch head")).toBeTruthy();
    expect(screen.getByText("updated 30s ago")).toBeTruthy();
  });

  it("shows detached and missing badges instead of a branch", () => {
    const { rerender } = render(OverviewRow, {
      props: {
        row: row({ detached: true }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("detached")).toBeTruthy();

    rerender({
      row: row({ missing: true }),
      now: NOW,
      onToggle: () => {},
      onOpen: () => {},
      onRemove: () => {},
    });
    expect(screen.getByText("missing")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Open alpha" }),
    ).toBeNull();
    expect(screen.getByText(/Folder not found on disk/)).toBeTruthy();
  });

  it("renders the last commit as short sha + summary + relative time", () => {
    render(OverviewRow, {
      props: {
        row: row({
          lastCommit: {
            sha: "abcdef1234567890",
            summary: "init commit",
            timestamp: NOW / 1000 - 3600,
          },
        }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("abcdef1")).toBeTruthy();
    expect(screen.getByText("init commit")).toBeTruthy();
  });
});

describe("OverviewRow files + PRs (always visible)", () => {
  it("groups files staged/changed/untracked with kind letters", () => {
    render(OverviewRow, {
      props: {
        row: row({
          entries: [
            entry("src/staged.ts", "modified", "unmodified"),
            entry("dirty.ts", "unmodified", "modified"),
            entry("new.ts", "unmodified", "untracked"),
            entry("conflict.ts", "conflicted", "modified"),
          ],
        }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByLabelText("Staged files")).toBeTruthy();
    expect(screen.getByLabelText("Changed files")).toBeTruthy();
    expect(screen.getByLabelText("Untracked files")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Show diff for src/staged.ts (staged)" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show diff for dirty.ts" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Show diff for new.ts (untracked)" }),
    ).toBeTruthy();
    // A conflicted entry groups under Staged (the index side carries it).
    expect(
      screen.getByRole("button", { name: "Show diff for conflict.ts (staged)" }),
    ).toBeTruthy();
  });

  it("shows an em-dash with tooltip when gh data is not loaded, PR rows otherwise", async () => {
    const { rerender } = render(OverviewRow, {
      props: {
        row: row({ prs: null }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(
      screen.getByTitle("open or expand for live GitHub data"),
    ).toBeTruthy();

    rerender({
      row: row({ prs: [] }),
      expanded: true,
      now: NOW,
      onToggle: () => {},
      onOpen: () => {},
      onRemove: () => {},
    });
    expect(screen.getByText("No open PRs")).toBeTruthy();

    rerender({
      row: row({ prs: [pr(7, { is_draft: true }), pr(8, { review_decision: "APPROVED" })] }),
      now: NOW,
      onToggle: () => {},
      onOpen: () => {},
      onRemove: () => {},
    });
    expect(
      screen.getByRole("button", { name: "Open pull request #7: Fix the thing 7" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Open pull request #8: Fix the thing 8" }),
    ).toBeTruthy();
    expect(screen.getAllByTitle("Approved").length).toBeGreaterThan(0);
  });

  it("renders a clean state when the working copy is clean", () => {
    render(OverviewRow, {
      props: {
        row: row({ prs: [] }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("Clean")).toBeTruthy();
    expect(screen.getByText("In sync with remote")).toBeTruthy();
  });
});

describe("OverviewRow expansion + actions", () => {
  function setup(opts: { row?: Partial<OverviewRowData>; expanded?: boolean } = {}): {
    onToggle: ReturnType<typeof vi.fn>;
    onOpen: ReturnType<typeof vi.fn>;
    onRemove: ReturnType<typeof vi.fn>;
  } {
    const onToggle = vi.fn();
    const onOpen = vi.fn();
    const onRemove = vi.fn();
    render(OverviewRow, {
      props: {
        row: row({
          entries: [entry("a.ts", "unmodified", "modified")],
          ...opts.row,
        }),
        expanded: opts.expanded ?? false,
        now: NOW,
        onToggle,
        onOpen,
        onRemove,
      },
    });
    return { onToggle, onOpen, onRemove };
  }

  it("clicking the header toggles; Enter/Space work too", async () => {
    const { onToggle } = setup();
    const head = screen.getByRole("button", { name: "Toggle diffs for alpha" });
    await fireEvent.click(head);
    expect(onToggle).toHaveBeenCalledWith("/repos/alpha");
    await fireEvent.keyDown(head, { key: "Enter" });
    expect(onToggle).toHaveBeenCalledTimes(2);
    await fireEvent.keyDown(head, { key: " " });
    expect(onToggle).toHaveBeenCalledTimes(3);
  });

  it("the Open button opens; the context menu offers Open + Remove", async () => {
    const { onOpen, onRemove } = setup();
    await fireEvent.click(screen.getByRole("button", { name: "Open alpha" }));
    expect(onOpen).toHaveBeenCalledWith("/repos/alpha");

    // The menu itself renders App-level; the row only hands the store the
    // entries (asserted here, then hidden).
    await fireEvent.contextMenu(
      screen.getByRole("button", { name: "Toggle diffs for alpha" }),
    );
    expect(contextMenu.open).toBe(true);
    const labels = contextMenu.entries
      .filter((item) => "label" in item)
      .map((item) => ("label" in item ? item.label : ""));
    expect(labels).toEqual(["Open", "Remove from library"]);
    const remove = contextMenu.entries.find(
      (item) => "label" in item && item.label === "Remove from library",
    ) as { run: () => void } | undefined;
    remove?.run();
    expect(onRemove).toHaveBeenCalledWith("/repos/alpha");
    contextMenu.hide();
    expect(contextMenu.open).toBe(false);
  });

  it("expanded rows stream the selected file's diff (lazy)", async () => {
    setup({ expanded: true });
    await tick();
    expect(streamDiff).toHaveBeenCalledTimes(1);
    const [repoId, oldSide, newSide, , paths, consumer] = streamDiff.mock.calls[0]!;
    expect(repoId).toBe("warm-1");
    expect([oldSide, newSide]).toEqual(["index", "worktree"]);
    expect(paths).toEqual(["a.ts"]);
    // Per-repo slot: two expanded rows must not supersede each other.
    expect(consumer).toBe("overview-diff-warm-1");
    await tick();
    await tick();
    // The diff pane mounted: its path header spells the file alongside the
    // file row (the viewer body itself is covered by DiffViewer's suite).
    expect(screen.getAllByText("a.ts").length).toBe(2);

    // Clicking another file row re-streams for that path (still one
    // component, the effect restarts).
    cleanup();
    streamDiff.mockClear();
    setup({
      expanded: true,
      row: {
        entries: [
          entry("a.ts", "unmodified", "modified"),
          entry("b.ts", "unmodified", "modified"),
        ],
      },
    });
    await tick();
    await fireEvent.click(screen.getByRole("button", { name: "Show diff for b.ts" }));
    await tick();
    expect(streamDiff).toHaveBeenCalledTimes(2);
    expect(streamDiff.mock.calls[1]![4]).toEqual(["b.ts"]);
  });

  it("unexpanded rows mount no diff stream", async () => {
    setup();
    await tick();
    expect(streamDiff).not.toHaveBeenCalled();
  });

  it("missing rows never stream diffs even when expanded", async () => {
    setup({ row: { missing: true }, expanded: true });
    await tick();
    expect(streamDiff).not.toHaveBeenCalled();
  });
});

describe("OverviewRow summary tier", () => {
  it("repo_summary-shaped rows render counts from entries", () => {
    const repoSummary: RepoSummary = {
      root: "/repos/alpha",
      name: "alpha",
      branch: "main",
      detached: false,
      ahead: 0,
      behind: 0,
      conflicts: 0,
      staged: 1,
      unstaged: 0,
      untracked: 0,
      entries: [entry("s.ts", "modified", "unmodified")],
      last_commit: null,
      remote_url: null,
      has_remote: false,
    };
    void repoSummary;
    render(OverviewRow, {
      props: {
        row: row({
          counts: { staged: 1, unstaged: 0, untracked: 0, conflicts: 0 },
          entries: [entry("s.ts", "modified", "unmodified")],
        }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByTitle("1 staged")).toBeTruthy();
    expect(screen.getByLabelText("Staged files")).toBeTruthy();
  });

  it("unloaded rows show skeletons, not fake data", () => {
    render(OverviewRow, {
      props: {
        row: row({ loaded: false }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("alpha")).toBeTruthy();
    expect(screen.queryByTitle("main")).toBeNull();
    expect(document.querySelectorAll("[data-slot='skeleton']").length).toBeGreaterThan(0);
  });
});

describe("OverviewRow modern card presentation", () => {
  it("renders repo type badge and icons for GitHub, GitLab, SSH Device and Local only", () => {
    const { rerender } = render(OverviewRow, {
      props: {
        row: row({ hasRemote: true, remoteUrl: "https://github.com/acme/alpha.git" }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("GitHub")).toBeTruthy();

    rerender({
      row: row({ hasRemote: true, remoteUrl: "https://gitlab.com/acme/alpha.git" }),
      now: NOW,
      onToggle: () => {},
      onOpen: () => {},
      onRemove: () => {},
    });
    expect(screen.getByText("GitLab")).toBeTruthy();

    rerender({
      row: row({ isRemoteDevice: true, root: "ssh://server/repos/alpha" }),
      now: NOW,
      onToggle: () => {},
      onOpen: () => {},
      onRemove: () => {},
    });
    expect(screen.getByText("SSH Device")).toBeTruthy();

    rerender({
      row: row({ hasRemote: false, remoteUrl: null }),
      now: NOW,
      onToggle: () => {},
      onOpen: () => {},
      onRemove: () => {},
    });
    expect(screen.getAllByText("Local only").length).toBeGreaterThan(0);
  });

  it("renders last accessed timestamp when available", () => {
    const testNow = 10_000_000;
    render(OverviewRow, {
      props: {
        row: row({ lastOpened: testNow - 7200_000 }),
        now: testNow,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByText("Accessed 2h ago")).toBeTruthy();
  });

  it("renders favorite star and pinned badge", () => {
    render(OverviewRow, {
      props: {
        row: row({ favorite: true, pinned: true }),
        now: NOW,
        onToggle: () => {},
        onOpen: () => {},
        onRemove: () => {},
      },
    });
    expect(screen.getByTitle("Favorite")).toBeTruthy();
    expect(screen.getByText("pinned")).toBeTruthy();
  });
});

