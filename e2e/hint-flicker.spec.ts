/**
 * Hint pill flicker (e2e): a pill's enter animation must never restart
 * while the pill is visible.
 *
 * Regression (user report: "all the tooltips flicker once on launch"):
 * every Hint pill's FIRST show used to blink — the pill popped in with no
 * fade, then ~300ms later faded in again from opacity 0. Cause: Hint
 * opens bits-ui's Root controlled (open=true at mount), so bits-ui still
 * counted the pill as "not yet delayed-open"; the pointer resting on the
 * freshly swapped trigger armed bits-ui's own delayDuration timer, whose
 * firing flipped the pill's data-state instant-open → delayed-open — the
 * state the theme fades in on. Each Hint instance flickered on its first
 * open (fresh instances mount constantly in virtualized lists), which
 * read app-wide as "every tooltip flickers once after launch". Fixed by
 * disabling bits-ui's internal timer (hint.svelte: delayDuration={0} on
 * the Root) and animating `instant-open` on the canonical pill
 * (tooltip-content.svelte).
 *
 * A MutationObserver installed before app code records a timestamped
 * lifecycle timeline of every pill (`[data-slot="tooltip-content"]`) —
 * mounts, unmounts, data-state flips and CSS animation starts. The red
 * signal is an enter animation starting >50ms after its pill mounted;
 * the exit fade-out (data-state=closed) is by design and excluded.
 *
 * Asserted on live DOM state against the mocked Tauri backend (see
 * tauri-mock.ts and smoke.spec.ts for the boot pattern).
 */

import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";
const DEEP_DIR = "app/src/main/java/com/raulshma/lenscast/streaming";
const rowPath = (n: number) => `${DEEP_DIR}/EncodedStreamHub${n}.kt`;

const entries = Array.from({ length: 200 }, (_, i) => ({
  path: rowPath(i),
  old_path: null,
  index: "unmodified",
  worktree: "modified",
}));

interface PillSnapshot {
  id: number;
  state: string | null;
  text: string;
  rect: { x: number; y: number; w: number; h: number };
  trigger: string;
}

interface PillEvent {
  t: number;
  kind: "add" | "remove" | "state" | "census" | "anim";
  id: number;
  state: string | null;
  text: string;
  rect?: { x: number; y: number; w: number; h: number };
  trigger?: string;
  anim?: string;
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
  // Pill lifecycle recorder — must be installed before app code runs.
  await page.addInitScript(() => {
    const w = window as typeof window & {
      __pillLog: PillEvent[];
      __lastCensus: string;
    };
    w.__pillLog = [];
    w.__lastCensus = "";
    const ids = new WeakMap<Element, number>();
    let nextId = 1;
    const idOf = (el: Element): number => {
      let id = ids.get(el);
      if (id === undefined) {
        id = nextId++;
        ids.set(el, id);
      }
      return id;
    };
    const log = (kind: PillEvent["kind"], el: Element): void => {
      w.__pillLog.push({
        t: Math.round(performance.now()),
        kind,
        id: idOf(el),
        state: el.getAttribute("data-state"),
        text: (el.textContent ?? "").slice(0, 120),
        ...describe(el),
      });
    };
    const describe = (el: Element): Partial<PillEvent> => {
      const r = el.getBoundingClientRect();
      let trigger = "?";
      const id = el.getAttribute("id");
      if (id) {
        const t = document.querySelector(`[aria-describedby="${id}"]`);
        trigger = t
          ? `${t.tagName.toLowerCase()}.${(t.className ?? "").toString().split(" ")[0] ?? ""}`
          : "no-trigger";
      }
      return {
        rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) },
        trigger,
      };
    };
    const census = (): void => {
      const pills = Array.from(document.querySelectorAll(SEL)).map((el) => {
        const d = describe(el);
        return { id: idOf(el), state: el.getAttribute("data-state"), text: (el.textContent ?? "").slice(0, 120), ...d };
      });
      const key = JSON.stringify(pills);
      if (key !== w.__lastCensus) {
        w.__lastCensus = key;
        for (const p of pills) {
          w.__pillLog.push({
            t: Math.round(performance.now()),
            kind: "census",
            id: p.id,
            state: p.state,
            text: p.text,
            rect: p.rect,
            trigger: p.trigger,
          });
        }
      }
    };
    const SEL = '[data-slot="tooltip-content"]';
    // The portal may add a wrapper around the pill, so match the node
    // itself AND any pill inside it (still queryable while detached).
    const pillsIn = (n: Node): Element[] => {
      if (n.nodeType !== 1) return [];
      const el = n as Element;
      const found = Array.from(el.querySelectorAll(SEL));
      if (el.matches(SEL)) found.unshift(el);
      return found;
    };
    new MutationObserver((muts) => {
      for (const m of muts) {
        if (m.type === "childList") {
          for (const n of m.addedNodes) for (const el of pillsIn(n)) log("add", el);
          for (const n of m.removedNodes) for (const el of pillsIn(n)) log("remove", el);
        } else if (
          m.type === "attributes" &&
          m.attributeName === "data-state"
        ) {
          log("state", m.target as Element);
        }
      }
      census();
    }).observe(document, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-state"],
    });
    // Position drift without DOM changes (re-anchoring) also matters.
    setInterval(census, 250);
    // Enter-animation restarts on a pill = the visible flicker. The pill
    // surfaces `tw-animate`'s fade-in as a CSS animation; log every start.
    document.addEventListener(
      "animationstart",
      (e) => {
        const path = e.composedPath();
        for (const n of path) {
          if (n instanceof Element && n.matches(SEL)) {
            w.__pillLog.push({
              t: Math.round(performance.now()),
              kind: "anim",
              id: idOf(n),
              state: n.getAttribute("data-state"),
              text: (n.textContent ?? "").slice(0, 40),
              anim: `${e.animationName}@${Math.round(e.elapsedTime * 1000)}ms`,
            });
            break;
          }
        }
      },
      { capture: true },
    );
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
  await page.goto("/");
}

