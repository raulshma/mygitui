/**
 * Playwright smoke (M12, contracts.md quality gates): SPA-level tests with
 * a mocked `window.__TAURI_INTERNALS__` (e2e/tauri-mock.ts) —
 *   1. app boots to the home / recent-repositories screen without console
 *      errors,
 *   2. open-repo flow: a pre-seeded recent repo opens as a tab, the status
 *      panel renders the 2 mocked entries, staging one records a `stage`
 *      invoke, and committing records a `commit` invoke.
 *
 * Assertions stay on visible DOM plus the mock's recorded invoke log.
 */

import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  trackErrors,
  type TauriMockHandle,
} from "./tauri-mock";
import { expectFullyVisible } from "./visible";

const RECENT_REPOS_KEY = "mygitui.recent-repos";

/** Boot the app with the Tauri mock (and optional pre-boot page setup). */
async function boot(
  page: Page,
  seedStorage: Record<string, string> = {},
): Promise<TauriMockHandle> {
  await page.addInitScript((seed) => {
    for (const [key, value] of Object.entries(seed)) {
      window.localStorage.setItem(key, value);
    }
  }, seedStorage);
  return installTauriMock(page);
}

test("app boots to the home/recent screen without console errors", async ({
  page,
}, testInfo) => {
  const mock = await boot(page);
  const tracked = trackErrors(page);

  await page.goto("/");

  // Stable landmark: the home screen's recent-repositories heading.
  await expect(
    page.getByRole("heading", { name: /Repositories/i }),
  ).toBeVisible();

  // The Tauri mock was consulted for first-launch argv (empty here).
  await expect
    .poll(() => mock.callsOf("cli_args_initial"))
    .toHaveLength(1);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("open-repo flow: recent card → status entries → stage → commit", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await page.goto("/");

  // 1. The seeded recent-repo card is listed on the home screen; click it.
  const repoCard = page.getByRole("button", { name: /repo\s*\/tmp\/repo/ });
  await expect(repoCard).toBeVisible();
  await repoCard.click();

  // 2. The tab opened: repo name + status entries render (rows are
  //    treeitems labelled with the full path; dir/base are split nodes).
  await expect(page.getByRole("treeitem", { name: "src/app.ts" })).toBeVisible();
  await expect(page.getByRole("treeitem", { name: "notes.md" })).toBeVisible();

  // The status came through the mocked command chain.
  await expect
    .poll(async () => (await mock.callsOf("repo_status")).length)
    .toBeGreaterThan(0);

  // 3. Stage the modified file via the hover action on its row.
  const statusRow = page.getByRole("treeitem", { name: "src/app.ts" });
  await statusRow.hover();
  await page.getByRole("button", { name: "Stage src/app.ts" }).click();
  const stageCalls = await mock.callsOf("stage");
  expect(stageCalls).toHaveLength(1);
  expect(stageCalls[0]).toMatchObject({
    repo_id: "r1",
    request: { unstage: false, targets: [{ file: "src/app.ts" }] },
  });

  // 4. Commit: type a message and submit the commit bar form.
  await page.locator("#commit-message").fill("test: smoke commit");
  await page.getByRole("button", { name: "Commit", exact: true }).click();

  const commitCalls = await mock.callsOf("commit");
  expect(commitCalls).toHaveLength(1);
  expect(commitCalls[0]).toMatchObject({
    repo_id: "r1",
    options: { message: "test: smoke commit" },
  });

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("gitignore menu escapes the status list clipping", async ({ page }, testInfo) => {
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();

  const row = page.getByRole("treeitem", { name: "notes.md" });
  await expect(row).toBeVisible();
  await row.hover();
  await page.getByRole("button", { name: "Ignore notes.md" }).click();

  const menu = page.getByRole("menu", { name: "Ignore pattern choices" });
  await expect(menu).toBeVisible();
  await expectFullyVisible(page, menu);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("files panel: browse tree, open a file in the workspace", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();

  // 1. Switch the primary sidebar to the Files panel (activity bar).
  await page.getByRole("button", { name: "Files", exact: true }).click();

  // 2. Root listing renders (dirs first, then files).
  const readmeRow = page.getByRole("treeitem", { name: /README\.md/ });
  await expect(readmeRow).toBeVisible({ timeout: 15_000 });

  // 3. Expand the src dir (lazy list_dir invoke).
  await page
    .getByRole("button", { name: "Expand src" })
    .click();
  const appRow = page.getByRole("treeitem", { name: /^app\.ts/ });
  await expect(appRow).toBeVisible({ timeout: 15_000 });
  await expect
    .poll(async () => (await mockCalls(page, "repo_list_dir")).length)
    .toBeGreaterThan(0);

  // 4. Open a file: the center workspace shows a tab + the CodeMirror doc.
  await appRow.click();
  const tab = page.getByRole("tab", { name: "app.ts" });
  // First open transforms the lazy CodeMirror chunk on the dev server.
  await expect(tab).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".cm-content")).toContainText(
    "export const app = 1;",
    { timeout: 30_000 },
  );
  await expect
    .poll(async () => (await mockCalls(page, "repo_read_file")).length)
    .toBeGreaterThan(0);

  // 4b. A second file opens alongside the first: its stat round-trip must
  //     swap the "Reading…" spinner for an editor reactively (regression
  //     guard: the meta swap used to mutate the raw, non-proxied object).
  await readmeRow.click();
  await expect(page.getByRole("tab", { name: "README.md" })).toBeVisible({
    timeout: 30_000,
  });
  // Both panes stay mounted, so both editors exist in the DOM.
  await expect(page.locator(".cm-content")).toHaveCount(2, { timeout: 30_000 });

  // 5. Diff cross-link routes through the status entries. README (now
  //    active) has no status entry, so reactivate the modified file first.
  await page.getByRole("tab", { name: "app.ts" }).click();
  await page.getByRole("button", { name: "Diff", exact: true }).click();
  await expect(page.getByRole("button", { name: "Edit", exact: true })).toBeVisible();

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("quick open (Ctrl+P): fuzzy pick opens the file", async ({
  page,
}, testInfo) => {
  // First open transforms the lazy QuickOpen chunk on the dev server.
  test.setTimeout(90_000);
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();

  // Press the keybind only once the repo is fully open: under load a fixed
  // wait could fire while the workspace is still initializing, and the
  // quick-open event would be swallowed.
  await expect(page.getByRole("treeitem", { name: "src/app.ts" })).toBeVisible({
    timeout: 15_000,
  });

  await page.keyboard.press("Control+p");
  const dialog = page.getByRole("dialog", { name: "Go to file" });
  // First open transforms the lazy QuickOpen chunk on the dev server.
  await expect(dialog).toBeVisible({ timeout: 30_000 });

  await page.getByRole("textbox", { name: "Go to file" }).fill("app");
  const option = page.getByRole("option", { name: /app\.ts/ });
  await expect(option).toBeVisible({ timeout: 15_000 });
  await page.keyboard.press("Enter");

  // The pick opened the file in the center workspace.
  await expect(page.getByRole("tab", { name: "app.ts" })).toBeVisible({
    timeout: 30_000,
  });
  await expect
    .poll(async () => (await mockCalls(page, "repo_file_index")).length)
    .toBeGreaterThan(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("markdown preview: split default, typing flows through, mode persists", async ({
  page,
}, testInfo) => {
  // Reload + reopen + first-time lazy-chunk transforms need headroom.
  test.setTimeout(90_000);
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();

  // Open the seeded markdown file from the Files panel.
  await page.getByRole("button", { name: "Files", exact: true }).click();
  const notesRow = page.getByRole("treeitem", { name: /notes\.md/ });
  await expect(notesRow).toBeVisible({ timeout: 15_000 });
  await notesRow.click();
  await expect(page.getByRole("tab", { name: "notes.md" })).toBeVisible({
    timeout: 30_000,
  });

  // Split is the default: the rendered preview sits beside the editor.
  const preview = page.locator(".md");
  // First open transforms the lazy markdown chunk on the dev server.
  await expect(preview).toBeVisible({ timeout: 30_000 });
  await expect(preview.locator("h1")).toHaveText(/notes/);

  // Typing in the editor flows into the preview after the debounce window.
  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("\n\n## added section");
  await expect(preview.locator("h2")).toHaveText(/added section/, {
    timeout: 5_000,
  });

  // Switch to preview-only: the editor column hides (CM stays mounted).
  await page.getByRole("button", { name: "Preview", exact: true }).click();
  await expect(page.locator(".cm-content")).toBeHidden();
  await expect(preview).toBeVisible();

  // The mode choice is persisted (editorPrefs → localStorage).
  expect(
    await page.evaluate(() => localStorage.getItem("mygitui.files.md-view")),
  ).toBe("preview");

  // Back to source-only: the preview unmounts entirely.
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(page.locator(".cm-content")).toBeVisible();
  await expect(page.locator(".md")).toHaveCount(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

/** Recorded invokes for one command (helper over the page-side mock). */
async function mockCalls(
  page: Page,
  command: string,
): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    (cmd) =>
      window.__TAURI_MOCK__.calls
        .filter((c) => c.cmd === cmd)
        .map((c) => c.args),
    command,
  );
}
