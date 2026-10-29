/**
 * Markdown preview model — parsing, sanitizing, fence enumeration and the
 * scroll-sync interpolation math, all component-free so it unit-tests
 * directly (and so the whole module only ever loads inside the lazy
 * MarkdownPreview chunk: markdown-it + DOMPurify never touch the editor
 * bundle or the entry graph).
 *
 * Pipeline (`renderMarkdown`): parse → stamp top-level blocks with
 * `data-source-line` (markdown-it tokens carry `.map` source ranges — this
 * is what makes editor↔preview scroll sync exact) → sanitize. Repo files
 * are untrusted input in a webview that can drive Tauri commands, so the
 * HTML always passes DOMPurify (script/`javascript:`/handlers stripped,
 * `class`/`data-*` kept for anchors + token classes).
 *
 * Fence highlighting (`highlightFence`) deliberately reuses the CodeMirror
 * grammar set (@codemirror/language-data) that the open file's editor
 * already loaded: zero extra highlighter bytes, and `tok-*` classes theme
 * via `--m3-*` CSS variables so the preview follows the dynamic color
 * scheme (shiki's fixed github themes — used by diffs — do not).
 */

import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";

/** One fenced code block discovered during render (for async highlighting). */
export interface MdFence {
  /** Stable id mirrored in the rendered `data-fence-id` attribute. */
  id: number;
  /** Info-string language tag ("" = plain). */
  lang: string;
  code: string;
}

export interface RenderedMarkdown {
  /** Sanitized HTML with `data-source-line` / `data-fence-id` attributes. */
  html: string;
  /** Fences awaiting async highlighting (plain text until replaced). */
  fences: MdFence[];
}

// -- parser -------------------------------------------------------------------

function createMarkdownParser() {
  return new MarkdownIt({
    html: true, // raw HTML is authors' intent — DOMPurify makes it safe
    linkify: true,
    breaks: false,
  });
}

const md = createMarkdownParser();

/** Longest fence we grammar-parse (beyond that: plain text, still escaped). */
const MAX_FENCE_CHARS = 100_000;

/** Escapes a raw-text slice for HTML (fence bodies and unstyled gaps). */
function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/**
 * Renders one document to unsanitized HTML (with anchors + fences). Split
 * from `renderMarkdown` so the perf gate can time the parse/render cost
 * alone — DOMPurify is linear but jsdom's implementation is ~100x slower
 * than the webview's native parser, so timing it there measures jsdom.
 */
export function renderMarkdownHtml(source: string): {
  html: string;
  fences: MdFence[];
} {
  const env = {};
  const tokens = md.parse(source, env);
  const fences: MdFence[] = [];

  for (const token of tokens) {
    if (token.level !== 0 || !token.map) continue;
    token.attrSet("data-source-line", String(token.map[0]));
    if (token.type === "fence") {
      const id = fences.length;
      token.attrSet("data-fence-id", String(id));
      fences.push({
        id,
        lang: token.info.trim().split(/\s+/)[0] ?? "",
        code: token.content,
      });
    }
  }

  return { html: md.renderer.render(tokens, md.options, env), fences };
}

/**
 * Renders + sanitizes one document. Top-level blocks (paragraph, heading,
 * list, blockquote, table, hr, fence — anything with a source map at
 * nesting level 0) get `data-source-line` so the component can measure
 * anchor offsets; nested blocks stay unstamped (anchors only need to be
 * monotonic, and fewer attributes keep big docs lighter).
 */
export function renderMarkdown(source: string): RenderedMarkdown {
  const { html: raw, fences } = renderMarkdownHtml(source);
  return { html: DOMPurify.sanitize(raw), fences };
}

// -- async fence highlighting (Lezer, reusing the editor's grammars) -----------

type LezerModules = typeof import("@lezer/highlight");
let lezer: LezerModules | null = null;
type LanguageDataModules = typeof import("@codemirror/language-data");
let languageData: LanguageDataModules | null = null;
type LanguageModules = typeof import("@codemirror/language");
let languageMod: LanguageModules | null = null;

/**
 * Resolved grammar per lang tag ("" / unknown → null, never retried —
 * permanent plain-text degrade, mirroring diff/highlight.ts).
 */
const grammarCache = new Map<
  string,
  Promise<import("@codemirror/language").LanguageSupport | null>
>();

/**
 * Highlights one fence to `tok-*` class spans (async: grammar loads on
 * demand). Unknown/failed languages resolve to null → caller keeps the
 * plain escaped text.
 */
