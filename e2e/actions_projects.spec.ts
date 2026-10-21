import { expect, test } from "@playwright/test";
import {
  installTauriMock,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

/**
 * Actions panel: detected projects render their scripts (label + exact
 * command) and Run feeds a synthetic action definition through the existing
 * action runner (auto-expanding the output).
 */
test("detected project scripts are visible and runnable", async ({ page }) => {
  const tracked = trackErrors(page);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      project_detect: [
        {
          kind: "node",
          name: "web",
          path: ".",
          detected_from: ["package.json"],
          scripts: [
            {
              id: "node:.:test",
              label: "test",
              command: "npm test",
              kind: "test",
              watch: false,
            },
            {
              id: "node:.:dev",
              label: "dev",
              command: "npm run dev",
              kind: "run",
              watch: true,
            },
          ],
        },
      ],
      action_run: "run-1",
    },
  });

  await openRecentRepo(page);
  await page
    .locator(".activity-bar")
    .getByRole("button", { name: "Actions" })
    .click();

  await expect(page.locator(".panel-title")).toHaveText("Actions");
  const panelPadding = await page.locator(".actions-panel").evaluate((el) => {
    const style = getComputedStyle(el);
    return {
      left: Number.parseFloat(style.paddingLeft),
      right: Number.parseFloat(style.paddingRight),
    };
  });
  expect(panelPadding.left).toBeGreaterThan(0);
  expect(panelPadding.right).toBeGreaterThan(0);
  await expect(page.getByText("Detected projects")).toBeVisible();
  const card = page.locator(".project-card").first();
  await expect(card).toContainText("JS/TS");
  await expect(card).toContainText("web");
  await expect(card).toContainText("package.json");
  await expect(card.locator(".script-command")).toHaveText([
    "npm test",
    "npm run dev",
  ]);
  await expect(card.locator(".script-kind.watch")).toHaveText("watch");

  await card.locator(".script-run").first().click();
  await expect
    .poll(async () => (await mock.callsOf("action_run")).length)
    .toBe(1);
  expect((await mock.callsOf("action_run"))[0]).toMatchObject({
    repo_id: "r1",
    name: "web: test",
    command: "npm test",
  });
  await expect(page.locator(".run-row.open")).toContainText("web: test");

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
