import { afterEach, describe, expect, it } from "vitest";
import {
  OVERVIEW_DEFAULTS,
  OverviewStore,
  OVERVIEW_STORAGE_KEY,
  parseOverviewState,
  wipeLegacyAgentKeys,
} from "./overviewStore.svelte";

/** Minimal DOM-Storage fake (in-memory, records removeItem calls). */
function fakeStorage(seed: Record<string, string> = {}): Storage {
  const map = new Map(Object.entries(seed));
  const store: Storage = {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    key: (index) => [...map.keys()][index] ?? null,
    removeItem: (key) => {
      map.delete(key);
    },
    setItem: (key, value) => {
      map.set(key, String(value));
    },
  };
  return store;
}

afterEach(() => {
  // The module singleton reads localStorage at import time; tests construct
  // their own instances, so nothing to reset here.
});

describe("parseOverviewState", () => {
  it("accepts the exact state shape", () => {
    expect(
      parseOverviewState({
        enabled: true,
        filter: "dirty",
        sort: "ahead",
        sortDir: "desc",
        chatRailOpen: false,
      }),
    ).toEqual({
      enabled: true,
      filter: "dirty",
      sort: "ahead",
      sortDir: "desc",
      chatRailOpen: false,
    });
  });

  it("drops payloads with unknown filter/sort/directions", () => {
    expect(
      parseOverviewState({
        enabled: false,
        filter: "bogus",
        sort: "name",
        sortDir: "asc",
        chatRailOpen: true,
      }),
    ).toBeNull();
    expect(
      parseOverviewState({
        enabled: false,
        filter: null,
        sort: "nope",
        sortDir: "asc",
        chatRailOpen: true,
      }),
    ).toBeNull();
    expect(
      parseOverviewState({
        enabled: false,
        filter: null,
        sort: "name",
        sortDir: "sideways",
        chatRailOpen: true,
      }),
    ).toBeNull();
    expect(parseOverviewState({ enabled: "yes" })).toBeNull();
    expect(parseOverviewState(null)).toBeNull();
    expect(parseOverviewState([1])).toBeNull();
  });
});

describe("OverviewStore persistence", () => {
  it("round-trips enabled + view prefs through the versioned envelope", () => {
    const storage = fakeStorage();
    const store = new OverviewStore(storage);
    expect(store.snapshot).toEqual(OVERVIEW_DEFAULTS);

    store.enable();
    store.setFilter("needs-push");
    store.setSort("ahead");
    store.setSortDir("desc");
    store.setChatRailOpen(false);

    const raw = storage.getItem(OVERVIEW_STORAGE_KEY);
    expect(raw).not.toBeNull();
    const envelope = JSON.parse(raw!) as { v: number; data: unknown };
    expect(envelope.v).toBe(1);
    expect(envelope.data).toEqual({
      enabled: true,
      filter: "needs-push",
      sort: "ahead",
      sortDir: "desc",
      chatRailOpen: false,
    });

    // A fresh instance reloads the persisted state.
    const reloaded = new OverviewStore(storage);
    expect(reloaded.enabled).toBe(true);
    expect(reloaded.filter).toBe("needs-push");
    expect(reloaded.sort).toBe("ahead");
    expect(reloaded.sortDir).toBe("desc");
    expect(reloaded.chatRailOpen).toBe(false);
  });

  it("parse-and-drop: a malformed payload re-defaults, never throws", () => {
    const storage = fakeStorage({
      [OVERVIEW_STORAGE_KEY]: JSON.stringify({ v: 1, data: { enabled: 3 } }),
    });
    const store = new OverviewStore(storage);
    expect(store.snapshot).toEqual(OVERVIEW_DEFAULTS);
  });

  it("legacy unversioned payloads still load (envelope v1 rule)", () => {
    const storage = fakeStorage({
      [OVERVIEW_STORAGE_KEY]: JSON.stringify({
        enabled: true,
        filter: null,
        sort: "name",
        sortDir: "asc",
        chatRailOpen: true,
      }),
    });
    const store = new OverviewStore(storage);
    expect(store.enabled).toBe(true);
  });

  it("setSort flips direction on the same column, resets on a new one", () => {
    const store = new OverviewStore(null);
    store.setSort("name"); // new column, asc default for name
    expect(store.sort).toBe("name");
    expect(store.sortDir).toBe("asc");
    store.setSort("name"); // same column → flip
    expect(store.sortDir).toBe("desc");
    store.setSort("ahead"); // new column → desc default (numbers)
    expect(store.sort).toBe("ahead");
    expect(store.sortDir).toBe("desc");
  });

  it("toggles and reports", () => {
    const store = new OverviewStore(null);
    expect(store.enabled).toBe(false);
    expect(store.toggle()).toBe(true);
    expect(store.enabled).toBe(true);
    expect(store.toggle()).toBe(false);
    store.enable();
    expect(store.enabled).toBe(true);
    store.disable();
    expect(store.enabled).toBe(false);
  });

  it("missing storage stays in-memory only", () => {
    const store = new OverviewStore(null);
    store.enable();
    expect(store.enabled).toBe(true);
  });
});

describe("wipeLegacyAgentKeys", () => {
  it("removes mygitui.agent and mygitui.agent.<root> keys, nothing else", () => {
    const storage = fakeStorage({
      "mygitui.agent": JSON.stringify({ v: 1, data: { enabled: true } }),
      "mygitui.agent./repos/alpha": JSON.stringify({ v: 1, data: {} }),
      "mygitui.agent./repos/beta": JSON.stringify({ v: 1, data: {} }),
      "mygitui.overview": "keep",
      "mygitui.recent-repos": "keep",
      "mygitui.agentfoo": "keep",
    });
    const removed = wipeLegacyAgentKeys(storage);
    expect(removed.sort()).toEqual([
      "mygitui.agent",
      "mygitui.agent./repos/alpha",
      "mygitui.agent./repos/beta",
    ]);
    expect(storage.getItem("mygitui.overview")).toBe("keep");
    expect(storage.getItem("mygitui.agentfoo")).toBe("keep");
    expect(storage.getItem("mygitui.agent")).toBeNull();
  });

  it("is idempotent and tolerates missing storage", () => {
    const storage = fakeStorage({ "mygitui.agent": "x" });
    expect(wipeLegacyAgentKeys(storage)).toEqual(["mygitui.agent"]);
    expect(wipeLegacyAgentKeys(storage)).toEqual([]);
    expect(wipeLegacyAgentKeys(null)).toEqual([]);
  });
});
