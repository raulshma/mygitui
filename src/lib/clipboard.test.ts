/**
 * Clipboard helper: the awaited write toasts exactly once (success or
 * failure), the label names the payload, and a missing `navigator.clipboard`
 * degrades to the failure toast instead of throwing.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dismissToast, getToasts } from "$lib/toast";
import { copyText } from "$lib/clipboard";

const originalClipboard = navigator.clipboard;

/** Swaps in a `navigator.clipboard` stub (jsdom ships none). */
function stubClipboard(writeText: (text: string) => Promise<void>): void {
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText },
    configurable: true,
  });
}

beforeEach(() => {
  for (const item of [...getToasts()]) dismissToast(item.id);
});

afterEach(() => {
  Object.defineProperty(navigator, "clipboard", {
    value: originalClipboard,
    configurable: true,
  });
  for (const item of [...getToasts()]) dismissToast(item.id);
  vi.restoreAllMocks();
});

describe("copyText", () => {
  it("toasts Copied <text> and resolves true when the write succeeds", async () => {
    stubClipboard(vi.fn(async () => {}));

    await expect(copyText("feature-x")).resolves.toBe(true);

    expect(getToasts()).toHaveLength(1);
    expect(getToasts()[0]!.message).toBe("Copied feature-x");
    expect(getToasts()[0]!.kind).toBe("info");
  });

  it("uses the label in the success toast when given", async () => {
    stubClipboard(vi.fn(async () => {}));

    await expect(copyText("a1b2c3d4e5", "SHA")).resolves.toBe(true);

    expect(getToasts()[0]!.message).toBe("Copied SHA");
  });

  it("toasts Copy failed and resolves false when the write rejects", async () => {
    stubClipboard(vi.fn(async () => Promise.reject(new Error("denied"))));

    await expect(copyText("feature-x", "branch name")).resolves.toBe(false);

    expect(getToasts()).toHaveLength(1);
    expect(getToasts()[0]!.message).toBe("Copy failed: branch name");
    expect(getToasts()[0]!.kind).toBe("error");
  });

  it("reports a failure instead of throwing when navigator.clipboard is missing", async () => {
    stubClipboard(vi.fn(async () => {}));
    delete (navigator as { clipboard?: unknown }).clipboard;

    await expect(copyText("whatever")).resolves.toBe(false);

    expect(getToasts()[0]!.message).toBe("Copy failed: clipboard");
    expect(getToasts()[0]!.kind).toBe("error");
  });
});
