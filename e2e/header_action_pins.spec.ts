/**
 * Repo-header action pins (e2e): every "More repository actions" entry —
 * the two header-owned submenus (Layout preset, Open with) and RepoView's
 * commands — can be pinned to the header bar. A pinned command renders as
 * an always-visible icon button that runs the same action; a pinned
 * submenu opens its picker (Open with as a split button: the main segment
 * launches the last-used app, the chevron opens the picker). Right-click
 * on a pinned button unpins it; the pin list persists across reloads
 * (repoActionPins, `mygitui.repoActionPins`).
 */

import { expect, test, type Page } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  linearLog,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

async function bootRepo(page: Page): Promise<void> {
  const { commits, rows } = linearLog(3);
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, { commands: historyCommands({ commits, rows }) });
  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();
}

/** Opens the header's ⋯ menu (fresh app detection per open is mocked). */
async function openMoreMenu(page: Page): Promise<void> {
  await page.getByRole("button", { name: "More repository actions" }).click();
  await expect(page.getByRole("menuitem", { name: "Layout preset" })).toBeVisible();
}

async function pinCommand(page: Page, label: string): Promise<void> {
  await openMoreMenu(page);
  await page.getByRole("button", { name: `Pin ${label} to the bar` }).click();
  // The toggle keeps the menu open (state stays visible); close it.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
}

test("pinning a command puts it on the bar, working and unpinnable", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await bootRepo(page);

  const barButton = page
    .locator(".repo-header")
    .getByRole("button", { name: "Clean untracked files…" });
  await expect(barButton).toHaveCount(0);

  await pinCommand(page, "Clean untracked files…");
  await expect(barButton).toBeVisible();

  // The pinned button runs the same command: the Clean dialog opens.
  await barButton.click();
  await expect(page.getByRole("dialog", { name: "Clean untracked paths" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Clean untracked paths" })).toHaveCount(0);

  // Right-click unpins (the railPins idiom).
  await barButton.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Unpin from bar" }).click();
  await expect(barButton).toHaveCount(0);

  expect(tracked.errors).toEqual([]);
});

test("pinning the Layout preset submenu opens its picker from the bar", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await bootRepo(page);

  const barButton = page
    .locator(".repo-header")
    .getByRole("button", { name: "Layout preset" });
  await expect(barButton).toHaveCount(0);

  await openMoreMenu(page);
  await page.getByRole("menuitem", { name: "Layout preset" }).click();
  await page
    .getByRole("menuitem", { name: "Pin Layout preset to bar" })
    .click();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);

  await expect(barButton).toBeVisible();

  // The pinned button opens the same preset picker as the submenu.
  await barButton.click();
  const group = page.getByRole("group", { name: "" }).or(page.getByRole("menu"));
  await expect(group.first()).toBeVisible();

  expect(tracked.errors).toEqual([]);
});

test("pins persist across reloads and the menu reflects pin state", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await bootRepo(page);

  await pinCommand(page, "Recent repositories");

  // Reload: the pin list restores from storage and the button returns.
  await page.reload();
  await expect(page.locator(".history .row").first()).toBeVisible();
  const barButton = page
    .locator(".repo-header")
    .getByRole("button", { name: "Recent repositories" });
  await expect(barButton).toBeVisible();

  // The menu row's toggle reflects the pinned state.
  await openMoreMenu(page);
  await expect(
    page.getByRole("button", { name: "Unpin Recent repositories from the bar" }),
  ).toBeVisible();

  expect(tracked.errors).toEqual([]);
});

test("pinning Open with gives a split button: last-used launch + picker", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(3);
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      ...historyCommands({ commits, rows }),
      detect_editors: [
        { id: "code", name: "Visual Studio Code" },
        { id: "explorer", name: "Files" },
      ],
    },
  });
  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();

  // Pin "Open with" from the overflow submenu.
  await openMoreMenu(page);
  await page.getByRole("menuitem", { name: "Open with" }).click();
  await page.getByRole("menuitem", { name: "Pin Open with to bar" }).click();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);

  // The pinned entry is a group: the main segment carries the last-used
  // app (default: the platform file manager), the chevron opens the picker.
  const group = page
    .locator(".repo-header")
    .getByRole("group", { name: "Open repository with an app" });
  const main = group.getByRole("button", { name: "Open with Files (last used)" });
  await expect(main).toBeVisible();

  // Main segment launches the last-used app in one click.
  await main.click();
  await expect
    .poll(async () => (await mock.callsOf("open_with")).length, { timeout: 5_000 })
    .toBe(1);
  expect((await mock.callsOf("open_with"))[0]).toMatchObject({
    app_id: "explorer",
    repo_root: "/tmp/repo",
  });

  // Chevron opens the picker (fresh detection per open) and picking another
  // app launches it — and becomes the new last-used on the main segment.
  await group
    .getByRole("button", { name: "Choose an app to open the repository with" })
    .click();
  const code = page.getByRole("menuitem", { name: "Visual Studio Code" });
  await expect(code).toBeVisible();
  await code.click();
  await expect
    .poll(async () => (await mock.callsOf("open_with")).length, { timeout: 5_000 })
    .toBe(2);
  expect((await mock.callsOf("open_with"))[1]).toMatchObject({ app_id: "code" });
  await expect(
    group.getByRole("button", { name: "Open with Visual Studio Code (last used)" }),
  ).toBeVisible();

  expect(tracked.errors).toEqual([]);
});
