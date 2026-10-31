/**
 * Safety net for the vendored style sheets (pruned to applied components by
 * scripts/prune-style-sheets.mjs): every cn-* marker the app applies must
 * keep its rules in every style sheet, so a component looks right under
 * every style. Known upstream gaps are listed — when a re-vendor fills one,
 * the absence assertion fails and the entry gets dropped.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const STYLES_DIR = dirname(fileURLToPath(import.meta.url));
const SRC_DIR = join(STYLES_DIR, "../../../../src");

/** Sheets keyed like the gap table below ("nova", "vega", …). */
const SHEETS: Record<string, Set<string>> = {};
for (const file of readdirSync(STYLES_DIR)) {
	const match = /^style-(.+)\.css$/.exec(file);
	if (match) {
		SHEETS[match[1]] = new Set(
			readFileSync(join(STYLES_DIR, file), "utf8").match(/cn-[a-z0-9-]+/g) ?? [],
		);
	}
}

/** Every cn-* marker applied anywhere in app sources (.svelte/.ts). */
function collectUsedTokens(): Set<string> {
	const used = new Set<string>();
	const walk = (dir: string): void => {
		for (const entry of readdirSync(dir)) {
			const full = join(dir, entry);
			if (statSync(full).isDirectory()) walk(full);
			else if (/\.(svelte|ts)$/.test(entry)) {
				for (const match of readFileSync(full, "utf8").matchAll(/cn-[a-z0-9-]+/g)) {
					used.add(match[0]);
				}
			}
		}
	};
	walk(SRC_DIR);
	return used;
}

/** Markers some sheets legitimately lack, upstream (sheet names). */
const UPSTREAM_GAPS: Record<string, readonly string[]> = {
	// Upstream ships the footer treatment in nova only; the other vendored
	// sheets never had it (those components keep local fallback classes).
	"cn-alert-dialog-footer": ["luma", "lyra", "maia", "mira", "rhea", "sera", "vega"],
	// lyra's upstream sheet carries no dialog-footer rules.
	"cn-dialog-footer": ["lyra"],
};

describe("style sheet coverage", () => {
	const used = collectUsedTokens();

	it("defines every applied marker in every sheet, except listed gaps", () => {
		for (const token of used) {
			// Markers no sheet ever styled (cn-menu-target and friends) are JS
			// hooks, not style utilities — nothing to cover.
			if (![...Object.values(SHEETS)].some((tokens) => tokens.has(token))) continue;
			const gaps = UPSTREAM_GAPS[token] ?? [];
			for (const [sheet, tokens] of Object.entries(SHEETS)) {
				if (gaps.includes(sheet)) continue;
				expect(tokens.has(token), `${sheet} is missing .${token}`).toBe(true);
			}
		}
	});

	it("keeps the gap table honest (a filled gap must drop its entry)", () => {
		for (const [token, sheets] of Object.entries(UPSTREAM_GAPS)) {
			for (const sheet of sheets) {
				expect(SHEETS[sheet]?.has(token), `${sheet} now defines .${token}; drop the gap entry`).toBe(
					false,
				);
			}
		}
	});
});
