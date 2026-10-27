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
  number?: number;
  status?: string;
  conclusion?: string;
  started_at?: string | null;
  completed_at?: string | null;
  durationLabel?: string | null;
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
  databaseId?: number;
  status?: string;
  conclusion?: string;
  started_at?: string | null;
  completed_at?: string | null;
  durationLabel?: string | null;
  steps: LogStep[];
  lineCount: number;
  errorCount: number;
  warningCount: number;
}

/** External metadata for a step used during parsing correlation. */
export interface KnownStepMeta {
  name: string;
  number?: number;
  status?: string;
  conclusion?: string;
  started_at?: string | null;
  completed_at?: string | null;
}

/** External metadata for a job used during parsing correlation. */
export interface KnownJobMeta {
  name: string;
  database_id?: number;
  status?: string;
  conclusion?: string;
  started_at?: string | null;
  completed_at?: string | null;
  steps?: KnownStepMeta[];
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
  /** Composite-action boundary markers (`##[start-action]` / `##[end-action]`). */
  action: "start" | "end" | null;
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
  let action: "start" | "end" | null = null;

  // Classic workflow commands: `##[group]Title`, `##[error]Message`,
  // `##[start-action display=…;id=…]`, …
  const classic = /^##\[([a-zA-Z][a-zA-Z-]*)(?:\s+[^\]]*)?\](.*)$/.exec(text);
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
      case "start-action":
        text = "";
        action = "start";
        break;
      case "end-action":
        text = "";
        action = "end";
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

  return { time, text, level, group, action };
}

interface MutableStep {
  key: string;
  name: string;
  number?: number;
  status?: string;
  conclusion?: string;
  started_at?: string | null;
  completed_at?: string | null;
  segments: LogSegment[];
  activeGroup: Extract<LogSegment, { kind: "group" }> | null;
}

interface MutableJob {
  key: string;
  name: string;
  databaseId?: number;
  status?: string;
  conclusion?: string;
  started_at?: string | null;
  completed_at?: string | null;
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
      lineCount += 1;
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
    number: step.number,
    status: step.status,
    conclusion: step.conclusion,
    started_at: step.started_at,
    completed_at: step.completed_at,
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
  return {
    key: job.key,
    name: job.name,
    databaseId: job.databaseId,
    status: job.status,
    conclusion: job.conclusion,
    started_at: job.started_at,
    completed_at: job.completed_at,
    steps,
    lineCount,
    errorCount,
    warningCount,
  };
}

/**
 * GitHub prints each step as a `##[group]Run …` / `##[group]Post …` block.
 * Those groups are step headers, not nested log groups.
 */
function isStepBoundaryTitle(title: string): boolean {
  return (
    /^run\s/i.test(title) ||
    /^post\s/i.test(title) ||
    /^complete job$/i.test(title)
  );
}

/** Drops the `Run `/`Post ` prefix GitHub adds to step group titles. */
function stripStepPrefix(name: string): string {
  return name.replace(/^run\s+/i, "").replace(/^post\s+/i, "");
}

/**
 * Similarity of a boundary group title to a known step name: 3 = same,
 * 2 = prefix, 1.x = shared significant words, 0 = unrelated.
 */
