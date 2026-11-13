/**
 * Unit tests for the AI feature layer: pure prompt builders, output
 * parsers, diff renderers, and the feature runners (opt-in gating, data
 * gathering, parsing) against an in-memory git client + router.
 */

import { describe, expect, it, vi } from "vitest";
import type { FileDiff } from "$lib/ipc/types";
import {
  branchNamePrompt,
  capDiff,
  commitMessagePrompt,
  diffSummaryText,
  explainHunkPrompt,
  formatFileDiff,
  formatHunkText,
  MAX_DIFF_CHARS,
  parseBranchName,
  parseCommitMessage,
  prTitleBodyPrompt,
  reviewStagedPrompt,
  runFeature,
  stashMessagePrompt,
  stagedDiffText,
  type FeatureDeps,
} from "./features";
import { AiError } from "./types";
import type { AiResult } from "./types";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const FILE: FileDiff = {
  path: "src/a.ts",
  old_path: null,
  binary: false,
  is_image: false,
  additions: 2,
  deletions: 1,
  hunks: [
    {
      old_start: 1,
      new_start: 1,
      lines: [
        { old_no: 1, new_no: 1, origin: " ", text: "const a = 1;", highlights: [] },
        { old_no: 2, new_no: null, origin: "-", text: "const b = 2;", highlights: [] },
        { old_no: null, new_no: 2, origin: "+", text: "const b = 3;", highlights: [] },
        { old_no: null, new_no: 3, origin: "=", text: "export { a, b };", highlights: [] },
      ],
    },
  ],
};

function aiResult(text: string): AiResult {
  return { text, model: "test-model", backend: "opencode", elapsedMs: 5 };
}

/** In-memory deps for runFeature (spies included). */
function makeDeps(opts?: {
  files?: FileDiff[];
  commits?: Array<{ summary: string; sha: string }>;
  generate?: (prompt: string) => string;
  optedIn?: boolean;
}): FeatureDeps & {
  git: FeatureDeps["git"];
  generate: ReturnType<typeof vi.fn>;
} {
  const generate = vi.fn(
    async (req: { prompt: string }) =>
      aiResult(opts?.generate?.(req.prompt) ?? "fix(ui): fix the thing\n\nBecause it was broken."),
  );
  return {
    git: {
      repoDiff: vi.fn(async () => opts?.files ?? [FILE]),
      streamLog: vi.fn(async (_repoId, _filter, onPage) => {
        const commits = opts?.commits ?? [{ summary: "add thing", sha: "abc1234" }];
        onPage({
          commits: commits.map((c) => ({
            sha: c.sha,
            parents: [],
            author: { name: "A", email: "a@b.c", time: 0, offset_minutes: 0 },
            committer: { name: "A", email: "a@b.c", time: 0, offset_minutes: 0 },
            message: c.summary,
            summary: c.summary,
            refs: [],
          })),
          rows: [],
          next_cursor: null,
          generation: 0,
        });
      }),
    },
    router: { generate },
    isOptedIn: () => opts?.optedIn ?? true,
    generate,
  };
}

// ---------------------------------------------------------------------------
// capDiff
// ---------------------------------------------------------------------------

describe("capDiff", () => {
  it("passes short diffs through unchanged", () => {
    expect(capDiff("hello", 100)).toBe("hello");
  });

  it("truncates long diffs at the cap with an explicit note", () => {
    const text = Array.from({ length: 100 }, (_, i) => `line-${i} of a long diff`).join("\n");
    const capped = capDiff(text, 200);
    expect(capped.length).toBeLessThanOrEqual(200 + 100); // note adds length
    expect(capped).toContain("[diff truncated: showing first");
    expect(capped).toContain(`of ${text.length} characters]`);
    expect(capped.length).toBeLessThan(text.length);
  });

  it("defaults the cap to MAX_DIFF_CHARS (400k, ~128k-token context)", () => {
    const text = Array.from(
      { length: 10_000 },
      (_, i) => `line ${i} of a fairly long diff used for the default-cap test`,
    ).join("\n");
    expect(text.length).toBeGreaterThan(MAX_DIFF_CHARS);
    const capped = capDiff(text);
    expect(capped.length).toBeLessThan(text.length);
    expect(capped).toContain("[diff truncated");
  });
});

