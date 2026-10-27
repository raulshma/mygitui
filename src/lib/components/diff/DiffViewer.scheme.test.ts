/**
 * DiffViewer ⇄ syntax-theme wiring: Shiki bakes colors at parse time, so
 * the viewer must push the applied scheme into the highlighter on mount and
 * again whenever the theme engine announces a scheme switch.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/svelte";
import { tick } from "svelte";
import { resetTransport } from "$lib/ipc/client";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import DiffViewer from "$lib/components/diff/DiffViewer.svelte";

const { setSchemeSpy, schemeRef } = vi.hoisted(() => ({
  setSchemeSpy: vi.fn(),
  schemeRef: { value: "light" as "light" | "dark" },
}));

vi.mock("svelte", async () => {
  const client = await import(
    // @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
    "../../../../node_modules/svelte/src/index-client.js"
  );
  return { ...client, tick: client.tick };
});

vi.mock("$lib/diff/highlight", () => ({
  cachedTokens: () => null,
  requestTokens: () => {},
  onTokensLanded: () => () => {},
  syntaxVersion: () => 0,
  setScheme: setSchemeSpy,
}));

vi.mock("$lib/theme", () => ({
  currentScheme: () => schemeRef.value,
  THEME_SCHEME_EVENT: "mygitui:theme-scheme",
}));

function line(origin: DiffLine["origin"], text: string): DiffLine {
  return { old_no: 1, new_no: 1, origin, text, highlights: [] };
}

function fixture(): FileDiff[] {
  const hunk: DiffHunk = {
    old_start: 1,
    new_start: 1,
    lines: [line(" ", "keep"), line("+", "added")],
  };
  return [
    {
      path: "src/a.ts",
      old_path: null,
      binary: false,
      is_image: false,
      additions: 1,
      deletions: 0,
      hunks: [hunk],
    },
  ];
}

class ResizeObserverStub implements ResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

beforeEach(() => {
  setSchemeSpy.mockReset();
  schemeRef.value = "light";
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
  if (typeof Element.prototype.scrollTo !== "function") {
    Element.prototype.scrollTo = () => {};
  }
  resetTransport();
});

afterEach(() => {
  resetTransport();
  cleanup();
  vi.unstubAllGlobals();
});

describe("DiffViewer syntax scheme", () => {
  it("pushes the applied scheme on mount and follows theme events", async () => {
    render(DiffViewer, { props: { files: fixture(), mode: "unified" } });
    await tick();
    await tick();

    expect(setSchemeSpy).toHaveBeenCalledWith("light");

    schemeRef.value = "dark";
    window.dispatchEvent(
      new CustomEvent("mygitui:theme-scheme", { detail: "dark" }),
    );
    await tick();

    expect(setSchemeSpy).toHaveBeenLastCalledWith("dark");
  });
});
