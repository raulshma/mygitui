/**
 * Audit test: every literal `appIcon("name")` call site in `src` must
 * resolve to a real icon. `appIcon` returns `null` for unknown names, and
 * call sites hand the result to `<Icon>` with a `!` — a name that misses
 * both the lucide and material-symbols sets makes `Icon` throw
 * `Cannot read properties of null (reading 'body')` at render time, which
 * takes down the whole mounting component (e.g. the terminal panel once
 * crashed on `appIcon("sliders-horizontal")` — a lucide SLUG, not the
 * semantic name `tune`).
 *
 * The rule (see scripts/generate-icon-sets.mjs): names are the semantic
 * keys of `sets/mapping.json`, never per-library slugs. Generated set keys
 * are exactly those semantic names, so checking membership in the two
 * static sets (lucide + material-symbols — appIcon's guaranteed fallback
 * chain) reproduces the runtime resolution before any chunk loads.
 *
 * Known limitation: dynamic call sites (`appIcon(MAP[key])` with the map
 * defined elsewhere) are not covered — only string-literal arguments.
 */

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { LUCIDE_ICONS } from "./sets/generated/lucide";
import { MATERIAL_SYMBOLS_ICONS } from "./sets/generated/material-symbols";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");

/** Collects every appIcon("...") literal under `src`, as file:line pairs. */
function collectCallSites(): { file: string; line: number; name: string }[] {
  const callSites: { file: string; line: number; name: string }[] = [];
  const pattern = /\bappIcon\(\s*(['"])([^'"\n]+)\1/g;

  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry);
      if (statSync(path).isDirectory()) {
        // Generated icon modules hold the data, not call sites.
        if (entry === "generated") continue;
        walk(path);
        continue;
      }
      if (!/\.(svelte|ts)$/.test(entry) || /\.test\.ts$/.test(entry)) continue;
      const source = readFileSync(path, "utf8");
      for (const match of source.matchAll(pattern)) {
        const line = source.slice(0, match.index).split("\n").length;
        callSites.push({
          file: path.slice(repoRoot.length + 1),
          line,
          name: match[2],
        });
      }
    }
  };
  walk(join(repoRoot, "src"));
  return callSites;
}

describe("appIcon call sites", () => {
  it("every literal name resolves in lucide or material-symbols", () => {
    const resolvable = new Set([
      ...Object.keys(LUCIDE_ICONS),
      ...Object.keys(MATERIAL_SYMBOLS_ICONS),
    ]);
    const broken = collectCallSites().filter(({ name }) => !resolvable.has(name));

    expect(
      broken.map(({ file, line, name }) => `${file}:${line} appIcon("${name}")`),
      `appIcon() returns null for unknown names; <Icon icon={appIcon("…")!}> then throws ` +
        `at render. Use the semantic names of sets/mapping.json (keys), not ` +
        `per-library slugs (e.g. "tune", not the lucide slug "sliders-horizontal").`,
    ).toEqual([]);
  });
});
