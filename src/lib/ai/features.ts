/**
 * AI feature prompt builders + runners (M6, lane H1).
 *
 * Prompt builders are PURE (no IO) and heavily tested: they render git data
 * into prompts with a hard input cap (24k chars for diffs — larger inputs
 * are budgeted per file by `./diffBudget` so every changed file stays
 * represented, with explicit truncation/omission notes) and shared
 * guardrails (never emit secrets, never emit placeholders like TODO).
 *
 * Runners gather data through the injectable git client
 * (`$lib/ipc/client`), build the prompt, route through the connection
 * supervisor (`supervisor.generate`) and parse the answer. Every runner
 * gates on per-repo opt-in BEFORE any data is gathered or sent.
 */

import type { DiffHunk, DiffSide, FileDiff, LogFilter, LogPage, RepoId } from "$lib/ipc/types";
import { budgetedDiffText, MAX_DIFF_CHARS } from "./diffBudget";
import type { ConnectionSupervisor } from "./connection";
import { AiError } from "./types";
import type { AiBackend, AiJobPhase, AiResult, FeatureKind } from "./types";

// ---------------------------------------------------------------------------
// Shared prompt plumbing
// ---------------------------------------------------------------------------

/** Hard cap for diff input in prompts (characters) — defined in {@link ./diffBudget}. */
export { MAX_DIFF_CHARS };

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
    "The input starts with a stat summary of ALL changed files; the diff content after it may be budgeted per file, so trust the summary when the change spans many files.",
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
    "The input starts with a stat summary of ALL changed files; the diff content after it may be budgeted per file. Review what is shown and note when the summary suggests more.",
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
// M13 prompt builders (all pure; call sites pre-gather the data)
// ---------------------------------------------------------------------------

/**
 * System + user prompt explaining one commit: its message plus the
 * parent→commit diff (budgeted by the caller). Free-form markdown answer.
 */
export function explainCommitPrompt(
  sha: string,
  message: string,
  diff: string,
): { system: string; prompt: string } {
  return {
    system: [
      "You explain Git commits to developers.",
      "Describe what changed and why it likely matters: intent, mechanics, and any risks worth knowing (behavior changes, migrations, breaking calls).",
      "Answer in concise GitHub-flavored markdown, grounded strictly in the provided message and diff.",
      GUARDRAILS,
    ].join(" "),
    prompt: [
      `Commit ${sha.slice(0, 10)}`,
      "Commit message:",
      message.trim(),
      "",
      "Diff against the first parent:",
      "```diff",
      diff,
      "```",
    ].join("\n"),
  };
}

/**
 * System + user prompt explaining one blamed line: what the line does and
 * how it came to be, from the line text, its blaming commit summary and an
 * optional file slice for surrounding context.
 */
export function explainBlamePrompt(
  path: string,
  line: number,
  lineText: string,
  commitSummary: string | null,
  fileSlice: string | null,
): { system: string; prompt: string } {
  const parts = [
    `File: ${path}`,
    `Line ${line}:`,
    "```",
    lineText,
    "```",
  ];
  if (commitSummary !== null) parts.push(`Introduced by commit: "${commitSummary}"`);
  if (fileSlice !== null) {
    parts.push("Surrounding file content:", "```", fileSlice, "```");
  }
  parts.push(
    "Explain this line: what it does, why it might be written this way, and what to watch out for when changing it. Be brief and concrete.",
  );
  return {
    system: [
      "You explain source code lines to developers, grounded strictly in the provided content — say so when the context is not enough to be sure.",
      "Answer in concise GitHub-flavored markdown.",
      GUARDRAILS,
    ].join(" "),
    prompt: parts.join("\n"),
  };
}

/**
 * Shared system + user prompt diagnosing a failed CI workflow run or
 * custom action run: the failure metadata plus a capped log tail.
 * Free-form markdown answer.
 */
export function diagnoseFailurePrompt(
  subject: string,
  meta: string,
  logTail: string,
): { system: string; prompt: string } {
  return {
    system: [
      "You diagnose failed CI / automation runs for developers.",
      "Identify the most likely failing step and root cause, quote the decisive log lines, and suggest the next concrete fix.",
      "If the log does not contain a clear cause, say what is missing and list the most plausible candidates.",
      "Answer in concise GitHub-flavored markdown.",
      GUARDRAILS,
    ].join(" "),
    prompt: [`${subject} failed.`, "Run details:", meta, "Log tail:", "```", logTail, "```"].join("\n"),
  };
}

/**
 * System + user prompt suggesting a resolution for ONE merge conflict:
 * the two sides plus optional surrounding content and merge hints.
 * The answer must be replacement text only — the parser strips everything
 * else, and the suggestion lands in the conflict editor's custom-text
 * field for the user to review (advisory contract: AI never applies).
 */
