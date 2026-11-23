/**
 * Tests for the contrast guardrails' math (src/lib/theme/contrast).
 *
 * The numbers that matter: the customizer's yellow light primary reads
 * ~1.3:1 against white and the red dark primary ~2.3:1 against the dark
 * background — the two reported unreadable themes. Repairs must pull both
 * past 4.5:1 with the hue intact, leave passing pairs byte-identical, and
 * hover must step AWAY from the foreground (contrast grows).
 */
import { describe, expect, it } from "vitest";

import {
	contrastRatio,
	hoverShift,
	inkFor,
	oklchToCss,
	parseOklch,
	relativeLuminance,
} from "./contrast";

const WHITE = "oklch(1 0 0)";
const BLACK_BG = "oklch(0.145 0 0)";
const YELLOW_LIGHT_PRIMARY = "oklch(0.852 0.199 91.936)";
const RED_DARK_PRIMARY = "oklch(0.444 0.177 26.899)";

describe("parseOklch", () => {
	it("parses the token form L C H", () => {
		expect(parseOklch("oklch(0.852 0.199 91.936)")).toEqual({
			l: 0.852,
			c: 0.199,
			h: 91.936,
			alpha: null,
		});
	});

	it("parses percentage lightness and alpha", () => {
		expect(parseOklch("oklch(50% 0.1 120 / 10%)")).toEqual({
			l: 0.5,
			c: 0.1,
			h: 120,
			alpha: 0.1,
		});
		expect(parseOklch("oklch(1 0 0 / 0.38)")).toEqual({
			l: 1,
			c: 0,
			h: 0,
			alpha: 0.38,
		});
	});

	it("rejects non-oklch values the token sets never use", () => {
		expect(parseOklch("red")).toBeNull();
		expect(parseOklch("color-mix(in srgb, red 50%, white)")).toBeNull();
		expect(parseOklch("oklch(0.5 0.1)")).toBeNull();
	});
});

describe("contrast math", () => {
	it("ranks known pairs (black/white = 21, the broken themes well under 4.5)", () => {
		expect(contrastRatio("oklch(0 0 0)", WHITE)).toBeCloseTo(21, 0);
		expect(contrastRatio(YELLOW_LIGHT_PRIMARY, WHITE)).toBeLessThan(2);
		expect(contrastRatio(RED_DARK_PRIMARY, BLACK_BG)).toBeLessThan(3);
		expect(contrastRatio("oklch(0.205 0 0)", "oklch(0.985 0 0)")).toBeGreaterThan(7);
	});

	it("monotonic lightness means monotonic luminance for neutrals", () => {
		const low = relativeLuminance(parseOklch("oklch(0.2 0 0)")!);
		const high = relativeLuminance(parseOklch("oklch(0.8 0 0)")!);
		expect(high).toBeGreaterThan(low);
	});

	it("round-trips through the css string form", () => {
		const color = { l: 0.852, c: 0.199, h: 91.936, alpha: null };
		expect(parseOklch(oklchToCss(color))).toEqual({
			l: 0.852,
			c: 0.199,
			h: 91.94,
			alpha: null,
		});
	});
});

describe("inkFor", () => {
	it("darkens the light yellow primary until it reads on white, hue intact", () => {
		const ink = inkFor(YELLOW_LIGHT_PRIMARY, WHITE);
		const parsed = parseOklch(ink)!;
		expect(contrastRatio(ink, WHITE)).toBeGreaterThanOrEqual(4.5);
		expect(parsed.h).toBeCloseTo(91.94, 0);
		expect(parsed.l).toBeLessThan(parseOklch(YELLOW_LIGHT_PRIMARY)!.l);
	});

	it("lightens the dark red primary until it reads on the dark background", () => {
		const ink = inkFor(RED_DARK_PRIMARY, BLACK_BG);
		const parsed = parseOklch(ink)!;
		expect(contrastRatio(ink, BLACK_BG)).toBeGreaterThanOrEqual(4.5);
		expect(parsed.h).toBeCloseTo(26.9, 0);
		expect(parsed.l).toBeGreaterThan(parseOklch(RED_DARK_PRIMARY)!.l);
	});

	it("returns passing pairs byte-identical (hand-tuned presets stay put)", () => {
		const fg = "oklch(0.985 0 0)";
		expect(inkFor(fg, "oklch(0.205 0 0)")).toBe(fg);
		expect(inkFor("oklch(0.205 0 0)", "oklch(0.97 0 0)")).toBe("oklch(0.205 0 0)");
	});

	it("passes unparsable values through untouched", () => {
		const value = "color-mix(in srgb, red 50%, white)";
		expect(inkFor(value, WHITE)).toBe(value);
	});
});

describe("hoverShift", () => {
	it("steps light fills away from a darker foreground (lighter hover)", () => {
		const hover = hoverShift(YELLOW_LIGHT_PRIMARY, "oklch(0.421 0.095 57.708)");
		const parsed = parseOklch(hover)!;
		expect(parsed.l).toBeCloseTo(0.892, 2);
		expect(parsed.c).toBeCloseTo(0.199, 3);
		expect(contrastRatio(hover, "oklch(0.421 0.095 57.708)")).toBeGreaterThan(
			contrastRatio(YELLOW_LIGHT_PRIMARY, "oklch(0.421 0.095 57.708)")!,
		);
	});

	it("steps dark fills away from a lighter foreground (darker hover)", () => {
		const hover = hoverShift("oklch(0.205 0 0)", "oklch(0.985 0 0)");
		expect(parseOklch(hover)!.l).toBeCloseTo(0.165, 2);
		expect(contrastRatio("oklch(0.985 0 0)", hover)).toBeGreaterThan(
			contrastRatio("oklch(0.985 0 0)", "oklch(0.205 0 0)")!,
		);
	});

	it("lightens dark-scheme fills away from their dark foreground", () => {
		expect(hoverShift("oklch(0.922 0 0)", "oklch(0.205 0 0)")).toBe(
			"oklch(0.962 0 0)",
		);
	});

	it("passes unparsable values through untouched", () => {
		expect(hoverShift("var(--x)", "oklch(0.5 0 0)")).toBe("var(--x)");
	});
});