function stepMatchScore(title: string, stepName: string): number {
  const a = normalizeStepName(stripStepPrefix(title))
    .replace(/^actions\//, "")
    .replace(/@v?[\d.]+$/, "");
  const b = normalizeStepName(stepName);
  if (!a || !b) return 0;
  if (a === b) return 3;
  if (b.startsWith(a) || a.startsWith(b)) return 2;
  const aWords = a.split(/[^a-z0-9]+/).filter((word) => word.length > 2);
  const bWords = b.split(/[^a-z0-9]+/).filter((word) => word.length > 2);
  let hits = 0;
  for (const word of aWords) if (bWords.includes(word)) hits += 1;
  return hits > 0 ? 1 + hits * 0.1 : 0;
}

/**
 * Metadata timestamps are second-precision while group headers carry
 * sub-second times; allow this much slack in either direction.
 */
const STEP_TIME_TOLERANCE_MS = 1_500;

/**
 * Picks the step a boundary group belongs to: best name match within the next
 * few steps (time-gated), else the next non-skipped step that had already
 * started when the group was emitted.
 */
function correlateBoundaryStep(
  title: string,
  timeMs: number | null,
  steps: KnownStepMeta[],
  current: number,
): number {
  let best = -1;
  let bestScore = 0;
  const end = Math.min(steps.length, current + 5);
  for (let i = current; i < end; i += 1) {
    const step = steps[i]!;
    if (step.conclusion === "skipped") continue;
    const startMs = step.started_at ? Date.parse(step.started_at) : NaN;
    const endMs = step.completed_at ? Date.parse(step.completed_at) : NaN;
    if (
      timeMs !== null &&
      !Number.isNaN(startMs) &&
      timeMs < startMs - STEP_TIME_TOLERANCE_MS
    ) {
      continue;
    }
    if (
      timeMs !== null &&
      !Number.isNaN(endMs) &&
      timeMs > endMs + STEP_TIME_TOLERANCE_MS
    ) {
      continue;
    }
    const score = stepMatchScore(title, step.name);
    if (score > bestScore) {
      bestScore = score;
      best = i;
    }
  }
  if (bestScore >= 1 && best > current) return best;

  // No name match: advance only when the next step had started by the time
  // the group header was printed. Buffered output can be timestamped late,
  // so this stays conservative.
  for (let i = current + 1; i < steps.length; i += 1) {
    const step = steps[i]!;
    if (step.conclusion === "skipped") continue;
    const startMs = step.started_at ? Date.parse(step.started_at) : NaN;
    if (Number.isNaN(startMs)) break;
    if (timeMs !== null && startMs <= timeMs + STEP_TIME_TOLERANCE_MS) return i;
    break;
  }
  return current;
}

/** Index of the next `Post …` step after `current` (or `current`). */
function nextPostStep(steps: KnownStepMeta[], current: number): number {
  for (let i = current + 1; i < steps.length; i += 1) {
    const step = steps[i]!;
    if (step.conclusion === "skipped") continue;
    if (/^post\s/i.test(step.name)) return i;
  }
  return current;
}

/** Index of the next `Complete job` step after `current` (or `current`). */
function nextCompleteJobStep(steps: KnownStepMeta[], current: number): number {
  for (let i = current + 1; i < steps.length; i += 1) {
    if (/^complete job$/i.test(steps[i]!.name)) return i;
  }
  return current;
}

/**
 * Parses one `gh run view --log` payload into jobs → steps → segments.
 * When `knownJobs` is provided (from workflow run details), correlates logs
 * with individual steps so each step's logs are separately collapsible.
 * Never throws: malformed lines become plain log lines. Empty input yields
 * an empty result.
 */
export function parseWorkflowLog(
  text: string,
  knownJobs?: KnownJobMeta[] | Record<string, KnownStepMeta[]> | KnownStepMeta[],
): ParsedRunLog {
  // `gh api …/jobs/<id>/logs` starts with a UTF-8 BOM; strip it wherever it
  // appears so the first record is not mistaken for an unstructured line.
  const normalized = text
    .replace(/\uFEFF/g, "")
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n");
  const rawLines = normalized.split("\n");
  if (rawLines.length > 0 && rawLines[rawLines.length - 1] === "") rawLines.pop();

  // Normalize knownJobs input into Map<string, KnownJobMeta>
  const jobMetaMap = new Map<string, KnownJobMeta>();
  let singleKnownJob: KnownJobMeta | null = null;
  if (Array.isArray(knownJobs)) {
    if (knownJobs.length > 0) {
      const first = knownJobs[0] as unknown as Record<string, unknown>;
      if (
        first &&
        (Array.isArray(first["steps"]) ||
          typeof first["database_id"] === "number" ||
          typeof first["databaseId"] === "number")
      ) {
        for (const job of knownJobs as KnownJobMeta[]) {
          jobMetaMap.set(job.name.toLowerCase().trim(), job);
        }
        if (knownJobs.length === 1) singleKnownJob = (knownJobs as KnownJobMeta[])[0] ?? null;
      } else if (first && typeof first["name"] === "string") {
        const synth: KnownJobMeta = {
          name: "Logs",
          steps: knownJobs as KnownStepMeta[],
        };
        jobMetaMap.set("logs", synth);
        singleKnownJob = synth;
      }
    }
  } else if (knownJobs && typeof knownJobs === "object") {
    for (const [name, steps] of Object.entries(knownJobs)) {
      const entry: KnownJobMeta = { name, steps: steps as KnownStepMeta[] };
      jobMetaMap.set(name.toLowerCase().trim(), entry);
    }
    if (Object.keys(knownJobs).length === 1) {
      singleKnownJob = [...jobMetaMap.values()][0] ?? null;
    }
  }

  interface LineItem {
    rawStepName: string;
    parsed: ParsedMessage;
  }
  const jobLines = new Map<string, LineItem[]>();
  const defaultJobName = singleKnownJob ? singleKnownJob.name : "Logs";

  for (const raw of rawLines) {
    const plain = stripAnsi(raw);
    // A completely empty line carries no record; tab-separated records with
    // an empty message are real output (multi-line command continuations).
    if (plain === "") continue;
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
    if (jobName === "") jobName = defaultJobName;

    const parsed = parseMessage(message);

    let list = jobLines.get(jobName);
    if (!list) {
      list = [];
      jobLines.set(jobName, list);
    }
    list.push({ rawStepName: stepName, parsed });
  }

  // Include known jobs with 0 log lines so they are not missing from the
  // view, and keep metadata order first (the log archive is usually in the
  // same order, but selected-job fallbacks are not).
  const findJobName = (name: string): string | undefined =>
    [...jobLines.keys()].find(
      (candidate) => candidate.toLowerCase().trim() === name.toLowerCase().trim(),
    );
  for (const known of jobMetaMap.values()) {
    if (findJobName(known.name) === undefined) jobLines.set(known.name, []);
  }
  const orderedJobNames: string[] = [];
  for (const known of jobMetaMap.values()) {
    const name = findJobName(known.name);
    if (name !== undefined && !orderedJobNames.includes(name)) orderedJobNames.push(name);
  }
  for (const name of jobLines.keys()) {
    if (!orderedJobNames.includes(name)) orderedJobNames.push(name);
  }

  const jobs: LogJob[] = [];
  for (const jobName of orderedJobNames) {
    const lines = jobLines.get(jobName) ?? [];
    const knownJob =
      jobMetaMap.get(jobName.toLowerCase().trim()) ??
      (singleKnownJob?.name.toLowerCase().trim() === jobName.toLowerCase().trim()
        ? singleKnownJob
        : null);

    const mutableJob: MutableJob = {
      key: jobName,
      name: jobName,
      databaseId: knownJob?.database_id,
      status: knownJob?.status,
      conclusion: knownJob?.conclusion,
      started_at: knownJob?.started_at,
      completed_at: knownJob?.completed_at,
      steps: new Map(),
    };

    if (knownJob?.steps && knownJob.steps.length > 0) {
      const knownStepsList = knownJob.steps;
      const knownStates = knownStepsList.map((ks, idx) => {
        const key = stepKey(jobName, ks.name);
        const step: MutableStep = {
          key,
          name: ks.name,
          number: ks.number ?? (idx + 1),
          status: ks.status,
          conclusion: ks.conclusion,
          started_at: ks.started_at,
          completed_at: ks.completed_at,
          segments: [],
          activeGroup: null,
        };
        mutableJob.steps.set(key, step);
        return step;
      });

      let currentStepIdx = 0;
      let actionDepth = 0;
      for (const { rawStepName, parsed } of lines) {
        // Composite-action markers only delimit nested groups inside a step;
        // they are not visible output.
        if (parsed.action === "start") {
          actionDepth += 1;
          continue;
        }
        if (parsed.action === "end") {
          actionDepth = Math.max(0, actionDepth - 1);
          continue;
        }

        const explicit =
          rawStepName !== "" && !/^unknown step$/i.test(rawStepName);
        if (explicit) {
          const matchIdx = knownStepsList.findIndex(
            (step) => normalizeStepName(step.name) === normalizeStepName(rawStepName),
          );
          if (matchIdx !== -1) currentStepIdx = matchIdx;
        }

        if (parsed.group === "start") {
          if (actionDepth === 0 && isStepBoundaryTitle(parsed.text)) {
            // The group header *is* the step title; rendering it again would
            // duplicate the step row. Explicit log step names win; otherwise
            // correlate by title and time.
            if (!explicit) {
              const timeMs = parsed.time ? Date.parse(parsed.time) : NaN;
              currentStepIdx = correlateBoundaryStep(
                parsed.text,
                Number.isNaN(timeMs) ? null : timeMs,
                knownStepsList,
                currentStepIdx,
              );
            }
            knownStates[currentStepIdx]!.activeGroup = null;
            continue;
          }
          startGroup(knownStates[currentStepIdx]!, parsed.text);
          continue;
        }
        if (parsed.group === "end") {
          endGroup(knownStates[currentStepIdx]!);
          continue;
        }

        // `gh` does not emit a `Post …` group for the automatic cleanup step
        // and uses a sentinel line for `Complete job`; advance on those so
        // their output lands in the right collapsible section.
        if (!explicit && parsed.text === "Post job cleanup.") {
          currentStepIdx = nextPostStep(knownStepsList, currentStepIdx);
          knownStates[currentStepIdx]!.activeGroup = null;
        } else if (!explicit && parsed.text === "Cleaning up orphan processes") {
          currentStepIdx = nextCompleteJobStep(knownStepsList, currentStepIdx);
          knownStates[currentStepIdx]!.activeGroup = null;
        }

        appendLine(knownStates[currentStepIdx]!, {
          time: parsed.time,
          text: parsed.text,
          level: parsed.level,
        });
      }
    } else {
      // No metadata: step names come from explicit log records when present,
      // otherwise from the `Run …` / `Post …` / `Complete job` boundary groups.
      const boundaryTitles: string[] = [];
      {
        let depth = 0;
        for (const { parsed } of lines) {
          if (parsed.action === "start") {
            depth += 1;
          } else if (parsed.action === "end") {
            depth = Math.max(0, depth - 1);
          } else if (
            parsed.group === "start" &&
            depth === 0 &&
            isStepBoundaryTitle(parsed.text)
          ) {
            boundaryTitles.push(parsed.text);
          }
        }
      }

      const ensureStep = (name: string): MutableStep => {
        const key = stepKey(jobName, name);
        let step = mutableJob.steps.get(key);
        if (!step) {
          step = { key, name, segments: [], activeGroup: null };
          mutableJob.steps.set(key, step);
        }
        return step;
      };

      const initialStepName = boundaryTitles.length > 0 ? "Set up job" : "Output";
      let currentStep: MutableStep | null = null;
      let currentName = "";
      let actionDepth = 0;
      let boundaryCursor = 0;

      const switchTo = (name: string): MutableStep => {
        currentName = name;
        currentStep = ensureStep(name);
        return currentStep;
      };

      const nextBoundaryTitle = (
        predicate: (title: string) => boolean,
      ): string | null => {
        for (let i = boundaryCursor; i < boundaryTitles.length; i += 1) {
          const title = boundaryTitles[i]!;
          if (predicate(title)) return title;
        }
        return null;
      };

      for (const { rawStepName, parsed } of lines) {
        if (parsed.action === "start") {
          actionDepth += 1;
          continue;
        }
        if (parsed.action === "end") {
          actionDepth = Math.max(0, actionDepth - 1);
          continue;
        }

        const explicit =
          rawStepName !== "" && !/^unknown step$/i.test(rawStepName);
        const boundaryGroup =
          parsed.group === "start" &&
          actionDepth === 0 &&
          isStepBoundaryTitle(parsed.text);

        if (boundaryGroup) {
          boundaryCursor += 1;
          if (explicit) {
            if (currentName !== rawStepName) switchTo(rawStepName);
          } else {
            switchTo(parsed.text);
          }
          currentStep!.activeGroup = null;
          continue;
        }

        if (explicit) {
          if (currentName !== rawStepName) switchTo(rawStepName);
        } else if (parsed.text === "Post job cleanup.") {
          const title = nextBoundaryTitle((value) => /^post\s/i.test(value));
          switchTo(title ?? "Post job cleanup.");
          currentStep!.activeGroup = null;
        } else if (parsed.text === "Cleaning up orphan processes") {
          const title = nextBoundaryTitle((value) => /^complete job$/i.test(value));
          switchTo(title ?? "Complete job");
          currentStep!.activeGroup = null;
        }

        if (!currentStep) currentStep = switchTo(initialStepName);

        if (parsed.group === "start") {
          startGroup(currentStep, parsed.text);
        } else if (parsed.group === "end") {
          endGroup(currentStep);
        } else {
          appendLine(currentStep, {
            time: parsed.time,
            text: parsed.text,
            level: parsed.level,
          });
        }
      }

      if (mutableJob.steps.size === 0) {
        const key = stepKey(jobName, "Output");
        mutableJob.steps.set(key, { key, name: "Output", segments: [], activeGroup: null });
      }
    }

    jobs.push(finalizeJob(mutableJob));
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
      lineCount += 1;
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