// ---------------------------------------------------------------------------
// Prompt builders
// ---------------------------------------------------------------------------

describe("commitMessagePrompt", () => {
  it("conventional style asks for Conventional Commits and a ≤50-char subject", () => {
    const { system, prompt } = commitMessagePrompt("DIFF TEXT", "conventional");
    expect(system).toContain("Conventional Commits");
    expect(system).toContain("50 characters");
    expect(system).toContain("WHY");
    expect(prompt).toContain("DIFF TEXT");
  });

  it("plain style omits the conventional-types requirement", () => {
    const { system } = commitMessagePrompt("DIFF", "plain");
    expect(system).not.toContain("Conventional Commits");
    expect(system).toContain("50 characters");
  });

  it("always carries the guardrails (no secrets, no placeholders)", () => {
    const { system } = commitMessagePrompt("DIFF", "conventional");
    expect(system).toContain("Never include secrets");
    expect(system).toContain("Never emit placeholders");
  });

  it("specifies the subject/blank/body output format", () => {
    const { prompt } = commitMessagePrompt("DIFF", "conventional");
    expect(prompt).toContain("The first line is the subject");
  });

  it("caps oversized diffs with a truncation note inside the prompt", () => {
    const big = "y".repeat(MAX_DIFF_CHARS * 2);
    const { prompt } = commitMessagePrompt(big, "conventional");
    expect(prompt).toContain("[diff truncated");
    expect(prompt.length).toBeLessThan(big.length);
  });
});

describe("other prompt builders", () => {
  it("prTitleBodyPrompt lists commit shas and file stats", () => {
    const { prompt } = prTitleBodyPrompt(
      [
        { summary: "add parser", sha: "aaa1234abcd" },
        { summary: "wire ui", sha: "bbb5678" },
      ],
      "- src/a.ts (+2/-1)",
    );
    expect(prompt).toContain("aaa1234");
    expect(prompt).toContain("bbb5678");
    expect(prompt).toContain("add parser");
    expect(prompt).toContain("- src/a.ts (+2/-1)");
  });

  it("explainHunkPrompt names the file and embeds the hunk", () => {
    const { prompt } = explainHunkPrompt("src/x.ts", "+ new line");
    expect(prompt).toContain("`src/x.ts`");
    expect(prompt).toContain("+ new line");
  });

  it("reviewStagedPrompt embeds the staged diff", () => {
    const { system, prompt } = reviewStagedPrompt("DIFF");
    expect(system).toContain("review");
    expect(prompt).toContain("DIFF");
  });

  it("stashMessagePrompt asks for a short one-line message", () => {
    const { system } = stashMessagePrompt("wip: refactor");
    expect(system).toContain("stash");
    expect(system).toContain("60 characters");
  });

  it("branchNamePrompt demands one kebab-case line", () => {
    const { system, prompt } = branchNamePrompt("Add dark mode toggle");
    expect(system).toContain("kebab-case");
    expect(prompt).toContain("Add dark mode toggle");
    expect(prompt).toContain("branch name only");
  });
});

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

describe("parseCommitMessage", () => {
  it("splits subject and body on the first blank line", () => {
    expect(parseCommitMessage("fix(ui): fix it\n\nWhy it broke.\nSecond para.")).toEqual({
      subject: "fix(ui): fix it",
      body: "Why it broke.\nSecond para.",
    });
  });

  it("strips code fences around the whole answer", () => {
    expect(parseCommitMessage("```\nfeat: add x\n\nBody here.\n```")).toEqual({
      subject: "feat: add x",
      body: "Body here.",
    });
  });

  it("tolerates a leading 'Subject:' label", () => {
    expect(parseCommitMessage("Subject: chore: tidy\n\nbody")).toEqual({
      subject: "chore: tidy",
      body: "body",
    });
  });

  it("keeps a subject-only answer (empty body)", () => {
    expect(parseCommitMessage("  docs: readme  \n")).toEqual({
      subject: "docs: readme",
      body: "",
    });
  });

  it("throws AiError bad-response on an empty subject", () => {
    expect(() => parseCommitMessage("")).toThrowError(AiError);
    expect(() => parseCommitMessage("```\n\n```")).toThrowError(AiError);
  });
});

