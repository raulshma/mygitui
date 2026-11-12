/**
 * Header controls single-click (e2e): every Hint-wrapped header control —
 * the sync split-button chevron, the 3-dot "More repository actions", the
 * layout-preset select — must respond to ONE click, and its pill must not
 * flash closed and re-open while the pointer rests on the trigger.
 *
 * Reproduces the user report: "many controls sometimes require clicking
 * twice to open (fetch/pull/push chevron, 3-dot repository actions)"; the
 * preset select "is lowercase before click, after click nothing opens but
 * the default becomes titlecase Default"; "the tooltip shows and flashes
 * then reshowss very quickly".
 */

import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";

const entries = Array.from({ length: 5 }, (_, i) => ({
  path: `app/src/EncodedStreamHub${i}.kt`,
  old_path: null,
  index: "unmodified",
  worktree: "modified",
}));

/** Install an event + UI-state tracer into the page. */
async function installTrace(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const log: Array<Record<string, unknown>> = [];
    (window as unknown as Record<string, unknown>).__trace = log;
    const name = (el: EventTarget | null): string => {
      if (el instanceof Element) {
        return (
          el.getAttribute("aria-label") ??
          `${el.tagName.toLowerCase()}.${(el.className?.toString() ?? "").split(" ")[0]}`
        );
      }
      return String(el);
    };
    for (const type of [
      "pointermove",
      "pointerdown",
      "pointerup",
      "pointerenter",
      "pointerleave",
      "pointerover",
      "pointerout",
      "mousedown",
      "mouseup",
      "click",
      "focus",
      "blur",
      "keydown",
    ]) {
      window.addEventListener(
        type,
        (e) => {
          log.push({
            t: Math.round(performance.now()),
            type,
            target: name(e.target),
            current: name(e.currentTarget),
            detail: "detail" in e ? (e as MouseEvent).detail : undefined,
          });
        },
        { capture: true, passive: true },
      );
    }
    // UI-state transitions, sampled: pill data-state, menu/listbox presence.
    const snap = (): string => {
      const pills = Array.from(
        document.querySelectorAll('[data-slot="tooltip-content"]'),
      )
        .map((el) => `pill:${el.getAttribute("data-state")}`)
        .join(",");
      const menu = document.querySelector('[role="menu"]') ? "menu:open" : "";
      const listbox = document.querySelector('[role="listbox"]')
        ? "listbox:open"
        : "";
      return [pills, menu, listbox].filter(Boolean).join(" ");
    };
    let last = "";
    window.setInterval(() => {
      const cur = snap();
      if (cur !== last) {
        log.push({ t: Math.round(performance.now()), ui: cur || "(none)" });
        last = cur;
      }
    }, 30);
  });
}

async function dumpTrace(page: Page): Promise<void> {
  const trace = await page.evaluate(
    () => (window as unknown as Record<string, unknown>).__trace,
  );
  console.log(
    "[trace]\n" +
      trace
        .map((e: Record<string, unknown>) => JSON.stringify(e))
        .join("\n"),
  );
}

async function boot(page: Page): Promise<void> {
  await page.addInitScript((seed) => {
    for (const [key, value] of Object.entries(seed)) {
      window.localStorage.setItem(key, value);
    }
  }, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });
  await installTauriMock(page, {
    commands: {
      repo_status: {
        branch: "main",
        head: "0123456789abcdef0123456789abcdef01234567",
        detached: false,
        ahead: 0,
        behind: 0,
        merging: false,
        rebasing: false,
        sequencer: false,
        entries,
      },
    },
  });
  await installTrace(page);
  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();
  await expect(
    page.getByRole("button", { name: "More sync actions" }),
  ).toBeVisible({ timeout: 20_000 });
}

/** Click without moving the pointer: down+up at the current position. */
async function clickHere(page: Page): Promise<void> {
  await page.mouse.down();
  await page.mouse.up();
}

