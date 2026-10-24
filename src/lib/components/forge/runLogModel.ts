/**
 * Pure model for GitHub Actions logs (no Svelte, no IPC — fully tested).
 *
 * `gh run view --log` prints tab-separated records:
 *
 *     <job name>\t<step name>\t<ISO timestamp> <message>
 *
 * Newer `gh` versions keep the same shape and may leave the step as
 * `UNKNOWN STEP`. Workflow commands (`##[group]Title`, `::group::Title`,
 * `##[error]…`, …) stay embedded in the message. This module turns that flat
 * stream into the job → step → collapsible-group tree the run detail view
 * renders, without trusting the input to be well formed.
 */

/** Severity used for line tinting and the log summary counters. */
export type LogLineLevel = "normal" | "error" | "warning" | "command" | "debug";

/** One rendered log line (timestamp split out, ANSI escapes removed). */
export interface LogLine {
  /** ISO timestamp as printed by `gh` ("" when the line has none). */
  time: string;
  /** Human-facing message, without the timestamp or workflow-command marker. */
  text: string;
  level: LogLineLevel;
}

/**
 * A step's content in visual order: loose lines and any `##[group]` sections
 * the workflow emitted. Group markers become collapsible blocks in the UI.
 */
export type LogSegment =
  | { kind: "lines"; key: string; lines: LogLine[] }
  | { kind: "group"; key: string; title: string; lines: LogLine[] };

/** One step within a job (the row GitHub shows above its logs). */
export interface LogStep {
  /** Stable id (`job\0step`) used for collapse state and scrolling. */
  key: string;
  name: string;
  segments: LogSegment[];
  lineCount: number;
  errorCount: number;
  warningCount: number;
  /** First/last ISO timestamp seen in this step (`null` when undated). */
  firstTime: string | null;
  lastTime: string | null;
}

/** One job of a run log. */
export interface LogJob {
  key: string;
  name: string;
  steps: LogStep[];
  lineCount: number;
  errorCount: number;
  warningCount: number;
}

/** A parsed run/job log with rolled-up counters. */
export interface ParsedRunLog {
  jobs: LogJob[];
  lineCount: number;
  errorCount: number;
  warningCount: number;
}

/** A highlighted slice of a line, used by the log search renderer. */
export interface HighlightPart {
  text: string;
  match: boolean;
}

const ANSI_OSC = /\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g;
const ANSI_CSI = /\u001b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g;
const ANSI_OTHER = /\u001b\([A-Za-z0-9]/g;

const ISO_TIME =
  /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2}))(?:\s|$)/;

/** Removes ANSI colour/control sequences from one line. */
export function stripAnsi(text: string): string {
  return text.replace(ANSI_OSC, "").replace(ANSI_CSI, "").replace(ANSI_OTHER, "");
}

/** Normalizes a step name for fuzzy matching between API and log records. */
export function normalizeStepName(name: string): string {
  return stripAnsi(name).trim().toLowerCase().replace(/\s+/g, " ");
}

interface ParsedMessage {
  time: string;
  text: string;
  level: LogLineLevel;
  group: "start" | "end" | null;
}

function cleanMessage(raw: string): string {
  return raw.replace(/^\s+/, "").replace(/\s+$/, "");
}

/**
 * Recognizes GitHub Actions workflow commands while keeping the visible
 * message. Group markers are reported separately so the caller can nest.
 */
