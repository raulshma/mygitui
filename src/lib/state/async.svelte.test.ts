/**
 * Tests for the shared async-data machine (`$lib/state/async.svelte`):
 * state transitions, refresh-over-ready `previous` handling, stale-run
 * drops, and reset invalidation.
 */
import { describe, expect, it } from "vitest";
import { AsyncMap } from "./async.svelte";

describe("AsyncMap", () => {
  it("reads unknown keys as idle", () => {
    const map = new AsyncMap<number>();
    expect(map.get("k")).toEqual({ state: "idle" });
  });

  it("lands a successful run as ready", async () => {
    const map = new AsyncMap<number>();
    const value = await map.run("k", () => Promise.resolve(7));
    expect(value).toBe(7);
    expect(map.get("k")).toEqual({ state: "ready", data: 7 });
  });

  it("lands a failed run as error with the described message", async () => {
    const map = new AsyncMap<number>();
    const value = await map.run("k", () => Promise.reject(new Error("boom")));
    expect(value).toBeNull();
    expect(map.get("k")).toEqual({
      state: "error",
      message: "boom",
      previous: null,
    });
  });

  it("keeps ready data as previous while a refresh is in flight", async () => {
    const map = new AsyncMap<number>();
    await map.run("k", () => Promise.resolve(1));
    let release!: (v: number) => void;
    const inFlight = map.run(
      "k",
      () => new Promise<number>((r) => (release = r)),
    );
    expect(map.get("k")).toEqual({ state: "loading", previous: 1 });
    release(2);
    expect(await inFlight).toBe(2);
    expect(map.get("k")).toEqual({ state: "ready", data: 2 });
  });

  it("keeps previous data visible when a refresh fails", async () => {
    const map = new AsyncMap<number>();
    await map.run("k", () => Promise.resolve(1));
    await map.run("k", () => Promise.reject(new Error("nope")));
    expect(map.get("k")).toEqual({
      state: "error",
      message: "nope",
      previous: 1,
    });
  });

  it("clears a previous error the moment a new run starts", async () => {
    const map = new AsyncMap<number>();
    await map.run("k", () => Promise.reject(new Error("first")));
    let release!: (v: number) => void;
    const inFlight = map.run(
      "k",
      () => new Promise<number>((r) => (release = r)),
    );
    expect(map.get("k")).toEqual({ state: "loading", previous: null });
    release(3);
    await inFlight;
    expect(map.get("k")).toEqual({ state: "ready", data: 3 });
  });

  it("drops a superseded run: its result never lands and it resolves null", async () => {
    const map = new AsyncMap<number>();
    let releaseA!: (v: number) => void;
    const a = map.run("k", () => new Promise<number>((r) => (releaseA = r)));
    const b = await map.run("k", () => Promise.resolve(2));
    expect(b).toBe(2);
    expect(map.get("k")).toEqual({ state: "ready", data: 2 });
    releaseA(1);
    expect(await a).toBeNull();
    expect(map.get("k")).toEqual({ state: "ready", data: 2 });
  });

  it("reset drops the entry and invalidates an in-flight run", async () => {
    const map = new AsyncMap<number>();
    await map.run("k", () => Promise.resolve(1));
    let release!: (v: number) => void;
    const inFlight = map.run(
      "k",
      () => new Promise<number>((r) => (release = r)),
    );
    map.reset("k");
    expect(map.get("k")).toEqual({ state: "idle" });
    release(9);
    expect(await inFlight).toBeNull();
    expect(map.get("k")).toEqual({ state: "idle" });
  });

  it("silent runs keep the visible entry while in flight", async () => {
    const map = new AsyncMap<number>();
    await map.run("k", () => Promise.resolve(1));
    let release!: (v: number) => void;
    const inFlight = map.run(
      "k",
      () => new Promise<number>((r) => (release = r)),
      { silent: true },
    );
    expect(map.get("k")).toEqual({ state: "ready", data: 1 });
    release(2);
    expect(await inFlight).toBe(2);
    expect(map.get("k")).toEqual({ state: "ready", data: 2 });
  });

  it("a silent failure keeps ready data visible under the error", async () => {
    const map = new AsyncMap<number>();
    await map.run("k", () => Promise.resolve(1));
    await map.run("k", () => Promise.reject(new Error("background")), {
      silent: true,
    });
    expect(map.get("k")).toEqual({
      state: "error",
      message: "background",
      previous: 1,
    });
  });

  it("onError observes a failure without changing the landed state", async () => {
    const map = new AsyncMap<number>();
    const seen: unknown[] = [];
    await map.run("k", () => Promise.reject(new Error("boom")), {
      onError: (err) => seen.push(err),
    });
    expect(seen).toHaveLength(1);
    expect(map.get("k")).toEqual({
      state: "error",
      message: "boom",
      previous: null,
    });
  });

  it("set writes a ready entry directly", () => {
    const map = new AsyncMap<number>();
    map.set("k", 5);
    expect(map.get("k")).toEqual({ state: "ready", data: 5 });
  });

  it("resetPrefix drops matching entries and invalidates their runs", async () => {
    const map = new AsyncMap<number>();
    map.set("r1:1:a", 1);
    map.set("r1:1:b", 2);
    map.set("r1:2:a", 3);
    let release!: (v: number) => void;
    const inFlight = map.run(
      "r1:1:b",
      () => new Promise<number>((r) => (release = r)),
    );
    map.resetPrefix("r1:1:");
    expect(map.get("r1:1:a")).toEqual({ state: "idle" });
    expect(map.get("r1:1:b")).toEqual({ state: "idle" });
    expect(map.get("r1:2:a")).toEqual({ state: "ready", data: 3 });
    release(9);
    expect(await inFlight).toBeNull();
  });
});
