import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BranchInfo, CommitInfo, ForgeSnapshot, LogPage, PrInfo, RepoStatus, StatusEntry, WorkflowRun } from "$lib/ipc/types";
import EditorEmptyOverview from "./EditorEmptyOverview.svelte";
import EditorPane from "./EditorPane.svelte";

const mockBranchesFor = vi.fn();
const mockStreamLog = vi.fn();
const mockSnapshotStaleFor = vi.fn();
const mockChipsFor = vi.fn();
const mockOpenExternal = vi.fn();
const mockCopyText = vi.fn();

vi.mock("$lib/clipboard", () => ({
  copyText: (text: string, label?: string) => mockCopyText(text, label),
}));

vi.mock("$lib/stores/refs.svelte", () => ({
  branchesFor: (repoId: string) => mockBranchesFor(repoId),
}));

vi.mock("$lib/ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc/client")>()),
  streamLog: (repoId: string, filter: any, onPage: (p: LogPage) => void) =>
    mockStreamLog(repoId, filter, onPage),
}));

vi.mock("$lib/stores/forge.svelte", () => ({
  forgeStore: {
    snapshotStaleFor: (repoId: string) => mockSnapshotStaleFor(repoId),
  },
  openExternal: (url: string) => mockOpenExternal(url),
}));

vi.mock("$lib/forge/chips.svelte", () => ({
  forgeChips: {
    chipsFor: (repoId: string, now?: number) => mockChipsFor(repoId, now),
  },
}));

function makeStatus(patch: Partial<RepoStatus> = {}): RepoStatus {
  return {
    branch: "main",
    head: "1111222233334444555566667777888899990000",
    detached: false,
    ahead: 0,
    behind: 0,
    merging: false,
    rebasing: false,
    sequencer: false,
    bisecting: false,
    entries: [],
    ...patch,
  };
}

