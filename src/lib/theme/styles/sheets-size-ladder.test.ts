/**
 * Density-ladder guard for the vendored style sheets (reads the files like
 * sheets-coverage.test.ts): every sheet keeps the canonical control ladder
 * of its density group — compact (mira), regular (nova, lyra, rhea), roomy
 * (vega, luma, maia), loose (sera) — so per-theme size steps stay
 * arithmetic and monotonic instead of drifting apart. A token matches
 * plain or variant-prefixed utilities (`data-[size=default]:h-9` counts as
 * h-9). Extend the ladder tables when a step is intentionally changed.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const STYLES_DIR = dirname(fileURLToPath(import.meta.url));

type Density = "compact" | "regular" | "roomy" | "loose";

/** Target marker → canonical tokens; `[]` = rule must exist, tokens unchecked. */
type Ladder = Record<string, readonly string[]>;

/** The `@apply` argument list of the `.cn-<marker> { … }` rule in a sheet. */
function applyOf(sheet: string, css: string, marker: string): string {
	const rule = new RegExp(`\\.${marker}\\s*\\{([^}]*)\\}`).exec(css);
	if (!rule) throw new Error(`${sheet}: .${marker} rule not found`);
	const apply = /@apply\s+([^;]+);/.exec(rule[1]);
	return apply ? apply[1] : "";
}

/** Plain or variant-prefixed token match (`data-[size=default]:w-8` has w-8). */
function hasToken(apply: string, token: string): boolean {
	return apply
		.replace(/;/g, " ")
		.split(/\s+/)
		.some((part) => part === token || part.endsWith(`:${token}`));
}

/**
 * Canonical ladder shared by every sheet of a density group: buttons
 * (xs/sm/default/lg ×5..11), inputs/selects, badge, tabs list, switch
 * track + knob, toast, table head, menu/command items and their content
 * padding, dialog/alert-dialog and popover padding.
 */
