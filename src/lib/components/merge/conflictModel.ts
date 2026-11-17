/**
 * Pure conflict-marker model for the M3 conflict editor (E1 lane).
 *
 * The backend materializes a conflicted workdir file as a 2-way marker
 * preview (`engine::merge::render_conflict_markers`, byte-exact):
 *
 * ```text
 * <<<<<<< <path>
 * <stage-2 "ours" bytes, newline-terminated>
 * =======
 * <stage-3 "theirs" bytes, newline-terminated>
 * >>>>>>> <path>
 * ```
 *
 * A path straight out of `merge_branch` / cherry-pick / revert can also
 * carry libgit2's own markers (`<<<<<<< HEAD` … `>>>>>>> <label>`), so the
 * parser accepts ANY `<<<<<<<`/`>>>>>>>` label and merely records whether
 * the labels match the D1 path format. Everything here is string-based
 * (multibyte-safe by construction); UTF-8 byte conversion happens only at
 * the IPC boundary, where resolved writes preserve the original file's byte
 * hygiene — the BOM ({@link hasUtf8Bom} / {@link encodeUtf8}) and its
 * dominant line-ending style ({@link detectEol} / {@link withEol}), combined
 * in {@link encodeResolved}. Files that are not valid UTF-8 are never
 * re-encoded ({@link decodeUtf8Strict} gates that in the editor).
 *
 * Malformed input degrades, never throws: an opening marker with no
 * separator, a separator with no closing marker, or loose `=======` /
 * `>>>>>>>` lines all pass through as literal text, so an unresolved
 * round-trip is byte-identical.
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Per-conflict choice. `custom` = user-edited text wins over the presets. */
export type ResolutionChoice = "ours" | "theirs" | "custom";

/** Non-conflict run of lines (the `text` includes interior `\n`, not a trailing one). */
export interface LineSegment {
  type: "line";
  text: string;
}

/** One `<<<<<<< … ======= … >>>>>>>` block. */
export interface ConflictSegment {
  type: "conflict";
  /** Ours section (interior lines joined with `\n`, no trailing newline). */
  ours: string;
  /** Theirs section, same shape as {@link ours}. */
  theirs: string;
  /** Label after `<<<<<<<` (the path for D1 previews, `HEAD` for git merges). */
  oursLabel: string;
  /** Label after `>>>>>>>`. */
  theirsLabel: string;
  /** True when both labels equal the file path (the D1 `Both` preview format). */
  pathLabeled: boolean;
  /** Exact original block text (with markers) — unresolved round-trips verbatim. */
  raw: string;
}

export type Segment = LineSegment | ConflictSegment;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

const bare = (line: string): string => (line.endsWith("\r") ? line.slice(0, -1) : line);

const isStartMarker = (line: string): boolean => {
  const b = bare(line);
  return b === "<<<<<<<" || b.startsWith("<<<<<<< ");
};

const isSeparator = (line: string): boolean => bare(line) === "=======";

const isEndMarker = (line: string): boolean => {
  const b = bare(line);
  return b === ">>>>>>>" || b.startsWith(">>>>>>> ");
};

/** The label text after a `<<<<<<<` / `>>>>>>>` marker (markers use ` <label>`). */
function labelOf(line: string): string {
  return bare(line).replace(/^[<>]{7} ?/, "").trim();
}

/**
 * Parses workdir text into line and conflict segments. `path` is the
 * workdir-relative file path, used only to flag the D1 path-labeled format
 * ({@link ConflictSegment.pathLabeled}); parsing itself accepts any labels.
 */
