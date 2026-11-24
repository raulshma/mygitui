import { expect, type Locator, type Page } from "@playwright/test";
import {
  createIssuesFixture,
  ISSUE_REMOTE,
  ISSUE_REPO_ROOT,
  issueMockCommands,
  type IssuesMockOptions,
} from "./issues-fixtures";
import {
  installTauriMock,
  openPanel,
  recentRepos,
  seedStorage,
  SESSION_KEY,
  type TauriMockHandle,
} from "./tauri-mock";

export const ISSUE_SYNC_KEY = `mygitui.issues.sync.${ISSUE_REPO_ROOT}`;
export const ISSUE_VIEW_KEY = `mygitui.issues.view.${ISSUE_REPO_ROOT}`;

/** Frontend-only durable envelope data (not an IPC wire shape). */
export interface IssueSyncState {
  newIssues: boolean;
  newComments: boolean;
  phase: string;
  issueCursor: string | null;
  commentCursor: string | null;
  seenIssueIds: string[];
  seenCommentIds: string[];
  baselineIssues: boolean;
  baselineComments: boolean;
  lastSuccessAt: string | null;
  lastError: string | null;
  outbox: Array<{
    id: string;
    kind: "issue" | "comment";
    issueId: string;
    issueNumber: number;
    commentId: string | null;
    title: string;
    actor: string;
    createdAt: string;
    detectedAt: string;
    url: string;
    readAt: string | null;
  }>;
}

export interface BootIssuesOptions {
  commands?: Record<string, unknown>;
  issues?: IssuesMockOptions;
}

