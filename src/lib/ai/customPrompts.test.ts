/**
 * Unit tests for the custom prompt library (item 56): store round-trip
 * (persist + reload, parse-and-drop, caps, dedupe), the shared template
 * interpolation helper (known/unknown/missing placeholders), and the
 * registry merge wiring (the provider registration the store performs).
 */

import { describe, expect, it } from "vitest";
import type { FeatureKind } from "./types";
import {
  CUSTOM_PROMPTS_KEY,
  CUSTOM_PROMPTS_CAP,
  CUSTOM_TEMPLATE_CAP,
  CustomPromptStore,
  parseCustomPrompts,
  renderTemplate,
} from "./customPrompts.svelte";
import { aiFeatureDef, aiFeatureLabel, featureWantsStreaming } from "./registry";

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

describe("renderTemplate (shared interpolation helper)", () => {
  it("replaces known placeholders, including padded forms", () => {
    expect(renderTemplate("Review {{diff}} please", { diff: "DIFF" })).toBe("Review DIFF please");
    expect(renderTemplate("{{ summary }} + {{commits}}", { summary: "S", commits: "C" })).toBe(
      "S + C",
    );
  });

  it("leaves unknown placeholders and valueless ones verbatim (debuggable)", () => {
    expect(renderTemplate("{{notes}} stays {{diff}}", { diff: "D" })).toBe("{{notes}} stays D");
    // A referenced-but-ungathered variable stays visible instead of
    // silently vanishing into the prompt.
    expect(renderTemplate("a {{diff}} b", {})).toBe("a {{diff}} b");
  });

  it("does not treat malformed braces as placeholders", () => {
    // Single braces and a lone closing brace are prose, not placeholders.
    expect(renderTemplate("{ diff } {diff {{diff}", { diff: "D" })).toBe(
      "{ diff } {diff {{diff}",
    );
  });
});

describe("parseCustomPrompts (parse-and-drop loader)", () => {
  it("keeps valid entries, caps templates, trims names, dedupes ids", () => {
    const entries = parseCustomPrompts([
      { id: "a", name: "  My prompt  ", template: "t", enabled: true },
      { id: "a", name: "dup", template: "t2", enabled: true },
      { id: "b", name: "x".repeat(200), template: "long".padEnd(CUSTOM_TEMPLATE_CAP + 10, "!"), enabled: false },
      { id: "", name: "no id", template: "", enabled: true },
      { id: "c", name: "  ", template: "", enabled: true },
      "junk",
    ]);
    expect(entries).not.toBeNull();
    const list = entries!;
    expect(list).toHaveLength(2);
    expect(list[0]).toEqual({ id: "a", name: "My prompt", template: "t", enabled: true });
    expect(list[1]!.name.length).toBe(120);
    expect(list[1]!.template.length).toBe(CUSTOM_TEMPLATE_CAP);
    expect(list[1]!.enabled).toBe(false);
  });

  it("rejects non-array payloads and caps the list", () => {
    expect(parseCustomPrompts("nope")).toBeNull();
    expect(parseCustomPrompts({ id: "a" })).toBeNull();
    const many = Array.from({ length: CUSTOM_PROMPTS_CAP + 5 }, (_, i) => ({
      id: `p${i}`,
      name: `p${i}`,
      template: "",
      enabled: true,
    }));
    expect(parseCustomPrompts(many)).toHaveLength(CUSTOM_PROMPTS_CAP);
  });
});

describe("CustomPromptStore", () => {
  it("starts empty and persists additions in the versioned envelope", () => {
    const storage = memStorage();
    const store = new CustomPromptStore({ storage, id: () => "id-1" });
    expect(store.entries).toEqual([]);

    const entry = store.add({ name: "Explain release", template: "Summarize {{commits}}" });
    expect(entry).toEqual({ id: "id-1", name: "Explain release", template: "Summarize {{commits}}", enabled: true });

    const raw = JSON.parse(storage.dump()[CUSTOM_PROMPTS_KEY] ?? "{}");
    expect(raw.v).toBe(1);
    expect(raw.data).toHaveLength(1);
    expect(raw.data[0].id).toBe("id-1");
  });

  it("round-trips entries through storage and drops malformed ones", () => {
    const storage = memStorage();
    const first = new CustomPromptStore({ storage, id: (() => {
      let n = 0;
      return () => `id-${++n}`;
    })() });
    first.add({ name: "One", template: "t1" });
    first.add({ name: "Two", template: "t2", enabled: false });

    const second = new CustomPromptStore({ storage });
    expect(second.entries.map((e) => [e.id, e.name, e.enabled])).toEqual([
      ["id-1", "One", true],
      ["id-2", "Two", false],
    ]);

    const junk = memStorage({ [CUSTOM_PROMPTS_KEY]: "{broken" });
    expect(new CustomPromptStore({ storage: junk }).entries).toEqual([]);
  });

  it("update patches name/template/enabled (with caps) and remove is idempotent", () => {
    const store = new CustomPromptStore({ storage: memStorage(), id: () => "x" });
    const entry = store.add({ name: "One", template: "t1" });

    store.update(entry.id, { name: "  Renamed  ", template: "t2", enabled: false });
    expect(store.get(entry.id)).toEqual({ id: "x", name: "Renamed", template: "t2", enabled: false });
    store.update(entry.id, { template: "!".repeat(CUSTOM_TEMPLATE_CAP + 5) });
    expect(store.get(entry.id)!.template.length).toBe(CUSTOM_TEMPLATE_CAP);
    // A blank name keeps the previous one; unknown ids are no-ops.
    store.update(entry.id, { name: "   " });
    expect(store.get(entry.id)!.name).toBe("Renamed");
    store.update("missing", { name: "nope" });
    expect(store.entries).toHaveLength(1);

    store.remove(entry.id);
    expect(store.entries).toEqual([]);
    store.remove(entry.id); // idempotent, no persist error
  });

  it("enabled() lists only enabled entries", () => {
    const store = new CustomPromptStore({ storage: memStorage() });
    store.add({ name: "on", template: "t", enabled: true });
    store.add({ name: "off", template: "t", enabled: false });
    expect(store.enabled().map((e) => e.name)).toEqual(["on"]);
  });
});

describe("store → registry wiring", () => {
  it("enabled entries surface as custom feature kinds and labels", () => {
    // The store constructor registers the provider with the registry; a
    // fresh instance over empty storage re-registers over the singleton.
    const storage = memStorage();
    const store = new CustomPromptStore({ storage, id: () => "wired" });
    const entry = store.add({ name: "Changelog entry", template: "t" });
    try {
      const kind = `custom.${entry.id}` as FeatureKind;
      expect(aiFeatureLabel(kind)).toBe("Changelog entry");
      expect(featureWantsStreaming(kind)).toBe(true);
      // Disabled entries keep their label (their history rows still exist)
      // but never merge into the runnable feature list.
      store.update(entry.id, { enabled: false });
      expect(aiFeatureLabel(kind)).toBe("Changelog entry");
      expect(aiFeatureDef(kind)).toBeDefined();
    } finally {
      store.remove(entry.id);
    }
  });
});
