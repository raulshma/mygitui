import { expect, test } from "@playwright/test";
import {
  installTauriMock,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

const PR = {
  number: 12,
  title: "Add the compact source-control view",
  head_ref_name: "main",
  base_ref_name: "develop",
  state: "OPEN",
  is_draft: false,
  url: "https://github.com/acme/repo/pull/12",
  created_at: "2026-10-01T10:00:00Z",
  updated_at: "2026-10-02T10:00:00Z",
  merged_at: null,
  closed_at: null,
  author: "octocat",
  body: "This PR makes the source-control rows compact.",
  additions: 12,
  deletions: 3,
  changed_files: 2,
  review_decision: "APPROVED",
  mergeable: "MERGEABLE",
  labels: ["ui", "compact"],
};

/** GitHub panel: rich PR metadata + merge/review dialogs wired to gh actions. */
test("PR metadata is visible and merge/review actions call gh commands", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      forge_status: { available: true, version: "2.63.3", authed: true },
      forge_context: {
        owner: "acme",
        repo: "repo",
        branch: "main",
        remote_url: "git@github.com:acme/repo.git",
      },
      pr_list: [PR],
      pr_checks: [],
      pr_merge: "merged",
      pr_review: "reviewed",
    },
  });

  await openRecentRepo(page);
  await page
    .locator(".activity-bar")
    .getByRole("button", { name: "GitHub" })
    .click();

  await expect(page.locator(".panel-title")).toHaveText("GitHub");
  const card = page.locator(".card.current");
  await expect(card).toContainText("Add the compact source-control view");
  await expect(card).toContainText("by octocat");
  await expect(card).toContainText("+12");
  await expect(card).toContainText("−3");
  await expect(card).toContainText("Approved");
  await expect(card).toContainText("ui");
  await expect(card).toContainText("Description");

  await card.getByRole("button", { name: "Merge", exact: true }).click();
  const mergeDialog = page.locator(".sdlg-dialog");
  await expect(mergeDialog).toContainText("Merge pull request");
  await mergeDialog.locator(".sdlg-primary").click();
  await expect
    .poll(async () => (await mock.callsOf("pr_merge")).length)
    .toBe(1);
  expect((await mock.callsOf("pr_merge"))[0]).toEqual({
    repo_id: "r1",
    number: 12,
    method: "squash",
    delete_branch: true,
    auto: false,
  });
  await expect(page.locator(".sdlg-dialog")).toHaveCount(0);

  await card.getByRole("button", { name: "Approve", exact: true }).click();
  const reviewDialog = page.locator(".sdlg-dialog");
  await expect(reviewDialog).toContainText("Approve #12");
  await reviewDialog.locator("textarea").fill("Looks good");
  await reviewDialog.locator(".sdlg-primary").click();
  await expect
    .poll(async () => (await mock.callsOf("pr_review")).length)
    .toBe(1);
  expect((await mock.callsOf("pr_review"))[0]).toEqual({
    repo_id: "r1",
    number: 12,
    action: "approve",
    body: "Looks good",
  });

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