const SHARED_LADDERS: Record<Density, Ladder> = {
	compact: {
		"cn-button-size-xs": ["h-5"],
		"cn-button-size-sm": ["h-6"],
		"cn-button-size-default": ["h-7"],
		"cn-button-size-lg": ["h-8"],
		"cn-button-size-icon-xs": ["size-5"],
		"cn-button-size-icon-sm": ["size-6"],
		"cn-button-size-icon": ["size-7"],
		"cn-button-size-icon-lg": ["size-8"],
		"cn-input": ["h-7"],
		"cn-select-trigger": ["h-7", "h-6"], // default + sm
		"cn-badge": ["h-5"],
		"cn-tabs-list": ["h-7"],
		"cn-switch": ["h-4", "w-7"],
		"cn-switch-thumb": ["size-3.5"],
		"cn-toast": ["h-10"],
		"cn-table-head": ["h-10"],
		"cn-dropdown-menu-item": ["min-h-6", "px-2", "py-1"],
		"cn-command-item": ["min-h-6", "px-2", "py-1"],
		"cn-dropdown-menu-content": ["min-w-32", "p-1"],
		"cn-command": ["p-1"],
		"cn-dialog-content": ["p-4", "gap-4"],
		"cn-alert-dialog-content": ["p-4", "gap-4"],
		"cn-popover-content": ["p-2.5"],
	},
	regular: {
		"cn-button-size-xs": ["h-7"],
		"cn-button-size-sm": ["h-7"],
		"cn-button-size-default": ["h-8"],
		"cn-button-size-lg": ["h-9"],
		"cn-button-size-icon-xs": ["size-7"],
		"cn-button-size-icon-sm": ["size-7"],
		"cn-button-size-icon": ["size-8"],
		"cn-button-size-icon-lg": ["size-9"],
		"cn-input": ["h-8"],
		"cn-select-trigger": ["h-8", "h-7"],
		"cn-badge": ["h-5"],
		"cn-tabs-list": ["h-8"],
		"cn-switch": ["h-[18px]", "w-8"],
		"cn-switch-thumb": ["size-4"],
		"cn-toast": ["h-10"],
		"cn-table-head": ["h-10"],
		"cn-dropdown-menu-item": ["min-h-7", "px-2", "py-1.5"],
		"cn-command-item": ["min-h-7", "px-2", "py-1.5"],
		"cn-dropdown-menu-content": ["min-w-32", "p-1"],
		"cn-command": ["p-1"],
		"cn-dialog-content": ["p-4", "gap-4"],
		"cn-alert-dialog-content": ["p-4", "gap-4"],
		"cn-popover-content": ["p-2.5"],
	},
	roomy: {
		"cn-button-size-xs": ["h-7"],
		"cn-button-size-sm": ["h-8"],
		"cn-button-size-default": ["h-9"],
		"cn-button-size-lg": ["h-10"],
		"cn-button-size-icon-xs": ["size-7"],
		"cn-button-size-icon-sm": ["size-8"],
		"cn-button-size-icon": ["size-9"],
		"cn-button-size-icon-lg": ["size-10"],
		"cn-input": ["h-9"],
		"cn-select-trigger": ["h-9", "h-8"],
		"cn-badge": ["h-5"],
		"cn-tabs-list": ["h-9"],
		"cn-switch": ["h-[18px]", "w-8"],
		"cn-switch-thumb": ["size-4"],
		"cn-toast": ["h-12"],
		"cn-table-head": ["h-12"],
		"cn-dropdown-menu-item": ["min-h-8", "px-3", "py-1.5"],
		"cn-command-item": ["min-h-8", "px-3", "py-1.5"],
		"cn-dropdown-menu-content": ["min-w-44", "p-1.5"],
		"cn-command": ["p-1.5"],
		"cn-dialog-content": ["p-6", "gap-6"],
		"cn-alert-dialog-content": ["p-6", "gap-6"],
		"cn-popover-content": ["p-3"],
	},
	loose: {
		"cn-button-size-xs": ["h-8"],
		"cn-button-size-sm": ["h-9"],
		"cn-button-size-default": ["h-10"],
		"cn-button-size-lg": ["h-11"],
		"cn-button-size-icon-xs": ["size-8"],
		"cn-button-size-icon-sm": ["size-9"],
		"cn-button-size-icon": ["size-10"],
		"cn-button-size-icon-lg": ["size-11"],
		"cn-input": ["h-10"],
		"cn-select-trigger": ["h-10", "h-9"],
		"cn-badge": ["h-5"],
		"cn-tabs-list": ["h-10"],
		"cn-switch": ["h-[18px]", "w-8"],
		"cn-switch-thumb": ["size-3.5"],
		"cn-toast": ["h-12"],
		"cn-table-head": ["h-12"],
		"cn-dropdown-menu-item": ["min-h-9", "px-3", "py-2"],
		"cn-command-item": ["min-h-9", "px-3", "py-2"],
		"cn-dropdown-menu-content": ["min-w-48", "p-1.5"],
		// sera's vendored sheet carries no command padding (component defaults).
		"cn-command": [],
		"cn-dialog-content": ["p-6", "gap-6"],
		"cn-alert-dialog-content": ["p-6", "gap-6"],
		"cn-popover-content": ["p-4"],
	},
};

/**
 * Per-sheet overrides for identities the ladder deliberately keeps: lyra's
 * upstream sheet never had menu-content/command padding, and the border-2
 * luma/rhea knobs are h/w pills sized inside their 18px track.
 */
const SHEET_LADDERS: Record<Density, Record<string, Ladder>> = {
	compact: {
		mira: {},
	},
	regular: {
		nova: {},
		lyra: {
			"cn-dropdown-menu-content": ["min-w-32"],
			"cn-command": [],
		},
		rhea: { "cn-switch-thumb": ["h-3.5", "w-4"] },
	},
	roomy: {
		vega: {},
		luma: { "cn-switch-thumb": ["h-3.5", "w-4.5"] },
		maia: {},
	},
	loose: {
		sera: {},
	},
};

describe("style sheet size ladders", () => {
	for (const [density, sheets] of Object.entries(SHEET_LADDERS) as [Density, Record<string, Ladder>][]) {
		it(`keeps ${density} sheets on the canonical ladder`, () => {
			for (const [sheet, overrides] of Object.entries(sheets)) {
				const css = readFileSync(join(STYLES_DIR, `style-${sheet}.css`), "utf8");
				const ladder = { ...SHARED_LADDERS[density], ...overrides };
				for (const [marker, tokens] of Object.entries(ladder)) {
					const apply = applyOf(sheet, css, marker);
					for (const token of tokens) {
						expect(hasToken(apply, token), `${sheet}: .${marker} is missing "${token}" (${density} ladder)`).toBe(
							true,
						);
					}
				}
			}
		});
	}
});
