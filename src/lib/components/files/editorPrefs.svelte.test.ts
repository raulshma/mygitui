/**
 * Tests for the file-editor preference flags (`editorPrefs`): the pure
 * parse helpers behind the localStorage reads, and the close-all confirm
 * flag's round-trip through localStorage (the wrap / md-view setters
 * share the same try/catch write pattern).
 */
import { beforeEach, describe, expect, it } from "vitest";
import {
  editorPrefs,
  parseCloseAllConfirm,
} from "./editorPrefs.svelte";

const CLOSE_ALL_CONFIRM_KEY = "mygitui.files.close-all-confirm";

describe("parseCloseAllConfirm", () => {
  it("defaults to asking when nothing (or anything else) is stored", () => {
    expect(parseCloseAllConfirm(null)).toBe(true);
    expect(parseCloseAllConfirm("")).toBe(true);
    expect(parseCloseAllConfirm("1")).toBe(true);
    expect(parseCloseAllConfirm("garbage")).toBe(true);
  });

  it("only an explicit stored '0' opts out", () => {
    expect(parseCloseAllConfirm("0")).toBe(false);
  });
});

describe("editorPrefs.closeAllConfirm", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("starts asking (default true)", () => {
    expect(editorPrefs.closeAllConfirm).toBe(true);
  });

  it("round-trips the opted-out choice through localStorage", () => {
    editorPrefs.setCloseAllConfirm(false);
    expect(editorPrefs.closeAllConfirm).toBe(false);
    expect(localStorage.getItem(CLOSE_ALL_CONFIRM_KEY)).toBe("0");

    editorPrefs.setCloseAllConfirm(true);
    expect(editorPrefs.closeAllConfirm).toBe(true);
    expect(localStorage.getItem(CLOSE_ALL_CONFIRM_KEY)).toBe("1");
  });
});
