/**
 * AI feature registry (M13).
 *
 * One declarative {@link AiFeatureDef} per feature. Everything generic
 * about a feature — its human label, button copy, icon, search keywords,
 * whether it prefers streaming — is declared HERE instead of being
 * hand-wired at each UI site. Consumers:
 *
 *   - command palette (`$lib/palette/commands.ts`) generates `ai.*`
 *     commands from the registry;
 *   - AI settings renders the per-feature toggles from it;
 *   - the status bar and the assistant panel's Activity tab label jobs
 *     via {@link aiFeatureLabel};
 *   - the master/per-feature gates (`AiStore.isFeatureEnabled`) iterate it.
 *
 * Adding feature #N = add a `FeatureKind` member (types.ts), a prompt
 * builder + parser + `runFeature` case (features.ts), a registry entry
 * here, and one UI call site. Nothing else.
 */

import type { AiJobPhase, FeatureKind } from "./types";

/** What the rest of the app can pass around as "an AI run kind". */
export type AiFeatureTag = FeatureKind | "chat";

/** Declarative description of one AI feature. */
export interface AiFeatureDef {
  /** The {@link FeatureKind} dispatched through `runFeature`. */
  kind: FeatureKind;
  /** Palette / activity / tooltip label ("Review staged changes"). */
  title: string;
  /** Inline button copy ("AI message"). */
  buttonLabel: string;
  /**
   * Semantic icon name for `appIcon()` — must exist in
   * `components/icons/sets/mapping.json`.
   */
  icon: string;
  /** Settings/palette search keywords. */
  keywords: string;
  /**
   * Prefer token streaming when the resolved backend supports it
   * (`generateStream` capability). Text-only features (explain, review,
   * diagnose) stream; structured-output features (commit message) may too
   * — the caller decides what progressive text means for its UI.
   */
  streaming?: boolean;
}

/**
 * All features, canonical order (settings toggle order, palette ordering
 * within the AI section, activity grouping).
 */
export const AI_FEATURES: readonly AiFeatureDef[] = [
  {
    kind: "commit-message",
    title: "Generate commit message",
    buttonLabel: "AI message",
    icon: "edit",
    keywords: "commit message conventional subject body generate write",
    streaming: true,
  },
  {
    kind: "pr-title-body",
    title: "Draft PR title and description",
    buttonLabel: "AI PR description",
    icon: "description",
    keywords: "pull request title body description generate draft",
    streaming: true,
  },
  {
    kind: "explain-hunk",
    title: "Explain this hunk",
    buttonLabel: "Explain hunk",
    icon: "difference",
    keywords: "explain hunk diff change understand",
    streaming: true,
  },
  {
    kind: "review-staged",
    title: "Review staged changes",
    buttonLabel: "AI review",
    icon: "verified",
    keywords: "review staged diff problems risks security check",
    streaming: true,
  },
  {
    kind: "stash-message",
    title: "Write stash message",
    buttonLabel: "AI stash message",
    icon: "backup",
    keywords: "stash message wip park generate",
    streaming: true,
  },
  {
    kind: "branch-name",
    title: "Suggest branch name",
    buttonLabel: "AI branch name",
    icon: "call-split",
    keywords: "branch name suggest kebab create",
  },
  // -- M13 additions ----------------------------------------------------------
  {
    kind: "explain-commit",
    title: "Explain this commit",
    buttonLabel: "AI explain",
    icon: "info",
    keywords: "explain commit changes what why understand",
    streaming: true,
  },
  {
    kind: "explain-blame",
    title: "Explain this line",
    buttonLabel: "AI explain line",
    icon: "history",
    keywords: "explain blame line history why code",
    streaming: true,
  },
  {
    kind: "diagnose-workflow",
    title: "Diagnose failed workflow",
    buttonLabel: "AI diagnose",
    icon: "bolt",
    keywords: "diagnose workflow ci failure log fix github actions",
    streaming: true,
  },
  {
    kind: "diagnose-action",
    title: "Diagnose failed action",
    buttonLabel: "AI diagnose",
    icon: "play-arrow",
    keywords: "diagnose action run failure log fix exit code",
    streaming: true,
  },
  {
    kind: "conflict-assist",
    title: "Suggest conflict resolution",
    buttonLabel: "AI suggest",
    icon: "merge-type",
    keywords: "conflict merge resolution suggest resolve ours theirs",
  },
  {
    kind: "release-notes",
    title: "Write release notes",
    buttonLabel: "AI release notes",
    icon: "compare-arrows",
    keywords: "release notes changelog range compare generate",
    streaming: true,
  },
];

/** The registry entry for `kind` (undefined for unknown kinds). */
export function aiFeatureDef(kind: FeatureKind): AiFeatureDef | undefined {
  return AI_FEATURES.find((def) => def.kind === kind);
}

/**
 * Human label for any AI run kind, chat included ("Assistant"). Falls
 * back to title-casing unknown kinds the way `kindLabel` does for ops.
 */
export function aiFeatureLabel(kind: AiFeatureTag): string {
  if (kind === "chat") return "Assistant";
  return aiFeatureDef(kind)?.title ?? kind.replace(/(^|-)(\p{L})/gu, (_, s, c: string) => (s ? " " : "") + c.toUpperCase());
}

/**
 * True when the feature wants token streaming. Callers still need the
 * provider-level `generateStream` capability check — this only says the
 * feature's UI is built for progressive text.
 */
export function featureWantsStreaming(kind: AiFeatureTag): boolean {
  if (kind === "chat") return true;
  return aiFeatureDef(kind)?.streaming === true;
}

// ---------------------------------------------------------------------------
// Job phase / elapsed labels (shared by the status bar, feature buttons and
// the assistant panel's Activity tab)
// ---------------------------------------------------------------------------

const AI_PHASE_LABELS: Record<AiJobPhase, string | null> = {
  queued: "queued",
  gathering: "gathering data",
  prompting: "building prompt",
  generating: "generating",
  parsing: "parsing result",
  done: null,
  cancelled: null,
  error: null,
};

/** Human label for a job phase (`null` for terminal phases). */
export function aiPhaseLabel(phase: AiJobPhase): string | null {
  return AI_PHASE_LABELS[phase];
}

/** "45s" / "2m 10s" elapsed labels — one formatter, shared with the result
 * footers (`usage.ts`), re-exported here under the consumers' name. */
export { aiResultSeconds as aiElapsedLabel } from "./usage";
