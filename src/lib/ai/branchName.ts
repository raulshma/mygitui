/**
 * branch-name feature wrapper (M12, lane C).
 *
 * `runFeature("branch-name", …)` takes a single free-form description
 * string; this module builds a sensible one from structured context
 * (what the work is + recent commit subjects + the branch we branch
 * from), runs the feature through the app-wide {@link ai} store (opt-in
 * gating and per-repo busy state included) and returns the parsed
 * kebab-case name.
 *
 * Pure builder is unit tested; `suggestBranchName` is the thin call path
 * the BranchPanel button (integration lane) invokes.
 */

import { ai } from "./ai.svelte";
import type { FeatureGitClient } from "./features";

/** Context for a branch-name suggestion. */
export interface BranchNameContext {
  /** What the branch is for, free-form ("fix the login crash"). */
  description: string;
  /** Recent commit subjects, oldest or newest first (used as flavor). */
  recentSubjects?: readonly string[];
  /** The branch we branch FROM, when known. */
  baseBranch?: string;
}

/** How many recent subjects are folded into the description. */
export const MAX_SUBJECTS = 5;

/**
 * Renders the context into the single description string the runner
 * expects: the user's description first, then up to five recent subjects,
 * then the base branch. Empty parts are skipped; the result is trimmed
 * and single-spaced (the prompt asks for one line).
 */
export function branchNameDescription(ctx: BranchNameContext): string {
  const parts: string[] = [];
  const description = ctx.description.replace(/\s+/g, " ").trim();
  if (description) parts.push(`What it is for: ${description}`);
  const subjects = (ctx.recentSubjects ?? [])
    .map((s) => s.replace(/\s+/g, " ").trim())
    .filter((s) => s.length > 0)
    .slice(0, MAX_SUBJECTS);
  if (subjects.length > 0) {
    parts.push(`Recent commit subjects: ${subjects.join("; ")}`);
  }
  const base = ctx.baseBranch?.trim();
  if (base) parts.push(`Branches from: ${base}`);
  return parts.join("\n");
}

/**
 * Suggests a kebab-case branch name for the repo. Throws `AiError`
 * (kind "opt-in" when the repo has not allowed AI; "unknown" when the
 * description is blank) — callers show the same guidance UX as the other
 * feature buttons.
 */
export async function suggestBranchName(
  repoId: string,
  context: BranchNameContext,
  git?: FeatureGitClient,
): Promise<string> {
  const outcome = await ai.run(
    "branch-name",
    { repoId, description: branchNameDescription(context) },
    git,
  );
  return outcome.branchName ?? outcome.result.text.trim();
}