async function pillLog(page: Page): Promise<PillEvent[]> {
  return page.evaluate(() => (window as unknown as { __pillLog: PillEvent[] }).__pillLog);
}

function dump(tag: string, events: PillEvent[]): void {
  console.log(`[pill-timeline:${tag}]`);
  for (const e of events) {
    const r = e.rect ? ` rect=${e.rect.x},${e.rect.y} ${e.rect.w}x${e.rect.h}` : "";
    const tr = e.trigger ? ` trigger=${e.trigger}` : "";
    console.log(
      `  t=${String(e.t).padStart(5)}ms ${e.kind.padEnd(6)} pill#${e.id} state=${e.state}${r}${tr} "${e.text}"`,
    );
  }
  if (events.length === 0) console.log("  (no pill events)");
}

test("A: boot with no interaction shows no pill flicker", async ({ page }) => {
  test.setTimeout(90_000); // cold dev-server compiles the panel
  const tracked = trackErrors(page);
  await boot(page);

  // Let the app fully settle: boot, repo auto-open, layout, fonts.
  await page.waitForTimeout(4_000);

  const events = await pillLog(page);
  dump("boot", events);
  expect(events, `pills appeared at launch: ${JSON.stringify(events)}`).toEqual([]);
  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("B: pill enter animation never restarts on a visible pill", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const tracked = trackErrors(page);
  await boot(page);

  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();
  const row10 = page.getByRole("treeitem", { name: rowPath(10) });
  await expect(row10).toBeVisible({ timeout: 20_000 });

  /** Late-enter-anims in the events recorded AFTER `mark`. */
  const now = async (): Promise<number> =>
    page.evaluate(() => Math.round(performance.now()));
  const lateAnimsSince = async (mark: number): Promise<PillEvent[]> => {
    const log = (await pillLog(page)).filter((e) => e.t >= mark);
    const addT = new Map(
      log.filter((e) => e.kind === "add").map((e) => [e.id, e.t]),
    );
    // Only ENTER animations while the pill is open count: the `exit`
    // fade-out fires in data-state=closed on close — by design, and
    // always "late" relative to mount.
    return log
      .filter(
        (e) =>
          e.kind === "anim" &&
          e.state !== null &&
          e.state !== "closed",
      )
      .filter((e) => e.t - (addT.get(e.id) ?? e.t) > 50);
  };

  // 1. First dwell after launch.
  let mark = await now();
  await row10.hover();
  await page.waitForTimeout(700); // dwell + any late flip-flop lands here
  const events = await pillLog(page);
  dump("first-dwell", events);
  const removes = events.filter((e) => e.kind === "remove");
  const closes = events.filter((e) => e.kind === "state" && e.state === "closed");
  expect(removes, "pill unmounted while the pointer dwelled").toEqual([]);
  expect(closes, "pill state flipped closed while dwelling").toEqual([]);

  // 2. Disarm, then dwell a FRESH instance (row 11): every pill's first
  //    show must behave the same, not only the first one after launch.
  await page.mouse.move(10, 300); // off the rows, not onto another trigger
  await page.waitForTimeout(600); // > UNARM_DELAY_MS (400)
  const row11 = page.getByRole("treeitem", { name: rowPath(11) });
  await row11.hover();
  await page.waitForTimeout(700);

  const late = await lateAnimsSince(mark); // both dwells
  expect(
    late,
    `enter animation restarted on a visible pill (the flicker): ${JSON.stringify(late)}`,
  ).toEqual([]);
  // The second dwell's pill is actually open at the end.
  const first = await page
    .locator('[data-slot="tooltip-content"]')
    .first()
    .getAttribute("data-state");
  expect(["delayed-open", "instant-open"]).toContain(first);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});
