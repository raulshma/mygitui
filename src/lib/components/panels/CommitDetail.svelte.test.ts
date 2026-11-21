/**
 * CommitDetail's Workstream C structure (plan §7): ONE meta summary row
 * (sha chip + signature + summary), the Details popover holding the rest of
 * the metadata (committer, parents, describe, trailer-stripped message,
 * structured trailers, notes), the demoted action set (Cherry-pick/Revert
 * visible; rebase/checkout/copy-sha in the overflow), and file rows whose
 * Blame/History actions surface only on hover/focus (never always-visible
 * button pairs) with the `b`/`h` keyboard actions.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Transport } from "$lib/ipc/client";
import { resetTransport, setTransport } from "$lib/ipc/client";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import CommitDetail from "./CommitDetail.svelte";
import type { CommitInfo } from "$lib/ipc/types";

// Keep the (lazy) Shiki pipeline out of the test entirely.
vi.mock("$lib/diff/highlight", () => ({
	cachedTokens: () => null,
	requestTokens: () => {},
	onTokensLanded: () => () => {},
	syntaxVersion: () => 0,
	setScheme: () => {},
}));

const PARENT = "1111111111111111111111111111111111111111";
const SHA = "2222222222222222222222222222222222222222";

function info(): CommitInfo {
	return {
		sha: SHA,
		parents: [PARENT],
		author: { name: "A U Thor", email: "a@t.co", time: 1700000000, offset_minutes: 0 },
		committer: { name: "Committer C", email: "c@t.co", time: 1700000100, offset_minutes: 0 },
		summary: "the summary line",
		refs: [],
	};
}

function line(
	origin: DiffLine["origin"],
	text: string,
	oldNo: number | null,
	newNo: number | null,
): DiffLine {
	return { old_no: oldNo, new_no: newNo, origin, text, highlights: [] };
}

function files(): FileDiff[] {
	const hunk: DiffHunk = {
		old_start: 1,
		new_start: 1,
		lines: [line("+", "added", null, 1)],
	};
	return [
		{
			path: "src/changed.ts",
			old_path: null,
			binary: false,
			is_image: false,
			additions: 1,
			deletions: 0,
			hunks: [hunk],
		},
	];
}

/** Full message with a trailer block (must never render inline). */
const FULL_MESSAGE = "the summary line\n\nBody prose.\n\nSigned-off-by: A U Thor <a@t.co>\nT3-Head-Moved-At: 2026-01-01T00:00:00Z\n";

const streams: Array<{ onmessage: (files: FileDiff[]) => void }> = [];

beforeEach(() => {
	streams.length = 0;
	if (typeof Element.prototype.scrollTo !== "function") {
		Element.prototype.scrollTo = () => {};
	}
	setTransport(((command: string, args?: Record<string, unknown>) => {
		if (command === "repo_diff_stream") {
			const channel = args?.on_page as { onmessage: (files: FileDiff[]) => void };
			streams.push(channel);
			return Promise.resolve();
		}
		if (command === "repo_commit_message") return Promise.resolve(FULL_MESSAGE);
		if (command === "repo_commit_notes") return Promise.resolve([]);
		if (command === "repo_commit_signature") {
			return Promise.resolve({ signed: false, valid: null, kind: null, detail: null });
		}
		return Promise.resolve(null);
	}) as unknown as Transport);
});

afterEach(() => {
	cleanup();
	resetTransport();
});

interface MountProps {
	info?: CommitInfo | null;
	onCherryPick?: () => void;
	onRevert?: () => void;
	onRebaseFromHere?: () => void;
	onCheckout?: () => void;
}

function mountDetail(props: MountProps = {}) {
	return render(CommitDetail, {
		repoId: "r1",
		info: props.info ?? info(),
		onCherryPick: props.onCherryPick ?? (() => {}),
		onRevert: props.onRevert ?? (() => {}),
		onRebaseFromHere: props.onRebaseFromHere,
		onCheckout: props.onCheckout,
	});
}

