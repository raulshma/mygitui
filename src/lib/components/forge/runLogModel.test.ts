/**
 * Unit tests for `runLogModel.ts`: `gh run view --log` parsing (jobs →
 * steps → groups), workflow commands, ANSI handling, filtering, match
 * counting, highlighting and timestamps. Pure — no IPC, no Svelte.
 */

import { describe, expect, it } from "vitest";
import {
  countLogMatches,
  filterParsedLog,
  firstErrorLine,
  formatLogTime,
  highlightText,
  normalizeStepName,
  parseWorkflowLog,
  stripAnsi,
  type LogSegment,
} from "./runLogModel";

const TS = "2026-10-20T02:00:00.1234567Z";

function line(job: string, step: string, time: string, message: string): string {
  return `${job}\t${step}\t${time} ${message}`;
}

describe("stripAnsi", () => {
  it("removes CSI colour codes and other escapes", () => {
    expect(stripAnsi("\u001b[31merror\u001b[0m")).toBe("error");
    expect(stripAnsi("\u001b[1;32mok\u001b[39m")).toBe("ok");
    expect(stripAnsi("\u001b(Btext")).toBe("text");
    expect(stripAnsi("\u001b]0;title\u0007text")).toBe("text");
    expect(stripAnsi("plain")).toBe("plain");
  });
});

describe("normalizeStepName", () => {
  it("lowercases and collapses whitespace", () => {
    expect(normalizeStepName("  Set   Up  ")).toBe("set up");
    expect(normalizeStepName("\u001b[1mBuild\u001b[0m")).toBe("build");
  });
});

describe("parseWorkflowLog", () => {
  it("groups tab-separated lines into jobs and steps", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Set up job", TS, "Checking out"),
        line("build", "Build", TS, "compiling"),
        line("lint", "Lint", TS, "clean"),
      ].join("\n"),
    );
    expect(parsed.jobs.map((job) => job.name)).toEqual(["build", "lint"]);
    expect(parsed.jobs[0]!.steps.map((step) => step.name)).toEqual([
      "Set up job",
      "Build",
    ]);
    expect(parsed.jobs[0]!.lineCount).toBe(2);
    expect(parsed.lineCount).toBe(3);
    expect(parsed.jobs[0]!.steps[0]!.segments[0]).toMatchObject({
      kind: "lines",
      lines: [{ time: TS, text: "Checking out", level: "normal" }],
    });
  });

  it("splits the ISO timestamp off the message", () => {
    const parsed = parseWorkflowLog(
      line("build", "Build", "2026-10-20T02:03:04Z", "hello"),
    );
    const logLine = (parsed.jobs[0]!.steps[0]!.segments[0] as LogSegment).lines[0]!;
    expect(logLine).toEqual({ time: "2026-10-20T02:03:04Z", text: "hello", level: "normal" });
  });

  it("keeps steps named UNKNOWN STEP under a friendly Output step", () => {
    const parsed = parseWorkflowLog(
      line("build", "UNKNOWN STEP", TS, "no step metadata"),
    );
    expect(parsed.jobs[0]!.steps[0]!.name).toBe("Output");
  });

  it("handles unstructured input without throwing", () => {
    const parsed = parseWorkflowLog("just an unstructured line\n");
    expect(parsed.jobs).toHaveLength(1);
    expect(parsed.jobs[0]!.name).toBe("Logs");
    expect(parsed.jobs[0]!.steps[0]!.name).toBe("Output");
    expect(parsed.jobs[0]!.steps[0]!.segments[0]!.lines[0]!.text).toBe(
      "just an unstructured line",
    );
  });

  it("normalizes CRLF and ignores a trailing newline", () => {
    const parsed = parseWorkflowLog(
      `${line("build", "Build", TS, "one")}\r\n${line("build", "Build", TS, "two")}\r\n`,
    );
    expect(parsed.lineCount).toBe(2);
    expect(parsed.jobs[0]!.steps[0]!.segments[0]!.lines).toHaveLength(2);
  });

  it("returns an empty result for empty input", () => {
    expect(parseWorkflowLog("")).toEqual({
      jobs: [],
      lineCount: 0,
      errorCount: 0,
      warningCount: 0,
    });
  });

  it("strips ANSI before parsing the record", () => {
    const parsed = parseWorkflowLog(
      `${line("build", "Build", TS, "\u001b[31merror: boom\u001b[0m")}\n`,
    );
    expect(parsed.jobs[0]!.steps[0]!.segments[0]!.lines[0]!.text).toBe(
      "error: boom",
    );
    expect(parsed.errorCount).toBe(1);
  });
});

