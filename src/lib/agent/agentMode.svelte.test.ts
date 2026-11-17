import { beforeEach, describe, expect, it } from "vitest";
import {
  AGENT_MODE_DEFAULTS,
  AGENT_STORAGE_KEY,
  AgentModeStore,
  parseAgentState,
} from "./agentMode.svelte";

/** A storage stub that never throws (write failures swallowed). */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k) => map.get(k) ?? null,
    key: (i) => [...map.keys()][i] ?? null,
    removeItem: (k) => map.delete(k),
    setItem: (k, v) => void map.set(k, v),
  };
}

describe("parseAgentState", () => {
  it("accepts a valid payload", () => {
    expect(parseAgentState({ enabled: true })).toEqual({ enabled: true });
    expect(parseAgentState({ enabled: false })).toEqual({ enabled: false });
  });

  it("rejects a non-boolean enabled", () => {
    expect(parseAgentState({ enabled: "yes" })).toBeNull();
    expect(parseAgentState({ enabled: 1 })).toBeNull();
    expect(parseAgentState({ enabled: null })).toBeNull();
    expect(parseAgentState({})).toBeNull();
  });

  it("rejects non-objects and null", () => {
    expect(parseAgentState(null)).toBeNull();
    expect(parseAgentState("nope")).toBeNull();
    expect(parseAgentState(42)).toBeNull();
    expect(parseAgentState([true])).toBeNull();
  });
});

describe("AgentModeStore", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("defaults to enabled=false with empty storage", () => {
    const store = new AgentModeStore(memoryStorage());
    expect(store.enabled).toBe(false);
  });

  it("set persists an enveloped payload and a new store rehydrates it", () => {
    const storage = memoryStorage();
    const store = new AgentModeStore(storage);
    store.set(true);
    expect(store.enabled).toBe(true);

    const raw = storage.getItem(AGENT_STORAGE_KEY);
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw as string)).toEqual({ v: 1, data: { enabled: true } });

    const reloaded = new AgentModeStore(storage);
    expect(reloaded.enabled).toBe(true);
  });

  it("toggle flips, returns the new value, and persists for rehydration", () => {
    const storage = memoryStorage();
    const store = new AgentModeStore(storage);
    expect(store.toggle()).toBe(true);
    expect(store.enabled).toBe(true);
    expect(store.toggle()).toBe(false);
    expect(store.enabled).toBe(false);

    const reloaded = new AgentModeStore(storage);
    expect(reloaded.enabled).toBe(false);
  });

  it("falls back to the default on malformed payloads without throwing", () => {
    const storage = memoryStorage();
    storage.setItem(AGENT_STORAGE_KEY, "{not json");
    expect(new AgentModeStore(storage).enabled).toBe(false);

    const wrongShape = memoryStorage();
    wrongShape.setItem(AGENT_STORAGE_KEY, JSON.stringify({ v: 1, data: { enabled: "yes" } }));
    expect(new AgentModeStore(wrongShape).enabled).toBe(false);
  });

  it("a throwing storage.setItem does not break the session", () => {
    const storage = memoryStorage();
    storage.setItem = () => {
      throw new Error("quota exceeded");
    };
    const store = new AgentModeStore(storage);
    expect(() => store.set(true)).not.toThrow();
    expect(store.enabled).toBe(true);
    expect(store.toggle()).toBe(false);
  });

  it("works without any storage (state lives in memory only)", () => {
    const store = new AgentModeStore(null);
    expect(store.enabled).toBe(false);
    store.set(true);
    expect(store.enabled).toBe(true);
  });

  it("defaults match AGENT_MODE_DEFAULTS", () => {
    expect(new AgentModeStore(memoryStorage()).enabled).toBe(AGENT_MODE_DEFAULTS.enabled);
  });
});
