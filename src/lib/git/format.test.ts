/**
 * Unit tests for the shared git formatting helpers: `classifyRef` /
 * `shortRefName`, `hashHue`, relative/absolute date formatting and
 * date-input conversion.
 */

import { describe, expect, it } from "vitest";
import {
  classifyRef,
  dayToUnixSeconds,
  formatDateTime,
  formatRelativeTime,
  hashHue,
  shortRefName,
} from "$lib/git/format";

// ---------------------------------------------------------------------------
// classifyRef / shortRefName
// ---------------------------------------------------------------------------

describe("classifyRef", () => {
  it("classifies fully-qualified refs", () => {
    expect(classifyRef("refs/heads/main")).toBe("local");
    expect(classifyRef("refs/heads/feature/x")).toBe("local");
    expect(classifyRef("refs/remotes/origin/main")).toBe("remote");
    expect(classifyRef("refs/tags/v1.0")).toBe("tag");
  });

  it("classifies plain git-log decorations", () => {
    expect(classifyRef("main")).toBe("local");
    expect(classifyRef("origin/dev")).toBe("remote"); // "/" + no HEAD → remote-ish
    expect(classifyRef("tag: v2.0")).toBe("tag");
  });

  it("treats HEAD decorations as head regardless of shape", () => {
    expect(classifyRef("HEAD")).toBe("head");
    expect(classifyRef("HEAD -> refs/heads/main")).toBe("head");
    expect(classifyRef("HEAD -> main")).toBe("head");
    expect(classifyRef("origin/HEAD")).toBe("head");
  });

  it("trims surrounding whitespace", () => {
    expect(classifyRef("  refs/heads/main  ")).toBe("local");
  });
});

describe("shortRefName", () => {
  it("strips known prefixes", () => {
    expect(shortRefName("refs/heads/main")).toBe("main");
    expect(shortRefName("refs/remotes/origin/dev")).toBe("origin/dev");
    expect(shortRefName("refs/tags/v1.0")).toBe("v1.0");
    expect(shortRefName("tag: v2.0")).toBe("v2.0");
    expect(shortRefName("HEAD -> refs/heads/main")).toBe("main");
    expect(shortRefName("HEAD -> main")).toBe("main");
    expect(shortRefName("HEAD")).toBe("HEAD");
    expect(shortRefName("origin/feature/x")).toBe("origin/feature/x");
  });
});

// ---------------------------------------------------------------------------
// hashHue
// ---------------------------------------------------------------------------

describe("hashHue", () => {
  it("returns a hue in [0, 360)", () => {
    for (const key of ["", "a", "0f1e2d3c4b5a69788695a4b3c2d1e0f", "main", "refs/heads/x"]) {
      const hue = hashHue(key);
      expect(hue).toBeGreaterThanOrEqual(0);
      expect(hue).toBeLessThan(360);
      expect(Number.isInteger(hue)).toBe(true);
    }
  });

  it("is stable for the same key and separates simple keys", () => {
    expect(hashHue("deadbeef")).toBe(hashHue("deadbeef"));
    expect(hashHue("a")).not.toBe(hashHue("b"));
  });
});

// ---------------------------------------------------------------------------
// Date formatting
// ---------------------------------------------------------------------------

const NOW = 1_700_000_000; // 2023-09-15T06:26:40Z

describe("formatRelativeTime", () => {
  it("formats past and future spans in the right unit", () => {
    expect(formatRelativeTime(NOW, { now: NOW, locale: "en" })).toBe("now");
    expect(formatRelativeTime(NOW - 30, { now: NOW, locale: "en" })).toBe("30 seconds ago");
    expect(formatRelativeTime(NOW - 120, { now: NOW, locale: "en" })).toBe("2 minutes ago");
    expect(formatRelativeTime(NOW + 300, { now: NOW, locale: "en" })).toBe("in 5 minutes");
    expect(formatRelativeTime(NOW - 7_200, { now: NOW, locale: "en" })).toBe("2 hours ago");
    expect(formatRelativeTime(NOW - 3 * 86_400, { now: NOW, locale: "en" })).toBe("3 days ago");
    expect(formatRelativeTime(NOW + 2 * 86_400, { now: NOW, locale: "en" })).toBe("in 2 days");
    expect(formatRelativeTime(NOW - 14 * 86_400, { now: NOW, locale: "en" })).toBe("2 weeks ago");
    expect(formatRelativeTime(NOW - 70 * 86_400, { now: NOW, locale: "en" })).toBe("2 months ago");
    expect(formatRelativeTime(NOW - 400 * 86_400, { now: NOW, locale: "en" })).toBe("last year");
  });

  it("uses auto numerals for single spans", () => {
    expect(formatRelativeTime(NOW - 86_400, { now: NOW, locale: "en" })).toBe("yesterday");
    expect(formatRelativeTime(NOW - 60, { now: NOW, locale: "en" })).toBe("1 minute ago");
  });

  it("rounds to the nearest whole unit", () => {
    // 95 minutes → 1.58 hours → rounds to 2 hours ago.
    expect(formatRelativeTime(NOW - 5_700, { now: NOW, locale: "en" })).toBe("2 hours ago");
  });
});

describe("formatDateTime", () => {
  it("formats with pinned locale/timezone options (caller-pinned = deterministic)", () => {
    expect(formatDateTime(NOW, "en", { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" }))
      .toBe("Nov 14, 2023, 10:13 PM");
  });
});

// ---------------------------------------------------------------------------
// Date-input conversion
// ---------------------------------------------------------------------------

describe("dayToUnixSeconds", () => {
  it("converts a date-input string to start/end of UTC day", () => {
    expect(dayToUnixSeconds("2026-01-02", "start")).toBe(1_767_312_000);
    expect(dayToUnixSeconds("2026-01-02", "end")).toBe(1_767_398_399);
  });

  it("returns null for empty, malformed, or rolled-over input", () => {
    expect(dayToUnixSeconds("", "start")).toBeNull();
    expect(dayToUnixSeconds("01/02/2026", "start")).toBeNull();
    expect(dayToUnixSeconds("2026-13-01", "start")).toBeNull();
    expect(dayToUnixSeconds("2026-02-30", "start")).toBeNull(); // engine would roll to Mar 2
  });
});