describe("EditorEmptyOverview", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockBranchesFor.mockResolvedValue([]);
    mockStreamLog.mockResolvedValue(undefined);
    mockSnapshotStaleFor.mockReturnValue(null);
    mockChipsFor.mockReturnValue({
      worst: null,
      pr: { kind: "neutral", text: "" },
      checks: { kind: "neutral", text: "" },
      runs: { kind: "neutral", text: "" },
    });
  });

  afterEach(() => {
    cleanup();
  });

  it("renders loading state when status is null", () => {
    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: null,
      },
    });

    expect(screen.getByText("Nothing open")).toBeTruthy();
    expect(screen.getByText("Reading repository…")).toBeTruthy();
    expect(screen.getByText("Command palette")).toBeTruthy();
  });

  it("renders clean working tree and branch info", async () => {
    mockBranchesFor.mockResolvedValue([
      {
        name: "main",
        remote: null,
        upstream: "refs/remotes/origin/main",
        ahead: 0,
        behind: 0,
        gone: false,
        is_head: true,
      },
    ]);

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        repoName: "my-cool-project",
        status: makeStatus({ branch: "main" }),
      },
    });

    expect(screen.getByText("my-cool-project")).toBeTruthy();
    expect(screen.getByText("main")).toBeTruthy();
    expect(screen.getByText("Clean working tree")).toBeTruthy();

    await tick();
    await tick();

    expect(screen.getByText("Up to date with origin/main")).toBeTruthy();
  });

  it("renders dirty working tree with staged, modified, untracked, and conflicted counts", () => {
    const entries: StatusEntry[] = [
      { path: "staged.ts", old_path: null, index: "modified", worktree: "unmodified" },
      { path: "unstaged.ts", old_path: null, index: "unmodified", worktree: "modified" },
      { path: "untracked.ts", old_path: null, index: "unmodified", worktree: "untracked" },
      { path: "conflict.ts", old_path: null, index: "conflicted", worktree: "modified" },
    ];

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({
          entries,
          ahead: 2,
          behind: 1,
        }),
      },
    });

    expect(screen.getByText("4 files:")).toBeTruthy();
    expect(screen.getByText("+1 staged")).toBeTruthy();
    expect(screen.getByText("~1 modified")).toBeTruthy();
    expect(screen.getByText("?1 untracked")).toBeTruthy();
    expect(screen.getByText("!1 conflicted")).toBeTruthy();
    expect(screen.getByText("↑ 2 to push")).toBeTruthy();
    expect(screen.getByText("↓ 1 to pull")).toBeTruthy();
  });

  it("renders detached HEAD correctly", () => {
    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({
          branch: null,
          detached: true,
          head: "abcdef1234567890",
        }),
      },
    });

    expect(screen.getByText("detached @ abcdef1")).toBeTruthy();
  });

  it("renders in-progress operations banner", () => {
    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ rebasing: true }),
      },
    });

    expect(screen.getByText("Rebase in progress")).toBeTruthy();
    cleanup();

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ bisecting: true }),
      },
    });

    expect(screen.getByText("Bisect in progress")).toBeTruthy();
    cleanup();

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ merging: true }),
      },
    });

    expect(screen.getByText("Merge in progress")).toBeTruthy();
  });

  it("renders unborn repository when head is null", () => {
    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ head: "" }),
      },
    });

    expect(screen.getByText("No commits yet (unborn repo)")).toBeTruthy();
  });

  it("renders local branch and upstream gone tracking states", async () => {
    mockBranchesFor.mockResolvedValue([
      {
        name: "feature-local",
        remote: null,
        upstream: null,
        ahead: 0,
        behind: 0,
        gone: false,
        is_head: true,
      },
    ]);

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ branch: "feature-local" }),
      },
    });

    await tick();
    await tick();

    expect(screen.getByText("Local branch (no upstream tracking)")).toBeTruthy();
    cleanup();

    mockBranchesFor.mockResolvedValue([
      {
        name: "feature-deleted",
        remote: null,
        upstream: "refs/remotes/origin/feature-deleted",
        ahead: 0,
        behind: 0,
        gone: true,
        is_head: true,
      },
    ]);

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ branch: "feature-deleted" }),
      },
    });

    await tick();
    await tick();

    expect(screen.getByText("Upstream gone")).toBeTruthy();
  });

  it("renders latest commit information when loaded via streamLog", async () => {
    const commit: CommitInfo = {
      sha: "1111222233334444555566667777888899990000",
      parents: [],
      author: { name: "Alice Developer", email: "alice@example.com", time: 1700000000, offset_minutes: 0 },
      committer: { name: "Alice Developer", email: "alice@example.com", time: 1700000000, offset_minutes: 0 },
      summary: "feat: add amazing feature",
      refs: [],
    };

    mockStreamLog.mockImplementation(async (_repoId: string, _filter: unknown, onPage: (page: LogPage) => void) => {
      onPage({
        commits: [commit],
        rows: [],
        next_cursor: null,
        generation: 1,
      });
    });

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus({ head: commit.sha }),
      },
    });

    await tick();
    await tick();

    expect(screen.getByText("feat: add amazing feature")).toBeTruthy();
    expect(screen.getByText("Alice Developer")).toBeTruthy();
    expect(screen.getByText("1111222")).toBeTruthy();
  });

  it("renders GitHub degraded state guidance", () => {
    mockChipsFor.mockReturnValue({
      worst: {
        state: {
          kind: "degraded",
          degraded: "no-gh",
          message: "Install gh",
          retryInMs: null,
        },
      },
    });

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus(),
      },
    });

    expect(screen.getByText("GitHub CLI (gh) not found")).toBeTruthy();
    expect(screen.getByText("gh auth login")).toBeTruthy();
    expect(screen.getByText("Documentation")).toBeTruthy();
  });

  it("renders GitHub PR, CI checks, and workflow run details when connected", () => {
    const pr: PrInfo = {
      number: 42,
      title: "Add awesome overview",
      head_ref_name: "feat/overview",
      head_ref_oid: "11112222",
      base_ref_name: "main",
      state: "OPEN",
      is_draft: false,
      url: "https://github.com/test/repo/pull/42",
      created_at: null,
      updated_at: null,
      merged_at: null,
      closed_at: null,
      author: "alice",
      body: "overview body",
      additions: 120,
      deletions: 15,
      changed_files: 3,
      review_decision: "APPROVED",
      mergeable: "MERGEABLE",
      labels: [],
    };

    const run: WorkflowRun = {
      database_id: 999,
      number: 14,
      name: "CI Pipeline",
      display_title: "CI Pipeline",
      event: "push",
      head_branch: "main",
      head_sha: "11112222",
      status: "completed",
      conclusion: "success",
      url: "https://github.com/test/repo/actions/runs/999",
      created_at: null,
      updated_at: null,
      started_at: null,
      attempt: 1,
      workflow_database_id: 10,
    };

    mockSnapshotStaleFor.mockReturnValue({
      branch: "main",
      head_sha: "11112222",
      pushed: true,
      pr,
      head_checks: [
        {
          name: "test",
          state: "pass",
        },
      ],
      pr_head_sha: null,
      pr_head_checks: null,
      run,
      hashes: { pr: "h1", checks: "h2", runs: "h3" },
      failure: null,
    });

    render(EditorEmptyOverview, {
      props: {
        repoId: "repo-1",
        status: makeStatus(),
      },
    });

    expect(screen.getByText("#42 Add awesome overview")).toBeTruthy();
    expect(screen.getByText("Open")).toBeTruthy();
    expect(screen.getByText("+120")).toBeTruthy();
    expect(screen.getByText("-15")).toBeTruthy();
    expect(screen.getByText("1 passed")).toBeTruthy();
    expect(screen.getByText("#14 CI Pipeline")).toBeTruthy();
    expect(screen.getByText("success")).toBeTruthy();
  });

  it("renders PR with draft, merged, and closed states and triggers external link", async () => {
    const makePr = (patch: Partial<PrInfo> = {}): PrInfo => ({
      number: 101,
      title: "Draft PR test",
      head_ref_name: "feat/draft",
      head_ref_oid: "1111",
      base_ref_name: "main",
      state: "OPEN",
      is_draft: true,
      url: "https://github.com/test/repo/pull/101",
      created_at: null,
      updated_at: null,
      merged_at: null,
      closed_at: null,
      author: "bob",
      body: "",
      additions: 10,
      deletions: 2,
      changed_files: 1,
      review_decision: "REVIEW_REQUIRED",
      mergeable: "MERGEABLE",
      labels: [],
      ...patch,
    });

    mockSnapshotStaleFor.mockReturnValue({
      branch: "main",
      head_sha: "1111",
      pushed: true,
      pr: makePr({ is_draft: true, state: "OPEN" }),
      head_checks: null,
      pr_head_sha: null,
      pr_head_checks: null,
      run: null,
      hashes: { pr: "h1", checks: "h2", runs: "h3" },
      failure: null,
    });

    render(EditorEmptyOverview, {
      props: { repoId: "repo-1", status: makeStatus() },
    });

    expect(screen.getByText("Draft")).toBeTruthy();
    expect(screen.getByText("#101 Draft PR test")).toBeTruthy();

    // Clicking PR title calls openExternal
    const prTitleBtn = screen.getByText("#101 Draft PR test");
    await fireEvent.click(prTitleBtn);
    expect(mockOpenExternal).toHaveBeenCalledWith("https://github.com/test/repo/pull/101");

    cleanup();

    // Merged state
    mockSnapshotStaleFor.mockReturnValue({
      branch: "main",
      head_sha: "1111",
      pushed: true,
      pr: makePr({ number: 102, title: "Merged PR", state: "MERGED", is_draft: false, url: "https://github.com/test/repo/pull/102" }),
      head_checks: null,
      pr_head_sha: null,
      pr_head_checks: null,
      run: null,
      hashes: { pr: "h1", checks: "h2", runs: "h3" },
      failure: null,
    });

    render(EditorEmptyOverview, {
      props: { repoId: "repo-1", status: makeStatus() },
    });
    expect(screen.getByText("Merged")).toBeTruthy();
    cleanup();

    // Closed state
    mockSnapshotStaleFor.mockReturnValue({
      branch: "main",
      head_sha: "1111",
      pushed: true,
      pr: makePr({ number: 103, title: "Closed PR", state: "CLOSED", is_draft: false }),
      head_checks: null,
      pr_head_sha: null,
      pr_head_checks: null,
      run: null,
      hashes: { pr: "h1", checks: "h2", runs: "h3" },
      failure: null,
    });

    render(EditorEmptyOverview, {
      props: { repoId: "repo-1", status: makeStatus() },
    });
    expect(screen.getByText("Closed")).toBeTruthy();
  });

  it("renders CI checks failing and running states and navigates to checks tab", async () => {
    mockSnapshotStaleFor.mockReturnValue({
      branch: "main",
      head_sha: "1111",
      pushed: true,
      pr: null,
      head_checks: [
        { name: "lint", state: "fail" },
        { name: "build", state: "pending" },
        { name: "unit-tests", state: "pass" },
      ],
      pr_head_sha: null,
      pr_head_checks: null,
      run: null,
      hashes: { pr: "h1", checks: "h2", runs: "h3" },
      failure: null,
    });

    const uiEvents: Array<{ type: string; detail: any }> = [];
    const handleUiEvent = (e: Event) => {
      uiEvents.push({ type: e.type, detail: (e as CustomEvent).detail });
    };
    document.addEventListener("focus-panel", handleUiEvent);

    try {
      render(EditorEmptyOverview, {
        props: { repoId: "repo-1", status: makeStatus() },
      });

      expect(screen.getByText("1 failing")).toBeTruthy();
      expect(screen.getByText("1 running")).toBeTruthy();
      expect(screen.getByText("1 passed")).toBeTruthy();

      const detailsBtn = screen.getByText("Details");
      await fireEvent.click(detailsBtn);
      expect(
        uiEvents.some(
          (ev) =>
            ev.type === "focus-panel" &&
            ev.detail?.panel === "forge" &&
            ev.detail?.tab === "checks",
        ),
      ).toBe(true);
    } finally {
      document.removeEventListener("focus-panel", handleUiEvent);
    }
  });

  it("renders running workflow and emits workflow-run-open on click", async () => {
    const run: WorkflowRun = {
      database_id: 888,
      number: 21,
      name: "Deploy Stage",
      display_title: "Deploy Stage",
      event: "push",
      head_branch: "main",
      head_sha: "11112222",
      status: "in_progress",
      conclusion: "",
      url: "https://github.com/test/repo/actions/runs/888",
      created_at: null,
      updated_at: null,
      started_at: "2026-11-24T00:00:00Z",
      attempt: 1,
      workflow_database_id: 11,
    };

    mockSnapshotStaleFor.mockReturnValue({
      branch: "main",
      head_sha: "11112222",
      pushed: true,
      pr: null,
      head_checks: null,
      pr_head_sha: null,
      pr_head_checks: null,
      run,
      hashes: { pr: "h1", checks: "h2", runs: "h3" },
      failure: null,
    });

    const uiEvents: Array<{ type: string; detail: any }> = [];
    const handleUiEvent = (e: Event) => {
      uiEvents.push({ type: e.type, detail: (e as CustomEvent).detail });
    };
    document.addEventListener("workflow-run-open", handleUiEvent);

    try {
      render(EditorEmptyOverview, {
        props: { repoId: "repo-1", status: makeStatus() },
      });

      expect(screen.getByText("#21 Deploy Stage")).toBeTruthy();
      expect(screen.getByText("in progress")).toBeTruthy();

      const runRow = screen.getByText("#21 Deploy Stage");
      await fireEvent.click(runRow);

      expect(
        uiEvents.some(
          (ev) =>
            ev.type === "workflow-run-open" &&
            ev.detail?.repoId === "repo-1" &&
            ev.detail?.runId === 888,
        ),
      ).toBe(true);
    } finally {
      document.removeEventListener("workflow-run-open", handleUiEvent);
    }
  });

  it("renders degraded states (not-authed, non-github) and handles copy and docs buttons", async () => {
    // not-authed
    mockChipsFor.mockReturnValue({
      worst: {
        state: {
          kind: "degraded",
          degraded: "not-authed",
          message: "Sign in with gh auth login",
          retryInMs: null,
        },
      },
    });

    render(EditorEmptyOverview, {
      props: { repoId: "repo-1", status: makeStatus() },
    });

    expect(screen.getByText("gh is installed but not authenticated")).toBeTruthy();
    expect(screen.getByText("gh auth login")).toBeTruthy();

    const copyBtn = screen.getByText("Copy");
    await fireEvent.click(copyBtn);
    expect(mockCopyText).toHaveBeenCalledWith("gh auth login", "command");
    cleanup();

    // not-repo (non-GitHub remote)
    mockChipsFor.mockReturnValue({
      worst: {
        state: {
          kind: "degraded",
          degraded: "not-repo",
          message: "Remote is GitLab",
          retryInMs: null,
        },
      },
    });

    render(EditorEmptyOverview, {
      props: { repoId: "repo-1", status: makeStatus() },
    });
    expect(screen.getByText("No GitHub remote")).toBeTruthy();
    expect(screen.getByText("Remote is GitLab")).toBeTruthy();
    cleanup();

    // no-gh with docs button
    mockChipsFor.mockReturnValue({
      worst: {
        state: {
          kind: "degraded",
          degraded: "no-gh",
          message: "Install gh CLI",
          retryInMs: null,
        },
      },
    });

    render(EditorEmptyOverview, {
      props: { repoId: "repo-1", status: makeStatus() },
    });

    const docsBtn = screen.getByText("Documentation");
    await fireEvent.click(docsBtn);
    expect(mockOpenExternal).toHaveBeenCalledWith("https://cli.github.com/manual/installation");
  });

  it("handles user action triggers and UI events", async () => {
    const onOpenFirstChange = vi.fn();
    const onBrowseHistory = vi.fn();

    const uiEvents: Array<{ type: string; detail: any }> = [];
    const handleUiEvent = (e: Event) => {
      uiEvents.push({ type: e.type, detail: (e as CustomEvent).detail });
    };

    document.addEventListener("branch-switcher-open", handleUiEvent);
    document.addEventListener("focus-panel", handleUiEvent);

    try {
      render(EditorEmptyOverview, {
        props: {
          repoId: "repo-1",
          status: makeStatus({
            entries: [{ path: "file.ts", old_path: null, index: "modified", worktree: "unmodified" }],
          }),
          onOpenFirstChange,
          onBrowseHistory,
        },
      });

      // Quick action: Open first change
      const firstChangeBtn = screen.getByText("Open first change");
      await fireEvent.click(firstChangeBtn);
      expect(onOpenFirstChange).toHaveBeenCalledTimes(1);

      // Quick action: Browse history
      const browseHistoryBtn = screen.getByText("Browse history");
      await fireEvent.click(browseHistoryBtn);
      expect(onBrowseHistory).toHaveBeenCalledTimes(1);

      // Quick action: Switch branch
      const switchBranchBtn = screen.getByText("Switch branch");
      await fireEvent.click(switchBranchBtn);
      expect(uiEvents.some((ev) => ev.type === "branch-switcher-open" && ev.detail?.repoId === "repo-1")).toBe(true);

      // Quick action: Open Forge (second button; header has another)
      const openForgeBtns = screen.getAllByText("Open Forge");
      await fireEvent.click(openForgeBtns[1]!);
      expect(uiEvents.some((ev) => ev.type === "focus-panel" && ev.detail?.panel === "forge")).toBe(true);

      // Card action: View changes
      const viewChangesBtn = screen.getByText("View changes");
      await fireEvent.click(viewChangesBtn);
      expect(uiEvents.some((ev) => ev.type === "focus-panel" && ev.detail?.panel === "status")).toBe(true);
    } finally {
      document.removeEventListener("branch-switcher-open", handleUiEvent);
      document.removeEventListener("focus-panel", handleUiEvent);
    }
  });

  it("integrates seamlessly into EditorPane's empty state", () => {
    render(EditorPane, {
      props: {
        repoId: "repo-1",
        repoName: "test-repo",
        status: makeStatus(),
      },
    });

    expect(screen.getByText("test-repo")).toBeTruthy();
    expect(screen.getByText("Git Status")).toBeTruthy();
    expect(screen.getByText("GitHub")).toBeTruthy();
    cleanup();

    // When commit is active, overview is NOT rendered
    const commit: CommitInfo = {
      sha: "aabbccdd11223344",
      parents: [],
      author: { name: "Dev", email: "dev@example.com", time: 1700000000, offset_minutes: 0 },
      committer: { name: "Dev", email: "dev@example.com", time: 1700000000, offset_minutes: 0 },
      summary: "Commit detail view active",
      refs: [],
    };

    render(EditorPane, {
      props: {
        repoId: "repo-1",
        repoName: "test-repo",
        status: makeStatus(),
        commit,
      },
    });

    expect(screen.queryByText("Git Status")).toBeNull();
    expect(screen.queryByText("GitHub")).toBeNull();
    expect(screen.getByText("Commit detail view active")).toBeTruthy();
  });
});
