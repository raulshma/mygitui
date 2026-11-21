/**
 * PR-diff shaping (E2, plan §9.4): converts `forge_pr_files` payloads
 * (REST `pulls/<n>/files` — path/status/stats plus a unified `patch` text
 * that is `null` above GitHub's size cap and for binaries) into the
 * `FileDiff[]` the shared {@link DiffViewer} renders. Pure and unit
 * tested; the viewer owns everything else (toolbar, virtualization,
 * narrow-container defaults).
 *
 * The REST patch carries bare `@@` hunks (no `diff --git`/`---`/`+++`
 * headers; GitHub omits `rename from/to` lines), so rename detection in
 * the viewer's header badge falls back to the +/- counters — `ForgePrFile`
 * has no `previous_filename`. `\ No newline at end of file` markers ride
 * as context ("=") lines.
 */

import type {
  DiffHunk,
  DiffLine,
  FileDiff,
  ForgePrFile,
} from "$lib/ipc/types";

/** `@@ -oldStart[,oldCount] +newStart[,newCount] @@ …` (counts default 1). */
const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** Parses one hunk's header; `null` when the line is not a hunk header. */
export function parseHunkHeader(line: string): {
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
} | null {
  const match = HUNK_HEADER.exec(line);
  if (match === null) return null;
  return {
    oldStart: Number(match[1]),
    oldCount: match[2] === undefined ? 1 : Number(match[2]),
    newStart: Number(match[3]),
    newCount: match[4] === undefined ? 1 : Number(match[4]),
  };
}

/** Maps one signed patch line to a {@link DiffLine} (`null` = skip). */
function patchLine(line: string, oldNo: number, newNo: number): {
  line: DiffLine;
  oldAdvance: number;
  newAdvance: number;
} | null {
  const sign = line.charAt(0);
  const text = line.slice(1);
  if (sign === "\\") {
    // `\ No newline at end of file` — context-flavored metadata.
    return {
      line: {
        old_no: oldNo > 0 ? oldNo : null,
        new_no: newNo > 0 ? newNo : null,
        origin: "=",
        text: text.replace(/^ ?/, ""),
        highlights: [],
      },
      oldAdvance: 0,
      newAdvance: 0,
    };
  }
  if (sign === "+") {
    return {
      line: { old_no: null, new_no: newNo, origin: "+", text, highlights: [] },
      oldAdvance: 0,
      newAdvance: 1,
    };
  }
  if (sign === "-") {
    return {
      line: { old_no: oldNo, new_no: null, origin: "-", text, highlights: [] },
      oldAdvance: 1,
      newAdvance: 0,
    };
  }
  if (sign === " ") {
    return {
      line: {
        old_no: oldNo > 0 ? oldNo : null,
        new_no: newNo > 0 ? newNo : null,
        origin: " ",
        text,
        highlights: [],
      },
      oldAdvance: 1,
      newAdvance: 1,
    };
  }
  // Anything else (an empty trailing line, a stray header) is not hunk
  // content — skip it.
  return null;
}

/** Parses one file's unified patch text into hunks (`[]` when unparsable). */
export function parsePrPatch(patch: string): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  let current: {
    hunk: DiffHunk;
    oldNo: number;
    oldEnd: number;
    newNo: number;
    newEnd: number;
  } | null = null;

  for (const raw of patch.split("\n")) {
    const line = raw.replace(/\r$/, "");
    if (current === null) {
      const header = parseHunkHeader(line);
      if (header === null) continue; // pre-hunk noise (rename headers, …)
      current = {
        hunk: { old_start: header.oldStart, new_start: header.newStart, lines: [] },
        oldNo: header.oldStart,
        oldEnd: header.oldStart + header.oldCount,
        newNo: header.newStart,
        newEnd: header.newStart + header.newCount,
      };
      continue;
    }
    if (HUNK_HEADER.test(line)) {
      hunks.push(current.hunk);
      const header = parseHunkHeader(line)!;
      current = {
        hunk: { old_start: header.oldStart, new_start: header.newStart, lines: [] },
        oldNo: header.oldStart,
        oldEnd: header.oldStart + header.oldCount,
        newNo: header.newStart,
        newEnd: header.newStart + header.newCount,
      };
      continue;
    }
    const parsed = patchLine(line, current.oldNo, current.newNo);
    if (parsed === null) continue;
    current.hunk.lines.push(parsed.line);
    current.oldNo = Math.min(current.oldNo + parsed.oldAdvance, current.oldEnd);
    current.newNo = Math.min(current.newNo + parsed.newAdvance, current.newEnd);
  }
  if (current !== null) hunks.push(current.hunk);
  return hunks;
}

/**
 * Converts the REST files payload to viewer-ready `FileDiff[]`. Files
 * without a patch (binary / over the size cap) become binary entries with
 * no hunks — the viewer renders its binary placeholder from the counters.
 */
export function prFilesToFileDiffs(files: ForgePrFile[]): FileDiff[] {
  return files.map((file) => ({
    path: file.path,
    old_path: null,
    binary: file.patch === null,
    is_image: false,
    additions: file.additions,
    deletions: file.deletions,
    hunks: file.patch === null ? [] : parsePrPatch(file.patch),
  }));
}
