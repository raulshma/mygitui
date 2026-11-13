/**
 * M13 AI transparency e2e (status-bar job rows, master switch, assistant
 * panel chat round-trip + activity tab):
 *
 *   1. status bar: an in-flight AI run renders a dedicated row (sparkle +
 *      feature label + phase + elapsed + Cancel) and Cancel kills it,
 *   2. master switch off: every AI affordance disappears (commit-bar
 *      button, health chip) and the assistant panel shows the off state,
 *   3. chat: a question streams an answer into the assistant panel and the
 *      Activity tab records the finished job.
 *
 * The OpenRouter HTTP surface is intercepted with `page.route` (models
 * probe → JSON; chat completions → SSE), so the whole AI pipeline runs for
 * real against fakes; the Tauri IPC is mocked (e2e/tauri-mock.ts).
 */

import { expect, test, type Page, type Route } from "@playwright/test";
import {
  installTauriMock,
  openPanel,
  openRecentRepo,
  recentRepos,
  seedStorage,
} from "./tauri-mock";

const AI_KEY = "mygitui.ai";
const OPTED_IN = {
  backend: "openrouter",
  allowFallback: true,
  repoOptIn: { r1: true },
};

const STAGED_DIFF: unknown[] = [
  {
    path: "src/app.ts",
    old_path: null,
    binary: false,
    is_image: false,
    additions: 2,
    deletions: 1,
    hunks: [
      {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 2,
        header: "@@",
        lines: [
          { origin: " ", text: "const x = 1;" },
          { origin: "+", text: "const y = 2;" },
        ],
      },
    ],
  },
];

/** A chat-completions SSE body: streamed deltas + a usage tail. */
function sseBody(chunks: string[]): string {
  const events = chunks.map(
    (content) =>
      `data: ${JSON.stringify({ id: "c1", choices: [{ delta: { content } }] })}\n\n`,
  );
  events.push(
    `data: ${JSON.stringify({
      id: "c1",
      choices: [{ delta: {}, finish_reason: "stop" }],
      usage: { prompt_tokens: 12, completion_tokens: 6, total_tokens: 18 },
    })}\n\n`,
  );
  events.push("data: [DONE]\n\n");
  return events.join("");
}

/** Intercepts the OpenRouter surface: models probe + (streaming) answers. */
async function interceptOpenRouter(
  page: Page,
  reply: string[],
  opts?: { hold?: boolean; delayMs?: number },
): Promise<void> {
  await page.route("**openrouter.ai/api/v1/models", async (route: Route) => {
    await route.fulfill({
      status: 200,
      content_type: "application/json",
      body: JSON.stringify({
        data: [{ id: "anthropic/claude-sonnet-4.5", name: "Claude Sonnet 4.5" }],
      }),
    });
  });
  await page.route("**openrouter.ai/api/v1/chat/completions", async (route: Route) => {
    if (opts?.hold) {
      // Never answer: the request stays in flight so the status-bar row is
      // observable before Cancel.
      return;
    }
    if (opts?.delayMs) await new Promise((resolve) => setTimeout(resolve, opts.delayMs));
    await route.fulfill({
      status: 200,
      content_type: "text/event-stream",
      body: sseBody(reply),
    });
  });
}

async function boot(page: Page, config: unknown): Promise<void> {
  await seedStorage(page, { ...recentRepos(["/tmp/repo"]), [AI_KEY]: JSON.stringify(config) });
}

test("status bar: an AI run shows a cancellable row", async ({ page }) => {
  await interceptOpenRouter(page, [], { hold: true });
  await installTauriMock(page, {
    commands: {
      repo_diff: STAGED_DIFF,
      // The keyring read behind the OpenRouter key (mock returns it verbatim).
      secrets_get: "sk-or-v1-e2e-key",
      opencode_serve_status: { running: false, url: null, error: "no cli" },
      opencode_serve_start: { running: false, url: null, error: "no cli" },
    },
  });
  await boot(page, OPTED_IN);
  await openRecentRepo(page);

  // Run a feature headlessly via the commit-bar button (repo is opted in).
  await page.getByRole("button", { name: "Generate commit message with AI" }).click();

  // The status-bar row appears with the feature label and a live phase…
  const row = page.getByRole("status", { name: "Generate commit message (AI) in progress" });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await expect(row).toContainText("Generate commit message");

  // …and Cancel kills the job: the row disappears.
  await row.getByRole("button", { name: "Cancel Generate commit message" }).click();
  await expect(row).toBeHidden({ timeout: 15_000 });
});

test("master switch off: AI affordances disappear, panel shows the off state", async ({ page }) => {
  await installTauriMock(page, {});
  await boot(page, { ...OPTED_IN, enabled: false });
  await openRecentRepo(page);

  // The commit-bar sparkle button is gone…
  await expect(
    page.getByRole("button", { name: "Generate commit message with AI" }),
  ).toHaveCount(0);
  // …and the health chip is hidden (it only renders when AI is on).
  await expect(page.locator(".ai-health-chip")).toHaveCount(0);

  // The assistant panel renders the "off" empty state with the way back in.
  await openPanel(page, "Assistant");
  const off = page.getByText("AI is turned off.");
  await expect(off).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Open AI settings" }),
  ).toBeVisible();
});

test("chat round-trip streams into the assistant panel and lands in Activity", async ({ page }) => {
  await interceptOpenRouter(page, ["The ", "repo ", "looks ", "good."]);
  await installTauriMock(page, {
    commands: {
      repo_diff: STAGED_DIFF,
      secrets_get: "sk-or-v1-e2e-key",
      opencode_serve_status: { running: false, url: null, error: "no cli" },
      opencode_serve_start: { running: false, url: null, error: "no cli" },
    },
  });
  await boot(page, OPTED_IN);
  await openRecentRepo(page);

  await openPanel(page, "Assistant");
  const section = page.getByRole("region", { name: "AI assistant" });

  // Opted in via seed: the composer is there. Ask a question.
  await section.getByRole("button", { name: "Explain working copy" }).click();

  // The answer streams in and settles as the assistant bubble.
  await expect(section.getByText("The repo looks good.")).toBeVisible({ timeout: 30_000 });

  // The Activity tab records the finished assistant job with usage.
  await section.getByRole("tab", { name: "Activity" }).click();
  const history = section.getByRole("list", { name: "AI run history" });
  await expect(history.getByText("Assistant")).toBeVisible();
  await expect(history.getByText("done")).toBeVisible();
  await expect(history.getByText("18 tokens")).toBeVisible();
});
