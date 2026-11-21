/**
 * Unit tests for the workspace drawer open-state store
 * (`$lib/layout/workspaceDrawers.svelte`): per-mode persistence through
 * splitPrefs, mode-flip refresh, and toggle semantics.
 *
 * The store (and the splitPrefs cache under it) is a process-wide
 * singleton, so every test imports a FRESH module instance via
 * `vi.resetModules` — same pattern as seeding unique keys, which the fixed
 * pref keys can't do.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DRAWER_PREF_KEY } from "./drawerPlan";

beforeEach(() => {
  vi.useFakeTimers();
  window.localStorage.clear();
});

afterEach(() => {
  vi.useRealTimers();
});

/** A fresh store + splitPrefs instance (empty caches, no pending writes). */
async function freshStore() {
  vi.resetModules();
  const mod = await import("./workspaceDrawers.svelte");
  return mod.workspaceDrawers;
}

describe("workspaceDrawers persistence", () => {
  it("setOpen writes through to the panel's per-mode split pref", async () => {
    const drawers = await freshStore();
    drawers.setOpen("files", "medium", true);
    vi.advanceTimersByTime(150);
    expect(
      window.localStorage.getItem(`mygitui.split.${DRAWER_PREF_KEY.files}`),
    ).toBe(JSON.stringify({ v: 2, data: { modes: { medium: true } } }));
    expect(drawers.isOpen("files")).toBe(true);
  });

  it("refreshMode re-reads the persisted state after a mode flip", async () => {
    const drawers = await freshStore();
    drawers.setOpen("status", "narrow", true);
    drawers.setOpen("status", "medium", false);
    vi.advanceTimersByTime(150);

    drawers.refreshMode("medium");
    expect(drawers.isOpen("status")).toBe(false);
    drawers.refreshMode("narrow");
    expect(drawers.isOpen("status")).toBe(true);
  });

  it("refreshMode applies the fallback for modes the user never toggled", async () => {
    const drawers = await freshStore();
    // Fresh install, entering Medium: the history drawer starts open.
    drawers.refreshMode("medium");
    expect(drawers.isOpen("history")).toBe(true);
    // …while files/status start closed (rail toggle is the affordance).
    expect(drawers.isOpen("files")).toBe(false);
    expect(drawers.isOpen("status")).toBe(false);
  });

  it("refreshMode leaves panels the mode keeps docked untouched", async () => {
    const drawers = await freshStore();
    drawers.setOpen("files", "medium", true);
    vi.advanceTimersByTime(150);
    drawers.refreshMode("wide");
    // Wide docks files; the stale open value stays but is never read.
    expect(drawers.isOpen("files")).toBe(true);
    // Re-entering Medium restores the persisted (open) state.
    drawers.refreshMode("medium");
    expect(drawers.isOpen("files")).toBe(true);
  });

  it("toggle flips and persists in one step", async () => {
    const drawers = await freshStore();
    drawers.refreshMode("narrow");
    // Narrow's fresh-install fallback opens history — the first toggle closes it.
    expect(drawers.isOpen("history")).toBe(true);
    expect(drawers.toggle("history", "narrow")).toBe(false);
    expect(drawers.isOpen("history")).toBe(false);
    expect(drawers.toggle("history", "narrow")).toBe(true);
    expect(drawers.isOpen("history")).toBe(true);
    vi.advanceTimersByTime(150);

    const reopened = await freshStore();
    reopened.refreshMode("narrow");
    expect(reopened.isOpen("history")).toBe(true);
  });

  it("keeps modes independent (closing in Narrow does not close in Medium)", async () => {
    const drawers = await freshStore();
    drawers.refreshMode("medium");
    drawers.setOpen("history", "medium", true);
    drawers.refreshMode("narrow");
    drawers.setOpen("history", "narrow", false);
    vi.advanceTimersByTime(150);

    const medium = await freshStore();
    medium.refreshMode("medium");
    expect(medium.isOpen("history")).toBe(true);
    const narrow = await freshStore();
    narrow.refreshMode("narrow");
    expect(narrow.isOpen("history")).toBe(false);
  });
});