export function conflictSuggestionPrompt(
  path: string,
  ours: string,
  theirs: string,
  context: string | null,
): { system: string; prompt: string } {
  const parts = [
    `Conflicted file: ${path}`,
    'Current branch side ("ours"):',
    "```",
    ours,
    "```",
    'Incoming side ("theirs"):',
    "```",
    theirs,
    "```",
  ];
  if (context !== null) parts.push("Surrounding content:", "```", context, "```");
  parts.push(
    "Suggest the resolved content for this conflicted region: merge both sides' intent, keep valid syntax, and drop the conflict markers.",
  );
  return {
    system: [
      "You suggest merge-conflict resolutions.",
      "Output ONLY the resolved replacement text — no explanations, no code fences, no conflict markers.",
      "If both sides express incompatible intents, prefer the incoming change and note nothing (the user reviews the result).",
      GUARDRAILS,
    ].join(" "),
    prompt: parts.join("\n"),
  };
}

/**
 * Extracts the resolved-region text from a model answer: strips code
 * fences, residual conflict markers and prose around the payload. The
 * editor's custom-text field is the review surface, so a slightly noisy
 * parse is survivable — but markers must never survive into it.
 */
export function parseConflictSuggestion(text: string): string {
  let out = text.trim();
  // Strip one wrapping fence pair when present (```…``` or ```lang…```).
  const fence = /^```[^\n]*\n([\s\S]*?)\n?```$/.exec(out);
  if (fence !== null) out = fence[1] ?? out;
  // Drop any leftover conflict-marker lines with their >>> / <<< rails.
  out = out
    .split("\n")
    .filter((line) => !/^(<{7}|={7}|>{7})/.test(line))
    .join("\n");
  return out.trim();
}

/**
 * System + user prompt writing release notes for a commit range: the
 * range's commits plus the aggregate file stats. Markdown answer shaped
 * for a changelog.
 */
export function releaseNotesPrompt(
  range: string,
  commits: Array<{ summary: string; sha: string }>,
  stats: string,
): { system: string; prompt: string } {
  const lines =
    commits.length > 0
      ? commits.map((c) => `- ${c.summary} (${c.sha.slice(0, 8)})`).join("\n")
      : "(no commits)";
  return {
    system: [
      "You write release notes / changelog entries for developers.",
      "Group the changes into sections (e.g. Features, Fixes, Other) with one concise bullet per user-visible change; skip pure chores unless notable.",
      "Never invent changes that are not in the provided commits.",
      "Answer in GitHub-flavored markdown.",
      GUARDRAILS,
    ].join(" "),
    prompt: [
      `Write release notes for ${range}.`,
      "Commits (newest first):",
      lines,
      "",
      "File stats across the range:",
      stats,
    ].join("\n"),
  };
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
  repoDiff(repoId: RepoId, oldSide: DiffSide, newSide: DiffSide): Promise<FileDiff[]>;
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
    onServing?: (backend: AiBackend, model: string | null) => void;
  }): Promise<AiResult>;
  /** Streaming twin (M13); absent routers fall back to `generate`. */
  generateStream?(
    req: {
      system?: string;
      prompt: string;
      model?: string;
      signal?: AbortSignal;
      sessionKey?: string;
      onServing?: (backend: AiBackend, model: string | null) => void;
    },
    onDelta: (delta: string) => void,
  ): Promise<AiResult>;
}

/** Everything a runner needs (defaults wire the real app singletons). */
export interface FeatureDeps {
  git: FeatureGitClient;
  router: FeatureRouter;
  /** Per-repo opt-in gate (checked before anything is gathered/sent). */
  isOptedIn(repoId: string): boolean;
}

/**
 * Lifecycle observers for a feature run (M13). The job store adapts these
 * onto an `AiJobTicket` (see `jobs.svelte.ts`); `signal` is the UI's
 * Cancel button — it combines with the runner's own 120s deadline.
 */
export interface AiRunHooks {
  /** Phase advance with optional human detail ("reading staged diff"). */
  onPhase?(phase: AiJobPhase, message?: string): void;
  /** The built prompt (system, user) — for the activity log's preview. */
  onPrompt?(system: string, prompt: string): void;
  /** Which backend/model routing picked (the job's live "via" badge). */
  onServing?(backend: AiBackend, model: string | null): void;
  /** Streamed text chunks (only when the caller wants progressive text). */
  onDelta?(text: string): void;
  /** Aborts the run from outside (provider observes `cancelled`). */
  signal?: AbortSignal;
}

