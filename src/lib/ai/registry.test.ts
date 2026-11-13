/**
 * Unit tests for the feature registry (M13): every FeatureKind has a
 * declarative entry, labels/streams resolve for every kind (chat included),
 * and the phase/elapsed labels behave.
 */

import { describe, expect, it } from "vitest";
import type { FeatureKind } from "./types";
import {
  AI_FEATURES,
  aiElapsedLabel,
  aiFeatureDef,
  aiFeatureLabel,
  aiPhaseLabel,
  featureWantsStreaming,
} from "./registry";

/** The registry must cover (exactly) the FeatureKind union. */
const ALL_KINDS: FeatureKind[] = [
  "commit-message",
  "pr-title-body",
  "explain-hunk",
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
