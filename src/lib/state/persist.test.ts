/**
 * Tests for the versioned storage envelope (`$lib/state/persist`):
 * unwrap/legacy reads, write round-trips, migration chains, and the
 * unknown-version drop rule.
 */
import { describe, expect, it } from "vitest";
import {
  loadVersioned,
  readEnvelope,
  writeEnvelope,
  type StorageLike,
} from "./persist";

function mapStorage(): StorageLike & { dump: (key: string) => string | null } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => void map.set(key, value),
    dump: (key) => map.get(key) ?? null,
  };
}

describe("readEnvelope", () => {
  it("unwraps a written envelope", () => {
    const storage = mapStorage();
    writeEnvelope(storage, "k", { pins: ["worktrees"] });
    expect(readEnvelope(storage, "k")).toEqual({
      version: 1,
      data: { pins: ["worktrees"] },
    });
  });

  it("reads a legacy unversioned payload as version 1", () => {
    const storage = mapStorage();
    storage.setItem("k", JSON.stringify(["worktrees", "reflog"]));
    expect(readEnvelope(storage, "k")).toEqual({
      version: 1,
      data: ["worktrees", "reflog"],
    });
  });

  it("returns null for absent, empty, malformed, or missing storage", () => {
    const storage = mapStorage();
    expect(readEnvelope(storage, "k")).toBeNull();
    storage.setItem("k", "");
    expect(readEnvelope(storage, "k")).toBeNull();
    storage.setItem("k", "not json");
    expect(readEnvelope(storage, "k")).toBeNull();
    expect(readEnvelope(null, "k")).toBeNull();
  });
});

describe("loadVersioned", () => {
  it("validates the unwrapped payload and passes it to parse", () => {
    const storage = mapStorage();
    writeEnvelope(storage, "k", { name: "x" });
    expect(
      loadVersioned({
        storage,
        key: "k",
        parse: (data) =>
          typeof data === "object" && data !== null && "name" in data
            ? (data as { name: string })
            : null,
      }),
    ).toEqual({ name: "x" });
  });

  it("resolves null when parse rejects the shape", () => {
    const storage = mapStorage();
    writeEnvelope(storage, "k", "not-a-shape");
    expect(
      loadVersioned({ storage, key: "k", parse: () => null }),
    ).toBeNull();
  });

  it("runs registered upgraders up to toVersion", () => {
    const storage = mapStorage();
    // A v1 payload of the old shape: plain string list.
    storage.setItem(
      "k",
      JSON.stringify({ v: 1, data: ["worktrees", "stashes"] }),
    );
    const migrated = loadVersioned<string[]>({
      storage,
      key: "k",
      toVersion: 2,
      migrations: {
        1: (data) => ({ pins: data }),
      },
      parse: (data) =>
        typeof data === "object" && data !== null && "pins" in data
          ? ((data as { pins: string[] }).pins)
          : null,
    });
    expect(migrated).toEqual(["worktrees", "stashes"]);
  });

  it("chains multiple migrations in order", () => {
    const storage = mapStorage();
    storage.setItem("k", JSON.stringify({ v: 1, data: 1 }));
    expect(
      loadVersioned<number>({
        storage,
        key: "k",
        toVersion: 3,
        migrations: {
          1: (n) => (n as number) + 1,
          2: (n) => (n as number) * 10,
        },
        parse: (n) => (typeof n === "number" ? n : null),
      }),
    ).toBe(20);
  });

  it("drops payloads newer than the registered migrations", () => {
    const storage = mapStorage();
    storage.setItem("k", JSON.stringify({ v: 2, data: { future: true } }));
    expect(
      loadVersioned({ storage, key: "k", parse: (d) => (typeof d === "object" ? (d as object) : null) }),
    ).toBeNull();
    // ...and a version with a missing rung in the chain.
    storage.setItem("k", JSON.stringify({ v: 1, data: 1 }));
    expect(
      loadVersioned<number>({
        storage,
        key: "k",
        toVersion: 3,
        migrations: { 2: (n) => (n as number) + 1 },
        parse: (n) => (typeof n === "number" ? n : null),
      }),
    ).toBeNull();
  });

  it("resolves null without storage", () => {
    expect(
      loadVersioned({ storage: null, key: "k", parse: () => "x" }),
    ).toBeNull();
  });
});
