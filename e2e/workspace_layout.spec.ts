import { expect, test, type Page } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  linearLog,
  openPanel,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

/**
 * VSCode-like workspace: the activity bar switches the primary sidebar, the
 * center editor shows either a file diff or a commit's detail, and both
 * sidebars collapse to persistent rails without unmounting their panels or
 * losing the editor.
 */
test("activity bar, sidebars and center editor share the workspace", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(3);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, {
    commands: historyCommands({ commits, rows }),
  });

  await openRecentRepo(page);

  const activity = page.locator(".activity-bar");
  const commitForm = page.getByRole("form", { name: "Create commit" });
  const editor = page.locator(".editor");

  // The activity rail replaces the horizontal tab strip: Status is active.
  await expect(activity).toBeVisible();
  await expect(
    activity.getByRole("button", { name: "Status" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".panel-title")).toHaveText("Status");
  await expect(commitForm).toBeVisible();
  await expect(page.locator(".changes-panel .row").first()).toBeVisible();
  await expect(page.locator(".editor .file-editor .epath")).toHaveText("src/app.ts");

  // Switching views keeps Status mounted (its draft/scroll state survives).
  await activity.getByRole("button", { name: "Branches" }).click();
  await expect(page.locator(".panel-title")).toHaveText("Branches");
  await expect(commitForm).toBeHidden();
  await expect(page.locator(".changes-panel")).toHaveCount(1);

  // A panel that is not in the layout yet is added on first click.
  await openPanel(page, "Repo");
  await expect(page.locator(".panel-title")).toHaveText("Repo");
  await expect(commitForm).toBeHidden();

  await activity.getByRole("button", { name: "Status" }).click();
  await expect(commitForm).toBeVisible();

  // Clicking a working-copy change opens its diff in the center editor.
  await page.locator(".changes-panel .row").first().click();
  await expect(page.locator(".editor .file-editor .epath")).toHaveText(
    "src/app.ts",
  );
  await expect(editor).toContainText("unstaged");
  await expect(editor.locator(".detail")).toHaveCount(0);

  // Clicking a commit replaces that diff with the commit detail — in the
  // same center pane, not inside the history sidebar.
  await page.locator("#commit-row-0").click();
  await expect(editor.locator(".detail")).toBeVisible();
  await expect(editor.locator(".dsummary").first()).toHaveText(
    "commit message 0",
  );
  await expect(editor.locator(".file-editor")).toHaveCount(0);
  await expect(page.locator(".history .detail")).toHaveCount(0);

  // Collapsing the primary sidebar leaves the activity rail in place, keeps
  // the panels mounted, and gives the center editor the freed width.
  const sidebarWidth = (await editor.boundingBox())!.width;
  await activity.getByRole("button", { name: "Hide sidebar" }).click();
  await expect(page.locator(".panel-title")).toBeHidden();
  await expect(page.locator(".changes-panel")).toBeHidden();
  await expect(activity).toBeVisible();
  await expect
    .poll(async () => (await editor.boundingBox())?.width ?? 0)
    .toBeGreaterThan(sidebarWidth);

  // The active activity icon re-expands the sidebar without losing the view.
  await activity.getByRole("button", { name: "Status" }).click();
  await expect(page.locator(".panel-title")).toHaveText("Status");
  await expect(commitForm).toBeVisible();
  await expect(editor.locator(".dsummary").first()).toHaveText(
    "commit message 0",
  );

  // Hiding history keeps it mounted behind the right-edge rail; the editor
  // expands and keeps the commit detail. The rail eats width from the tree,
  // but only the rightmost pane may pay for it - the primary sidebar (and
  // every other left-hand pane) must keep its exact size.
  const historyWidth = (await editor.boundingBox())!.width;
  const sidebarWidthBeforeRail = (await page
    .locator(".changes-panel")
    .boundingBox())!.width;
  await page
    .getByRole("button", { name: "Hide history", exact: true })
    .click();
  await expect(page.locator(".history")).toHaveCount(1);
  await expect(page.locator(".history")).toBeHidden();
  const historyRail = page.getByRole("button", {
    name: "Show history sidebar",
  });
  await expect(historyRail).toBeVisible();
  await expect
    .poll(async () => (await editor.boundingBox())?.width ?? 0)
    .toBeGreaterThan(historyWidth);
  await expect
    .poll(async () =>
      Math.abs(
        (await page.locator(".changes-panel").boundingBox())!.width -
          sidebarWidthBeforeRail,
      ),
    )
    .toBeLessThan(0.5);
  await expect(editor.locator(".dsummary").first()).toHaveText(
    "commit message 0",
  );

  // The rail re-opens the same mounted history (selection intact) without
  // stealing the editor back.
  await historyRail.click();
  await expect(page.locator(".history")).toBeVisible();
  await expect(editor.locator(".dsummary").first()).toHaveText(
    "commit message 0",
  );
  await expect(page.locator(".history .row.selected")).toHaveCount(1);

  // Both sidebars collapsed: the editor owns the workspace, and both panels
  // remain mounted (the guard only forbids hiding the last remaining pane).
  await activity.getByRole("button", { name: "Hide sidebar" }).click();
  await page
    .getByRole("button", { name: "Hide history", exact: true })
    .click();
  await expect(activity).toBeVisible();
  await expect(page.locator(".panel-title")).toBeHidden();
  await expect(page.locator(".changes-panel")).toHaveCount(1);
  await expect(page.locator(".changes-panel")).toBeHidden();
  await expect(page.locator(".history")).toHaveCount(1);
  await expect(page.locator(".history")).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Show history sidebar" }),
  ).toBeVisible();
  await expect(editor.locator(".dsummary").first()).toHaveText(
    "commit message 0",
  );

  expect(tracked.errors).toEqual([]);
});

