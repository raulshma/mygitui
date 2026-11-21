/**
 * DiffToolbar — the ONE diff control row (plan §7.4). Covers the component
 * contract on top of bits-ui: the segmented Split|Unified picker reports
 * `aria-pressed` and routes through `onModeChange`, the Wrap and
 * Ignore-whitespace icon toggles carry the toolbar's historical accessible
 * names ("Wrap long lines" / "Ignore whitespace") so palette/e2e targets
 * keep working, and the overflow menu hosts the demoted commands (copy
 * diff, open in editor, hide-ws, word highlights).
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DiffToolbar from "./DiffToolbar.svelte";
import { diffPrefs } from "$lib/diff/diffPrefs.svelte";

// The icon resolver needs the real module (static lucide import) — no mock
// needed; Icon.svelte renders whatever appIcon returns (or nothing).

afterEach(cleanup);

beforeEach(() => {
	diffPrefs.ignoreWs = false;
	diffPrefs.hideWs = false;
	diffPrefs.wordHighlights = true;
});

interface Props {
	mode: "split" | "unified";
	wrap: boolean;
	fileCount: number;
	additions: number;
	deletions: number;
	onModeChange: (mode: "split" | "unified") => void;
	onWrapChange: (wrap: boolean) => void;
	onCopyDiff?: () => void;
	onOpenInEditor?: () => void;
}

function baseProps(overrides: Partial<Props> = {}): Props {
	return {
		mode: "split",
		wrap: true,
		fileCount: 3,
		additions: 12,
		deletions: 4,
		onModeChange: () => {},
		onWrapChange: () => {},
		...overrides,
	};
}

describe("DiffToolbar segmented mode picker", () => {
	it("presses Split when mode is split and reports changes", async () => {
		const onModeChange = vi.fn();
		render(DiffToolbar, { ...baseProps(), onModeChange });
		await tick();

		const split = screen.getByRole("button", { name: "Split" });
		const unified = screen.getByRole("button", { name: "Unified" });
		expect(split.getAttribute("aria-pressed")).toBe("true");
		expect(unified.getAttribute("aria-pressed")).toBe("false");

		await fireEvent.click(unified);
		expect(onModeChange).toHaveBeenCalledWith("unified");
	});

	it("presses Unified when mode is unified", async () => {
		render(DiffToolbar, { ...baseProps({ mode: "unified" }) });
		await tick();
		expect(
			screen.getByRole("button", { name: "Unified" }).getAttribute("aria-pressed"),
		).toBe("true");
	});
});

describe("DiffToolbar icon toggles", () => {
	it("keeps the Wrap long lines name and pressed state", async () => {
		const onWrapChange = vi.fn();
		render(DiffToolbar, { ...baseProps({ wrap: true, onWrapChange }) });
		await tick();

		const wrap = screen.getByRole("button", { name: "Wrap long lines" });
		expect(wrap.getAttribute("aria-pressed")).toBe("true");
		await fireEvent.click(wrap);
		expect(onWrapChange).toHaveBeenCalledWith(false);
	});

	it("keeps the Ignore whitespace name wired to diffPrefs", async () => {
		render(DiffToolbar, { ...baseProps() });
		await tick();

		const ignore = screen.getByRole("button", { name: "Ignore whitespace" });
		expect(ignore.getAttribute("aria-pressed")).toBe("false");
		await fireEvent.click(ignore);
		await tick();
		expect(diffPrefs.ignoreWs).toBe(true);
		expect(ignore.getAttribute("aria-pressed")).toBe("true");
	});
});

describe("DiffToolbar overflow menu", () => {
	it("hides copy/open items when no callbacks are given", async () => {
		render(DiffToolbar, { ...baseProps() });
		await tick();

		await fireEvent.click(screen.getByRole("button", { name: "More diff options" }));
		await tick();

		expect(screen.getByRole("menuitemcheckbox", { name: "Hide whitespace changes" })).toBeTruthy();
		expect(screen.getByRole("menuitemcheckbox", { name: "Word highlights" })).toBeTruthy();
		expect(screen.queryByText("Copy diff")).toBeNull();
		expect(screen.queryByText("Open in editor")).toBeNull();
	});

	it("offers copy diff and open in editor when callbacks exist", async () => {
		const onCopyDiff = vi.fn();
		const onOpenInEditor = vi.fn();
		render(DiffToolbar, { ...baseProps({ onCopyDiff, onOpenInEditor }) });
		await tick();

		await fireEvent.click(screen.getByRole("button", { name: "More diff options" }));
		await tick();

		await fireEvent.click(screen.getByText("Copy diff"));
		await tick();
		expect(onCopyDiff).toHaveBeenCalledOnce();
	});

	it("flips the hide-whitespace checkbox through diffPrefs", async () => {
		render(DiffToolbar, { ...baseProps({ onCopyDiff: () => {} }) });
		await tick();

		await fireEvent.click(screen.getByRole("button", { name: "More diff options" }));
		await tick();
		await fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "Hide whitespace changes" }));
		await tick();
		expect(diffPrefs.hideWs).toBe(true);
	});
});

describe("DiffToolbar stats cluster", () => {
	it("renders the file/add/delete counts", async () => {
		render(DiffToolbar, { ...baseProps() });
		await tick();

		expect(screen.getByText("3 files")).toBeTruthy();
		expect(screen.getByText("+12")).toBeTruthy();
		expect(screen.getByText("−4")).toBeTruthy();
	});
});
