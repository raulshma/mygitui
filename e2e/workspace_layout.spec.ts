import { expect, test } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  linearLog,
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
  await expect(page.getByText("Nothing open")).toBeVisible();

  // Switching views keeps Status mounted (its draft/scroll state survives).
  await activity.getByRole("button", { name: "Branches" }).click();
  await expect(page.locator(".panel-title")).toHaveText("Branches");
  await expect(commitForm).toBeHidden();
  await expect(page.locator(".changes-panel")).toHaveCount(1);

  // A panel that is not in the layout yet is added on first click.
  await activity.getByRole("button", { name: "Repo", exact: true }).click();
  await expect(page.locator(".panel-title")).toHaveText("Repo");
  await expect(commitForm).toBeHidden();

  await activity.getByRole("button", { name: "Status" }).click();
  await expect(commitForm).toBeVisible();

  // Clicking a working-copy change opens its diff in the center editor.
  await page.locator(".changes-panel .row").first().click();
  await expect(page.locator(".editor .file-editor .epath")).toHaveText(
    "src/app.ts",
  );
  await expect(editor).toContainText("working copy");
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
  // expands and keeps the commit detail.
  const historyWidth = (await editor.boundingBox())!.width;
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