test("sync chevron opens when the press spans the dwell swap", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await boot(page);
  const chevron = page.getByRole("button", { name: "More sync actions" });
  await chevron.hover();
  // A real user aims then presses ~300ms after the pointer lands — exactly
  // when the Hint's dwell timer swaps the idle trigger for the real one.
  // The press must survive that swap: down at ~280ms, up at ~360ms.
  await page.waitForTimeout(280);
  await page.mouse.down();
  await page.waitForTimeout(80);
  await page.mouse.up();
  await dumpTrace(page);
  await expect(page.getByRole("menuitem", { name: "Fetch" })).toBeVisible();
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("layout preset select opens when the press spans the dwell swap", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await boot(page);
  const trigger = page.getByRole("combobox", { name: "Layout preset" });
  await trigger.hover();
  await page.waitForTimeout(280);
  await page.mouse.down();
  await page.waitForTimeout(80);
  await page.mouse.up();
  try {
    await expect(page.getByRole("option", { name: "Default" })).toBeVisible({
      timeout: 4_000,
    });
  } catch (err) {
    await dumpTrace(page);
    throw err;
  }
  // The listbox must STAY open — not dismiss on the click that opened it.
  await page.waitForTimeout(400);
  await expect(page.getByRole("option", { name: "Default" })).toBeVisible();
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("sync chevron opens on a fast click (no dwell)", async ({ page }) => {
  const tracked = trackErrors(page);
  await boot(page);
  const chevron = page.getByRole("button", { name: "More sync actions" });
  await chevron.hover();
  // Click immediately — well inside the 300ms dwell, i.e. still idle.
  await clickHere(page);
  await expect(page.getByRole("menuitem", { name: "Fetch" })).toBeVisible();
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("sync chevron opens on a single click after dwell", async ({ page }) => {
  const tracked = trackErrors(page);
  await boot(page);
  const chevron = page.getByRole("button", { name: "More sync actions" });
  await chevron.hover();
  await expect
    .poll(
      () =>
        page.evaluate(() =>
          Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          ).some((el) => el.getAttribute("data-state") !== "closed"),
        ),
      { timeout: 5_000 },
    )
    .toBe(true);
  await page.waitForTimeout(250); // pill fully settled, swap done
  await clickHere(page);
  await expect(page.getByRole("menuitem", { name: "Fetch" })).toBeVisible();
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("layout preset select opens on a fast click", async ({ page }) => {
  const tracked = trackErrors(page);
  await boot(page);
  const trigger = page.getByRole("combobox", { name: "Layout preset" });
  await trigger.hover();
  await clickHere(page);
  await expect(page.getByRole("option", { name: "Default" })).toBeVisible();
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("layout preset select opens on a single click after dwell", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await boot(page);
  const trigger = page.getByRole("combobox", { name: "Layout preset" });
  await trigger.hover();
  await expect
    .poll(
      () =>
        page.evaluate(() =>
          Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          ).some((el) => el.getAttribute("data-state") !== "closed"),
        ),
      { timeout: 5_000 },
    )
    .toBe(true);
  await page.waitForTimeout(250);
  await clickHere(page);
  await expect(page.getByRole("option", { name: "Default" })).toBeVisible();
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("3-dot repository actions opens on a single click after dwell", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await boot(page);
  const more = page.getByRole("button", { name: "More repository actions" });
  await more.hover();
  await expect
    .poll(
      () =>
        page.evaluate(() =>
          Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          ).some((el) => el.getAttribute("data-state") !== "closed"),
        ),
      { timeout: 5_000 },
    )
    .toBe(true);
  await page.waitForTimeout(250);
  await clickHere(page);
  await expect(page.locator('[role="menu"]')).toBeVisible();
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("pill stays open while the pointer rests on the trigger", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await boot(page);
  const more = page.getByRole("button", { name: "More repository actions" });
  await more.hover();
  await expect
    .poll(
      () =>
        page.evaluate(() =>
          Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          ).some((el) => el.getAttribute("data-state") !== "closed"),
        ),
      { timeout: 5_000 },
    )
    .toBe(true);
  // Emulate a resting hand: micro-jitter within the trigger while sampling
  // pill data-state at high rate. The pill must never flash closed and
  // re-open (data-state "closed" between opens = visible flash: the exit
  // animation starts, then the enter animation replays).
  const box = await more.boundingBox();
  if (!box) throw new Error("trigger has no box");
  const transitions = await page.evaluate(
    ({ cx, cy }) =>
      new Promise<string[]>((resolve) => {
        const seen: string[] = [];
        let last = "";
        const iv = window.setInterval(() => {
          const cur = Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          )
            .map((el) => el.getAttribute("data-state"))
            .join(",");
          if (cur !== last) {
            seen.push(`${Math.round(performance.now())}: ${cur}`);
            last = cur;
          }
        }, 10);
        // Sub-pixel pointer jitter within the trigger, like a resting hand.
        let n = 0;
        const jitter = window.setInterval(() => {
          window.dispatchEvent(
            new PointerEvent("pointermove", {
              clientX: cx + (n % 2),
              clientY: cy + ((n + 1) % 2),
              bubbles: true,
            }),
          );
          n++;
        }, 90);
        window.setTimeout(() => {
          window.clearInterval(iv);
          window.clearInterval(jitter);
          resolve(seen);
        }, 1_500);
      }),
    { cx: box.x + box.width / 2, cy: box.y + box.height / 2 },
  );
  console.log("[pill transitions]", transitions.join(" | "));
  const firstOpen = transitions.findIndex((s) =>
    /instant-open|delayed-open/.test(s),
  );
  const closedAfterOpen = transitions.findIndex(
    (s, i) => i > firstOpen && /closed|^[\d]+: $/.test(s),
  );
  expect(firstOpen, "pill never opened").toBeGreaterThanOrEqual(0);
  expect(
    closedAfterOpen,
    `pill flashed closed then re-opened: ${transitions.join(" | ")}`,
  ).toBe(-1);
  await dumpTrace(page);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("pill stays closed after its trigger is pressed", async ({ page }) => {
  const tracked = trackErrors(page);
  await boot(page);
  const more = page.getByRole("button", { name: "More repository actions" });
  await more.hover();
  await expect
    .poll(
      () =>
        page.evaluate(() =>
          Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          ).some((el) => el.getAttribute("data-state") !== "closed"),
        ),
      { timeout: 5_000 },
    )
    .toBe(true);
  // Press the trigger: the pill closes (clicking a control must dismiss its
  // hint). The pointer then rests on the pressed control — micro-jitter
  // must NOT flash the pill right back over the active control.
  const box = await more.boundingBox();
  if (!box) throw new Error("trigger has no box");
  await page.mouse.down();
  await page.mouse.up();
  const transitions = await page.evaluate(
    ({ cx, cy }) =>
      new Promise<string[]>((resolve) => {
        const seen: string[] = [];
        let last = "";
        const iv = window.setInterval(() => {
          const cur = Array.from(
            document.querySelectorAll('[data-slot="tooltip-content"]'),
          )
            .map((el) => el.getAttribute("data-state"))
            .join(",");
          if (cur !== last) {
            seen.push(`${Math.round(performance.now())}: ${cur}`);
            last = cur;
          }
        }, 10);
        let n = 0;
        const jitter = window.setInterval(() => {
          window.dispatchEvent(
            new PointerEvent("pointermove", {
              clientX: cx + (n % 2),
              clientY: cy + ((n + 1) % 2),
              bubbles: true,
            }),
          );
          n++;
        }, 90);
        window.setTimeout(() => {
          window.clearInterval(iv);
          window.clearInterval(jitter);
          resolve(seen);
        }, 1_800);
      }),
    { cx: box.x + box.width / 2, cy: box.y + box.height / 2 },
  );
  console.log("[after press]", transitions.join(" | "));
  const reopened = transitions.findIndex((s) =>
    /instant-open|delayed-open/.test(s),
  );
  expect(
    reopened,
    `pill re-showed itself over the pressed control: ${transitions.join(" | ")}`,
  ).toBe(-1);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});