const WORKFLOW_RUN = {
  database_id: 101,
  number: 42,
  name: "CI",
  display_title: "Editor safety checks",
  event: "push",
  head_branch: "main",
  head_sha: "0123456789abcdef0123456789abcdef01234567",
  status: "completed",
  conclusion: "success",
  url: "https://github.com/acme/repo/actions/runs/101",
  created_at: "2026-10-20T08:00:00Z",
  updated_at: "2026-10-20T08:04:00Z",
  started_at: "2026-10-20T08:00:00Z",
  attempt: 1,
  workflow_database_id: 9001,
};

async function openTextEditor(page: Page) {
  const tracked = trackErrors(page);
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      ...historyCommands(linearLog(3)),
      forge_status: { available: true, version: "2.95.0", authed: true },
      forge_context: {
        owner: "acme", repo: "repo", branch: "main",
        remote_url: "git@github.com:acme/repo.git",
      },
      workflow_runs: [WORKFLOW_RUN],
      workflow_run_detail: { run: WORKFLOW_RUN, jobs: [] },
      workflow_run_logs: "All checks passed\n",
    },
  });
  await openRecentRepo(page);
  // FileWorkspace and CodeMirror must remain lazy until a file is opened.
  await expect(page.locator(".editor .file-editor .epath")).toHaveText("src/app.ts");
  await expect(page.locator(".workspace")).toHaveCount(0);
  await openPanel(page, "Files");
  await page.getByRole("button", { name: "Expand src" }).click();
  await page.getByRole("treeitem", { name: /^app\.ts/ }).click();
  const content = page.locator(".cm-content");
  await expect(content).toHaveText("export const app = 1;", { timeout: 30_000 });
  return { mock, content, tracked };
}

for (const destination of ["diff", "commit", "workflow"] as const) {
  test(`dirty CodeMirror text and undo survive the ${destination} view`, async ({ page }) => {
    const { mock, content, tracked } = await openTextEditor(page);
    await content.click();
    await page.keyboard.press("Control+End");
    await page.keyboard.insertText("// unsaved editor change");
    await expect(content).toContainText("// unsaved editor change");
    await expect(page.locator(".workspace .dirty")).toBeVisible();

    if (destination === "diff") {
      await page.getByRole("button", { name: "Diff", exact: true }).click();
      await expect(page.locator(".editor .file-editor .epath")).toHaveText("src/app.ts");
    } else if (destination === "commit") {
      await page.locator("#commit-row-0").click();
      await expect(page.locator(".editor .dsummary").first()).toHaveText("commit message 0");
    } else {
      await openPanel(page, "GitHub");
      await page.getByRole("tab", { name: "Actions", exact: true }).click();
      await page.locator(".wf-run-main").click();
      await expect(page.locator(".run-view")).toContainText("Editor safety checks");
    }
    await expect(content).toBeHidden();
    // Global save must not act on the retained, hidden editor.
    await page.keyboard.press("Control+s");

    // Return through the natural affordances: the diff header's Edit button,
    // or the file tree (switching the sidebar back to Files when needed).
    if (destination === "diff") {
      await page.getByRole("button", { name: "Edit", exact: true }).click();
    } else {
      if (destination === "workflow") await openPanel(page, "Files");
      await page.getByRole("treeitem", { name: /^app\.ts/ }).click();
    }
    await expect(content).toBeVisible();
    await expect(content).toContainText("// unsaved editor change");
    await expect(page.locator(".workspace .dirty")).toBeVisible();
    expect(await mock.callsOf("repo_write_file")).toEqual([]);
    // No replacement editor read from disk; real undo still reaches the original doc.
    expect(await mock.callsOf("repo_read_file")).toHaveLength(1);
    await content.click();
    await page.keyboard.press("Control+z");
    await expect(content).toHaveText("export const app = 1;");
    await expect(page.locator(".editor .detail, .editor .run-view, .editor .file-editor")).toHaveCount(0);
    expect(tracked.errors).toEqual([]);
  });
}
