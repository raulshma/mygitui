/**
 * Unit tests for the stacked active-view store
 * (`$lib/layout/workspaceStacked.svelte`): persistence through the
 * `mygitui.stacked.view` envelope key, hydrate-once semantics, and the
 * fallback contract.
 *
 * The store is a process-wide singleton, so every test imports a FRESH
 * module instance via `vi.resetModules` — same pattern as
 * workspaceDrawers.svelte.test.ts.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { STACKED_DEFAULT_VIEW, STACKED_PRIMARY_VIEWS } from "./stackedPlan";
import { STACKED_VIEW_PREF_KEY } from "./workspaceStacked.svelte";

beforeEach(() => {
  window.localStorage.clear();
});

/** A fresh store instance (empty caches, un-hydrated). */
async function freshStore() {
  vi.resetModules();
  const mod = await import("./workspaceStacked.svelte");
  return mod.workspaceStacked;
}

describe("workspaceStacked", () => {
  it("starts on the editor default before hydration", async () => {
    const stacked = await freshStore();
    expect(stacked.active).toBe(STACKED_DEFAULT_VIEW);
  });

  it("refresh hydrates the persisted view once", async () => {
    window.localStorage.setItem(
      STACKED_VIEW_PREF_KEY,
      JSON.stringify({ v: 1, data: "history" }),
    );
    const stacked = await freshStore();
    stacked.refresh("files");
    expect(stacked.active).toBe("history");
    // Later calls keep the hydrated choice (in-session switches win).
    stacked.refresh("status");
    expect(stacked.active).toBe("history");
  });

  it("refresh falls back when nothing was persisted", async () => {
    const stacked = await freshStore();
    stacked.refresh("forge");
    expect(stacked.active).toBe("forge");
  });

  it("refresh falls back when the persisted id is unknown", async () => {
    window.localStorage.setItem(
      STACKED_VIEW_PREF_KEY,
      JSON.stringify({ v: 1, data: "not-a-panel" }),
    );
    const stacked = await freshStore();
    stacked.refresh("diff");
    expect(stacked.active).toBe("diff");
  });

  it("refresh survives storage being unavailable", async () => {
    vi.stubGlobal("localStorage", undefined);
    try {
      const stacked = await freshStore();
      stacked.refresh("files");
      expect(stacked.active).toBe("files");
      // Switching still works in-memory (the write is swallowed).
      stacked.setActive("status");
      expect(stacked.active).toBe("status");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("setActive switches and writes through", async () => {
    const stacked = await freshStore();
    for (const view of STACKED_PRIMARY_VIEWS) {
      stacked.setActive(view);
      expect(stacked.active).toBe(view);
    }
    // Immediate write (no debounce — one small write per explicit action).
    expect(
      JSON.parse(
        window.localStorage.getItem(STACKED_VIEW_PREF_KEY) ?? "null",
      ),
    ).toEqual({ v: 1, data: "forge" });
  });

  it("a reloaded session resumes the last stacked view", async () => {
    const stacked = await freshStore();
    stacked.setActive("terminal");
    const reloaded = await freshStore();
    reloaded.refresh("diff");
    expect(reloaded.active).toBe("terminal");
  });

  it("an in-session switch wins over a later refresh (hydrate once)", async () => {
    const stacked = await freshStore();
    stacked.refresh("diff");
    stacked.setActive("files");
    // Re-entering Stacked re-runs the caller's refresh — no clobbering.
    stacked.refresh("status");
    expect(stacked.active).toBe("files");
  });
});
