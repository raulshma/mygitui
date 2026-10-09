/**
 * AI feature prompt builders + runners (M6, lane H1).
 *
 * Prompt builders are PURE (no IO) and heavily tested: they render git data
 * into prompts with a hard input cap (24k chars for diffs — larger inputs
 * are truncated with an explicit note so the model knows) and shared
 * guardrails (never emit secrets, never emit placeholders like TODO).
 *
 * Runners gather data through the injectable git client
 * (`$lib/ipc/client`), build the prompt, route through the connection
 * supervisor (`supervisor.generate`) and parse the answer. Every runner
 * gates on per-repo opt-in BEFORE any data is gathered or sent.
 */

import type { DiffHunk, FileDiff, LogFilter, LogPage, RepoId } from "$lib/ipc/types";
import type { ConnectionSupervisor } from "./connection";
import { AiError } from "./types";
import type { AiResult, FeatureKind } from "./types";

// ---------------------------------------------------------------------------
// Shared prompt plumbing
// ---------------------------------------------------------------------------

/** Hard cap for diff input in prompts (characters). */
export const MAX_DIFF_CHARS = 24_000;

/** Guardrails appended to every system prompt. */
export const GUARDRAILS = [
  "Never include secrets, API keys, tokens, passwords or credentials in your output, even if they appear in the provided data.",
  'Never emit placeholders such as "TODO", "FIXME", "TBD", "..." or "<placeholder>" — always produce the finished text.',
  "Base your answer strictly on the provided data; do not invent changes that are not shown.",
].join(" ");

/** Truncates `text` to `max` characters, appending an explicit note. */
export function capDiff(text: string, max: number = MAX_DIFF_CHARS): string {
  if (text.length <= max) return text;
  const total = text.length;
  const capped = text.slice(0, max);
  // Cut back to the last complete line so the model never sees a half hunk.
  const lastNewline = capped.lastIndexOf("\n");
  const shown = lastNewline > 0 ? capped.slice(0, lastNewline) : capped;
  return `${shown}\n\n[diff truncated: showing first ${shown.length} of ${total} characters]`;
}

/** Which output format a commit message request asks for. */
export type CommitStyle = "conventional" | "plain";

/** Shared subject/body output contract for message-style prompts. */
const MESSAGE_FORMAT = [
  "Output format — exactly this shape, no code fences, no extra commentary:",
  "<subject line>",
  "",
  "<optional body paragraphs>",
  "The first line is the subject; everything after the first blank line is the body.",
].join("\n");

/**
 * System + user prompt for a commit message from the staged diff.
 * `conventional` enforces Conventional Commits (type prefix, ≤50-char
 * subject, imperative mood); `plain` just asks for a good subject.
 */
export function commitMessagePrompt(
  stagedDiff: string,
  style: CommitStyle = "conventional",
): { system: string; prompt: string } {
  const styleRules =
    style === "conventional"
      ? [
          "Follow the Conventional Commits specification: the subject starts with a type such as feat, fix, docs, style, refactor, perf, test, build, ci or chore, optionally with a scope like `feat(ui):`.",
          "Keep the subject at most 50 characters, in imperative mood, with no trailing period.",
        ]
      : [
          "Write a clear, concise subject line of at most 50 characters in imperative mood with no trailing period.",
        ];
  const system = [
    "You write git commit messages for a desktop Git client.",
    ...styleRules,
    "The body explains WHY the change was made, not WHAT changed mechanically (the diff already shows what) — omit the body entirely when there is nothing meaningful to add.",
    "Wrap body lines at about 72 characters.",
    GUARDRAILS,
  ].join("\n\n");
  const prompt = [
    "Write a commit message for the following staged changes.",
    "",
    "Staged diff:",
    "```",
    capDiff(stagedDiff),
    "```",
    "",
    MESSAGE_FORMAT,
  ].join("\n");
  return { system, prompt };
}

/**
 * Parses a model answer in the subject/blank/body format. Tolerates code
 * fences, leading "Subject:"-style labels and stray blank lines. Throws
 * `AiError("bad-response")` when no usable subject exists.
 */
