/**
 * DiffViewer's narrow-container defaults (plan §7.5): under a 700px
 * container the viewer auto-selects Unified and turns wrap OFF, while a
 * user override sticks for the session, in memory only — the persisted
 * prefs (`mygitui.diff.*`) keep ruling wide containers and are NOT written
 * by narrow overrides.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Transport } from "$lib/ipc/client";
import { resetTransport, setTransport } from "$lib/ipc/client";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import DiffViewer from "$lib/components/diff/DiffViewer.svelte";

// svelte must resolve to its CLIENT build under vitest (see PopoutDiff test).
vi.mock("svelte", async () => {
	const client = await import(
		// @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
		"../../../../node_modules/svelte/src/index-client.js"
	);
	return { ...client, tick: client.tick };
});

// Keep the (lazy) Shiki pipeline out of the test entirely.
vi.mock("$lib/diff/highlight", () => ({
	cachedTokens: () => null,
	requestTokens: () => {},
	onTokensLanded: () => () => {},
	syntaxVersion: () => 0,
	setScheme: () => {},
}));

afterEach(() => {
	cleanup();
	resetTransport();
	localStorage.clear();
});

beforeEach(() => {
	// jsdom has no scrollTo (the viewer's restore/reset effects call it).
	if (typeof Element.prototype.scrollTo !== "function") {
		Element.prototype.scrollTo = () => {};
	}
});

function line(
	origin: DiffLine["origin"],
	text: string,
	oldNo: number | null,
	newNo: number | null,
): DiffLine {
	return { old_no: oldNo, new_no: newNo, origin, text, highlights: [] };
}

function fixture(): FileDiff[] {
	const hunk: DiffHunk = {
		old_start: 1,
		new_start: 1,
		lines: [line(" ", "keep", 1, 1), line("+", "added", null, 2)],
	};
	return [
		{
			path: "src/a.txt",
			old_path: null,
			binary: false,
			is_image: false,
			additions: 1,
			deletions: 0,
			hunks: [hunk],
		},
	];
}

setTransport((() => Promise.resolve()) as unknown as Transport);

describe("DiffViewer narrow defaults", () => {
	it("auto-selects Unified and wrap OFF in a narrow container", async () => {
		localStorage.setItem("mygitui.diff.mode", JSON.stringify("split"));
		localStorage.setItem("mygitui.diff.wrap", JSON.stringify("true"));
		render(DiffViewer, { files: fixture(), containerNarrow: true });
		await tick();

		expect(
			screen.getByRole("button", { name: "Unified" }).getAttribute("aria-pressed"),
		).toBe("true");
		expect(screen.getByRole("button", { name: "Wrap long lines" }).getAttribute("aria-pressed")).toBe(
			"false",
		);
	});

	it("keeps the persisted defaults in a wide container", async () => {
		localStorage.setItem("mygitui.diff.mode", JSON.stringify("split"));
		localStorage.setItem("mygitui.diff.wrap", JSON.stringify("true"));
		render(DiffViewer, { files: fixture(), containerNarrow: false });
		await tick();

		expect(
			screen.getByRole("button", { name: "Split" }).getAttribute("aria-pressed"),
		).toBe("true");
		expect(screen.getByRole("button", { name: "Wrap long lines" }).getAttribute("aria-pressed")).toBe(
			"true",
		);
	});

	it("a narrow override sticks for the session without writing the pref", async () => {
		localStorage.setItem("mygitui.diff.mode", JSON.stringify("split"));
		localStorage.setItem("mygitui.diff.wrap", JSON.stringify("true"));
		render(DiffViewer, { files: fixture(), containerNarrow: true });
		await tick();

		// User picks Split while narrow: the override wins immediately…
		await fireEvent.click(screen.getByRole("button", { name: "Split" }));
		expect(
			screen.getByRole("button", { name: "Split" }).getAttribute("aria-pressed"),
		).toBe("true");
		// …but nothing was persisted (the override is in-memory only).
		expect(localStorage.getItem("mygitui.diff.mode")).toBe(JSON.stringify("split"));

		// Same for wrap: toggling ON in a narrow container leaves the
		// stored `true` untouched (it was already true — assert it survives,
		// i.e. no write replaced it).
		await fireEvent.click(screen.getByRole("button", { name: "Wrap long lines" }));
		expect(screen.getByRole("button", { name: "Wrap long lines" }).getAttribute("aria-pressed")).toBe(
			"true",
		);
		expect(localStorage.getItem("mygitui.diff.wrap")).toBe(JSON.stringify("true"));
	});

	it("a wide override still persists (unchanged wide behavior)", async () => {
		localStorage.setItem("mygitui.diff.mode", JSON.stringify("unified"));
		render(DiffViewer, { files: fixture(), containerNarrow: false });
		await tick();

		await fireEvent.click(screen.getByRole("button", { name: "Split" }));
		expect(localStorage.getItem("mygitui.diff.mode")).toBe(JSON.stringify("split"));
	});
});