describe("workflow commands", () => {
  it("turns classic group markers into collapsible segments", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "before"),
        line("build", "Build", TS, "##[group]Runner info"),
        line("build", "Build", TS, "Image: ubuntu-latest"),
        line("build", "Build", TS, "##[endgroup]"),
        line("build", "Build", TS, "after"),
      ].join("\n"),
    );
    const segments = parsed.jobs[0]!.steps[0]!.segments;
    expect(segments.map((segment) => segment.kind)).toEqual([
      "lines",
      "group",
      "lines",
    ]);
    expect(segments[1]).toMatchObject({
      kind: "group",
      title: "Runner info",
      lines: [{ text: "Image: ubuntu-latest" }],
    });
    expect(segments[0]!.lines[0]!.text).toBe("before");
    expect(segments[2]!.lines[0]!.text).toBe("after");
  });

  it("supports modern ::group:: commands", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "::group::Modern"),
        line("build", "Build", TS, "inside"),
        line("build", "Build", TS, "::endgroup::"),
      ].join("\n"),
    );
    expect(parsed.jobs[0]!.steps[0]!.segments[0]).toMatchObject({
      kind: "group",
      title: "Modern",
    });
  });

  it("closes an open group when a new one starts (flat, no nesting)", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "##[group]A"),
        line("build", "Build", TS, "a"),
        line("build", "Build", TS, "##[group]B"),
        line("build", "Build", TS, "b"),
        line("build", "Build", TS, "##[endgroup]"),
        line("build", "Build", TS, "tail"),
      ].join("\n"),
    );
    const segments = parsed.jobs[0]!.steps[0]!.segments;
    expect(segments).toHaveLength(3);
    expect(segments[0]).toMatchObject({ kind: "group", title: "A" });
    expect(segments[1]).toMatchObject({ kind: "group", title: "B" });
    expect(segments[2]!.lines.map((entry) => entry.text)).toEqual(["tail"]);
  });

  it("names groups with an empty title Details", () => {
    const parsed = parseWorkflowLog(
      [line("build", "Build", TS, "##[group]"), line("build", "Build", TS, "detail")].join("\n"),
    );
    expect(parsed.jobs[0]!.steps[0]!.segments[0]).toMatchObject({
      kind: "group",
      title: "Details",
    });
  });

  it("maps error/warning/debug/command commands to line levels", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "##[error]bad"),
        line("build", "Build", TS, "##[warning]careful"),
        line("build", "Build", TS, "##[debug]trace"),
        line("build", "Build", TS, "##[command]run"),
        line("build", "Build", TS, "::error::modern bad"),
        line("build", "Build", TS, "::warning::modern careful"),
      ].join("\n"),
    );
    const lines = parsed.jobs[0]!.steps[0]!.segments[0]!.lines;
    expect(lines.map((entry) => entry.level)).toEqual([
      "error",
      "warning",
      "debug",
      "command",
      "error",
      "warning",
    ]);
    expect(lines.map((entry) => entry.text)).toEqual([
      "bad",
      "careful",
      "trace",
      "run",
      "modern bad",
      "modern careful",
    ]);
    expect(parsed.errorCount).toBe(2);
    expect(parsed.warningCount).toBe(2);
  });

  it("infers error/warning levels from plain text prefixes", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "error: plain"),
        line("build", "Build", TS, "fatal: nope"),
        line("build", "Build", TS, "warning: careful"),
        line("build", "Build", TS, "not an error"),
      ].join("\n"),
    );
    expect(
      parsed.jobs[0]!.steps[0]!.segments[0]!.lines.map((entry) => entry.level),
    ).toEqual(["error", "error", "warning", "normal"]);
  });

  it("keeps blank timestamped lines but drops empty records", () => {
    const parsed = parseWorkflowLog(
      `${line("build", "Build", TS, "")}\n`,
    );
    expect(parsed.lineCount).toBe(1);
    expect(parsed.jobs[0]!.steps[0]!.segments[0]!.lines[0]).toMatchObject({
      time: TS,
      text: "",
    });
  });

  it("rolls up counters and time ranges per step and job", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", "2026-10-20T02:00:05Z", "##[error]late"),
        line("build", "Build", "2026-10-20T02:00:00Z", "##[warning]early"),
      ].join("\n"),
    );
    const step = parsed.jobs[0]!.steps[0]!;
    expect(step.errorCount).toBe(1);
    expect(step.warningCount).toBe(1);
    expect(step.firstTime).toBe("2026-10-20T02:00:00Z");
    expect(step.lastTime).toBe("2026-10-20T02:00:05Z");
    expect(parsed.jobs[0]!.errorCount).toBe(1);
    expect(parsed.jobs[0]!.warningCount).toBe(1);
  });
});

