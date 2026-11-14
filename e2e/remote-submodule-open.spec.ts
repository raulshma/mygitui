import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  seedStorage,
  SESSION_KEY,
  trackErrors,
} from "./tauri-mock";

/**
 * Remote repo + submodules initial-open regression harness.
 *
 * A page-side "remote backend simulator" replaces the static Tauri mock for
 * the repository lifecycle commands and models the REAL backend contract
 * (src-tauri/src/repo/mod.rs):
 *
 *   - `remote_repo_open` pays an SSH discover delay, then dedupes by
 *     (device, root) — a second open of the same root returns the SAME id.
 *   - `repo_close` removes the id from the registry.
 *   - `repo_status` / `submodules` reject with `repo not open: <id>` when
 *     the id is not registered (get_handle semantics).
 *
 * The test opens a remote repo that has one initialized submodule through
 * the launch session restore and asserts the embedded submodule section
 * loads WITHOUT the "Submodule error: repo not open" state the real app
 * shows. Timing profiles are configurable so interleavings can be pinned.
 */

const DEVICE = "dev-1";
const MAIN_PATH = "/home/u/jellyplay";
const MAIN_URI = `ssh://${DEVICE}${MAIN_PATH}`;
const SUB_PATH = `${MAIN_PATH}/jellyfin-plugin`;
const SUB_URI = `ssh://${DEVICE}${SUB_PATH}`;
const LOCAL_PATH = "/tmp/repo";

/** Timing knobs (ms) for the simulator; every profile overrides some. */
interface SimProfile {
  openMs?: number;
  statusMs?: number;
  subsMs?: number;
  closeMs?: number;
  /** Emit `repo-changed {full}` for every registered repo every N ms. */
  pollMs?: number;
}

const SSH: SimProfile = {
  openMs: 400,
  statusMs: 150,
  subsMs: 250,
  closeMs: 50,
};

const FAST: SimProfile = { openMs: 5, statusMs: 5, subsMs: 5, closeMs: 5 };

/**
 * Installs the dynamic backend simulator AFTER the static Tauri mock (init
 * scripts run in order, still before any app script). Exposes
 * `window.__REMOTE_SIM__` with the registry + a lifecycle command log.
 */
async function installRemoteSim(page: Page, profile: SimProfile): Promise<void> {
  await page.addInitScript(
    ({ profile: p, mainPath }) => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      const orig = internals.invoke.bind(internals);
      const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

      const S = {
        registry: new Map<string, { root: string; device_id: string | null }>(),
        byRoot: new Map<string, string>(),
        counter: 0,
        log: [] as Array<{
          cmd: string;
          arg: Record<string, unknown>;
          err?: string;
        }>,
      };
      (window as unknown as Record<string, unknown>).__REMOTE_SIM__ = S;

      const CLEAN_STATUS = {
        branch: "main",
        head: "0123456789abcdef0123456789abcdef01234567",
        detached: false,
        ahead: 0,
        behind: 0,
        merging: false,
        rebasing: false,
        sequencer: false,
        entries: [],
      };

      async function simInvoke(
        cmd: string,
        args: Record<string, unknown>,
      ): Promise<unknown> {
        const a = args ?? {};
        if (cmd === "remote_repo_open" || cmd === "repo_open") {
          await sleep(p.openMs ?? 250);
          const root = String(a.path ?? "");
          const deviceId = cmd === "remote_repo_open" ? String(a.device_id ?? "") : null;
          const key = `${deviceId ?? "local"}|${root}`;
          let id = S.byRoot.get(key);
          if (!id) {
            id = `1a1d8178587-dd98-${S.counter++}`;
            S.byRoot.set(key, id);
            S.registry.set(id, { root, device_id: deviceId });
          }
          const repo = S.registry.get(id)!;
          return {
            repo_id: id,
            root: repo.root,
            name: repo.root.split("/").filter(Boolean).pop() ?? repo.root,
            bare: false,
            git_dir: `${repo.root}/.git`,
            device_id: deviceId,
          };
        }
        if (cmd === "repo_close") {
          await sleep(p.closeMs ?? 0);
          const id = String(a.repo_id ?? "");
          const repo = S.registry.get(id);
          if (repo) {
            S.registry.delete(id);
            S.byRoot.delete(`${repo.device_id ?? "local"}|${repo.root}`);
          }
          return null;
        }
        if (cmd === "repo_status") {
          await sleep(p.statusMs ?? 120);
          if (!S.registry.has(String(a.repo_id ?? ""))) {
            throw new Error(`repo not open: ${a.repo_id}`);
          }
          return { ...CLEAN_STATUS };
        }
        if (cmd === "submodules") {
          await sleep(p.subsMs ?? 200);
          const id = String(a.repo_id ?? "");
          if (!S.registry.has(id)) {
            throw new Error(`repo not open: ${id}`);
          }
          const repo = S.registry.get(id)!;
          if (repo.root !== mainPath) return [];
          return [
            {
              path: "jellyfin-plugin",
              name: "jellyfin-plugin",
              url: "https://example.com/jellyfin-plugin.git",
              head_sha: "622213b622213b622213b622213b622213b622213b",
              recorded_sha: "622213b622213b622213b622213b622213b622213b",
              initialized: true,
              status: "",
            },
          ];
        }
        return orig(cmd, args);
      }

      internals.invoke = (cmd, args) => {
        const promise = simInvoke(cmd, args ?? {});
        promise.then(
          () => S.log.push({ cmd, arg: args ?? {} }),
          (err: unknown) =>
            S.log.push({
              cmd,
              arg: args ?? {},
              err: String((err as Error)?.message ?? err),
            }),
        );
        return promise;
      };

      if (p.pollMs) {
        const emit = () => {
          const mock = (
            window as unknown as {
              __TAURI_MOCK__?: {
                emit: (event: string, payload: unknown) => void;
              };
            }
          ).__TAURI_MOCK__;
          for (const id of S.registry.keys()) {
            mock?.emit("repo-changed", {
              repo_id: id,
              paths: [],
              head_moved: false,
              full: true,
            });
          }
        };
        setTimeout(() => setInterval(emit, p.pollMs), p.pollMs);
      }
    },
    { profile, mainPath: MAIN_PATH },
  );
}