export function parseCommitMessage(text: string): { subject: string; body: string } {
  let clean = text.trim();
  const fence = clean.match(/^```[a-zA-Z]*\n([\s\S]*?)\n?```$/);
  if (fence) clean = fence[1].trim();
  const lines = clean.split("\n");
  let subject = (lines.shift() ?? "").trim();
  subject = subject.replace(/^(?:subject|title)\s*:\s*/i, "");
  const body = lines.join("\n").replace(/^\n+/, "").trimEnd();
  if (subject.length === 0) {
    throw new AiError("bad-response", "AI returned an empty commit subject", {});
  }
  return { subject, body };
}

/** Pull request title + body prompt from the branch's commits + diff stats. */
export function prTitleBodyPrompt(
  commits: Array<{ summary: string; sha: string }>,
  diffSummary: string,
): { system: string; prompt: string } {
  const system = [
    "You write pull request titles and descriptions for a desktop Git client.",
    "The title is a single line (at most 72 characters, imperative mood, no trailing period).",
    "The body summarizes the motivation and the main changes as short bullet points.",
    GUARDRAILS,
  ].join("\n\n");
  const commitLines = commits
    .map((c) => `- ${c.sha.slice(0, 7)} ${c.summary}`)
    .join("\n");
  const prompt = [
    "Write a pull request title and description for this branch.",
    "",
    "Commits (oldest intent, newest last is not guaranteed):",
    commitLines || "(no commits listed)",
    "",
    "Changed files:",
    diffSummary || "(no file stats)",
    "",
    MESSAGE_FORMAT,
  ].join("\n");
  return { system, prompt };
}

/** Prompt explaining a single diff hunk in a file. */
export function explainHunkPrompt(path: string, hunkText: string): { system: string; prompt: string } {
  const system = [
    "You explain code diffs to developers using a desktop Git client.",
    "Explain concisely what the change does and any risk it carries (2–5 sentences).",
    GUARDRAILS,
  ].join("\n\n");
  const prompt = [
    `Explain this change in \`${path}\`:`,
    "",
    "```diff",
    capDiff(hunkText),
    "```",
  ].join("\n");
  return { system, prompt };
}

/** Prompt reviewing the full staged diff for problems. */
export function reviewStagedPrompt(stagedDiff: string): { system: string; prompt: string } {
  const system = [
    "You are a careful code reviewer inside a desktop Git client.",
    "Review the staged changes: correctness risks, edge cases, security issues and style problems. Be concrete and reference file paths. When everything looks fine, say so briefly.",
    GUARDRAILS,
  ].join("\n\n");
  const prompt = [
    "Review the following staged changes.",
    "",
    "Staged diff:",
    "```",
    capDiff(stagedDiff),
    "```",
  ].join("\n");
  return { system, prompt };
}

/** Prompt for a stash message from a short change summary. */
export function stashMessagePrompt(summary: string): { system: string; prompt: string } {
  const system = [
    "You write short git stash messages for a desktop Git client.",
    "One line, at most 60 characters, describing what is being stashed and why it is parked.",
    GUARDRAILS,
  ].join("\n\n");
  const prompt = [
    "Write a stash message for these work-in-progress changes:",
    "",
    summary,
    "",
    MESSAGE_FORMAT,
  ].join("\n");
  return { system, prompt };
}

/** Prompt for a kebab-case branch name from a free-form description. */
export function branchNamePrompt(description: string): { system: string; prompt: string } {
  const system = [
    "You suggest git branch names for a desktop Git client.",
    "Answer with ONE line: a kebab-case branch name (lowercase words separated by hyphens, no spaces, no slashes, no type prefix unless the description asks for one).",
    GUARDRAILS,
  ].join("\n\n");
  const prompt = `Suggest a branch name for: ${description}\n\nAnswer with the branch name only.`;
  return { system, prompt };
}

/**
 * Extracts the kebab-case branch name from a model answer (last non-empty
 * line, sanitized to `[a-z0-9-]`, trailing hyphens stripped). Throws
 * `AiError("bad-response")` when nothing usable remains.
 */