/** Boots the normal app/repo shell with the deterministic Issues backend. */
export async function bootIssues(
  page: Page,
  options: BootIssuesOptions = {},
): Promise<TauriMockHandle> {
  if (process.env.E2E_ISSUES_DEBUG === "1") {
    page.on("pageerror", (error) => console.log("issues pageerror", error.stack));
    page.on("console", (message) => {
      if (message.type() === "error") console.log("issues console error", message.text());
    });
  }
  await seedStorage(page, {
    ...recentRepos([ISSUE_REPO_ROOT]),
    // The production Issues transport resolves the persisted GitHub remote;
    // seed the same root-keyed override instead of letting it fall back to
    // the ambiguous name "origin".
    [`mygitui.forge.remote.${ISSUE_REPO_ROOT}`]: JSON.stringify(ISSUE_REMOTE),
    // Restore the repo directly instead of driving the Overview's Open button.
    // This keeps Issues behavior independent of unrelated Overview navigation.
    [SESSION_KEY]: JSON.stringify({
      roots: [ISSUE_REPO_ROOT],
      activeRoot: ISSUE_REPO_ROOT,
    }),
  });
  const mock = await installTauriMock(page, {
    commands: { ...issueMockCommands(), ...(options.commands ?? {}) },
    issues: options.issues ?? createIssuesFixture(),
  });
  await page.goto("/");
  await expect(page.getByRole("tab", { name: "repo — /tmp/repo" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Switch branch — main", exact: true })).toBeVisible();
  return mock;
}

/**
 * Opens the center through the compact GitHub-panel entry or command palette.
 * Selector assumptions are intentionally centralized for the UI integration.
 */
export async function openIssues(
  page: Page,
  via: "sidebar" | "palette" | "event" = "event",
): Promise<void> {
  if (via === "event") {
    // Behavior specs enter through the same UI event as sidebar/palette. The
    // dedicated opening spec covers those two user-facing entry points.
    await page.evaluate(() => {
      document.dispatchEvent(
        new CustomEvent("issues-open", { detail: { repoId: "r1" } }),
      );
    });
  } else if (via === "palette") {
    await page.keyboard.press("Control+k");
    const palette = page.getByRole("dialog", { name: "Command palette" });
    await expect(palette).toBeVisible();
    await palette.getByRole("combobox", { name: "Search commands" }).fill("issues");
    await palette.getByRole("option", { name: /issues/i }).click();
  } else {
    await openPanel(page, "GitHub");
    await page
      .getByRole("complementary", { name: "GitHub", exact: true })
      .getByRole("button", { name: "Issues", exact: true })
      .click();
  }
  await expect(page.getByRole("textbox", { name: "Search issues", exact: true })).toBeVisible();
}

export function issuesCenter(page: Page): Locator {
  return page.locator(".issues-center, [data-issues-center]").first();
}

export function issueList(page: Page): Locator {
  return issuesCenter(page).getByRole("listbox", { name: "Issues", exact: true });
}

export function issueOption(page: Page, number: number): Locator {
  return issueList(page)
    .getByRole("option")
    .filter({ hasText: new RegExp(`#${number}(?:\\s|$)`) })
    .first();
}

export function issueDetail(page: Page): Locator {
  return issuesCenter(page).locator(".issue-detail, [data-issue-detail]").first();
}

export async function selectIssue(page: Page, number: number): Promise<void> {
  await issueOption(page, number).click();
  await expect(
    issuesCenter(page).getByRole("heading", { name: new RegExp(`#${number}\\b`) }),
  ).toBeVisible();
}

export function activityPopover(page: Page): Locator {
  return page.getByRole("dialog", { name: "Issue activity", exact: true });
}

export function subscriptionToggle(page: Page, name: "New issues" | "New comments"): Locator {
  return activityPopover(page)
    .getByRole("switch", { name, exact: true })
    .or(activityPopover(page).getByRole("checkbox", { name, exact: true }));
}

export async function enableSubscriptions(
  page: Page,
  mock: TauriMockHandle,
  options: { issues?: boolean; comments?: boolean } = { issues: true, comments: true },
): Promise<void> {
  await issuesCenter(page).getByRole("button", { name: /^Issue activity(?:\s|$)/ }).click();
  await expect(activityPopover(page)).toBeVisible();
  for (const [name, enabled] of [
    ["New issues", options.issues ?? false],
    ["New comments", options.comments ?? false],
  ] as const) {
    const toggle = subscriptionToggle(page, name);
    await expect(toggle).toBeVisible();
    if (enabled && !(await toggle.isChecked())) await toggle.check();
    if (!enabled && (await toggle.isChecked())) await toggle.uncheck();
  }
  await expect
    .poll(async () => (await mock.callsOf("issues_activity_since")).length)
    .toBeGreaterThan(0);
  await expect.poll(async () => (await readIssueSync(page))?.phase).toBe("ready");
  const trigger = issuesCenter(page).getByRole("button", { name: /^Issue activity(?:\s|$)/ });
  if ((await trigger.getAttribute("aria-expanded")) === "true") await trigger.click();
  await expect(activityPopover(page)).toHaveCount(0);
}

/** Selects a native select, Bits-style combobox, or filter menu option. */
export async function chooseOption(
  scope: Page | Locator,
  name: string | RegExp,
  optionName: string,
): Promise<void> {
  const root: Page = "page" in scope ? scope.page() : scope;
  const combobox = scope.getByRole("combobox", { name, exact: typeof name === "string" });
  if ((await combobox.count()) > 0) {
    if (await combobox.evaluate((element) => element.tagName === "SELECT")) {
      await combobox.selectOption({ label: optionName });
      return;
    }
    await combobox.click();
    await root.getByRole("option", { name: optionName, exact: true }).click();
    return;
  }
  await scope.getByRole("button", { name, exact: typeof name === "string" }).click();
  const checkbox = root
    .getByRole("checkbox", { name: optionName, exact: true })
    .or(root.getByRole("menuitemcheckbox", { name: optionName, exact: true }));
  if ((await checkbox.count()) > 0) {
    await checkbox.check();
    await root.keyboard.press("Escape");
    return;
  }
  await root.getByRole("option", { name: optionName, exact: true }).click();
}

export async function readIssueSync(page: Page): Promise<IssueSyncState | null> {
  return page.evaluate((key) => {
    const raw = localStorage.getItem(key);
    if (raw === null) return null;
    try {
      return (JSON.parse(raw) as { data?: IssueSyncState }).data ?? null;
    } catch {
      return null;
    }
  }, ISSUE_SYNC_KEY);
}

export async function readIssueViewPrefs(page: Page): Promise<{
  query?: Record<string, unknown>;
  splitRatio?: number;
} | null> {
  return page.evaluate((key) => {
    const raw = localStorage.getItem(key);
    if (raw === null) return null;
    try {
      return (JSON.parse(raw) as { data?: { query?: Record<string, unknown>; splitRatio?: number } }).data ?? null;
    } catch {
      return null;
    }
  }, ISSUE_VIEW_KEY);
}

/**
 * Playwright cannot truly minimize a headless page. Overriding the standard
 * visibility getter and dispatching the same events is deterministic and is
 * exactly what the subscription/scheduler visibility sources observe.
 */
export async function setPageHidden(page: Page, hidden: boolean): Promise<void> {
  await page.evaluate((isHidden) => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => (isHidden ? "hidden" : "visible"),
    });
    document.dispatchEvent(new Event("visibilitychange"));
    if (!isHidden) {
      document.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("focus"));
    }
  }, hidden);
}

export function activityToast(page: Page): Locator {
  return page.locator(".toaster .toast").filter({
    hasText: /\b(?:issue|comment)s?\b.* in acme\/repo/,
  });
}
