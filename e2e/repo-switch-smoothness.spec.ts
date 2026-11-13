import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

/**
 * Repo-switch smoothness regression: activating another repo's tab must be
 * a smooth, snappy transition — the workspace must not remount, flash the
 * chunk-loading placeholder, or move vertically. The first cut of the
 * switch transition dipped the whole workspace 3px down and flashed it to
 * 35% opacity; on a full-window surface that reads as "the whole UI
 * twitches" on every switch (frame-sampled probe, 2026-11). The contract
 * below pins the replacement: an opacity-only settle (floor 0.6) with zero
 * positional movement.
 *
 * Two tabs are needed to switch at all; the second rides the `ssh://`
 * remote-open path (the `repo_open` mock always reports the same root, so
 * two local paths would dedupe into one tab).
 */

const SESSION = {
  roots: ["/tmp/repo", "ssh://dev1/tmp/other"],
  activeRoot: "/tmp/repo",
};

const DIFF_PAGE = [
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
        new_start: 1,
        lines: [
          { old_no: 1, new_no: 1, origin: " ", text: "export const app = 1;", highlights: [] },
          { old_no: 2, new_no: null, origin: "-", text: "export const old = 2;", highlights: [] },
          { old_no: null, new_no: 2, origin: "+", text: "export const fresh = 3;", highlights: [] },
        ],
      },
    ],
  },
];

/** Shell surfaces whose rect must not move across a switch. */
const STABLE_SELECTORS: Record<string, string> = {
  repoHeader: ".repo-header",
  workspace: ".repo-workspace",
  layoutArea: ".layout-area",
  activityBar: ".activity-bar",
};

/** Per-rAF rect/opacity samples while the switch animation plays out. */
async function sampleSwitch(
  page: Page,
  selectors: Record<string, string>,
): Promise<void> {
  await page.evaluate((selectors: Record<string, string>) => {
    const g = window as unknown as {
      __probe: { frames: Record<string, unknown>[]; done: boolean };
    };
    g.__probe = { frames: [], done: false };

    const marked = new Map<Element, string>();
    for (const sel of Object.values(selectors)) {
      for (const el of document.querySelectorAll(sel)) {
        const id = Math.random().toString(36).slice(2);
        (el as HTMLElement).dataset.probeId = id;
        marked.set(el, id);
      }
    }

    const t0 = performance.now();
    const frames = g.__probe.frames;
    const sample = () => {
      const frame: Record<string, unknown> = {
        t: Math.round(performance.now() - t0),
        nodes: {} as Record<string, unknown>,
      };
      const nodes = frame.nodes as Record<string, unknown>;
      for (const [name, sel] of Object.entries(selectors)) {
        const el = document.querySelector(sel);
        if (!el) {
          nodes[name] = "absent";
          continue;
        }
        const r = el.getBoundingClientRect();
        nodes[name] = {
          x: Math.round(r.x * 10) / 10,
          y: Math.round(r.y * 10) / 10,
          w: Math.round(r.width * 10) / 10,
          h: Math.round(r.height * 10) / 10,
          kept: (el as HTMLElement).dataset.probeId === marked.get(el),
        };
      }
      const ws = document.querySelector(".repo-workspace");
      frame.wsOpacity = ws ? getComputedStyle(ws).opacity : null;
      frame.repoLoading = document.querySelector(".repo-loading") !== null;
      frames.push(frame);
      if (performance.now() - t0 < 700) requestAnimationFrame(sample);
      else g.__probe.done = true;
    };
    requestAnimationFrame(sample);
  }, selectors);
}

async function collectSamples(page: Page) {
  return page.evaluate(
    () =>
      (window as unknown as { __probe: { frames: Record<string, unknown>[] } })
        .__probe.frames,
  );
}

test("switching repo tabs keeps the workspace still and mounted", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  await seedStorage(page, {
    "mygitui.recent-repos": JSON.stringify([
      { path: "/tmp/repo", lastOpened: Date.now(), pinned: false },
      { path: "ssh://dev1/tmp/other", lastOpened: Date.now() - 1000, pinned: false },
    ]),
    "mygitui.session": JSON.stringify(SESSION),
  });

  await installTauriMock(page, {
    commands: {
      remote_repo_open: {
        repo_id: "r2",
        root: "/tmp/other",
        name: "other",
        bare: false,
        git_dir: "/tmp/other/.git",
      },
      repo_diff_stream: DIFF_PAGE,
    },
  });

  await page.goto("/");
  await expect(page.locator(".repo-workspace")).toBeVisible();
  const repoTabs = page.locator(".tabbar button[role='tab']");
  await expect(repoTabs).toHaveCount(2);
  // Let the launch loads (status, diff stream, history) settle before
  // measuring — only the switch itself is under test.
  await page.waitForTimeout(800);

  // Start the per-frame sampler, click the second repo tab mid-stream, and
  // let the 140ms transition (plus load round-trips) play out.
  await sampleSwitch(page, STABLE_SELECTORS);
  await repoTabs.nth(1).click();
  await expect(repoTabs.nth(1)).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".repo-header .repo-name")).toHaveText("other");
  await page.waitForFunction(
    () =>
      (window as unknown as { __probe: { done: boolean } }).__probe.done ===
      true,
  );
  const frames = (await collectSamples(page)) as Array<{
    t: number;
    nodes: Record<string, Record<string, number | boolean> | "absent">;
    wsOpacity: string | null;
    repoLoading: boolean;
  }>;

  // The switch animation must actually have been observed: the workspace
  // opacity dips at the swap and ramps back to 1.
  const opacities = frames
    .map((f) => (f.wsOpacity === null ? null : Number(f.wsOpacity)))
    .filter((o): o is number => o !== null);
  expect(opacities.length).toBeGreaterThan(10);
  expect(Math.min(...opacities)).toBeGreaterThanOrEqual(0.55);
  expect(opacities[opacities.length - 1]).toBeCloseTo(1, 5);

  // No surface may move during (or after) the switch, and none may be
  // replaced (a remount would drop its probe marker).
  let prev: Record<string, Record<string, number>> | null = null;
  for (const f of frames) {
    expect(f.repoLoading, `frame ${f.t}: chunk-loading placeholder flashed`).toBe(false);
    const cur: Record<string, Record<string, number>> = {};
    for (const [name, val] of Object.entries(f.nodes)) {
      expect(val, `frame ${f.t}: ${name} vanished`).not.toBe("absent");
      const v = val as { x: number; y: number; w: number; h: number; kept: boolean };
      expect(v.kept, `frame ${f.t}: ${name} was remounted`).toBe(true);
      cur[name] = { x: v.x, y: v.y, w: v.w, h: v.h };
    }
    if (prev !== null) {
      for (const [name, box] of Object.entries(cur)) {
        const p = prev[name]!;
        for (const key of ["x", "y", "w", "h"] as const) {
          expect(
            Math.abs(box[key] - p[key]),
            `frame ${f.t}: ${name}.${key} moved ${p[key]}→${box[key]}`,
          ).toBeLessThanOrEqual(0.5);
        }
      }
    }
    prev = cur;
  }

  expect(tracked.errors).toEqual([]);
});
