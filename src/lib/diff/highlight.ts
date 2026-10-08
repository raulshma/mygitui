/**
 * Syntax highlighting for the diff viewer (M11) — async Shiki, render-safe.
 *
 * Model: the virtualized DiffRow render path must never await. So:
 *  - `cachedTokens(text, path)` returns token spans SYNCHRONOUSLY when the
 *    (lang, line-text) pair is already in the memo cache — rows that were
 *    highlighted once render instantly on re-scroll;
 *  - `requestTokens(path, texts)` kicks a background parse (idle-ish: one
 *    batch per animation frame window) and bumps `version` when tokens land,
 *    which subscribers (DiffViewer) use to re-render the visible window;
 *  - unknown languages / oversize lines never enter the cache (plain text).
 *
 * The highlighter is lazily created on first use with the `github-light` +
 * `github-dark` themes; languages load on demand and are memoized. Failures
 * (no WASM, unsupported grammar) degrade to plain text permanently — never
 * throw into the render path.
 *
 * Color scheme: `setScheme("light" | "dark")` selects the theme; tokens are
 * re-requested by the owner on scheme change (cache is theme-keyed only via
 * a full reset — diffs re-request what is visible).
 */

let version = 0;

/** Bumped whenever new tokens land (subscribers re-render). */
export function syntaxVersion(): number {
  return version;
}

export function setScheme(scheme: "light" | "dark"): void {
  currentScheme = scheme;
  cache.clear();
  version++;
}

let currentScheme: "light" | "dark" = "dark";

export type TokenSpan = { text: string; color: string | null };

/** Extension → Shiki language id (small, high-signal map). */
const LANG_BY_EXT: Record<string, string> = {
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  mjs: "javascript",
  cjs: "javascript",
  rs: "rust",
  py: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  cs: "csharp",
  rb: "ruby",
  php: "php",
  swift: "swift",
  md: "markdown",
  json: "json",
  yml: "yaml",
  yaml: "yaml",
  toml: "toml",
  html: "html",
  css: "css",
  scss: "scss",
  sh: "shellscript",
  bash: "shellscript",
  sql: "sql",
  xml: "xml",
};

/** Lines longer than this are not highlighted (grammar cost guard). */
const MAX_LINE_CHARS = 800;
/** Distinct cache entries cap (LRU-ish by insertion; oldest dropped). */
const CACHE_CAP = 20_000;

export function languageFor(path: string): string | null {
  const base = path.split(/[\\/]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return null;
  const ext = base.slice(dot + 1).toLowerCase();
  return LANG_BY_EXT[ext] ?? null;
}

// -- highlighter lifecycle ----------------------------------------------------

type ShikiHighlighter = Awaited<
  ReturnType<(typeof import("shiki"))["createHighlighter"]>
>;

let highlighterPromise: Promise<ShikiHighlighter | null> | null = null;
const loadedLangs = new Set<string>();

async function getHighlighter(lang: string): Promise<ShikiHighlighter | null> {
  highlighterPromise ??= (async () => {
    try {
      const shiki = await import("shiki");
      return await shiki.createHighlighter({
        themes: ["github-light", "github-dark"],
        langs: [],
      });
    } catch {
      return null; // permanent plain-text degrade
    }
  })();
  const highlighter = await highlighterPromise;
  if (highlighter === null) return null;
  if (!loadedLangs.has(lang)) {
    try {
      await highlighter.loadLanguage(lang as Parameters<typeof highlighter.loadLanguage>[0]);
      loadedLangs.add(lang);
    } catch {
      return null; // this language never highlights
    }
  }
  return highlighter;
}

// -- memo cache ---------------------------------------------------------------

/** Key: `${scheme}\u0000${lang}\u0000${text}`. */
const cache = new Map<string, TokenSpan[]>();

function cacheKey(lang: string, text: string): string {
  return `${currentScheme}\u0000${lang}\u0000${text}`;
}

/** Synchronous render-path lookup: tokens only when already parsed. */
export function cachedTokens(text: string, path: string): TokenSpan[] | null {
  const lang = languageFor(path);
  if (lang === null || text.length > MAX_LINE_CHARS) return null;
  return cache.get(cacheKey(lang, text)) ?? null;
}

/** Schedules a background parse for the visible lines (best effort). */
export function requestTokens(path: string, texts: string[]): void {
  const lang = languageFor(path);
  if (lang === null) return;
  const pending = texts.filter(
    (text) =>
      text.length > 0 &&
      text.length <= MAX_LINE_CHARS &&
      !cache.has(cacheKey(lang, text)),
  );
  if (pending.length === 0) return;
  void (async () => {
    const highlighter = await getHighlighter(lang);
    if (highlighter === null) return;
    const theme = currentScheme === "dark" ? "github-dark" : "github-light";
    let landed = false;
    for (const text of pending) {
      const key = cacheKey(lang, text);
      if (cache.has(key)) continue;
      try {
        // codeToTokens over one line: newline-free input yields one token line.
        const result = highlighter.codeToTokens(text, {
          lang: lang as Parameters<typeof highlighter.codeToTokens>[1] extends infer O ? O extends { lang: infer L } ? L : never : never,
          theme,
        } as never);
        const line = result.tokens[0] ?? [];
        const spans: TokenSpan[] = line.map((token) => ({
          text: token.content,
          color: token.color ?? null,
        }));
        if (cache.size >= CACHE_CAP) {
          // Drop the oldest ~10% (insertion order); keep it simple.
          for (const oldKey of Array.from(cache.keys()).slice(0, CACHE_CAP / 10)) {
            cache.delete(oldKey);
          }
        }
        cache.set(key, spans);
        landed = true;
      } catch {
        cache.set(key, [{ text, color: null }]); // never retry failures
      }
    }
    if (landed) {
      version++;
      for (const listener of listeners) listener();
    }
  })();
}

type Listener = () => void;
const listeners = new Set<Listener>();

/** Subscribes to "new tokens landed" (returns the unlisten). */
export function onTokensLanded(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
