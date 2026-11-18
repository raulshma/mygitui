/**
 * Tests for the settings export/import bundle
 * (`$lib/components/settings/settingsTransfer`): export collection across
 * the known key inventory (envelope + plain + per-root prefixes),
 * structural import validation (wrapper/version/data), key-level
 * parse-and-drop validation (one bad payload fails the whole import), and
 * the per-key replace apply round-trip.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { APP_SETTINGS_KEY, APP_SETTINGS_DEFAULTS } from "$lib/state/appSettings.svelte";
import { APPEARANCE_STORAGE_KEY } from "$lib/theme/appearance.svelte";
import { KEYBINDS_STORAGE_KEY } from "$lib/palette/keybinds";
import { RAIL_PINS_KEY } from "$lib/stores/railPins.svelte";
import { HEADER_PINS_KEY } from "$lib/stores/headerPins.svelte";
import { AUTOFETCH_STORAGE_KEY } from "$lib/stores/autofetch.svelte";
import { STORAGE_KEY as RECENT_REPOS_KEY } from "$lib/entry/recentRepos";
import { WORKSPACE_SESSION_KEY_PREFIX, emptyStoredSession } from "$lib/stores/workspaceSession.svelte";
import { FAVORITE_REPOS_KEY } from "$lib/stores/favoriteRepos.svelte";
import { COMMIT_RECENTS_PREFIX, COMMIT_WIDTH_PREFIX } from "$lib/commit/commitExtras.svelte";
import {
  applySettingsImport,
  collectSettingsExport,
  parseSettingsImport,
  SETTINGS_EXPORT_MARKER,
  validateSettingsImport,
  type SettingsExportFile,
} from "./settingsTransfer";

function envelope(data: unknown): string {
  return JSON.stringify({ v: 1, data });
}

beforeEach(() => {
  localStorage.clear();
});

/** Seeds a representative store set (all recognized key shapes). */
function seedStorage(): void {
  localStorage.setItem(APP_SETTINGS_KEY, envelope({ ...APP_SETTINGS_DEFAULTS, confirmDiscard: false }));
  localStorage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify({ style: "nova" }));
  localStorage.setItem(KEYBINDS_STORAGE_KEY, JSON.stringify({ "repo.refresh": "f6" }));
  localStorage.setItem(RAIL_PINS_KEY, envelope(["stashes", "reflog"]));
  localStorage.setItem(HEADER_PINS_KEY, envelope(["fetch", "push"]));
  localStorage.setItem(AUTOFETCH_STORAGE_KEY, envelope({ "/a/repo": 5 }));
  localStorage.setItem(
    RECENT_REPOS_KEY,
    JSON.stringify([{ path: "/a/repo", pinned: true, lastOpened: 1 }]),
  );
  localStorage.setItem(
    `${WORKSPACE_SESSION_KEY_PREFIX}/a/repo`,
    envelope(emptyStoredSession()),
  );
  localStorage.setItem(FAVORITE_REPOS_KEY, envelope(["/a/repo"]));
  localStorage.setItem(`${COMMIT_RECENTS_PREFIX}/a/repo`, envelope(["msg one"]));
  localStorage.setItem(`${COMMIT_WIDTH_PREFIX}/a/repo`, envelope(72));
}

describe("collectSettingsExport", () => {
  it("bundles the known keys unwrapped, prefix families included", () => {
    seedStorage();
    const file = collectSettingsExport(localStorage);
    expect(file[SETTINGS_EXPORT_MARKER]).toBe(true);
    expect(file.version).toBe(1);
    expect(file.exportedAt).not.toBe("");
    // Envelope stores export their envelope DATA…
    expect(file.data[APP_SETTINGS_KEY]).toMatchObject({ confirmDiscard: false });
    expect(file.data[RAIL_PINS_KEY]).toEqual(["stashes", "reflog"]);
    expect(file.data[`${WORKSPACE_SESSION_KEY_PREFIX}/a/repo`]).toBeDefined();
    expect(file.data[`${COMMIT_RECENTS_PREFIX}/a/repo`]).toEqual(["msg one"]);
    expect(file.data[`${COMMIT_WIDTH_PREFIX}/a/repo`]).toBe(72);
    // …plain stores export the value verbatim.
    expect(file.data[APPEARANCE_STORAGE_KEY]).toEqual({ style: "nova" });
    expect(file.data[KEYBINDS_STORAGE_KEY]).toEqual({ "repo.refresh": "f6" });
    expect(file.data[RECENT_REPOS_KEY]).toHaveLength(1);
  });

  it("skips absent keys and non-bundle keys", () => {
    localStorage.setItem(APP_SETTINGS_KEY, envelope(APP_SETTINGS_DEFAULTS));
    localStorage.setItem("mygitui.palette.recents", JSON.stringify(["x"]));
    const file = collectSettingsExport(localStorage);
    expect(Object.keys(file.data)).toEqual([APP_SETTINGS_KEY]);
  });
});