export function parseConflicts(text: string, path: string): Segment[] {
  const lines = text.split("\n");
  const segments: Segment[] = [];
  let literal: string[] = [];
  let i = 0;

  const flushLiteral = (): void => {
    if (literal.length > 0) {
      segments.push({ type: "line", text: literal.join("\n") });
      literal = [];
    }
  };

  while (i < lines.length) {
    const line = lines[i]!;
    if (!isStartMarker(line)) {
      literal.push(line);
      i += 1;
      continue;
    }
    // Opening marker: find its separator, then its closing marker. Either
    // missing → the marker line is literal and scanning continues after it.
    let sep = i + 1;
    while (sep < lines.length && !isSeparator(lines[sep]!)) sep += 1;
    if (sep >= lines.length) {
      literal.push(line);
      i += 1;
      continue;
    }
    let end = sep + 1;
    while (end < lines.length && !isEndMarker(lines[end]!)) end += 1;
    if (end >= lines.length) {
      literal.push(line);
      i += 1;
      continue;
    }
    flushLiteral();
    const ours = lines.slice(i + 1, sep).join("\n");
    const theirs = lines.slice(sep + 1, end).join("\n");
    const oursLabel = labelOf(line);
    const theirsLabel = labelOf(lines[end]!);
    segments.push({
      type: "conflict",
      ours,
      theirs,
      oursLabel,
      theirsLabel,
      pathLabeled: oursLabel === path && theirsLabel === path,
      raw: lines.slice(i, end + 1).join("\n"),
    });
    i = end + 1;
  }
  flushLiteral();
  return segments;
}

/** Number of conflict blocks (the editor's "n"). */
export function countConflicts(segments: Segment[]): number {
  return segments.reduce((n, s) => (s.type === "conflict" ? n + 1 : n), 0);
}

// ---------------------------------------------------------------------------
// Resolution + serialization
// ---------------------------------------------------------------------------

/**
 * Serializes segments back to file text. Conflict `index` counts conflicts
 * in order (0-based). An unresolved conflict keeps its exact original block
 * text. Precedence: an explicit `custom` choice (or a custom text with no
 * choice yet — custom edits win over button choices) uses
 * `customs.get(index)`, falling back to ours; `ours`/`theirs` take their
 * sections. Segments carry no boundary newlines — `join("\n")` restores the
 * original line structure, so a chosen section never gains or loses the
 * newline that separated it from what follows.
 */
export function applyResolutions(
  segments: Segment[],
  resolutions: Map<number, ResolutionChoice>,
  customs: Map<number, string>,
): string {
  const out: string[] = [];
  let index = -1;
  for (const segment of segments) {
    if (segment.type === "line") {
      out.push(segment.text);
      continue;
    }
    index += 1;
    let choice = resolutions.get(index);
    if (choice === undefined && customs.has(index)) choice = "custom";
    if (choice === undefined) {
      out.push(segment.raw);
      continue;
    }
    let chosen: string;
    if (choice === "theirs") {
      chosen = segment.theirs;
    } else if (choice === "custom") {
      chosen = customs.get(index) ?? segment.ours;
    } else {
      chosen = segment.ours;
    }
    out.push(chosen);
  }
  return out.join("\n");
}

/** Indices of conflicts that have no resolution choice yet, in order. */
export function unresolvedConflicts(
  segments: Segment[],
  resolutions: Map<number, ResolutionChoice>,
  customs: Map<number, string>,
): number[] {
  const out: number[] = [];
  let index = -1;
  for (const segment of segments) {
    if (segment.type !== "conflict") continue;
    index += 1;
    if (!resolutions.has(index) && !customs.has(index)) out.push(index);
  }
  return out;
}

/**
 * The content a conflict currently resolves to (or would resolve to with
 * `choice`, defaulting to the unchosen view): custom text when present,
 * else the chosen side, else ours. Drives the Edit-textarea prefill and
 * the per-block status line.
 */
export function effectiveContent(
  segment: ConflictSegment,
  resolutions: Map<number, ResolutionChoice>,
  customs: Map<number, string>,
  index: number,
): string {
  const custom = customs.get(index);
  if (custom !== undefined) return custom;
  const choice = resolutions.get(index);
  if (choice === "theirs") return segment.theirs;
  return segment.ours;
}

