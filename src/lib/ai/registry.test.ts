/**
 * Unit tests for the feature registry (M13): every FeatureKind has a
 * declarative entry, labels/streams resolve for every kind (chat included),
 * and the phase/elapsed labels behave.
 */

import { afterEach, describe, expect, it } from "vitest";
import type { FeatureKind } from "./types";
import {
  AI_FEATURES,
  aiElapsedLabel,
  aiFeatureDef,
  aiFeatureLabel,
  aiPhaseLabel,
  allFeatureDefs,
  customKindId,
  featureWantsStreaming,
  isCustomKind,
  registerCustomPromptProvider,
} from "./registry";

/** The registry must cover (exactly) the BUILTIN FeatureKind members. */
const ALL_KINDS: FeatureKind[] = [
  "commit-message",
  "pr-title-body",
  "explain-hunk",
  "explain-selection",
  "review-staged",
  "stash-message",
  "branch-name",
  "explain-commit",
  "explain-blame",
  "diagnose-workflow",
  "diagnose-action",
  "conflict-assist",
  "release-notes",
];

describe("AI feature registry", () => {
  it("has exactly one def per FeatureKind, in canonical order", () => {
    expect(AI_FEATURES.map((def) => def.kind)).toEqual(ALL_KINDS);
    for (const def of AI_FEATURES) {
      expect(def.title.length).toBeGreaterThan(0);
      expect(def.buttonLabel.length).toBeGreaterThan(0);
      expect(def.keywords.length).toBeGreaterThan(0);
    }
  });

  it("aiFeatureDef resolves every kind and rejects unknown ones", () => {
    for (const kind of ALL_KINDS) {
      expect(aiFeatureDef(kind)?.kind).toBe(kind);
    }
    expect(aiFeatureDef("nope" as FeatureKind)).toBeUndefined();
  });

  it("aiFeatureLabel names chat and title-cases unknown kinds", () => {
    expect(aiFeatureLabel("chat")).toBe("Assistant");
    expect(aiFeatureLabel("commit-message")).toBe("Generate commit message");
    expect(aiFeatureLabel("explain-commit")).toBe("Explain this commit");
    expect(aiFeatureLabel("mystery-kind" as never)).toBe("Mystery Kind");
  });

  it("featureWantsStreaming: registry-declared, chat always", () => {
    expect(featureWantsStreaming("chat")).toBe(true);
    expect(featureWantsStreaming("explain-commit")).toBe(true);
    expect(featureWantsStreaming("branch-name")).toBe(false);
    expect(featureWantsStreaming("conflict-assist")).toBe(false);
  });
});

describe("custom prompt merge (query-time, item 56)", () => {
  afterEach(() => {
    // Restore the empty provider so later suites see a clean registry.
    registerCustomPromptProvider(() => []);
  });

  it("resolves custom kinds against the registered library", () => {
    registerCustomPromptProvider(() => [
      { id: "changelog", name: "Write a changelog entry", enabled: true },
      { id: "off", name: "Disabled prompt", enabled: false },
    ]);
    const kind = "custom.changelog" as FeatureKind;
    expect(isCustomKind(kind)).toBe(true);
    expect(customKindId(kind)).toBe("changelog");
    expect(isCustomKind("commit-message")).toBe(false);
    expect(customKindId("commit-message")).toBeNull();
    // The def (title/keywords/streaming) synthesizes from the entry name.
    expect(aiFeatureDef(kind)?.title).toBe("Write a changelog entry");
    expect(aiFeatureLabel(kind)).toBe("Write a changelog entry");
    expect(featureWantsStreaming(kind)).toBe(true);
    // Unknown ids and builtins still behave as before.
    expect(aiFeatureDef("custom.missing" as FeatureKind)).toBeUndefined();
    expect(aiFeatureDef("commit-message")?.kind).toBe("commit-message");
  });

  it("labels disabled entries (their history rows still exist) but never merges them", () => {
    registerCustomPromptProvider(() => [{ id: "off", name: "Disabled prompt", enabled: false }]);
    expect(aiFeatureLabel("custom.off" as FeatureKind)).toBe("Disabled prompt");
    expect(
      allFeatureDefs()
        .filter((def) => isCustomKind(def.kind))
        .map((def) => def.kind),
    ).toEqual([]);
  });

  it("allFeatureDefs appends enabled customs after the builtins", () => {
    registerCustomPromptProvider(() => [
      { id: "a", name: "A", enabled: true },
      { id: "b", name: "B", enabled: false },
    ]);
    const defs = allFeatureDefs();
    expect(defs.slice(0, AI_FEATURES.length)).toEqual([...AI_FEATURES]);
    expect(defs.slice(AI_FEATURES.length).map((def) => def.kind)).toEqual(["custom.a"]);
  });

  it("empty provider (library not yet loaded) keeps everything builtin", () => {
    registerCustomPromptProvider(() => []);
    expect(aiFeatureDef("custom.any" as FeatureKind)).toBeUndefined();
    // No def → the generic title-case fallback (dots are not separators).
    expect(aiFeatureLabel("custom.any" as FeatureKind)).toBe("Custom.any");
    expect(allFeatureDefs()).toEqual([...AI_FEATURES]);
  });
});

describe("AI phase / elapsed labels", () => {
  it("labels in-flight phases; terminal phases have none", () => {
    expect(aiPhaseLabel("queued")).toBe("queued");
    expect(aiPhaseLabel("gathering")).toBe("gathering data");
    expect(aiPhaseLabel("generating")).toBe("generating");
    expect(aiPhaseLabel("done")).toBeNull();
    expect(aiPhaseLabel("cancelled")).toBeNull();
    expect(aiPhaseLabel("error")).toBeNull();
  });

  it("formats elapsed like the forge durations", () => {
    expect(aiElapsedLabel(0)).toBe("0s");
    expect(aiElapsedLabel(45_000)).toBe("45s");
    expect(aiElapsedLabel(130_000)).toBe("2m 10s");
  });
});