describe("parseSettingsImport", () => {
  it("rejects non-JSON, foreign files and bad versions", () => {
    expect(parseSettingsImport("nope").ok).toBe(false);
    expect(parseSettingsImport("{}").ok).toBe(false);
    expect(
      parseSettingsImport(JSON.stringify({ [SETTINGS_EXPORT_MARKER]: true, version: 99, data: {} }))
        .ok,
    ).toBe(false);
  });

  it("accepts a well-formed bundle", () => {
    const exportFile = collectSettingsExport(localStorage);
    const result = parseSettingsImport(JSON.stringify(exportFile));
    expect(result.ok).toBe(true);
  });
});

describe("validateSettingsImport", () => {
  const base: SettingsExportFile = {
    [SETTINGS_EXPORT_MARKER]: true,
    version: 1,
    exportedAt: "now",
    data: {},
  };

  it("accepts valid payloads and ignores unrecognized keys", () => {
    const file: SettingsExportFile = {
      ...base,
      data: {
        [RAIL_PINS_KEY]: ["reflog"],
        "mygitui.unknown.future": { whatever: true },
      },
    };
    const result = validateSettingsImport(file);
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.keys).toEqual([RAIL_PINS_KEY]);
  });

  it("fails the whole import when one recognized payload is invalid", () => {
    const file: SettingsExportFile = {
      ...base,
      data: {
        [RAIL_PINS_KEY]: ["reflog"],
        [AUTOFETCH_STORAGE_KEY]: { "/a": "five" }, // minutes must be numbers
      },
    };
    const result = validateSettingsImport(file);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain(AUTOFETCH_STORAGE_KEY);
  });

  it("rejects malformed keybind combos, header pins, and workspace sessions", () => {
    expect(
      validateSettingsImport({
        ...base,
        data: { [KEYBINDS_STORAGE_KEY]: { "repo.refresh": "not a combo++" } },
      }).ok,
    ).toBe(false);
    expect(
      validateSettingsImport({ ...base, data: { [HEADER_PINS_KEY]: ["fetch", "deploy"] } }).ok,
    ).toBe(false);
    expect(
      validateSettingsImport({
        ...base,
        data: { [`${WORKSPACE_SESSION_KEY_PREFIX}/a`]: { centerKind: "bogus" } },
      }).ok,
    ).toBe(false);
    expect(
      validateSettingsImport({ ...base, data: { [FAVORITE_REPOS_KEY]: "nope" } }).ok,
    ).toBe(false);
  });
});

describe("applySettingsImport", () => {
  it("replaces keys per-key, re-wrapping envelope stores", () => {
    seedStorage();
    const exportFile = collectSettingsExport(localStorage);
    // Simulate a machine with different data, then apply the bundle.
    localStorage.clear();
    localStorage.setItem(APP_SETTINGS_KEY, envelope({ ...APP_SETTINGS_DEFAULTS }));
    const applied = applySettingsImport(localStorage, exportFile);
    expect(applied).toContain(APP_SETTINGS_KEY);
    expect(applied).toContain(RAIL_PINS_KEY);
    expect(JSON.parse(localStorage.getItem(APP_SETTINGS_KEY)!)).toEqual({
      v: 1,
      data: { ...APP_SETTINGS_DEFAULTS, confirmDiscard: false },
    });
    expect(JSON.parse(localStorage.getItem(RAIL_PINS_KEY)!)).toEqual({
      v: 1,
      data: ["stashes", "reflog"],
    });
    // Plain stores stay plain.
    expect(JSON.parse(localStorage.getItem(APPEARANCE_STORAGE_KEY)!)).toEqual({
      style: "nova",
    });
    // Prefix families come back under their full per-root keys.
    expect(
      JSON.parse(localStorage.getItem(`${COMMIT_RECENTS_PREFIX}/a/repo`)!),
    ).toEqual({ v: 1, data: ["msg one"] });
  });

  it("round-trips a collect → apply cycle losslessly", () => {
    seedStorage();
    const before = collectSettingsExport(localStorage);
    localStorage.clear();
    applySettingsImport(localStorage, before);
    const after = collectSettingsExport(localStorage);
    expect(after.data).toEqual(before.data);
  });
});