function parseMessage(raw: string): ParsedMessage {
  const withTime = ISO_TIME.exec(raw);
  const time = withTime?.[1] ?? "";
  let text = cleanMessage(withTime ? raw.slice(withTime[0].length) : raw);
  let level: LogLineLevel = "normal";
  let group: "start" | "end" | null = null;

  // Classic workflow commands: `##[group]Title`, `##[error]Message`, …
  const classic = /^##\[([a-zA-Z]+)\](.*)$/.exec(text);
  if (classic) {
    const command = classic[1]!.toLowerCase();
    const rest = cleanMessage(classic[2] ?? "");
    switch (command) {
      case "group":
        text = rest || "Details";
        group = "start";
        break;
      case "endgroup":
        text = "";
        group = "end";
        break;
      case "error":
        level = "error";
        text = rest;
        break;
      case "warning":
        level = "warning";
        text = rest;
        break;
      case "debug":
        level = "debug";
        text = rest;
        break;
      case "command":
        level = "command";
        text = rest;
        break;
      case "section":
      case "notice":
        text = rest;
        break;
      default:
        text = rest;
        break;
    }
  } else {
    // Newer workflow commands: `::group::Title`, `::error::Message`, etc.
    const modern = /^::([a-zA-Z]+)(?:\s+[^:]*)?::(.*)$/.exec(text);
    if (modern) {
      const command = modern[1]!.toLowerCase();
      const rest = cleanMessage(modern[2] ?? "");
      switch (command) {
        case "group":
          text = rest || "Details";
          group = "start";
          break;
        case "endgroup":
          text = "";
          group = "end";
          break;
        case "error":
          level = "error";
          text = rest;
          break;
        case "warning":
          level = "warning";
          text = rest;
          break;
        case "debug":
          level = "debug";
          text = rest;
          break;
        case "notice":
          text = rest;
          break;
        default:
          text = rest;
          break;
      }
    }
  }

  if (level === "normal" && text !== "") {
    if (/^(?:error|fatal)\b/i.test(text)) level = "error";
    else if (/^warning\b/i.test(text)) level = "warning";
  }

  return { time, text, level, group };
}

interface MutableStep {
  key: string;
  name: string;
  segments: LogSegment[];
  activeGroup: Extract<LogSegment, { kind: "group" }> | null;
}

interface MutableJob {
  key: string;
  name: string;
  steps: Map<string, MutableStep>;
}

function stepKey(jobName: string, name: string): string {
  return `${jobName}\u0000${name}`;
}

function ensureLineSegment(step: MutableStep): Extract<LogSegment, { kind: "lines" }> {
  const last = step.segments[step.segments.length - 1];
  if (last?.kind === "lines") return last;
  const segment: Extract<LogSegment, { kind: "lines" }> = {
    kind: "lines",
    key: `${step.key}\u0000lines-${step.segments.length}`,
    lines: [],
  };
  step.segments.push(segment);
  return segment;
}

function appendLine(step: MutableStep, line: LogLine): void {
  if (step.activeGroup) {
    step.activeGroup.lines.push(line);
    return;
  }
  ensureLineSegment(step).lines.push(line);
}

function startGroup(step: MutableStep, title: string): void {
  // GitHub groups are normally flat. If a workflow opens one while another
  // is active, close the outer one rather than nesting collapsibles.
  step.activeGroup = null;
  const group: Extract<LogSegment, { kind: "group" }> = {
    kind: "group",
    key: `${step.key}\u0000group-${step.segments.length}`,
    title: title || "Details",
    lines: [],
  };
  step.segments.push(group);
  step.activeGroup = group;
}

function endGroup(step: MutableStep): void {
  step.activeGroup = null;
}

function finalizeStep(step: MutableStep): LogStep {
  const segments = step.segments.filter((segment) => segment.lines.length > 0);
  let lineCount = 0;
  let errorCount = 0;
  let warningCount = 0;
  let firstTime: string | null = null;
  let lastTime: string | null = null;
  for (const segment of segments) {
    for (const line of segment.lines) {
      if (line.text !== "" || line.time !== "") lineCount += 1;
      if (line.level === "error") errorCount += 1;
      if (line.level === "warning") warningCount += 1;
      if (line.time !== "") {
        if (firstTime === null || line.time < firstTime) firstTime = line.time;
        if (lastTime === null || line.time > lastTime) lastTime = line.time;
      }
    }
  }
  return {
    key: step.key,
    name: step.name,
    segments,
    lineCount,
    errorCount,
    warningCount,
    firstTime,
    lastTime,
  };
}