describe("filterParsedLog", () => {
  const parsed = parseWorkflowLog(
    [
      line("build", "Build", TS, "compiling widget"),
      line("build", "Build", TS, "needle in the haystack"),
      line("build", "Unit tests", TS, "all good"),
      line("lint", "Lint", TS, "no findings"),
    ].join("\n"),
  );

  it("returns the input for an empty query", () => {
    expect(filterParsedLog(parsed, "")).toBe(parsed);
    expect(filterParsedLog(parsed, "   ")).toBe(parsed);
  });

  it("keeps matching lines and recounts", () => {
    const filtered = filterParsedLog(parsed, "NEEDLE");
    expect(filtered.jobs).toHaveLength(1);
    expect(filtered.jobs[0]!.steps).toHaveLength(1);
    expect(filtered.jobs[0]!.steps[0]!.segments[0]!.lines).toHaveLength(1);
    expect(filtered.jobs[0]!.steps[0]!.segments[0]!.lines[0]!.text).toContain(
      "needle",
    );
    expect(filtered.lineCount).toBe(1);
  });

  it("keeps every step of a matching job", () => {
    const filtered = filterParsedLog(parsed, "lint");
    expect(filtered.jobs).toHaveLength(1);
    expect(filtered.jobs[0]!.name).toBe("lint");
    expect(filtered.jobs[0]!.steps).toHaveLength(1);
    expect(filtered.lineCount).toBe(1);
  });

  it("keeps a matching step's full content", () => {
    const filtered = filterParsedLog(parsed, "unit tests");
    expect(filtered.jobs[0]!.steps.map((step) => step.name)).toEqual([
      "Unit tests",
    ]);
    expect(filtered.lineCount).toBe(1);
  });

  it("can hide non-matching content inside a matching step", () => {
    const filtered = filterParsedLog(parsed, "needle");
    expect(filtered.errorCount).toBe(0);
    expect(filterParsedLog(parsed, "missing").jobs).toHaveLength(0);
  });
});

describe("countLogMatches", () => {
  it("counts every case-insensitive occurrence", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "abc ABC abc"),
        line("build", "Build", TS, "nothing"),
      ].join("\n"),
    );
    expect(countLogMatches(parsed, "abc")).toBe(3);
    expect(countLogMatches(parsed, "  ")).toBe(0);
    expect(countLogMatches(parsed, "zzz")).toBe(0);
  });
});

describe("highlightText", () => {
  it("splits around every case-insensitive match", () => {
    expect(highlightText("Build failed: build", "BUILD")).toEqual([
      { text: "Build", match: true },
      { text: " failed: ", match: false },
      { text: "build", match: true },
    ]);
  });

  it("returns one plain part when nothing matches or the query is empty", () => {
    expect(highlightText("abc", "zzz")).toEqual([{ text: "abc", match: false }]);
    expect(highlightText("abc", "")).toEqual([{ text: "abc", match: false }]);
  });

  it("treats regex metacharacters as literal text", () => {
    expect(highlightText("a.b(c)", ".")).toEqual([
      { text: "a", match: false },
      { text: ".", match: true },
      { text: "b(c)", match: false },
    ]);
  });
});

describe("formatLogTime", () => {
  it("extracts HH:MM:SS and leaves unknown input empty", () => {
    expect(formatLogTime("2026-10-20T02:03:04.1234567Z")).toBe("02:03:04");
    expect(formatLogTime("2026-10-20T02:03:04Z")).toBe("02:03:04");
    expect(formatLogTime("")).toBe("");
    expect(formatLogTime("not a time")).toBe("");
  });
});

describe("firstErrorLine", () => {
  it("returns the first error in job/step order", () => {
    const parsed = parseWorkflowLog(
      [
        line("build", "Build", TS, "fine"),
        line("build", "Test", TS, "##[error]first"),
        line("lint", "Lint", TS, "##[error]second"),
      ].join("\n"),
    );
    expect(firstErrorLine(parsed)?.text).toBe("first");
    expect(firstErrorLine(parseWorkflowLog(""))).toBeNull();
  });
});
