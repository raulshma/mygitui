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
 * action runner, opening the run log in the editor.
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
  const runView = page.locator(".action-run-view");
  await expect(runView).toBeVisible();
  await expect(runView.locator(".run-title")).toHaveText("web: test");
  await expect(runView.locator(".command")).toHaveText("npm test");
  await expect(runView.locator(".status-chip")).toHaveText("running");

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});

test("action run popout renders the stashed snapshot and follows live output", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  const snapshot = {
    run_id: "act-1",
    repo_id: "r1",
    name: "Lint",
    command: "npm run lint",
    lines: ["first line"],
    dropped: 0,
    done: false,
    exit_code: null,
    order: 3,
    status: "running",
  };
  await seedStorage(page, {
    "mygitui.action-run-popout.act-1": JSON.stringify(snapshot),
  });
  const mock = await installTauriMock(page);
  await page.goto("/?panel=actionrun&repo=r1&actionRun=act-1");

  const view = page.locator(".action-run-view");
  await expect(view).toBeVisible();
  await expect(view.locator(".run-title")).toHaveText("Lint");
  await expect(view.locator(".command")).toHaveText("npm run lint");
  await expect(view.locator(".status-chip")).toHaveText("running");
  await expect(view.locator(".log-body")).toContainText("first line");

  await mock.emit("action-output", {
    run_id: "act-1",
    repo_id: "r1",
    name: "Lint",
    line: "second line",
    done: true,
    exit_code: 0,
  });
  await expect(view.locator(".log-body")).toContainText("second line");
  await expect(view.locator(".status-chip")).toHaveText("exit 0");
  await expect(view.locator(".log-state")).toHaveText("Finished");

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
