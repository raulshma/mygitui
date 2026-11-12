/**
 * Data-variant guard for the vendored style sheets (reads the files like
 * sheets-coverage.test.ts): bits-ui components express state through
 * `data-state="open|closed|checked|unchecked"` and
 * `data-orientation="horizontal|vertical"` — never through bare presence
 * attributes. Tailwind's bare `data-open:` variant compiles to an
 * attribute-presence selector (`[data-open]`), which never matches, so any
 * `data-open:`/`data-checked:`/… token in a sheet is a dead style: the
 * switch track lost its colors and every menu/dialog lost its open/close
 * animation this way once. Sheets must spell these out as
 * `data-[state=open]:`, `data-[orientation=horizontal]:`, … Presence
 * attributes bits-ui actually emits (`data-active`, `data-highlighted`,
 * `data-placeholder`, `data-selected`, `data-inset`) stay bare.
 */
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const STYLES_DIR = dirname(fileURLToPath(import.meta.url));

/** Bare variants that can never match a bits-ui attribute. */
const DEAD_BARE_VARIANTS = [
	"data-open:",
	"data-closed:",
	"data-checked:",
	"data-unchecked:",
	"data-horizontal:",
	"data-vertical:",
	"data-indeterminate:",
] as const;

describe("style sheet data variants", () => {
	it("spells state/orientation variants out (bare forms never match bits-ui)", () => {
		for (const file of readdirSync(STYLES_DIR)) {
			if (!/^style-.+\.css$/.test(file)) continue;
			const css = readFileSync(join(STYLES_DIR, file), "utf8");
			for (const variant of DEAD_BARE_VARIANTS) {
				expect(css.includes(variant), `${file} uses the dead bare variant "${variant}" — use data-[state=…]: or data-[orientation=…]: instead`).toBe(false);
			}
		}
	});

	it("keeps the state-form variants present where the bare ones used to be", () => {
		// Spot-check the rule that motivated the guard: the switch track must
		// color itself through data-state in every sheet.
		for (const file of readdirSync(STYLES_DIR)) {
			if (!/^style-.+\.css$/.test(file)) continue;
			const css = readFileSync(join(STYLES_DIR, file), "utf8");
			expect(css.includes("data-[state=checked]:bg-primary"), `${file}: .cn-switch must color the track via data-[state=checked]`).toBe(true);
			expect(css.includes("data-[state=unchecked]:bg-input"), `${file}: .cn-switch must color the track via data-[state=unchecked]`).toBe(true);
		}
	});
});