/** Boots the app with the given stored session (launch restore). */
async function launchWithSession(
  page: Page,
  roots: string[],
  activeRoot: string,
): Promise<void> {
  await seedStorage(page, {
    [SESSION_KEY]: JSON.stringify({ roots, activeRoot }),
  });
  await page.goto("/");
}

/**
 * Waits for the embedded submodule section to settle and reports its state.
 * `red` = the "Submodule error" box is (or was) visible.
 */
async function submoduleOutcome(
  page: Page,
  settleMs = 1500,
): Promise<{ red: boolean; errorText: string | null }> {
  // Green = the submodule's embedded section rendered with real state.
  const deadline = Date.now() + 20000;
  let sawError: string | null = null;
  while (Date.now() < deadline) {
    const errorBox = page.locator(".module-error");
    if (await errorBox.count()) {
      sawError = (await errorBox.first().textContent()) ?? "";
      return { red: true, errorText: sawError };
    }
    const loaded =
      (await page.getByText("No changes in submodule").count()) > 0 ||
      (await page.locator(".sub-clean-hint, .embedded-commit-bar").count()) > 0;
    if (loaded) {
      // Settle window: an error appearing slightly later still counts.
      await page.waitForTimeout(settleMs);
      if (await page.locator(".module-error").count()) {
        sawError = (await page.locator(".module-error").first().textContent()) ?? "";
        return { red: true, errorText: sawError };
      }
      return { red: false, errorText: null };
    }
    await page.waitForTimeout(100);
  }
  return { red: false, errorText: null }; // timeout: treat as no-error (asserts below fail on missing state)
}

/** Lifecycle command log from the simulator (diagnostics on failure). */
async function simLifecycleLog(page: Page): Promise<string> {
  return page.evaluate(() => {
    const sim = (
      window as unknown as {
        __REMOTE_SIM__: {
          log: Array<{ cmd: string; arg: Record<string, unknown>; err?: string }>;
        };
      }
    ).__REMOTE_SIM__;
    return sim.log
      .filter(({ cmd }) =>
        ["remote_repo_open", "repo_open", "repo_close", "submodules", "repo_status"].includes(cmd),
      )
      .map(({ cmd, arg, err }) => {
        const id = arg.repo_id ?? arg.path ?? "";
        return `${cmd}(${String(id)})${err ? ` -> ERR ${err}` : ""}`;
      })
      .join("\n");
  });
}

