/**
 * Unit tests for the per-repo eviction registry (`repoEvictors.ts`): a pure
 * Set — registration order, unregister semantics and the reset helper.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  registerRepoEvictor,
  resetRepoEvictors,
  runRepoEvictors,
} from "./repoEvictors";

beforeEach(() => {
  resetRepoEvictors();
});

describe("repoEvictors", () => {
  it("runs every registered evictor with the repo id, in registration order", () => {
    const calls: string[] = [];
    registerRepoEvictor((repoId) => calls.push(`first:${repoId}`));
    registerRepoEvictor((repoId) => calls.push(`second:${repoId}`));

    runRepoEvictors("repo-1");

    expect(calls).toEqual(["first:repo-1", "second:repo-1"]);
  });

  it("unregister stops one evictor without touching the others", () => {
    const first = vi.fn();
    const second = vi.fn();
    const unregister = registerRepoEvictor(first);
    registerRepoEvictor(second);

    unregister();
    runRepoEvictors("repo-1");

    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith("repo-1");
  });

  it("unregistering twice (or an unknown callback) is a no-op", () => {
    const evictor = vi.fn();
    const unregister = registerRepoEvictor(evictor);

    unregister();
    expect(() => unregister()).not.toThrow();

    runRepoEvictors("repo-1");
    expect(evictor).not.toHaveBeenCalled();
  });

  it("one throwing evictor does not starve the others (fan-out resilience)", () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const after = vi.fn();
      registerRepoEvictor(() => {
        throw new Error("cache wedged");
      });
      registerRepoEvictor(after);

      expect(() => runRepoEvictors("repo-1")).not.toThrow();
      expect(after).toHaveBeenCalledWith("repo-1");
      expect(errSpy).toHaveBeenCalled();
    } finally {
      errSpy.mockRestore();
    }
  });

  it("reset drops every registration (test teardown)", () => {
    const evictor = vi.fn();
    registerRepoEvictor(evictor);

    resetRepoEvictors();
    runRepoEvictors("repo-1");

    expect(evictor).not.toHaveBeenCalled();
  });

  it("an empty registry runs nothing and never throws", () => {
    expect(() => runRepoEvictors("repo-1")).not.toThrow();
  });
});
