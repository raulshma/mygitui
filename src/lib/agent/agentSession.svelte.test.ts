/**
 * Tests for the per-repo Agent-mode session store (`agentSession`):
 * parse validation, load/save round-trips through the versioned
 * localStorage envelope, ratio clamping on read, per-root independence,
 * the debounced write, and flush/forget eviction.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  AGENT_SESSION_DEBOUNCE_MS,
  AGENT_SESSION_KEY_PREFIX,
  AgentSessionStore,
  emptyAgentSession,
  flushAgentSessions,
  parseStoredAgentSession,
  patchAgentSession,
  resetAgentSessions,
  type StoredAgentSession,
} from "./agentSession.svelte";

const ROOT = "/repo";
const KEY = AGENT_SESSION_KEY_PREFIX + ROOT;

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

/** A valid stored payload (the shape AgentView writes). */
function validSession(): StoredAgentSession {
  return {
    selectedPath: "src/a.ts",
    selectedSide: "index",
    leftRatio: 0.3,
    rightRatio: 0.68,
  };
}

beforeEach(() => {
  localStorage.clear();
  resetAgentSessions();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("parseStoredAgentSession", () => {
  it("accepts a valid payload", () => {
    expect(parseStoredAgentSession(validSession())).toEqual(validSession());
  });

  it("fills absent fields with defaults", () => {
    expect(parseStoredAgentSession({})).toEqual(emptyAgentSession());
    expect(
      parseStoredAgentSession({ selectedPath: "a.ts" }),
    ).toMatchObject({ selectedPath: "a.ts", selectedSide: "worktree" });
  });

  it("drops wrong-typed selection fields (parse-and-drop)", () => {
    expect(parseStoredAgentSession({ ...validSession(), selectedPath: 42 })).toBeNull();
    expect(parseStoredAgentSession({ ...validSession(), selectedSide: "staged" })).toBeNull();
  });

  it("degrades a malformed ratio to the default (like scroll entries)", () => {
    expect(
      parseStoredAgentSession({ ...validSession(), leftRatio: "0.5" }),
    ).toMatchObject({ leftRatio: emptyAgentSession().leftRatio, rightRatio: 0.68 });
  });

  it("rejects non-object payloads", () => {
    expect(parseStoredAgentSession(null)).toBeNull();
    expect(parseStoredAgentSession(42)).toBeNull();
    expect(parseStoredAgentSession("nope")).toBeNull();
    expect(parseStoredAgentSession([validSession()])).toBeNull();
  });

  it("clamps ratios into the splitter range on read", () => {
    expect(parseStoredAgentSession({ leftRatio: 2 })?.leftRatio).toBeLessThanOrEqual(1);
    expect(parseStoredAgentSession({ rightRatio: -2 })?.rightRatio).toBeGreaterThanOrEqual(0);
  });
});

describe("AgentSessionStore", () => {
  it("defaults to an empty session with empty storage", () => {
    const store = new AgentSessionStore({ storage: memoryStorage() });
    expect(store.ensure(ROOT)).toEqual(emptyAgentSession());
  });

  it("patch persists an enveloped payload and a new store rehydrates it", () => {
    const storage = memoryStorage();
    const store = new AgentSessionStore({ storage });
    store.patch(ROOT, validSession());
    // Debounced: nothing written yet.
    expect(storage.getItem(KEY)).toBeNull();
    store.flush();

    const fresh = new AgentSessionStore({ storage });
    expect(fresh.ensure(ROOT)).toEqual(validSession());
  });

  it("keys sessions per root", () => {
    const store = new AgentSessionStore({ storage: memoryStorage() });
    store.patch("/repo-a", { selectedPath: "a.ts" });
    store.patch("/repo-b", { selectedPath: "b.ts" });
    expect(store.ensure("/repo-a").selectedPath).toBe("a.ts");
    expect(store.ensure("/repo-b").selectedPath).toBe("b.ts");
  });

  it("drops a malformed stored payload instead of throwing", () => {
    const storage = memoryStorage();
    storage.setItem(KEY, JSON.stringify({ v: 1, data: { selectedSide: 7 } }));
    const store = new AgentSessionStore({ storage });
    expect(store.ensure(ROOT)).toEqual(emptyAgentSession());
  });

  it("flush() writes the pending debounce immediately", () => {
    vi.useFakeTimers();
    try {
      const storage = memoryStorage();
      const store = new AgentSessionStore({ storage });
      store.patch(ROOT, { selectedPath: "late.ts" });
      expect(storage.getItem(KEY)).toBeNull();
      store.flush();
      const raw = storage.getItem(KEY);
      expect(raw).not.toBeNull();
      const envelope = JSON.parse(raw!) as { v: number; data: StoredAgentSession };
      expect(envelope.data.selectedPath).toBe("late.ts");
      // And the debounce timer does not double-write afterwards.
      vi.advanceTimersByTime(AGENT_SESSION_DEBOUNCE_MS);
      expect(JSON.parse(storage.getItem(KEY)!).data.selectedPath).toBe("late.ts");
    } finally {
      vi.useRealTimers();
    }
  });

  it("forget evicts memory and storage", () => {
    const storage = memoryStorage();
    const store = new AgentSessionStore({ storage });
    store.patch(ROOT, { selectedPath: "a.ts" });
    store.flush();
    expect(storage.getItem(KEY)).not.toBeNull();

    store.forget(ROOT);
    expect(storage.getItem(KEY)).toBeNull();
    expect(store.ensure(ROOT)).toEqual(emptyAgentSession());
  });
});

describe("agentSession module singleton", () => {
  it("patch + flush round-trips through the app storage", () => {
    patchAgentSession(ROOT, { selectedPath: "kept.ts" });
    flushAgentSessions();
    expect(localStorage.getItem(KEY)).not.toBeNull();
  });
});
