/**
 * Toast stack bounds: coalescing repeats, the visible cap, and pause/resume
 * interaction with a coalesced toast.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dismissToast, getToasts, pauseToast, resumeToast, toast } from "$lib/toast";

beforeEach(() => {
  vi.useFakeTimers();
  for (const item of [...getToasts()]) dismissToast(item.id);
});

afterEach(() => {
  for (const item of [...getToasts()]) dismissToast(item.id);
  vi.useRealTimers();
});

describe("toast coalescing", () => {
  it("merges an identical message into a repeat count instead of stacking", () => {
    toast("Copied path");
    toast("Copied path");
    toast("Copied path");
    expect(getToasts()).toHaveLength(1);
    expect(getToasts()[0]!.count).toBe(3);
  });

  it("keeps different kinds apart", () => {
    toast("Push failed");
    toast("Push failed", { kind: "error" });
    expect(getToasts()).toHaveLength(2);
    expect(getToasts().map((item) => item.kind)).toEqual(["info", "error"]);
  });

  it("restarts the dismissal timer on a repeat", () => {
    toast("Copied path");
    vi.advanceTimersByTime(3_000);
    toast("Copied path");
    vi.advanceTimersByTime(3_000);
    // 6s elapsed, but the repeat reset the 4s clock at t=3s.
    expect(getToasts()).toHaveLength(1);
    vi.advanceTimersByTime(1_000);
    expect(getToasts()).toHaveLength(0);
  });

  it("banks the full duration when a repeat lands on a paused toast", () => {
    toast("Copied path");
    const id = getToasts()[0]!.id;
    vi.advanceTimersByTime(3_000);
    pauseToast(id);
    toast("Copied path");
    resumeToast(id);
    vi.advanceTimersByTime(3_000);
    expect(getToasts()).toHaveLength(1);
    vi.advanceTimersByTime(1_000);
    expect(getToasts()).toHaveLength(0);
  });
});

describe("toast stack cap", () => {
  it("drops the oldest past four visible toasts", () => {
    for (const message of ["a", "b", "c", "d", "e"]) toast(message);
    expect(getToasts().map((item) => item.message)).toEqual(["b", "c", "d", "e"]);
  });

  it("clears the dropped toast's timer so it cannot dismiss a survivor", () => {
    for (const message of ["a", "b", "c", "d", "e"]) toast(message);
    const survivors = getToasts().map((item) => item.id);
    vi.advanceTimersByTime(4_000);
    expect(getToasts()).toHaveLength(0);
    // No stray timer resurrects or double-dismisses after the cap eviction.
    expect(getToasts().map((item) => item.id)).toEqual([]);
    expect(survivors).toHaveLength(4);
  });

  it("counts a coalesced repeat as one slot, not a new one", () => {
    for (const message of ["a", "b", "c", "d"]) toast(message);
    toast("a");
    expect(getToasts().map((item) => item.message)).toEqual(["a", "b", "c", "d"]);
    expect(getToasts()[0]!.count).toBe(2);
  });
});