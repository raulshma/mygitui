/**
 * Custom actions persistence e2e (regression):
 * a repo-scoped def added through the panel's add form must still be listed
 * after the app is relaunched — even though the backend mints a fresh
 * repo_id per process (repo::next_repo_id: timestamp + pid + counter), so
 * the def's repo binding must not be the volatile repo_id. A same-context
 * `page.reload()` replays the SPA-level relaunch path (module init →
 * parseDefs → session restore → panel render); the mock presents a
 * different repo_id after the reload, exactly like the real backend does.
 * The Tauri IPC is mocked (e2e/tauri-mock.ts).
 */

import { expect, test } from "@playwright/test";
import {
  installTauriMock,
  openPanel,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

test("repo-scoped custom action survives a relaunch with a fresh repo id", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  // Launch 1: the (mocked) backend hands out repo_id "r1".
  await installTauriMock(page);
  await openRecentRepo(page);
  await openPanel(page, "Actions");

  // Add one repo-scoped action through the real form (the form's default
  // scope is "this repo").
  await page.getByLabel("New action name").fill("Persist check");
  await page.getByLabel("New action command").fill("echo persisted");
  await page.locator(".add-form .btn.add").click();

  const row = page.locator(".def-list .def-row");
  await expect(row).toHaveCount(1);
  const nameInput = row.getByRole("textbox", { name: "Action name", exact: true });
  const commandInput = row.getByRole("textbox", {
    name: "Action command",
    exact: true,
  });
  await expect(nameInput).toHaveValue("Persist check");
  await expect(commandInput).toHaveValue("echo persisted");

  // Relaunch: a fresh backend process mints "r2" for the same repo path.
  await page.evaluate(() =>
    sessionStorage.setItem("e2e-repo-id", "r2"),
  );
  await page.reload();

  // The app restores the open repo and the last active panel (Actions was
  // active at shutdown), so the persisted defs must be visible without any
  // interaction — clicking the panel button again would toggle it closed.
  await expect(page.getByText("/tmp/repo").first()).toBeVisible();
  const nameBox = page.getByRole("textbox", { name: "Action name", exact: true });
  await expect(nameBox).toBeVisible();
  await expect(nameBox).toHaveValue("Persist check");
  await expect(
    page.getByRole("textbox", { name: "Action command", exact: true }),
  ).toHaveValue("echo persisted");
  await expect(page.locator(".def-list .def-row")).toHaveCount(1);

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
