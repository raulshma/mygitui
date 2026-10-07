/**
 * Byte-safe utilities for word-level diff highlights.
 *
 * `DiffLine.highlights` carries `[start, end)` pairs of BYTE offsets into the
 * line's UTF-8 text (produced by imara-diff on the Rust side, see
 * `src-tauri/src/engine/libgit2.rs`). JavaScript strings are UTF-16, so a
 * naive `text.slice(start, end)` is wrong for any line containing non-ASCII.
 *
 * Policy for offsets that land INSIDE a multi-byte character (a word token
 * boundary can split one when e.g. an accent-insensitive tokenizer runs on
 * composed text): clamp OUTWARD — the highlight includes every character the
 * byte range touches. Changed words stay fully visible; no invalid surrogates
 * are ever produced.
 */

/** One renderable piece of a line: plain or highlighted. */
export interface TextSegment {
  text: string;
  /** true when the segment is covered by a word-highlight byte range. */
  hl: boolean;
}

/**
 * Parallel arrays mapping char indices to byte offsets. `charIndices[j]` is a
 * real char boundary (never mid-surrogate-pair) and `byteOffsets[j]` the UTF-8
 * offset of the text before it. Always sorted, first entry 0.
 */
interface BoundaryMap {
  charIndices: Uint32Array;
  byteOffsets: Uint32Array;
}

/** UTF-8 byte width of the code point starting with code unit `cu` (surrogate aware). */
function byteWidth(first: number, second: number): number {
  if (first >= 0xd800 && first <= 0xdbff && second >= 0xdc00 && second <= 0xdfff) {
    return 4; // surrogate pair = U+10000..U+10FFFF
  }
  return first < 0x80 ? 1 : first < 0x800 ? 2 : 3;
}

function buildBoundaryMap(text: string): BoundaryMap {
  const n = text.length;
  // Upper bound: one boundary per code unit + sentinel; trimmed after fill.
  const chars = new Uint32Array(n + 1);
  const bytes = new Uint32Array(n + 1);
  let bytePos = 0;
  let j = 0;
  for (let i = 0; i < n; ) {
    chars[j] = i;
    bytes[j] = bytePos;
    j++;
    const cu = text.charCodeAt(i);
    const pair = cu >= 0xd800 && cu <= 0xdbff ? text.charCodeAt(i + 1) : 0;
    const w = byteWidth(cu, pair);
    bytePos += w;
    i += w === 4 ? 2 : 1;
  }
  chars[j] = n;
  bytes[j] = bytePos;
  return { charIndices: chars.subarray(0, j + 1), byteOffsets: bytes.subarray(0, j + 1) };
}

/** Total UTF-8 byte length of `text` (char-boundary walk, no allocation). */
export function utf8Length(text: string): number {
  let total = 0;
  for (let i = 0; i < text.length; ) {
    const cu = text.charCodeAt(i);
    const pair = cu >= 0xd800 && cu <= 0xdbff ? text.charCodeAt(i + 1) : 0;
    const w = byteWidth(cu, pair);
    total += w;
    i += w === 4 ? 2 : 1;
  }
  return total;
}

/** Largest real char boundary whose byte offset is <= `byteOffset` (clamped to [0, n]). */
function charIndexRoundDown(map: BoundaryMap, byteOffset: number): number {
  const bytes = map.byteOffsets;
  let lo = 0;
  let hi = bytes.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (bytes[mid] <= byteOffset) lo = mid;
    else hi = mid - 1;
  }
  return map.charIndices[lo];
}

/** Smallest real char boundary whose byte offset is >= `byteOffset` (clamped to [0, n]). */
function charIndexRoundUp(map: BoundaryMap, byteOffset: number): number {
  const bytes = map.byteOffsets;
  let lo = 0;
  let hi = bytes.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (bytes[mid] >= byteOffset) hi = mid;
    else lo = mid + 1;
  }
  // byteOffset beyond EOF: clamp to n (bytes[hi] < byteOffset can only happen there).
  return map.charIndices[hi];
}

/**
 * Byte-offset → char-index conversion. `round` picks the boundary when the
 * offset lands inside a multi-byte character: "down" for range starts,
 * "up" for range ends (clamp outward).
 */
export function byteToCharIndex(text: string, byteOffset: number, round: "down" | "up"): number {
  const map = buildBoundaryMap(text);
  return round === "down" ? charIndexRoundDown(map, byteOffset) : charIndexRoundUp(map, byteOffset);
}

/**
 * Byte-safe substring: includes every character the byte range touches
 * (outward clamping on both ends), clamped to the string.
 */
export function sliceByBytes(text: string, startByte: number, endByte: number): string {
  if (text.length === 0) return "";
  const map = buildBoundaryMap(text);
  const total = map.byteOffsets[map.byteOffsets.length - 1];
  const s = Math.max(0, Math.min(startByte, endByte));
  const e = Math.max(startByte, endByte);
  if (e <= 0 || s >= total) return "";
  const cs = charIndexRoundDown(map, s);
  const ce = charIndexRoundUp(map, Math.min(e, total));
  return ce > cs ? text.slice(cs, ce) : "";
}

/** Sort + merge overlapping/adjacent ranges, clamped to [0, totalBytes]. */
function normalizeRanges(
  highlights: readonly [number, number][],
  totalBytes: number,
): [number, number][] {
  const clamped: [number, number][] = [];
  for (const [rawStart, rawEnd] of highlights) {
    const start = Math.max(0, Math.min(rawStart, rawEnd));
    const end = Math.min(totalBytes, Math.max(rawStart, rawEnd));
    if (start < end) clamped.push([start, end]);
  }
  clamped.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: [number, number][] = [];
  for (const range of clamped) {
    const last = merged[merged.length - 1];
    if (last && range[0] <= last[1]) {
      if (range[1] > last[1]) last[1] = range[1];
    } else {
      merged.push([range[0], range[1]]);
    }
  }
  return merged;
}

/**
 * Split `text` into segments, marking the parts covered by byte-range
 * highlights. Always returns a faithful concatenation of the input (segments
 * join back to `text` exactly); degenerate ranges are dropped.
 */
export function applyHighlights(
  text: string,
  highlights: readonly [number, number][] | null | undefined,
): TextSegment[] {
  if (!highlights || highlights.length === 0) {
    return text.length ? [{ text, hl: false }] : [];
  }
  const map = buildBoundaryMap(text);
  const totalBytes = map.byteOffsets[map.byteOffsets.length - 1];
  const ranges = normalizeRanges(highlights, totalBytes);
  if (ranges.length === 0) {
    return text.length ? [{ text, hl: false }] : [];
  }

  const segments: TextSegment[] = [];
  let cursor = 0;
  for (const [startByte, endByte] of ranges) {
    const start = charIndexRoundDown(map, startByte);
    const end = charIndexRoundUp(map, endByte);
    if (end <= start || start < cursor) continue; // fully inside previous segment
    if (start > cursor) segments.push({ text: text.slice(cursor, start), hl: false });
    segments.push({ text: text.slice(start, end), hl: true });
    cursor = end;
  }
  if (cursor < text.length) segments.push({ text: text.slice(cursor), hl: false });
  return segments;
}
