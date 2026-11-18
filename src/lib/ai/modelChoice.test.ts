/**
 * Unit tests for the per-run model choice store (item 55): the per-repo
 * override round-trip (persist + reload, parse-and-drop), set/clear
 * semantics, and the recent-models derivation from the job history
 * (backend filter, success-only, dedupe, cap). The override threading
 * onto the provider request is covered in `ai.svelte.test.ts` (it needs
 * that file's IPC mock transport).
 */

import { describe, expect, it, vi } from "vitest";
import { MODEL_CHOICE_KEY, ModelChoiceStore, parseModelChoice, recentModels } from "./modelChoice.svelte";
import type { AiJobRecord } from "./jobs.svelte";

function memStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return {
    getItem: (key: string) => data[key] ?? null,
    setItem: (key: string, value: string) => {
      data[key] = value;
    },
    removeItem: (key: string) => {
      delete data[key];
    },
    dump: () => ({ ...data }),
  };
}

type Storage = ReturnType<typeof memStorage>;

function record(overrides: Partial<AiJobRecord>): AiJobRecord {
  return {
    id: "r",
    repoId: "repo-1",
    repoName: null,
    feature: "commit-message",
    status: "done",
    startedAt: 1_000,
    elapsedMs: 5,
    backend: "opencode",
    model: "m",
    usage: null,
    error: null,
    text: null,
    promptPreview: null,
    ...overrides,
  };
}

describe("ModelChoiceStore", () => {
  it("starts with no override and survives malformed storage", () => {
    expect(new ModelChoiceStore(memStorage()).overrideFor("repo-1")).toBeNull();
    const junk = memStorage({ [MODEL_CHOICE_KEY]: "{not json" });
    expect(new ModelChoiceStore(junk).overrideFor("repo-1")).toBeNull();
  });

  it("sets, replaces and clears the per-repo override", () => {
    const store = new ModelChoiceStore(memStorage());
    store.setOverride("repo-1", "openai/gpt-5");
    expect(store.overrideFor("repo-1")).toBe("openai/gpt-5");
    store.setOverride("repo-1", "anthropic/claude-opus-4.6");
    expect(store.overrideFor("repo-1")).toBe("anthropic/claude-opus-4.6");
    // A blank/undefined value clears (back to the settings default).
    store.setOverride("repo-1", "  ");
    expect(store.overrideFor("repo-1")).toBeNull();
    store.setOverride("repo-1", "openai/gpt-5");
    store.setOverride("repo-1", null);
    expect(store.overrideFor("repo-1")).toBeNull();
    // Choices are per repo — another repo is untouched.
    store.setOverride("repo-2", "openai/gpt-5");
    expect(store.overrideFor("repo-1")).toBeNull();
    expect(store.overrideFor("repo-2")).toBe("openai/gpt-5");
    store.clearAll();
    expect(store.overrideFor("repo-2")).toBeNull();
  });

  it("round-trips the override through the versioned envelope", () => {
    const storage = memStorage();
    const first = new ModelChoiceStore(storage);
    first.setOverride("repo-1", "openai/gpt-5");
    // Envelope shape on disk: { v, data: { byRepo } }.
    const raw = JSON.parse(storage.dump()[MODEL_CHOICE_KEY] ?? "{}");
    expect(raw.v).toBe(1);
    expect(raw.data.byRepo["repo-1"]).toBe("openai/gpt-5");
    const second = new ModelChoiceStore(storage);
    expect(second.overrideFor("repo-1")).toBe("openai/gpt-5");
  });

  it("drops junk rows on load (parse-and-drop)", () => {
    expect(
      parseModelChoice({ byRepo: { "repo-1": "m", "": "m2", "repo-3": 42, "repo-4": "" } }),
    ).toEqual({ byRepo: { "repo-1": "m" } });
    expect(parseModelChoice({ byRepo: "nope" })).toBeNull();
    expect(parseModelChoice(null)).toBeNull();
    expect(parseModelChoice({})).toBeNull();
  });
});

describe("recentModels (job history derivation)", () => {
  it("keeps only successful runs on the requested backend, newest first", () => {
    const history: AiJobRecord[] = [
      record({ model: "fresh", backend: "opencode", status: "done" }),
      record({ model: "failed", backend: "opencode", status: "error" }),
      record({ model: "cancelled", backend: "opencode", status: "cancelled" }),
      record({ model: "other-backend", backend: "openrouter", status: "done" }),
      record({ model: null, backend: "opencode", status: "done" }),
      record({ model: "older", backend: "opencode", status: "done" }),
    ];
    expect(recentModels(history, "opencode")).toEqual(["fresh", "older"]);
    expect(recentModels(history, "openrouter")).toEqual(["other-backend"]);
  });

  it("dedupes and caps at three by default", () => {
    const history = ["m1", "m2", "m3", "m1", "m4", "m5"].map((model, i) =>
      record({ model, backend: "opencode", status: "done", startedAt: i }),
    );
    expect(recentModels(history, "opencode")).toEqual(["m1", "m2", "m3"]);
    expect(recentModels(history, "opencode", 5)).toEqual(["m1", "m2", "m3", "m4", "m5"]);
  });
});