export function parseBranchName(text: string): string {
  const lines = text.trim().split("\n").filter((l) => l.trim().length > 0);
  const raw = (lines.at(-1) ?? "")
    .trim()
    .replace(/^[-`*\s]+/, "")
    .replace(/[`*]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
  if (raw.length === 0) {
    throw new AiError("bad-response", "AI returned no usable branch name", {});
  }
  return raw;
}

// ---------------------------------------------------------------------------
// Git data → prompt text (pure renderers over FileDiff)
// ---------------------------------------------------------------------------

/** Renders one {@link FileDiff} as unified-diff-style text. */
export function formatFileDiff(file: FileDiff): string {
  if (file.binary || file.is_image) {
    return `Binary file ${file.path} changed (+${file.additions}/-${file.deletions})`;
  }
  const old = file.old_path && file.old_path !== file.path ? file.old_path : file.path;
  const out: string[] = [`--- a/${old}`, `+++ b/${file.path}`];
  for (const hunk of file.hunks) {
    out.push(`@@ -${hunk.old_start} +${hunk.new_start} @@`);
    for (const line of hunk.lines) {
      const marker = line.origin === "=" ? " " : line.origin;
      out.push(`${marker}${line.text}`);
    }
  }
  return out.join("\n");
}

/** Renders the staged diff (list of {@link FileDiff}) as prompt text. */
export function stagedDiffText(files: FileDiff[]): string {
  return files.map(formatFileDiff).join("\n");
}

/**
 * Renders ONE hunk as unified-diff-style text (the per-hunk slice of
 * {@link formatFileDiff}) — the `hunk` input the explain-hunk runner wants.
 */
export function formatHunkText(hunk: DiffHunk): string {
  const out: string[] = [`@@ -${hunk.old_start} +${hunk.new_start} @@`];
  for (const line of hunk.lines) {
    const marker = line.origin === "=" ? " " : line.origin;
    out.push(`${marker}${line.text}`);
  }
  return out.join("\n");
}

/** Renders per-file add/del stats (used by the PR body prompt). */
export function diffSummaryText(files: FileDiff[]): string {
  return files
    .map((f) => `- ${f.path} (+${f.additions}/-${f.deletions})`)
    .join("\n");
}

// ---------------------------------------------------------------------------
// Feature runners
// ---------------------------------------------------------------------------

/** Git data providers a runner needs (subset of `$lib/ipc/client`). */
export interface FeatureGitClient {
  repoDiff(
    repoId: RepoId,
    oldSide: "head",
    newSide: "index",
  ): Promise<FileDiff[]>;
  streamLog(
    repoId: RepoId,
    filter: LogFilter,
    onPage: (page: LogPage) => void,
  ): Promise<void>;
}

/** Request routing surface (subset of {@link ConnectionSupervisor}). */
export interface FeatureRouter {
  generate(req: {
    system?: string;
    prompt: string;
    model?: string;
    signal?: AbortSignal;
    sessionKey?: string;
  }): Promise<AiResult>;
}

/** Everything a runner needs (defaults wire the real app singletons). */
export interface FeatureDeps {
  git: FeatureGitClient;
  router: FeatureRouter;
  /** Per-repo opt-in gate (checked before anything is gathered/sent). */
  isOptedIn(repoId: string): boolean;
}

/** Inputs a runner accepts (pre-gathered data or the id to gather for). */
export interface FeatureCtx {
  repoId: string;
  /** explain-hunk: file path. */
  path?: string;
  /** explain-hunk: the hunk text to explain. */
  hunk?: string;
  /** branch-name: free-form description. */
  description?: string;
  /** stash-message / override: pre-rendered diff or summary text. */
  diff?: string;
  /** pr-title-body: pre-gathered commits (else first log page). */
  commits?: Array<{ summary: string; sha: string }>;
  /** Model override (backend-specific id). */
  model?: string;
}

/** A finished feature run. */
export interface FeatureOutcome {
  kind: FeatureKind;
  /** Raw generation result. */
  result: AiResult;
  /** commit-message / pr-title-body / stash-message: parsed subject+body. */
  message?: { subject: string; body: string };
  /** branch-name: parsed kebab-case name. */
  branchName?: string;
  /** explain-hunk / review-staged: the free-form text (same as result.text). */
  text?: string;
}

const EMPTY_LOG_FILTER: LogFilter = {
  text: null,
  regex: false,
  author: null,
  path: null,
  refs: [],
  follow: false,
};

/** Gathers the staged diff for a repo (head → index). */
async function gatherStagedDiff(git: FeatureGitClient, repoId: string): Promise<string> {
  const files = await git.repoDiff(repoId, "head", "index");
  return stagedDiffText(files);
}

/** Gathers the first log page's commits for a repo. */
async function gatherCommits(
  git: FeatureGitClient,
  repoId: string,
): Promise<Array<{ summary: string; sha: string }>> {
  return new Promise((resolve, reject) => {
    void git.streamLog(repoId, EMPTY_LOG_FILTER, (page) => {
      resolve(page.commits.map((c) => ({ summary: c.summary, sha: c.sha })));
    }).catch(reject);
  });
}

/**
 * Runs one AI feature end to end: opt-in gate → gather → prompt → generate
 * (through the supervisor, backend selection + fallback included) → parse.
 * Throws {@link AiError} on any failure; nothing is retried here.
 */
export async function runFeature(
  kind: FeatureKind,
  ctx: FeatureCtx,
  deps: FeatureDeps,
): Promise<FeatureOutcome> {
  if (!deps.isOptedIn(ctx.repoId)) {
    throw new AiError(
      "opt-in",
      "AI features are not enabled for this repository — allow them in the AI prompt or AI settings",
      {},
    );
  }

  let built: { system: string; prompt: string };
  switch (kind) {
    case "commit-message": {
      const diff = ctx.diff ?? (await gatherStagedDiff(deps.git, ctx.repoId));
      if (diff.trim().length === 0) {
        throw new AiError("bad-response", "nothing is staged to write a commit message for", {});
      }
      built = commitMessagePrompt(diff, "conventional");
      break;
    }
    case "pr-title-body": {
      const commits = ctx.commits ?? (await gatherCommits(deps.git, ctx.repoId));
      const files = await deps.git.repoDiff(ctx.repoId, "head", "index");
      built = prTitleBodyPrompt(commits, diffSummaryText(files));
      break;
    }
    case "explain-hunk": {
      if (!ctx.path || !ctx.hunk) {
        throw new AiError("unknown", "explain-hunk needs a file path and hunk text", {});
      }
      built = explainHunkPrompt(ctx.path, ctx.hunk);
      break;
    }
    case "review-staged": {
      const diff = ctx.diff ?? (await gatherStagedDiff(deps.git, ctx.repoId));
      if (diff.trim().length === 0) {
        throw new AiError("bad-response", "nothing is staged to review", {});
      }
      built = reviewStagedPrompt(diff);
      break;
    }
    case "stash-message": {
      const summary = ctx.diff ?? (await gatherStagedDiff(deps.git, ctx.repoId));
      built = stashMessagePrompt(capDiff(summary, 2_000));
      break;
    }
    case "branch-name": {
      if (!ctx.description || ctx.description.trim().length === 0) {
        throw new AiError("unknown", "branch-name needs a description", {});
      }
      built = branchNamePrompt(ctx.description.trim());
      break;
    }
  }

  const result = await deps.router.generate({
    system: built.system,
    prompt: built.prompt,
    model: ctx.model,
    sessionKey: ctx.repoId,
  });

  const outcome: FeatureOutcome = { kind, result, text: result.text };
  switch (kind) {
    case "commit-message":
    case "pr-title-body":
    case "stash-message":
      outcome.message = parseCommitMessage(result.text);
      break;
    case "branch-name":
      outcome.branchName = parseBranchName(result.text);
      break;
    case "explain-hunk":
    case "review-staged":
      break;
  }
  return outcome;
}