describe("parseBranchName", () => {
  it("takes the last non-empty line", () => {
    expect(parseBranchName("Sure!\n\nadd-dark-mode")).toBe("add-dark-mode");
  });

  it("sanitizes to lowercase kebab-case (markdown, spaces, slashes)", () => {
    expect(parseBranchName("- **`Add Dark Mode/Toggle!`**")).toBe("add-dark-mode-toggle");
  });

  it("collapses repeated separators and strips edge hyphens", () => {
    expect(parseBranchName("--weird--name--")).toBe("weird-name");
  });

  it("throws AiError bad-response when nothing usable remains", () => {
    expect(() => parseBranchName("```\n***\n```")).toThrowError(AiError);
  });
});

// ---------------------------------------------------------------------------
// Diff renderers
// ---------------------------------------------------------------------------

describe("formatFileDiff / stagedDiffText", () => {
  it("renders unified-diff-style text with hunk headers and markers", () => {
    const text = formatFileDiff(FILE);
    expect(text).toContain("--- a/src/a.ts");
    expect(text).toContain("+++ b/src/a.ts");
    expect(text).toContain("@@ -1 +1 @@");
    expect(text).toContain("\n-const b = 2;");
    expect(text).toContain("\n+const b = 3;");
    // '=' context lines render as a leading space, not '='.
    expect(text).toContain(" export { a, b };");
    expect(text).not.toContain("=export");
  });

  it("renders binary files as a note instead of hunks", () => {
    const binary: FileDiff = { ...FILE, path: "img.png", binary: true };
    expect(formatFileDiff(binary)).toContain("Binary file img.png");
  });

  it("uses old_path for renames in the '---' header", () => {
    const renamed: FileDiff = { ...FILE, path: "new.ts", old_path: "old.ts" };
    expect(formatFileDiff(renamed)).toContain("--- a/old.ts");
    expect(formatFileDiff(renamed)).toContain("+++ b/new.ts");
  });

  it("joins all files in stagedDiffText", () => {
    const text = stagedDiffText([FILE, { ...FILE, path: "b.ts" }]);
    expect(text).toContain("+++ b/src/a.ts");
    expect(text).toContain("+++ b/b.ts");
  });

  it("diffSummaryText lists path with +/- counts", () => {
    expect(diffSummaryText([FILE])).toBe("- src/a.ts (+2/-1)");
  });
});

