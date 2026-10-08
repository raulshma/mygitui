/**
 * Unit tests for the per-repo conflicts cache (`conflictsStore.svelte.ts`).
 * The IPC client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { conflicts } from "$lib/ipc/client";
import type { ConflictFile } from "$lib/ipc/types";
import { ConflictsStore } from "./conflictsStore.svelte";

vi.mock("$lib/ipc/client", () => ({
  conflicts: vi.fn(),
}));

const mockConflicts = vi.mocked(conflicts);

function file(path: string): ConflictFile {
  return {
    path,
    has_base: true,
    has_ours: true,
    has_theirs: true,
    source: "merge",
  };
}

beforeEach(() => {
  mockConflicts.mockReset();
});

describe("ConflictsStore", () => {
  it("reports count 0 / no error for repos never loaded", () => {
    const store = new ConflictsStore();
    expect(store.count("r1")).toBe(0);
    expect(store.files("r1")).toEqual([]);
    expect(store.isLoading("r1")).toBe(false);
    expect(store.error("r1")).toBeNull();
  });

  it("load fetches, caches and exposes the count", async () => {
    const store = new ConflictsStore();
    mockConflicts.mockResolvedValue([file("a.txt"), file("b.txt")]);

    const files = await store.load("r1");

    expect(mockConflicts).toHaveBeenCalledWith("r1");
    expect(files.map((f) => f.path)).toEqual(["a.txt", "b.txt"]);
    expect(store.count("r1")).toBe(2);
    expect(store.files("r1")).toHaveLength(2);
    expect(store.isLoading("r1")).toBe(false);
    expect(store.error("r1")).toBeNull();
  });

  it("refetches on every load and updates the cache", async () => {
    const store = new ConflictsStore();
    mockConflicts.mockResolvedValueOnce([file("a.txt")]);
    mockConflicts.mockResolvedValueOnce([file("a.txt"), file("b.txt"), file("c.txt")]);

    await store.load("r1");
    await store.load("r1");

    expect(mockConflicts).toHaveBeenCalledTimes(2);
    expect(store.count("r1")).toBe(3);
  });

  it("joins an in-flight load instead of stacking fetches", async () => {
    const store = new ConflictsStore();
    let release!: (files: ConflictFile[]) => void;
    mockConflicts.mockReturnValue(
      new Promise<ConflictFile[]>((resolve) => {
        release = resolve;
      }),
    );

    const p1 = store.load("r1");
    const p2 = store.load("r1");
    release([file("a.txt")]);
    const [f1, f2] = await Promise.all([p1, p2]);

    expect(mockConflicts).toHaveBeenCalledTimes(1);
    expect(f1).toEqual(f2);
    expect(store.count("r1")).toBe(1);
  });

  it("never throws: failures land in error and keep the old cache", async () => {
    const store = new ConflictsStore();
    mockConflicts.mockResolvedValueOnce([file("a.txt")]);
    await store.load("r1");

    mockConflicts.mockRejectedValueOnce(new Error("repo closed"));
    const files = await store.load("r1");

    expect(files).toHaveLength(1);
    expect(store.error("r1")).toBe("repo closed");
    expect(store.count("r1")).toBe(1);
    expect(store.isLoading("r1")).toBe(false);
  });

  it("keeps repos isolated", async () => {
    const store = new ConflictsStore();
    mockConflicts.mockResolvedValueOnce([file("a.txt")]);
    mockConflicts.mockResolvedValueOnce([]);

    await store.load("r1");
    await store.load("r2");

    expect(store.count("r1")).toBe(1);
    expect(store.count("r2")).toBe(0);
  });

  it("clear drops only the target repo's cache", async () => {
    const store = new ConflictsStore();
    mockConflicts.mockResolvedValue([file("a.txt")]);
    await store.load("r1");
    await store.load("r2");

    store.clear("r1");

    expect(store.count("r1")).toBe(0);
    expect(store.error("r1")).toBeNull();
    expect(store.count("r2")).toBe(1);
  });

  it("clearAll resets every repo", async () => {
    const store = new ConflictsStore();
    mockConflicts.mockResolvedValue([file("a.txt")]);
    await store.load("r1");
    await store.load("r2");

    store.clearAll();

    expect(store.count("r1")).toBe(0);
    expect(store.count("r2")).toBe(0);
  });
});
