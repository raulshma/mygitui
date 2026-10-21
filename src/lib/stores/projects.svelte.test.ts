/**
 * Unit tests for the detected-projects cache (`$lib/stores/projects.svelte`).
 * The IPC client is fully mocked — no Tauri runtime is touched.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { projectDetect } from "$lib/ipc/client";
import type { ProjectInfo } from "$lib/ipc/client";
import { ProjectsStore, projectsStore } from "$lib/stores/projects.svelte";

vi.mock("$lib/ipc/client", () => ({
  projectDetect: vi.fn(),
}));

const mockDetect = vi.mocked(projectDetect);

const NODE: ProjectInfo = {
  kind: "node",
  name: "web",
  path: ".",
  detected_from: ["package.json"],
  scripts: [
    {
      id: "node:.:test",
      label: "test",
      command: "npm test",
      kind: "test",
      watch: false,
    },
  ],
};

beforeEach(() => {
  vi.resetAllMocks();
  mockDetect.mockResolvedValue([NODE]);
});

describe("ProjectsStore", () => {
  it("ensure scans once and caches the result", async () => {
    const store = new ProjectsStore();
    await store.ensure("r1");
    expect(mockDetect).toHaveBeenCalledTimes(1);
    expect(mockDetect).toHaveBeenCalledWith("r1");
    expect(store.projectsFor("r1")).toEqual([NODE]);
    expect(store.hasDetected("r1")).toBe(true);

    await store.ensure("r1");
    expect(mockDetect).toHaveBeenCalledTimes(1);
  });

  it("dedupes concurrent ensure calls", async () => {
    let resolve: (value: ProjectInfo[]) => void = () => {};
    mockDetect.mockImplementationOnce(
      () =>
        new Promise<ProjectInfo[]>((done) => {
          resolve = done;
        }),
    );
    const store = new ProjectsStore();
    const first = store.ensure("r1");
    const second = store.ensure("r1");
    expect(mockDetect).toHaveBeenCalledTimes(1);
    expect(store.loadingFor("r1")).toBe(true);
    resolve([NODE]);
    await Promise.all([first, second]);
    expect(store.projectsFor("r1")).toEqual([NODE]);
    expect(store.loadingFor("r1")).toBe(false);
  });

  it("refresh forces a rescan and clears a previous error", async () => {
    mockDetect.mockRejectedValueOnce(new Error("scan failed"));
    const store = new ProjectsStore();
    await store.ensure("r1");
    expect(store.errorFor("r1")).toContain("scan failed");
    expect(store.projectsFor("r1")).toEqual([]);
    expect(store.loadingFor("r1")).toBe(false);

    // A failed scan is not retried implicitly on every panel mount.
    await store.ensure("r1");
    expect(mockDetect).toHaveBeenCalledTimes(1);

    const projects = await store.refresh("r1");
    expect(projects).toEqual([NODE]);
    expect(store.errorFor("r1")).toBeNull();
  });

  it("drops stale responses (latest sequence wins)", async () => {
    let resolveFirst: (value: ProjectInfo[]) => void = () => {};
    mockDetect.mockImplementationOnce(
      () =>
        new Promise<ProjectInfo[]>((done) => {
          resolveFirst = done;
        }),
    );
    const store = new ProjectsStore();
    const first = store.refresh("r1");
    const second = store.refresh("r1");
    resolveFirst([{ ...NODE, name: "stale" }]);
    await Promise.all([first, second]);
    expect(store.projectsFor("r1")).toEqual([NODE]);
    expect(store.loadingFor("r1")).toBe(false);
  });

  it("clear drops one repo or all cached results", async () => {
    const store = new ProjectsStore();
    await store.ensure("r1");
    await store.ensure("r2");
    store.clear("r1");
    expect(store.hasDetected("r1")).toBe(false);
    expect(store.hasDetected("r2")).toBe(true);
    store.clear();
    expect(store.hasDetected("r2")).toBe(false);
  });
});

describe("singleton", () => {
  it("exports a shared ProjectsStore instance", () => {
    expect(projectsStore).toBeInstanceOf(ProjectsStore);
  });
});
