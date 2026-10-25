import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import {
  installTauriMock,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

const FAILED_RUN = {
  database_id: 101,
  number: 42,
  name: "CI",
  display_title: "Fix flaky workflow test",
  event: "pull_request",
  head_branch: "main",
  head_sha: "0123456789abcdef0123456789abcdef01234567",
  status: "completed",
  conclusion: "failure",
  url: "https://github.com/acme/repo/actions/runs/101",
  created_at: "2026-10-20T08:00:00Z",
  updated_at: "2026-10-20T08:04:10Z",
  started_at: "2026-10-20T08:00:30Z",
  attempt: 2,
  workflow_database_id: 9001,
};

const RUNNING_RUN = {
  database_id: 102,
  number: 43,
  name: "Nightly",
  display_title: "Nightly build",
  event: "schedule",
  head_branch: "main",
  head_sha: "1123456789abcdef0123456789abcdef01234567",
  status: "in_progress",
  conclusion: "",
  url: "https://github.com/acme/repo/actions/runs/102",
  created_at: "2026-10-20T09:00:00Z",
  updated_at: "2026-10-20T09:00:30Z",
  started_at: "2026-10-20T09:00:30Z",
  attempt: 1,
  workflow_database_id: 9002,
};

const SUCCESS_RUN = {
  database_id: 103,
  number: 41,
  name: "Deploy",
  display_title: "Release v1.2.3",
  event: "push",
  head_branch: "release/1.2",
  head_sha: "2123456789abcdef0123456789abcdef01234567",
  status: "completed",
  conclusion: "success",
  url: "https://github.com/acme/repo/actions/runs/103",
  created_at: "2026-10-19T18:00:00Z",
  updated_at: "2026-10-19T18:02:00Z",
  started_at: "2026-10-19T18:00:10Z",
  attempt: 1,
  workflow_database_id: 9003,
};

const FAILED_DETAIL = {
  run: FAILED_RUN,
  jobs: [
    {
      database_id: 555,
      name: "build",
      status: "completed",
      conclusion: "failure",
      started_at: "2026-10-20T08:00:30Z",
      completed_at: "2026-10-20T08:04:10Z",
      url: "https://github.com/acme/repo/actions/runs/101/job/555",
      steps: [
        {
          number: 1,
          name: "Checkout",
          status: "completed",
          conclusion: "success",
          started_at: "2026-10-20T08:00:30Z",
          completed_at: "2026-10-20T08:00:35Z",
        },
        {
          number: 2,
          name: "npm test",
          status: "completed",
          conclusion: "failure",
          started_at: "2026-10-20T08:00:35Z",
          completed_at: "2026-10-20T08:04:10Z",
        },
      ],
    },
  ],
};

const WORKFLOWS = [
  {
    id: 9001,
    name: "CI",
    path: ".github/workflows/ci.yml",
    state: "active",
  },
  {
    id: 9002,
    name: "Nightly",
    path: ".github/workflows/nightly.yml",
    state: "active",
  },
  {
    id: 9003,
    name: "Old deploy",
    path: ".github/workflows/old-deploy.yml",
    state: "disabled_manually",
  },
];

/** Boots the app with gh ready and the Actions fixtures installed. */
async function setup(page: Page) {
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  return installTauriMock(page, {
    commands: {
      forge_status: { available: true, version: "2.95.0", authed: true },
      forge_context: {
        owner: "acme",
        repo: "repo",
        branch: "main",
        remote_url: "git@github.com:acme/repo.git",
      },
      pr_list: [],
      pr_checks: [],
      workflow_runs: [FAILED_RUN, RUNNING_RUN, SUCCESS_RUN],
      workflow_list: WORKFLOWS,
      workflow_run_detail: FAILED_DETAIL,
      workflow_run_logs: "build\tCheckout\tok\nbuild\tnpm test\tFAILED\nnpm ERR! test failed\n",
      workflow_run_cancel: "cancelled run 102",
      workflow_run_rerun: "rerun requested",
      workflow_dispatch: "dispatched CI",
    },
  });
}

async function openActionsTab(page: Page): Promise<void> {
  await openRecentRepo(page);
  await page
    .locator(".activity-bar")
    .getByRole("button", { name: "GitHub" })
    .click();
  await expect(page.locator(".panel-title")).toHaveText("GitHub");
  await page.getByRole("tab", { name: "Actions" }).click();
  await expect(page.locator(".wf-runs")).toBeVisible();
}

test("Actions tab lists runs, expands jobs/steps and opens logs", async ({ page }) => {
  const tracked = trackErrors(page);
  const mock = await setup(page);
  await openActionsTab(page);

  const runs = page.locator(".wf-run");
  await expect(runs).toHaveCount(3);
  await expect(runs.nth(0)).toContainText("CI");
  await expect(runs.nth(0)).toContainText("Fix flaky workflow test");
  await expect(runs.nth(0)).toContainText("#42");
  await expect(runs.nth(0)).toContainText("attempt 2");
  await expect(runs.nth(0)).toContainText("Failed");
  await expect(runs.nth(1)).toContainText("Nightly");
  await expect(runs.nth(1)).toContainText("Running");
  await expect(runs.nth(2)).toContainText("Release v1.2.3");
  await expect(runs.nth(2)).toContainText("Success");

  // Only active runs offer cancel; only completed runs offer rerun.
  await expect(runs.nth(0).getByRole("button", { name: "Cancel" })).toHaveCount(0);
  await expect(runs.nth(0).getByRole("button", { name: "Rerun", exact: true })).toBeVisible();
  await expect(runs.nth(0).getByRole("button", { name: "Rerun failed" })).toBeVisible();
  await expect(runs.nth(1).getByRole("button", { name: "Cancel" })).toBeVisible();
  await expect(runs.nth(2).getByRole("button", { name: "Rerun", exact: true })).toBeVisible();
  await expect(runs.nth(2).getByRole("button", { name: "Rerun failed" })).toHaveCount(0);

  // Opening a run shows its detail in the editor (WorkflowRunView): a jobs
  // pane plus the combined log of every job.
  await runs.nth(0).locator(".wf-run-main").click();
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_detail")).length)
    .toBeGreaterThanOrEqual(1);
  expect((await mock.callsOf("workflow_run_detail"))[0]).toEqual({
    repo_id: "r1",
    run_id: 101,
  });
  const view = page.locator(".run-view");
  await expect(view).toBeVisible();
  await expect(view).toContainText("Fix flaky workflow test");

  const job = view.locator(".job-row").filter({ hasText: "build" });
  await expect(job).toHaveCount(1);
  await expect(job).toContainText("Failed");

  // The combined log loads with the view, before any job is selected.
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_logs")).length)
    .toBeGreaterThanOrEqual(1);
  expect((await mock.callsOf("workflow_run_logs"))[0]).toEqual({
    repo_id: "r1",
    run_id: 101,
    job_id: null,
    failed_only: false,
  });
  const log = view.locator(".log-viewer");
  await expect(log).toContainText("npm ERR! test failed");

  // Selecting the job reveals its steps and fetches that job's log.
  await job.click();
  await expect(view.locator(".step-row")).toHaveCount(2);
  await expect(view.locator(".steps")).toContainText("Checkout");
  await expect(view.locator(".steps")).toContainText("npm test");
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_logs")).length)
    .toBe(2);
  expect((await mock.callsOf("workflow_run_logs"))[1]).toEqual({
    repo_id: "r1",
    run_id: 101,
    job_id: 555,
    failed_only: false,
  });

  // "Failed only" re-queries the log with the flag and scopes the pane.
  await view.getByRole("button", { name: /Failed only/ }).click();
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_logs")).length)
    .toBe(3);
  expect((await mock.callsOf("workflow_run_logs"))[2]).toEqual({
    repo_id: "r1",
    run_id: 101,
    job_id: 555,
    failed_only: true,
  });
  await expect(view.locator(".scope-chip")).toContainText("failed steps");

  // Closing the run view returns to the plain editor.
  await view.getByRole("button", { name: "Close run details" }).click();
  await expect(view).toHaveCount(0);

  // Status filter re-queries gh with the selected status.
  await page.getByLabel("Run status filter").selectOption("failure");
  await expect
    .poll(async () => (await mock.callsOf("workflow_runs")).at(-1)?.status_filter)
    .toBe("failure");

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});

test("cancel, rerun and dispatch call the gh workflow commands", async ({ page }) => {
  const tracked = trackErrors(page);
  const mock = await setup(page);
  await openActionsTab(page);

  const runs = page.locator(".wf-run");
  const running = runs.filter({ hasText: "Nightly build" });
  const failed = runs.filter({ hasText: "Fix flaky workflow test" });

  await running.getByRole("button", { name: "Cancel" }).click();
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_cancel")).length)
    .toBe(1);
  expect((await mock.callsOf("workflow_run_cancel"))[0]).toEqual({
    repo_id: "r1",
    run_id: 102,
  });

  await failed.getByRole("button", { name: "Rerun", exact: true }).click();
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_rerun")).length)
    .toBe(1);
  expect((await mock.callsOf("workflow_run_rerun"))[0]).toEqual({
    repo_id: "r1",
    run_id: 101,
    failed_only: false,
  });

  await failed.getByRole("button", { name: "Rerun failed" }).click();
  await expect
    .poll(async () => (await mock.callsOf("workflow_run_rerun")).length)
    .toBe(2);
  expect((await mock.callsOf("workflow_run_rerun"))[1]).toEqual({
    repo_id: "r1",
    run_id: 101,
    failed_only: true,
  });

  // Dispatch: active workflows only, default ref = current branch, raw fields.
  await page.locator(".toolbar").getByRole("button", { name: "Run workflow" }).click();
  const dialog = page.locator(".sdlg-dialog");
  await expect(dialog).toBeVisible();
  const select = dialog.getByRole("combobox");
  await expect(select.locator("option")).toHaveCount(2);
  await expect(select).toHaveValue("CI");
  await expect(dialog.getByLabel("Branch or tag to run on")).toHaveValue("main");
  await dialog
    .getByLabel("Workflow inputs")
    .fill("version=1.2.3\n\n# ignored\ndry_run=true");
  await dialog.getByRole("button", { name: "Run workflow" }).click();

  await expect
    .poll(async () => (await mock.callsOf("workflow_dispatch")).length)
    .toBe(1);
  expect((await mock.callsOf("workflow_dispatch"))[0]).toEqual({
    repo_id: "r1",
    workflow: "CI",
    git_ref: "main",
    inputs: [
      { key: "version", value: "1.2.3" },
      { key: "dry_run", value: "true" },
    ],
  });
  await expect(dialog).toHaveCount(0);

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
