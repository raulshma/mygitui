/**
 * PopoutDiff stale-page guard: bumping `reload` (Refresh button, debounced
 * repo change) re-runs the streaming effect and resets `files` while the
 * superseded stream may still be paging — its late pages must NOT
 * interleave into the fresh array as duplicates (twin of RepoView's
 * `diffToken` guard; see the guard comment in the component).
 *
 * The stream is driven through the client's injectable transport
 * (`setTransport`): each `repo_diff_stream` call gets a channel whose pages
 * the test delivers on demand, plus a promise the test resolves to end the
 * stream. Shiki stays out (`$lib/diff/highlight` mock, same as the
 * DiffViewer component tests).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import type { Transport } from "$lib/ipc/client";
import { resetTransport, setTransport } from "$lib/ipc/client";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import PopoutDiff from "./PopoutDiff.svelte";

// svelte must resolve to its CLIENT build under vitest (see GraphCanvas.test.ts):
// without it the bare import resolves to index-server.js and mounting throws.
vi.mock("svelte", () =>
  // @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
  import("../../../../node_modules/svelte/src/index-client.js"),
);

// Keep the (lazy) Shiki pipeline out of the test entirely.
vi.mock("$lib/diff/highlight", () => ({
  cachedTokens: () => null,
  requestTokens: () => {},
  onTokensLanded: () => () => {},
  syntaxVersion: () => 0,
  setScheme: () => {},
}));

// ---------------------------------------------------------------------------
// Fixtures + stream harness
// ---------------------------------------------------------------------------

function file(path: string): FileDiff {
  const line: DiffLine = {
    old_no: 1,
    new_no: 1,
    origin: "+",
    text: "x",
    highlights: [],
  };
  const hunk: DiffHunk = { old_start: 1, new_start: 1, lines: [line] };
  return {
    path,
    old_path: null,
    binary: false,
    is_image: false,
    additions: 1,
    deletions: 0,
    hunks: [hunk],
  };
}

interface StreamCall {
  deliver: (files: FileDiff[]) => void;
  finish: () => void;
}

const streams: StreamCall[] = [];

beforeEach(() => {
  streams.length = 0;
  if (typeof Element.prototype.scrollTo !== "function") {
    Element.prototype.scrollTo = () => {};
  }
  setTransport(((command: string, args?: Record<string, unknown>) => {
    if (command !== "repo_diff_stream") return Promise.resolve();
    const channel = args?.on_page as {
      onmessage: (files: FileDiff[]) => void;
    };
    return new Promise<void>((resolve) => {
      streams.push({
        deliver: (files) => channel.onmessage(files),
        finish: () => resolve(),
      });
    });
  }) as Transport);
});

afterEach(() => {
  cleanup();
  resetTransport();
});

// ---------------------------------------------------------------------------
// Stale pages (superseded stream racing a reload)
// ---------------------------------------------------------------------------

describe("PopoutDiff stale-page guard", () => {
  it("drops a superseded stream's late pages instead of interleaving them", async () => {
    render(PopoutDiff, { repoId: "r1" });
    await tick();
    expect(streams).toHaveLength(1);

    streams[0]!.deliver([file("src/a.ts"), file("src/b.ts")]);
    await tick();

    // Refresh bumps `reload`: the effect re-runs, `files` resets and a
    // second stream opens — while the first is still mid-flight.
    await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await tick();
    expect(streams).toHaveLength(2);

    // Race: the superseded stream delivers one more page AFTER the reset.
    streams[0]!.deliver([file("src/stale.ts")]);
    streams[1]!.deliver([file("src/fresh.ts")]);
    streams[1]!.finish();
    streams[0]!.finish(); // its completion must not clobber the fresh state
    await tick();
    await tick();

    // Only the fresh stream's file counted — the stale page was dropped
    // (without the generation guard the status reads "2 files": the stale
    // entry interleaved into the reset array). The header status span is
    // the single role="status" once loading cleared; DiffViewer's body
    // also renders the path, so query by role, not by text.
    expect(screen.getByRole("status").textContent).toBe("1 file");
  });
});