/** Gathering detail per feature kind (`null` = no git IO for this kind). */
const GATHER_LABELS: Record<FeatureKind, string | null> = {
  "commit-message": "reading staged diff",
  "pr-title-body": "reading commits and diff",
  "explain-hunk": null,
  "review-staged": "reading staged diff",
  "stash-message": "reading staged diff",
  "branch-name": null,
  // M13 kinds take pre-gathered ctx (their call sites own the data), so
  // there is nothing to gather inside the runner.
  "explain-commit": null,
  "explain-blame": null,
  "diagnose-workflow": null,
  "diagnose-action": null,
  "conflict-assist": null,
  "release-notes": null,
};

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
  // -- M13 additions (call sites pre-gather; runners shape the prompt) ----
  /** explain-commit: the commit's sha (display in the prompt header). */
  sha?: string;
  /** explain-blame: the 1-based line number. */
  line?: number;
  /** explain-blame: the line's text. */
  lineText?: string;
  /** explain-blame: the blaming commit's summary (when known). */
  commitSummary?: string;
  /** explain-blame: surrounding file slice (repo_read_file, capped). */
  fileSlice?: string;
  /** diagnose-*: run metadata (workflow name/branch/steps or command/exit). */
  runMeta?: string;
  /** conflict-assist: the "ours" side of the conflict. */
  ours?: string;
  /** conflict-assist: the "theirs" side of the conflict. */
  theirs?: string;
  /** release-notes: human label of the commit range ("v1.2..v1.3"). */
  range?: string;
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

/**
 * Deadline for the first log page. `repo_log_stream` resolves as soon as the
 * backend ACCEPTS the command (it walks on a spawned thread), so a stream
 * superseded before its first page — a history refresh or watcher bump
 * cancels it — would otherwise leave this promise pending forever and wedge
 * the repo's AI busy flag.
 */
const LOG_FIRST_PAGE_TIMEOUT_MS = 10_000;

/** Ceiling for one generation, so a stalled backend cannot wedge the UI. */
const FEATURE_TIMEOUT_MS = 120_000;