describe("CommitDetail meta summary row", () => {
	it("keeps the summary + sha chip in one header row", async () => {
		mountDetail();
		streams[0]?.onmessage(files());
		await tick();

		// The `.dsummary` DOM contract (e2e history_click/filehistory).
		const summary = document.querySelector(".dsummary");
		expect(summary?.textContent).toContain("the summary line");
		// The sha chip offers copy-on-click.
		expect(screen.getByRole("button", { name: `Copy sha ${SHA}` }).textContent).toBe(
			SHA.slice(0, 7),
		);
	});

	it("renders the raw sha NOT as text next to the summary", async () => {
		mountDetail();
		streams[0]?.onmessage(files());
		await tick();
		// The full sha lives only in aria labels/clipboard flows; the header
		// shows the 7-char chip.
		expect(document.querySelector(".dhead")?.textContent).not.toContain(SHA);
	});
});

describe("CommitDetail action row", () => {
	it("shows Cherry-pick/Revert and the overflow holds the rest", async () => {
		const onCherryPick = vi.fn();
		const onRebaseFromHere = vi.fn();
		mountDetail({ onCherryPick, onRebaseFromHere });
		streams[0]?.onmessage(files());
		await tick();

		expect(screen.getByRole("button", { name: "Cherry-pick" })).toBeTruthy();
		expect(screen.getByRole("button", { name: "Revert" })).toBeTruthy();
		// Demoted commands live in the ⋯ overflow (reachable, not visible).
		expect(screen.queryByText("Rebase from here")).toBeNull();
		await fireEvent.click(screen.getByRole("button", { name: "More commit actions" }));
		await tick();
		const rebase = screen.getByText("Rebase from here");
		await fireEvent.click(rebase);
		expect(onRebaseFromHere).toHaveBeenCalledOnce();
	});
});

describe("CommitDetail Details popover", () => {
	it("holds committer, parents, message and structured trailers", async () => {
		mountDetail();
		streams[0]?.onmessage(files());
		await tick();

		const trigger = screen.getByRole("button", { name: "Commit details" });
		await fireEvent.click(trigger);
		// bits-ui portals the content on the frame after the open toggle.
		await new Promise((resolve) => requestAnimationFrame(resolve));
		await tick();
		await tick();


		// Committer + clickable parent (the dd also carries the timestamps,
		// so the name needs a substring match).
		expect(screen.getByText("Committer C", { exact: false })).toBeTruthy();
		const content = document.querySelector('[data-slot="popover-content"]')!;
		const parent = [...content.querySelectorAll("button")].find((b) =>
			(b.textContent ?? "").trim() === PARENT.slice(0, 7),
		);
		expect(parent).toBeTruthy();
		// The body renders; the raw trailer lines do NOT render inline.
		expect(screen.getByText("Body prose.", { exact: false })).toBeTruthy();
		expect(screen.getByText("T3-Head-Moved-At")).toBeTruthy();
		expect(screen.getByText("A U Thor <a@t.co>")).toBeTruthy();
		expect(content.textContent).not.toContain("T3-Head-Moved-At: 2026");
	});
});

describe("CommitDetail file rows", () => {
	it("shows letter/path/stats and hides Blame/History until hover/focus", async () => {
		mountDetail();
		streams[0]?.onmessage(files());
		await tick();

		// The changed-files list (DOM contract: `.filelist`) carries the
		// status letter, the path and the ± stats. (The path text also
		// occurs inside the diff's own file-header row — query the list.)
		const list = document.querySelector(".filelist")!;
		expect(list.querySelector(".fstat")?.textContent).toBe("A");
		expect(list.querySelector(".fpath")?.textContent).toBe("src/changed.ts");
		expect(list.querySelector(".stats")?.textContent).toContain("+1");
		// The row actions exist but are hover/focus-revealed only (the
		// class-driven reveal: hidden until the row is hovered/focused).
		const blame = screen.getByRole("button", { name: "Blame src/changed.ts" });
		expect(blame).toBeTruthy();
		// Reveal is class-driven on the row's action span: hidden until the
		// row is hovered/focused (group-hover/group-focus-within).
		const actions = document.querySelector(".filelist li .factions")!;
		expect(actions.className).toContain("opacity-0");
		expect(actions.className).toContain("group-hover:opacity-100");
		expect(actions.className).toContain("group-focus-within:opacity-100");
	});

	it("blames via the keyboard `b` on a focused row", async () => {
		mountDetail();
		streams[0]?.onmessage(files());
		await tick();

		const row = document.querySelector(".filelist li")!;
		await fireEvent.keyDown(row, { key: "b" });
		await tick();
		expect(document.querySelector(".dtab")?.textContent).toContain("Blame: src/changed.ts");
	});
});