function finalizeJob(job: MutableJob): LogJob {
  const steps = [...job.steps.values()].map(finalizeStep);
  let lineCount = 0;
  let errorCount = 0;
  let warningCount = 0;
  for (const step of steps) {
    lineCount += step.lineCount;
    errorCount += step.errorCount;
    warningCount += step.warningCount;
  }
  return { key: job.key, name: job.name, steps, lineCount, errorCount, warningCount };
}

/**
 * Parses one `gh run view --log` payload into jobs → steps → segments.
 * Never throws: malformed lines become plain log lines. Empty input yields
 * an empty result.
 */
export function parseWorkflowLog(text: string): ParsedRunLog {
  const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const rawLines = normalized.split("\n");
  if (rawLines.length > 0 && rawLines[rawLines.length - 1] === "") rawLines.pop();

  const jobs = new Map<string, MutableJob>();
  for (const raw of rawLines) {
    const plain = stripAnsi(raw);
    const parts = plain.split("\t");
    let jobName: string;
    let stepName: string;
    let message: string;
    if (parts.length >= 3) {
      jobName = parts[0]!.trim();
      stepName = parts[1]!.trim();
      message = parts.slice(2).join("\t");
    } else if (parts.length === 2) {
      jobName = parts[0]!.trim();
      stepName = "";
      message = parts[1]!;
    } else {
      jobName = "";
      stepName = "";
      message = plain;
    }
    if (jobName === "") jobName = "Logs";
    if (stepName === "" || /^unknown step$/i.test(stepName)) stepName = "Output";

    const parsed = parseMessage(message);
    const jobKey = jobName;
    let job = jobs.get(jobKey);
    if (!job) {
      job = { key: jobKey, name: jobName, steps: new Map() };
      jobs.set(jobKey, job);
    }
    const key = stepKey(jobName, stepName);
    let step = job.steps.get(key);
    if (!step) {
      step = { key, name: stepName, segments: [], activeGroup: null };
      job.steps.set(key, step);
    }

    if (parsed.group === "start") {
      startGroup(step, parsed.text);
      continue;
    }
    if (parsed.group === "end") {
      endGroup(step);
      continue;
    }
    // Keep blank lines only when they carry a timestamp (real separators) so
    // a trailing newline does not become an empty row.
    if (parsed.text === "" && parsed.time === "") continue;
    appendLine(step, { time: parsed.time, text: parsed.text, level: parsed.level });
  }

  const finalized = [...jobs.values()].map(finalizeJob);
  let lineCount = 0;
  let errorCount = 0;
  let warningCount = 0;
  for (const job of finalized) {
    lineCount += job.lineCount;
    errorCount += job.errorCount;
    warningCount += job.warningCount;
  }
  return { jobs: finalized, lineCount, errorCount, warningCount };
}

function countSegments(segments: LogSegment[]): {
  lineCount: number;
  errorCount: number;
  warningCount: number;
} {
  let lineCount = 0;
  let errorCount = 0;
  let warningCount = 0;
  for (const segment of segments) {
    for (const line of segment.lines) {
      if (line.text !== "" || line.time !== "") lineCount += 1;
      if (line.level === "error") errorCount += 1;
      if (line.level === "warning") warningCount += 1;
    }
  }
  return { lineCount, errorCount, warningCount };
}

/**
 * Keeps only matching lines (case-insensitive substring). A job or step name
 * that matches keeps its whole block, which is what a user expects when they
 * search for a step title. Empty query returns the input unchanged.
 */