// ---------------------------------------------------------------------------
// UTF-8 boundary (multibyte-safe: strings everywhere else)
// ---------------------------------------------------------------------------

/**
 * Decodes IPC bytes (`number[]` from `repo_read_file`) as UTF-8. Lenient:
 * invalid sequences become U+FFFD, so the editor always has something to
 * show. The leading BOM is consumed (TextDecoder's default) — parsed text
 * never carries U+FEFF, and {@link encodeResolved} re-emits the original's
 * BOM bytes on write.
 */
export function decodeUtf8(bytes: number[] | Uint8Array): string {
  return new TextDecoder().decode(Uint8Array.from(bytes));
}

/**
 * Strict decode for the write-back gate: null when the bytes are not valid
 * UTF-8. Re-encoding such a file from decoded text can never round-trip —
 * every invalid byte would come back as U+FFFD's EF BF BD — so callers must
 * write the original bytes instead of editor text for these files.
 */
export function decodeUtf8Strict(bytes: number[] | Uint8Array): string | null {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bytes));
  } catch {
    return null;
  }
}

/** True when the bytes open with a UTF-8 BOM (EF BB BF). */
export function hasUtf8Bom(bytes: number[] | Uint8Array): boolean {
  return bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf;
}

/** Line-ending style a file is written back with. */
export type Eol = "lf" | "crlf";

/**
 * The file's dominant line-ending style: "crlf" when strictly more than half
 * of its `\n`s belong to a `\r\n` pair (ties and CRLF-less files count as
 * lf). Pragmatic whole-file rule for mixed endings — per-line `\r` inside
 * the parsed segments round-trips verbatim regardless, so {@link withEol}
 * only ever re-styles content written fresh (custom resolutions) plus stray
 * lone `\n`s in a predominantly-CRLF file.
 */
export function detectEol(text: string): Eol {
  const total = (text.match(/\n/g) ?? []).length;
  const crlf = (text.match(/\r\n/g) ?? []).length;
  return crlf * 2 > total ? "crlf" : "lf";
}

/**
 * Re-styles line endings to `eol`. "lf" is a no-op: pass-through text keeps
 * its per-line `\r` untouched and nothing is normalized away. "crlf" turns
 * every bare `\n` into `\r\n` (the normalize-then-convert dance leaves
 * existing `\r\n` and lone `\r` alone), so custom content typed into the
 * editor lands with the file's dominant style.
 */
export function withEol(text: string, eol: Eol): string {
  if (eol === "lf") return text;
  return text.replace(/\r\n/g, "\n").replace(/\n/g, "\r\n");
}

/** Encodes text to the UTF-8 bytes `conflict_resolve` expects. */
export function encodeUtf8(text: string, opts?: { bom?: boolean }): Uint8Array {
  const body = new TextEncoder().encode(text);
  if (opts?.bom !== true) return body;
  // TextEncoder never emits a BOM; re-emit the original's explicitly so a
  // BOM'd file stays BOM'd after a resolved write (decodeUtf8 stripped it).
  const out = new Uint8Array(body.length + 3);
  out[0] = 0xef;
  out[1] = 0xbb;
  out[2] = 0xbf;
  out.set(body, 3);
  return out;
}

/**
 * The bytes a resolved file is written with: the resolved text re-encoded,
 * with the original's BOM re-emitted ({@link decodeUtf8} consumed it) and
 * its dominant line-ending style restored ({@link withEol}). `originalBytes`
 * is the same load the editor decoded; the internal re-decode keeps this
 * function the single authority on what "original style" means.
 */
export function encodeResolved(
  resolvedText: string,
  originalBytes: number[] | Uint8Array,
): Uint8Array {
  return encodeUtf8(withEol(resolvedText, detectEol(decodeUtf8(originalBytes))), {
    bom: hasUtf8Bom(originalBytes),
  });
}
