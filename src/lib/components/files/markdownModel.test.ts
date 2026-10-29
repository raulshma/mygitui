import { describe, expect, it } from "vitest";
import {
  highlightFence,
  isMarkdownPath,
  lineForOffset,
  offsetForLine,
  renderMarkdown,
  renderMarkdownHtml,
  type LineAnchor,
} from "./markdownModel";

describe("renderMarkdown", () => {
  it("renders GFM structure: headings, lists, tables, links", () => {
    const { html } = renderMarkdown(
      [
        "# Title",
        "",
        "- a",
        "- b",
        "",
        "| x | y |",
        "|---|---|",
        "| 1 | 2 |",
        "",
        "See [docs](https://example.com).",
      ].join("\n"),
    );
    expect(html).toContain("<h1");
    expect(html).toContain("<ul");
    expect(html).toContain("<table");
    expect(html).toContain('href="https://example.com"');
  });

  it("stamps top-level blocks with source lines", () => {
    const { html } = renderMarkdown("# H\n\npara one\n\npara two\n");
    const h1 = html.match(/<h1[^>]*data-source-line="(\d+)"/);
    expect(h1?.[1]).toBe("0");
    const paras = [...html.matchAll(/<p[^>]*data-source-line="(\d+)"/g)].map(
      (m) => Number(m[1]),
    );
    expect(paras).toEqual([2, 4]);
  });

  it("does not stamp nested blocks (anchors stay top-level)", () => {
    const { html } = renderMarkdown("- item\n  - nested\n");
    // Outer list carries the anchor; the nested list does not.
    expect(html.match(/data-source-line/g)?.length).toBe(1);
  });

  it("enumerates fences with stable ids and language tags", () => {
    const { fences, html } = renderMarkdown(
      "```ts\nconst a = 1;\n```\n\ntext\n\n```python\nx = 2\n```\n",
    );
    expect(fences.map((f) => [f.id, f.lang])).toEqual([
      [0, "ts"],
      [1, "python"],
    ]);
    expect(html).toContain('data-fence-id="0"');
    expect(html).toContain('data-fence-id="1"');
  });

  it("sanitizes scripts, handlers and javascript: URLs", () => {
    const { html } = renderMarkdown(
      [
        "<script>alert(1)</script>",
        "",
        '<img src="x" onerror="alert(1)">',
        "",
        "[click](javascript:alert(1))",
      ].join("\n"),
    );
    expect(html).not.toContain("<script");
    expect(html).not.toContain("onerror");
    // markdown-it v15 drops the link token entirely (literal text), so the
    // danger sign is a live href, not the substring in text content.
    expect(html).not.toContain('href="javascript:');
    expect(html).not.toContain("<a href");
  });

  it("keeps class and data-* attributes through sanitization", () => {
    const { html } = renderMarkdown('```ts\nlet a = 1;\n```\n');
    expect(html).toMatch(/<code[^>]*data-fence-id="0"/);
    expect(html).toMatch(/data-source-line="0"/);
  });
});

describe("highlightFence", () => {
  it("returns tok-* spans for a known language", async () => {
    const out = await highlightFence("ts", 'const a = "x";\n');
    expect(out).not.toBeNull();
    expect(out).toContain("tok-");
    expect(out).toContain("&quot;x&quot;"); // quotes escaped inside spans
  });

  it("resolves null for unknown languages (permanent plain text)", async () => {
    expect(await highlightFence("not-a-lang", "x")).toBeNull();
  });

  it("keeps raw text when no tokens match (plain word)", async () => {
    const out = await highlightFence("md", "plain\n");
    expect(out).toContain("plain");
  });
});

describe("scroll mapping", () => {
  const anchors: LineAnchor[] = [
    { line: 0, top: 0, height: 40 },
    { line: 10, top: 200, height: 60 },
    { line: 20, top: 600, height: 40 },
  ];

  it("interpolates between anchors", () => {
    expect(offsetForLine(5, anchors, 20)).toBe(100);
    expect(offsetForLine(15, anchors, 20)).toBe(400);
  });

  it("extrapolates past the ends at the fallback line height", () => {
    expect(offsetForLine(-2, anchors, 20)).toBe(0); // clamped at 0
    expect(offsetForLine(25, anchors, 20)).toBe(600 + 5 * 20);
    expect(offsetForLine(0, anchors, 20)).toBe(0);
  });

  it("maps offsets back to lines", () => {
    expect(lineForOffset(100, anchors, 20)).toBeCloseTo(5, 5);
    expect(lineForOffset(400, anchors, 20)).toBeCloseTo(15, 5);
    expect(lineForOffset(0, anchors, 20)).toBe(0);
    // Past the last anchor's bottom: extrapolate at the fallback height.
    expect(lineForOffset(10_000, anchors, 20)).toBeCloseTo(20 + 468, 5);
  });

  it("handles a single anchor without division hazards", () => {
    const single: LineAnchor[] = [{ line: 3, top: 50, height: 20 }];
    expect(offsetForLine(9, single, 10)).toBe(50 + 6 * 10);
    expect(lineForOffset(50, single, 10)).toBe(3);
    expect(lineForOffset(500, single, 10)).toBe(3 + 43);
  });

  it("empty anchors are inert", () => {
    expect(offsetForLine(4, [], 10)).toBe(0);
    expect(lineForOffset(4, [], 10)).toBe(0);
  });
});

describe("isMarkdownPath", () => {
  it("matches markdown extensions case-insensitively", () => {
    expect(isMarkdownPath("README.md")).toBe(true);
    expect(isMarkdownPath("docs/Notes.MARKDOWN")).toBe(true);
    expect(isMarkdownPath("a/b/notes.mdown")).toBe(true);
    expect(isMarkdownPath("notes.mkd")).toBe(true);
  });

  it("rejects other files and edge cases", () => {
    expect(isMarkdownPath("src/app.ts")).toBe(false);
    expect(isMarkdownPath("md")).toBe(false);
    expect(isMarkdownPath("a.mdown.txt")).toBe(false);
    expect(isMarkdownPath("")).toBe(false);
  });
});

describe("perf gate", () => {
  it("parses + renders a 500KB document under the budget", () => {
    // ~500KB: many short paragraphs (worst case: block-per-line anchoring).
    // Timing covers parse/render/anchoring only — sanitize is linear but
    // jsdom's DOMPurify is ~100x slower than the webview's native parser,
    // so including it would measure jsdom, not our pipeline.
    const chunk = "Paragraph line with **bold** and `code` and a [link](https://x.dev).\n\n";
    let doc = "";
    while (doc.length < 500_000) doc += chunk;
    const start = performance.now();
    const { html, fences } = renderMarkdownHtml(doc);
    const elapsed = performance.now() - start;
    expect(html.length).toBeGreaterThan(0);
    expect(fences).toHaveLength(0);
    // Loaded-runner budget (see fileTreeModel.test.ts rationale): solo this
    // runs ~170ms; under full-suite contention ~4x. Any super-linear
    // regression on 500KB would take tens of seconds — still caught.
    expect(elapsed).toBeLessThan(1000);
  });
});