export function filterParsedLog(parsed: ParsedRunLog, query: string): ParsedRunLog {
  const needle = query.trim().toLowerCase();
  if (!needle) return parsed;
  const jobs: LogJob[] = [];
  for (const job of parsed.jobs) {
    const jobMatches = job.name.toLowerCase().includes(needle);
    const steps: LogStep[] = [];
    for (const step of job.steps) {
      const stepMatches = jobMatches || step.name.toLowerCase().includes(needle);
      const segments: LogSegment[] = [];
      for (const segment of step.segments) {
        if (stepMatches) {
          segments.push(segment);
          continue;
        }
        if (segment.kind === "group" && segment.title.toLowerCase().includes(needle)) {
          segments.push(segment);
          continue;
        }
        const lines = segment.lines.filter((line) =>
          line.text.toLowerCase().includes(needle),
        );
        if (lines.length > 0) {
          segments.push({ ...segment, lines } as LogSegment);
        }
      }
      if (segments.length === 0) continue;
      const counts = countSegments(segments);
      steps.push({
        ...step,
        segments,
        lineCount: counts.lineCount,
        errorCount: counts.errorCount,
        warningCount: counts.warningCount,
      });
    }
    if (steps.length === 0) continue;
    let lineCount = 0;
    let errorCount = 0;
    let warningCount = 0;
    for (const step of steps) {
      lineCount += step.lineCount;
      errorCount += step.errorCount;
      warningCount += step.warningCount;
    }
    jobs.push({ ...job, steps, lineCount, errorCount, warningCount });
  }
  let lineCount = 0;
  let errorCount = 0;
  let warningCount = 0;
  for (const job of jobs) {
    lineCount += job.lineCount;
    errorCount += job.errorCount;
    warningCount += job.warningCount;
  }
  return { jobs, lineCount, errorCount, warningCount };
}

/**
 * Counts case-insensitive occurrences of `query` across every line of a
 * parsed log. Empty queries count 0. Pure.
 */
export function countLogMatches(parsed: ParsedRunLog, query: string): number {
  const needle = query.trim().toLowerCase();
  if (!needle) return 0;
  let matches = 0;
  for (const job of parsed.jobs) {
    for (const step of job.steps) {
      for (const segment of step.segments) {
        for (const line of segment.lines) {
          const haystack = line.text.toLowerCase();
          let cursor = 0;
          while (cursor < haystack.length) {
            const index = haystack.indexOf(needle, cursor);
            if (index === -1) break;
            matches += 1;
            cursor = index + needle.length;
          }
        }
      }
    }
  }
  return matches;
}

/**
 * Splits `text` around case-insensitive occurrences of `query` for safe
 * rendering (no `{@html}`; the caller renders spans).
 */
export function highlightText(text: string, query: string): HighlightPart[] {
  const needle = query.trim();
  if (needle === "") return [{ text, match: false }];
  const lowerText = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: HighlightPart[] = [];
  let cursor = 0;
  while (cursor < text.length) {
    const index = lowerText.indexOf(lowerNeedle, cursor);
    if (index === -1) {
      parts.push({ text: text.slice(cursor), match: false });
      break;
    }
    if (index > cursor) parts.push({ text: text.slice(cursor, index), match: false });
    parts.push({ text: text.slice(index, index + needle.length), match: true });
    cursor = index + needle.length;
  }
  return parts.length > 0 ? parts : [{ text, match: false }];
}

/** `HH:MM:SS` from an ISO timestamp (empty when the input has no time). */
export function formatLogTime(timestamp: string): string {
  const time = /T(\d{2}:\d{2}:\d{2})/.exec(timestamp)?.[1];
  return time ?? "";
}

/** First error line of a parsed log, useful for auto-scroll/focus. */
export function firstErrorLine(parsed: ParsedRunLog): LogLine | null {
  for (const job of parsed.jobs) {
    for (const step of job.steps) {
      for (const segment of step.segments) {
        for (const line of segment.lines) {
          if (line.level === "error") return line;
        }
      }
    }
  }
  return null;
}