describe("formatHunkText", () => {
  it("renders one hunk with header, markers and '=' as context", () => {
    const text = formatHunkText(FILE.hunks[0]);
    expect(text.startsWith("@@ -1 +1 @@")).toBe(true);
    expect(text).toContain("\n-const b = 2;");
    expect(text).toContain("\n+const b = 3;");
    expect(text).toContain(" export { a, b };");
    expect(text).not.toContain("=export");
  });

  it("is exactly the per-hunk slice of formatFileDiff (minus file headers)", () => {
    const whole = formatFileDiff(FILE);
    const slice = formatHunkText(FILE.hunks[0]);
    expect(whole.endsWith(slice)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Feature runners
// ---------------------------------------------------------------------------

describe("runFeature", () => {
  it("commit-message gathers the staged diff and parses the answer", async () => {
    const deps = makeDeps();
    const outcome = await runFeature("commit-message", { repoId: "r1" }, deps);

    expect(deps.git.repoDiff).toHaveBeenCalledWith("r1", "head", "index");
    expect(deps.generate).toHaveBeenCalledTimes(1);
    const req = deps.generate.mock.calls[0][0];
    expect(req.system).toContain("Conventional Commits");
    expect(req.prompt).toContain("+++ b/src/a.ts");
    expect(req.sessionKey).toBe("r1");

    expect(outcome.kind).toBe("commit-message");
    expect(outcome.message).toEqual({
      subject: "fix(ui): fix the thing",
      body: "Because it was broken.",
    });
    expect(outcome.result.backend).toBe("opencode");
  });

  it("commit-message refuses with kind opt-in when the repo is not opted in (gathers nothing)", async () => {
    const deps = makeDeps({ optedIn: false });
    await expect(runFeature("commit-message", { repoId: "r1" }, deps)).rejects.toMatchObject({
      name: "AiError",
      kind: "opt-in",
    });
    expect(deps.git.repoDiff).not.toHaveBeenCalled();
    expect(deps.generate).not.toHaveBeenCalled();
  });

  it("commit-message fails cleanly when nothing is staged", async () => {
    const deps = makeDeps({ files: [] });
    await expect(runFeature("commit-message", { repoId: "r1" }, deps)).rejects.toMatchObject({
      kind: "bad-response",
    });
    expect(deps.generate).not.toHaveBeenCalled();
  });

  it("review-staged sends the diff and returns free-form text", async () => {
    const deps = makeDeps({ generate: () => "Looks fine, one nit." });
    const outcome = await runFeature("review-staged", { repoId: "r1" }, deps);
    expect(outcome.text).toBe("Looks fine, one nit.");
    expect(outcome.message).toBeUndefined();
  });

  it("pr-title-body gathers commits and diff stats", async () => {
    const deps = makeDeps();
    await runFeature("pr-title-body", { repoId: "r1" }, deps);
    const req = deps.generate.mock.calls[0][0];
    expect(deps.git.streamLog).toHaveBeenCalled();
    expect(req.prompt).toContain("abc1234");
    expect(req.prompt).toContain("- src/a.ts (+2/-1)");
  });

  it("explain-hunk needs path + hunk", async () => {
    const deps = makeDeps();
    await expect(
      runFeature("explain-hunk", { repoId: "r1" }, deps),
    ).rejects.toMatchObject({ kind: "unknown" });
    await expect(
      runFeature("explain-hunk", { repoId: "r1", path: "a.ts", hunk: "+x" }, deps),
    ).resolves.toMatchObject({ kind: "explain-hunk" });
    expect(deps.git.repoDiff).not.toHaveBeenCalled();
  });

  it("branch-name gathers nothing and parses a kebab-case name", async () => {
    const deps = makeDeps({ generate: () => "Here you go:\n\nfeat/dark-mode-toggle" });
    const outcome = await runFeature(
      "branch-name",
      { repoId: "r1", description: "dark mode toggle" },
      deps,
    );
    // Slashes are sanitized away (the prompt asks for slash-free names).
    expect(outcome.branchName).toBe("feat-dark-mode-toggle");
    expect(deps.git.repoDiff).not.toHaveBeenCalled();
  });

  it("stash-message uses a provided summary without touching the git client", async () => {
    const deps = makeDeps({ generate: () => "wip: half-done refactor\n\nParking it." });
    const outcome = await runFeature(
      "stash-message",
      { repoId: "r1", diff: "refactor middleware (half done)" },
      deps,
    );
    expect(outcome.message?.subject).toBe("wip: half-done refactor");
    expect(deps.git.repoDiff).not.toHaveBeenCalled();
  });

  it("propagates router failures untouched (no auto-retry)", async () => {
    const deps = makeDeps();
    deps.router.generate = vi.fn(async () => {
      throw new AiError("unavailable", "backend down", { backend: "opencode" });
    });
    await expect(runFeature("commit-message", { repoId: "r1" }, deps)).rejects.toMatchObject({
      kind: "unavailable",
    });
    expect(deps.router.generate).toHaveBeenCalledTimes(1);
  });

  it("fails instead of hanging when the log stream never sends a page", async () => {
    // repo_log_stream resolves on acceptance, so a stream superseded before
    // its first page used to leave pr-title-body pending forever.
    vi.useFakeTimers();
    try {
      const deps = makeDeps();
      deps.git.streamLog = vi.fn(async () => {});
      const pending = runFeature("pr-title-body", { repoId: "r1" }, deps);
      const assertion = expect(pending).rejects.toMatchObject({ kind: "unavailable" });
      await vi.advanceTimersByTimeAsync(10_000);
      await assertion;
    } finally {
      vi.useRealTimers();
    }
  });

  it("aborts a generation that never settles and surfaces a timeout", async () => {
    vi.useFakeTimers();
    try {
      const deps = makeDeps();
      let seen: AbortSignal | undefined;
      deps.router.generate = vi.fn(
        (req: { signal?: AbortSignal }) =>
          new Promise<AiResult>((_resolve, reject) => {
            seen = req.signal;
            req.signal?.addEventListener("abort", () =>
              reject(new AiError("unavailable", "aborted", {})),
            );
          }),
      );
      const pending = runFeature("commit-message", { repoId: "r1" }, deps);
      const assertion = expect(pending).rejects.toMatchObject({
        kind: "unavailable",
        message: "the AI backend did not finish in time",
      });
      await vi.advanceTimersByTimeAsync(120_000);
      await assertion;
      expect(seen?.aborted).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("passes an abort signal to every generation", async () => {
    const deps = makeDeps();
    await runFeature("commit-message", { repoId: "r1" }, deps);
    expect(deps.generate.mock.calls[0][0].signal).toBeInstanceOf(AbortSignal);
  });
});

// ---------------------------------------------------------------------------
// M13 prompt builders + parsers
// ---------------------------------------------------------------------------

import {
  conflictSuggestionPrompt,
  diagnoseFailurePrompt,
  explainBlamePrompt,
  explainCommitPrompt,
  parseConflictSuggestion,
  releaseNotesPrompt,
} from "./features";

describe("M13 prompt builders", () => {
  it("explainCommitPrompt carries sha, message and diff with guardrails", () => {
    const built = explainCommitPrompt("abcdef1234567890", "feat: add thing\n\nbody", "-old\n+new");
    expect(built.system).toContain("explain Git commits");
    expect(built.system).toContain("Never include secrets");
    expect(built.prompt).toContain("Commit abcdef1234");
    expect(built.prompt).toContain("feat: add thing");
    expect(built.prompt).toContain("```diff\n-old\n+new\n```");
  });

  it("explainBlamePrompt includes line number/text and optional parts", () => {
    const withAll = explainBlamePrompt("src/a.ts", 7, "let x = 1;", "introduce x", "1| let x = 1;");
    expect(withAll.prompt).toContain("File: src/a.ts");
    expect(withAll.prompt).toContain("Line 7:");
    expect(withAll.prompt).toContain('Introduced by commit: "introduce x"');
    expect(withAll.prompt).toContain("1| let x = 1;");

    const minimal = explainBlamePrompt("f.ts", 1, "x", null, null);
    expect(minimal.prompt).not.toContain("Introduced by");
    expect(minimal.prompt).not.toContain("Surrounding file content");
  });

  it("diagnoseFailurePrompt frames the subject, meta and log tail", () => {
    const built = diagnoseFailurePrompt("The workflow run", "Workflow: CI\nBranch: main", "error: boom");
    expect(built.system).toContain("diagnose failed CI");
    expect(built.prompt).toContain("The workflow run failed.");
    expect(built.prompt).toContain("Workflow: CI");
    expect(built.prompt).toContain("error: boom");
  });

  it("conflictSuggestionPrompt demands replacement text only", () => {
    const built = conflictSuggestionPrompt("a.ts", "ours lines", "theirs lines", "context");
    expect(built.system).toContain("ONLY the resolved replacement text");
    expect(built.prompt).toContain('"ours"');
    expect(built.prompt).toContain('"theirs"');
    expect(built.prompt).toContain("context");
    const bare = conflictSuggestionPrompt("a.ts", "o", "t", null);
    expect(bare.prompt).not.toContain("Surrounding content");
  });

  it("releaseNotesPrompt lists commits and stats for the range", () => {
    const built = releaseNotesPrompt(
      "v1..v2",
      [
        { summary: "add feature", sha: "aaa111bbb222" },
        { summary: "fix bug", sha: "ccc333ddd444" },
      ],
      "Files changed (2):",
    );
    expect(built.prompt).toContain("Write release notes for v1..v2.");
    expect(built.prompt).toContain("- add feature (aaa111bb)");
    expect(built.prompt).toContain("Files changed (2):");
    const empty = releaseNotesPrompt("x", [], "(none)");
    expect(empty.prompt).toContain("(no commits)");
  });
});

describe("parseConflictSuggestion", () => {
  it("passes plain replacement text through", () => {
    expect(parseConflictSuggestion("const a = 1;\nconst b = 2;")).toBe("const a = 1;\nconst b = 2;");
  });

  it("strips one wrapping code fence", () => {
    expect(parseConflictSuggestion("```ts\nconst a = 1;\n```")).toBe("const a = 1;");
  });

  it("never lets conflict markers survive", () => {
    const noisy = [
      "Here is the resolution:",
      "```",
      "<<<<<<< HEAD",
      "ours",
      "=======",
      "theirs",
      ">>>>>>> branch",
      "resolved line",
      "```",
    ].join("\n");
    const parsed = parseConflictSuggestion(noisy);
    expect(parsed).not.toContain("<<<<<<<");
    expect(parsed).not.toContain("=======");
    expect(parsed).not.toContain(">>>>>>>");
    expect(parsed).toContain("resolved line");
  });
});

// ---------------------------------------------------------------------------
// M13 runFeature cases (pre-gathered ctx → prompt → generate)
// ---------------------------------------------------------------------------

describe("runFeature M13 kinds", () => {
  it("explain-commit validates its required ctx", async () => {
    const deps = makeDeps();
    await expect(
      runFeature("explain-commit", { repoId: "r1" }, deps),
    ).rejects.toMatchObject({ kind: "unknown" });
    expect(deps.generate).not.toHaveBeenCalled();
  });

  it("explain-commit builds from pre-gathered message + diff and returns text", async () => {
    const deps = makeDeps({ generate: () => "It adds a feature." });
    const outcome = await runFeature(
      "explain-commit",
      { repoId: "r1", sha: "abc1234def", description: "feat: thing", diff: "-a\n+b" },
      deps,
    );
    expect(outcome.text).toBe("It adds a feature.");
    expect(deps.generate.mock.calls[0][0].prompt).toContain("Commit abc1234def");
    expect(deps.generate.mock.calls[0][0].prompt).toContain("feat: thing");
  });

  it("explain-blame accepts the minimal line ctx and optional extras", async () => {
    const deps = makeDeps();
    const outcome = await runFeature(
      "explain-blame",
      { repoId: "r1", path: "a.ts", line: 3, lineText: "let x = 1;" },
      deps,
    );
    expect(outcome.text).toBeTruthy();
    expect(deps.generate.mock.calls[0][0].prompt).toContain("Line 3:");

    await runFeature(
      "explain-blame",
      {
        repoId: "r1",
        path: "a.ts",
        line: 3,
        lineText: "let x = 1;",
        commitSummary: "introduce x",
        fileSlice: "1| let x = 1;",
      },
      deps,
    );
    expect(deps.generate.mock.calls[1][0].prompt).toContain("introduce x");
    await expect(
      runFeature("explain-blame", { repoId: "r1", path: "a.ts" }, deps),
    ).rejects.toMatchObject({ kind: "unknown" });
  });

  it("diagnose-workflow / diagnose-action share the builder; log is required", async () => {
    const deps = makeDeps({ generate: () => "The failing step is X." });
    const outcome = await runFeature(
      "diagnose-workflow",
      { repoId: "r1", runMeta: "Workflow: CI", diff: "log tail" },
      deps,
    );
    expect(outcome.text).toBe("The failing step is X.");
    expect(deps.generate.mock.calls[0][0].prompt).toContain("The workflow run failed.");

    await runFeature(
      "diagnose-action",
      { repoId: "r1", runMeta: "Exit code: 1", diff: "log tail" },
      deps,
    );
    expect(deps.generate.mock.calls[1][0].prompt).toContain("The action run failed.");

    await expect(
      runFeature("diagnose-action", { repoId: "r1", runMeta: "x" }, deps),
    ).rejects.toMatchObject({ kind: "unknown" });
  });

  it("conflict-assist parses the suggestion into text", async () => {
    const deps = makeDeps({ generate: () => "```\nresolved\n```" });
    const outcome = await runFeature(
      "conflict-assist",
      { repoId: "r1", path: "a.ts", ours: "o", theirs: "t" },
      deps,
    );
    expect(outcome.text).toBe("resolved");
    await expect(
      runFeature("conflict-assist", { repoId: "r1", path: "a.ts", ours: "o" }, deps),
    ).rejects.toMatchObject({ kind: "unknown" });
  });

  it("release-notes requires commits and passes stats through ctx.diff", async () => {
    const deps = makeDeps({ generate: () => "## Features\n- thing" });
    const outcome = await runFeature(
      "release-notes",
      {
        repoId: "r1",
        range: "v1..v2",
        commits: [{ summary: "add", sha: "abc" }],
        diff: "Files changed (1):",
      },
      deps,
    );
    expect(outcome.text).toContain("## Features");
    expect(deps.generate.mock.calls[0][0].prompt).toContain("Files changed (1):");
    await expect(
      runFeature("release-notes", { repoId: "r1", range: "v1..v2" }, deps),
    ).rejects.toMatchObject({ kind: "unknown" });
  });
});