export function highlightFence(lang: string, code: string): Promise<string | null> {
  if (code.length > MAX_FENCE_CHARS) return Promise.resolve(null);
  const key = lang.toLowerCase();
  let pending = grammarCache.get(key);
  if (pending === undefined) {
    pending = (async () => {
      try {
        [languageMod, lezer, languageData] = await Promise.all([
          import("@codemirror/language"),
          import("@lezer/highlight"),
          import("@codemirror/language-data"),
        ]);
        return await resolveGrammar(key);
      } catch {
        return null;
      }
    })();
    grammarCache.set(key, pending);
  }
  return pending.then((support) => (support === null ? null : highlightWith(support, code)));
}

/** Finds + loads the Lezer grammar for an info-string lang tag (or null). */
async function resolveGrammar(
  lang: string,
): Promise<import("@codemirror/language").LanguageSupport | null> {
  if (languageMod === null || languageData === null) return null;
  const want = lang.toLowerCase();
  const desc =
    languageData.languages.find(
      (d) =>
        d.name.toLowerCase() === want ||
        d.alias.some((a) => a.toLowerCase() === want) ||
        d.extensions.some((e) => e.toLowerCase() === want),
    ) ??
    languageMod.LanguageDescription.matchFilename(languageData.languages, `x.${want}`);
  if (!desc) return null;
  try {
    const support = await desc.load();
    return support.language ? support : null;
  } catch {
    return null; // grammar failed to load — plain text, permanently
  }
}

/** Parses + tree-highlights one fence body with the resolved grammar. */
function highlightWith(
  support: import("@codemirror/language").LanguageSupport,
  code: string,
): string | null {
  if (lezer === null || support.language === undefined) return null;
  try {
    // LRLanguage parses via its `.parser` (Language has no parse method).
    const tree = support.language.parser.parse(code);
    const out: string[] = [];
    let lastTo = 0;
    lezer.highlightTree(tree, [lezer.classHighlighter], (from, to, classes) => {
      if (from > lastTo) out.push(escapeHtml(code.slice(lastTo, from)));
      out.push(`<span class="${classes}">${escapeHtml(code.slice(from, to))}</span>`);
      lastTo = to;
    });
    if (lastTo < code.length) out.push(escapeHtml(code.slice(lastTo)));
    return out.join("");
  } catch {
    return null;
  }
}

// -- scroll-sync interpolation ---------------------------------------------------

/** One measured anchor: a `data-source-line` block's source line + box. */
export interface LineAnchor {
  line: number;
  /** offsetTop within the preview scroll content (px). */
  top: number;
  /** Element height (px) — drives extrapolation past the last anchor. */
  height: number;
}

/**
 * Preview scrollTop that puts source `line` at the viewport top:
 * linear between the bracketing anchors, clamped inside, extrapolated
 * past the ends at the last anchor's own pace.
 */
export function offsetForLine(
  line: number,
  anchors: LineAnchor[],
  fallbackLineHeight: number,
): number {
  if (anchors.length === 0) return 0;
  const first = anchors[0];
  if (line <= first.line) return Math.max(0, first.top - (first.line - line) * fallbackLineHeight);
  const last = anchors[anchors.length - 1];
  if (line >= last.line) return last.top + (line - last.line) * fallbackLineHeight;
  let lo = 0;
  let hi = anchors.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (anchors[mid].line <= line) lo = mid;
    else hi = mid;
  }
  const a = anchors[lo];
  const b = anchors[hi];
  const t = (line - a.line) / Math.max(1, b.line - a.line);
  return a.top + t * (b.top - a.top);
}

/**
 * Inverse of `offsetForLine`: source line (float — callers floor it) whose
 * block sits at preview `offset` px. Anchors must be sorted by top (they
 * are, in document order).
 */
export function lineForOffset(
  offset: number,
  anchors: LineAnchor[],
  fallbackLineHeight: number,
): number {
  if (anchors.length === 0) return 0;
  const first = anchors[0];
  if (offset <= first.top) return first.line;
  const last = anchors[anchors.length - 1];
  if (offset >= last.top + last.height) {
    return last.line + (offset - last.top - last.height) / Math.max(1, fallbackLineHeight);
  }
  let lo = 0;
  let hi = anchors.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (anchors[mid].top <= offset) lo = mid;
    else hi = mid;
  }
  const a = anchors[lo];
  const b = anchors[hi];
  const span = Math.max(1, b.top - a.top);
  const t = Math.min(1, Math.max(0, (offset - a.top) / span));
  return a.line + t * (b.line - a.line);
}

// -- helpers ---------------------------------------------------------------------

export { isMarkdownPath } from "./markdownPath";