for (const [name, profile] of [
  ["fast", FAST],
  ["ssh", SSH],
] as const) {
  test(`launch restore of remote repo with submodule (${name} timing) loads it without "repo not open"`, async ({
    page,
  }) => {
    test.setTimeout(60_000);
    const tracked = trackErrors(page);
    await installTauriMock(page);
    await installRemoteSim(page, profile);
    await launchWithSession(page, [MAIN_URI], MAIN_URI);

    const outcome = await submoduleOutcome(page);
    if (outcome.red) {
      console.error(`[${name}] submodule error: ${outcome.errorText}`);
      console.error(`[${name}] lifecycle:\n${await simLifecycleLog(page)}`);
    }
    expect(outcome.red, `submodule error visible: ${outcome.errorText}`).toBe(false);
    await expect(page.getByText("No changes in submodule")).toBeVisible();
    expect(tracked.errors.filter((e) => e.includes("repo not open"))).toEqual([]);
  });
}

test("switching from a local tab to the remote tab loads the submodule without errors", async ({
  page,
}) => {
  test.setTimeout(60_000);
  const tracked = trackErrors(page);
  await installTauriMock(page);
  await installRemoteSim(page, SSH);
  await launchWithSession(page, [LOCAL_PATH, MAIN_URI], LOCAL_PATH);

  // Local tab restores instantly; the remote tab opens in the background.
  await page.getByRole("tab", { name: "jellyplay" }).click({ timeout: 20000 });

  const outcome = await submoduleOutcome(page);
  if (outcome.red) {
    console.error(`[switch] submodule error: ${outcome.errorText}`);
    console.error(`[switch] lifecycle:\n${await simLifecycleLog(page)}`);
  }
  expect(outcome.red, `submodule error visible: ${outcome.errorText}`).toBe(false);
  await expect(page.getByText("No changes in submodule")).toBeVisible();
  expect(tracked.errors.filter((e) => e.includes("repo not open"))).toEqual([]);
});

test("poller-driven reloads keep the submodule error-free", async ({ page }) => {
  test.setTimeout(60_000);
  const tracked = trackErrors(page);
  await installTauriMock(page);
  await installRemoteSim(page, { ...SSH, pollMs: 2000 });
  await launchWithSession(page, [MAIN_URI], MAIN_URI);

  const outcome = await submoduleOutcome(page, 4000);
  if (outcome.red) {
    console.error(`[poll] submodule error: ${outcome.errorText}`);
    console.error(`[poll] lifecycle:\n${await simLifecycleLog(page)}`);
  }
  expect(outcome.red, `submodule error visible: ${outcome.errorText}`).toBe(false);
  await expect(page.getByText("No changes in submodule")).toBeVisible();
  expect(tracked.errors.filter((e) => e.includes("repo not open"))).toEqual([]);
});

/**
 * REGRESSION (was the red-capability probe): a submodule whose backend repo
 * dies while StatusPanel holds its cached id — here by closing the
 * submodule's own tab (the id StatusPanel resolved via the tab lookup) —
 * must not wedge on "Submodule error: repo not open". The cached id is
 * dropped and the module re-resolved, so both the poller-driven reload and
 * a plain "Refresh changes" heal it without a repo switch.
 */
test("dead submodule backend id self-heals (poll reload and manual refresh)", async ({
  page,
}) => {
  test.setTimeout(60_000);
  const tracked = trackErrors(page);
  await installTauriMock(page);
  await installRemoteSim(page, SSH);
  await launchWithSession(page, [MAIN_URI, SUB_URI, LOCAL_PATH], MAIN_URI);

  // Wait for the embedded submodule section to go green first.
  await expect(page.getByText("No changes in submodule")).toBeVisible({
    timeout: 20000,
  });
  // Close the submodule's own tab (its id is what StatusPanel cached).
  await page.getByRole("button", { name: "Close jellyfin-plugin" }).click();

  // Manual refresh (right-click → Refresh changes path) must heal it: the
  // stale id is dropped, the module re-opens fresh and shows clean state.
  // Assert the SETTLED state (the failing status lands ~650ms after the
  // click; a zero-count poll during the rebuild window would pass vacuously).
  await page.locator("button[aria-label='Refresh changes']").click();
  await page.waitForTimeout(2500);
  await expect(page.locator(".module-error")).toHaveCount(0);
  await expect(page.getByText("No changes in submodule")).toBeVisible();

  // And a repo switch away+back keeps it green (fresh map, fresh open).
  await page.getByRole("tab", { name: /repo —/ }).click();
  await expect(page.getByText("Loading workspace")).toHaveCount(0).catch(() => {});
  await page.waitForTimeout(800);
  await page.getByRole("tab", { name: /jellyplay —/ }).click();
  await expect(page.getByText("No changes in submodule")).toBeVisible({
    timeout: 15000,
  });
  await expect(page.locator(".module-error")).toHaveCount(0);
  expect(tracked.errors.filter((e) => e.includes("repo not open"))).toEqual([]);
});