/** Gathers the first log page's commits for a repo. */
export async function gatherCommits(
  git: FeatureGitClient,
  repoId: string,
): Promise<Array<{ summary: string; sha: string }>> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const firstPage = new Promise<Array<{ summary: string; sha: string }>>((resolve, reject) => {
    timer = setTimeout(
      () =>
        reject(
          new AiError("unavailable", "timed out reading the repository history", {}),
        ),
      LOG_FIRST_PAGE_TIMEOUT_MS,
    );
    void git.streamLog(repoId, EMPTY_LOG_FILTER, (page) => {
      resolve(page.commits.map((c) => ({ summary: c.summary, sha: c.sha })));
    }).catch(reject);
  });
  try {
    return await firstPage;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Runs one AI feature end to end: opt-in gate → gather → prompt → generate
 * (through the supervisor, backend selection + fallback included) → parse.
 * Throws {@link AiError} on any failure; nothing is retried here.
 * `hooks` (M13) reports phase progress and carries the external cancel
 * signal — both optional, so existing callers are unaffected.
 */
export async function runFeature(
  kind: FeatureKind,
  ctx: FeatureCtx,
  deps: FeatureDeps,
  hooks?: AiRunHooks,
): Promise<FeatureOutcome> {
  if (!deps.isOptedIn(ctx.repoId)) {
    throw new AiError(
      "opt-in",
      "AI features are not enabled for this repository — allow them in the AI prompt or AI settings",
      {},
    );
  }

  const gatherLabel = GATHER_LABELS[kind];
  if (gatherLabel !== null) hooks?.onPhase?.("gathering", gatherLabel);

  let built: { system: string; prompt: string };
  switch (kind) {
    case "commit-message": {
      // When gathering, budget the rendered diff across files (stat
      // summary + fair per-file slices) so big changes keep every file
      // represented; a pre-rendered `ctx.diff` is passed through as-is.
      let diff: string;
      if (ctx.diff !== undefined) {
        diff = ctx.diff;
      } else {
        const files = await deps.git.repoDiff(ctx.repoId, "head", "index");
        if (files.length === 0) {
          throw new AiError("bad-response", "nothing is staged to write a commit message for", {});
        }
        diff = budgetedDiffText(files);
      }
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
      let diff: string;
      if (ctx.diff !== undefined) {
        diff = ctx.diff;
      } else {
        const files = await deps.git.repoDiff(ctx.repoId, "head", "index");
        if (files.length === 0) {
          throw new AiError("bad-response", "nothing is staged to review", {});
        }
        diff = budgetedDiffText(files);
      }
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
      let description = ctx.description?.trim() ?? "";
      if (description.length === 0) {
        // No explicit description: fall back to the branch's recent work
        // (first log page) so "suggest a name" works without any input —
        // the create-flow's empty-query case.
        const commits = await gatherCommits(deps.git, ctx.repoId);
        const lines = commits
          .slice(0, 10)
          .map((c) => `- ${c.summary}`)
          .join("\n");
        if (lines.length === 0) {
          throw new AiError("unknown", "branch-name needs a description or branch history", {});
        }
        description = `Recent commits on the current branch:\n${lines}`;
      }
      built = branchNamePrompt(description);
      break;
    }
    // -- M13 (pre-gathered ctx; the runner validates and shapes) ------------
    case "explain-commit": {
      if (!ctx.sha || !ctx.description || !ctx.diff) {
        throw new AiError("unknown", "explain-commit needs the commit message and its diff", {});
      }
      built = explainCommitPrompt(ctx.sha, ctx.description, ctx.diff);
      break;
    }
    case "explain-blame": {
      if (!ctx.path || ctx.line === undefined || ctx.lineText === undefined) {
        throw new AiError("unknown", "explain-blame needs a file path and line", {});
      }
      built = explainBlamePrompt(
        ctx.path,
        ctx.line,
        ctx.lineText,
        ctx.commitSummary ?? null,
        ctx.fileSlice ?? null,
      );
      break;
    }
    case "diagnose-workflow":
    case "diagnose-action": {
      if (!ctx.diff) {
        throw new AiError("unknown", "diagnosis needs the failed run's log", {});
      }
      const subject =
        kind === "diagnose-workflow" ? "The workflow run" : "The action run";
      built = diagnoseFailurePrompt(subject, ctx.runMeta ?? "(no run details)", ctx.diff);
      break;
    }
    case "conflict-assist": {
      if (!ctx.path || ctx.ours === undefined || ctx.theirs === undefined) {
        throw new AiError("unknown", "conflict-assist needs both sides of the conflict", {});
      }
      built = conflictSuggestionPrompt(ctx.path, ctx.ours, ctx.theirs, ctx.diff ?? null);
      break;
    }
    case "release-notes": {
      if (!ctx.commits || ctx.commits.length === 0) {
        throw new AiError("unknown", "release-notes needs the range's commits", {});
      }
      built = releaseNotesPrompt(
        ctx.range ?? "the selected range",
        ctx.commits,
        ctx.diff ?? "(no file stats)",
      );
      break;
    }
  }

  hooks?.onPhase?.("prompting");
  hooks?.onPrompt?.(built.system, built.prompt);

  // Deadline + external cancel share one controller: whichever fires
  // first aborts the request; the catch below distinguishes the two so a
  // user cancel reports "cancelled", not a timeout.
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), FEATURE_TIMEOUT_MS);
  const external = hooks?.signal;
  const onExternalAbort = () => controller.abort();
  if (external !== undefined) {
    if (external.aborted) controller.abort();
    else external.addEventListener("abort", onExternalAbort, { once: true });
  }
  hooks?.onPhase?.("generating");
  let result: AiResult;
  const request = {
    system: built.system,
    prompt: built.prompt,
    model: ctx.model,
    sessionKey: ctx.repoId,
    signal: controller.signal,
    onServing: hooks?.onServing,
  };
  try {
    // Streaming when a consumer wants deltas and the router supports it
    // (supervisor falls back to one-shot per backend capability anyway).
    if (hooks?.onDelta !== undefined && deps.router.generateStream !== undefined) {
      result = await deps.router.generateStream(request, (delta) => hooks.onDelta?.(delta));
    } else {
      result = await deps.router.generate(request);
    }
  } catch (err) {
    // Without this the repo's busy flag never clears and every later AI
    // request for it is refused while the spinner runs forever.
    if (controller.signal.aborted) {
      if (external?.aborted === true) {
        throw new AiError("cancelled", "cancelled", {});
      }
      throw new AiError("unavailable", "the AI backend did not finish in time", {});
    }
    throw err;
  } finally {
    clearTimeout(timer);
    external?.removeEventListener("abort", onExternalAbort);
  }

  hooks?.onPhase?.("parsing");

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
    case "conflict-assist":
      // The suggestion IS the text — but never let conflict markers
      // survive into the editor's custom-text field.
      outcome.text = parseConflictSuggestion(result.text);
      break;
    case "explain-commit":
    case "explain-blame":
    case "diagnose-workflow":
    case "diagnose-action":
    case "release-notes":
      // Free-form markdown answers; the text field carries them as-is.
      break;
  }
  return outcome;
}
