import { beforeEach, describe, expect, it } from "vitest";
import { getLocale, setLocale } from "$lib/i18n";
import {
  APP_SETTINGS_DEFAULTS,
  APP_SETTINGS_KEY,
  AppSettingsStore,
  parseAppSettings,
  UI_SCALE_MAX,
  UI_SCALE_MIN,
} from "./appSettings.svelte";

/** A storage stub that never throws (write failures swallowed). */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k) => map.get(k) ?? null,
    key: (i) => [...map.keys()][i] ?? null,
    removeItem: (k) => map.delete(k),
    setItem: (k, v) => void map.set(k, v),
  };
}

describe("parseAppSettings", () => {
  it("re-defaults malformed payloads", () => {
    expect(parseAppSettings(null)).toEqual(APP_SETTINGS_DEFAULTS);
    expect(parseAppSettings("nope")).toEqual(APP_SETTINGS_DEFAULTS);
    expect(parseAppSettings({ checkpointsEnabled: "yes" })).toEqual(APP_SETTINGS_DEFAULTS);
  });

  it("accepts valid fields and drops unknown ones", () => {
    const out = parseAppSettings({
      checkpointsEnabled: false,
      terminalFontSize: 15.6,
      defaultAutofetchMinutes: 10,
      confirmDiscard: false,
      openFavoritesOnLaunch: true,
      useCommitTemplate: false,
      uiScalePercent: 110,
      aiMonthlyTokenBudget: 2000000,
      aiBudgetResetDay: 15,
      locale: "en",
      forgeSnapshotSeconds: 90,
      forgeFailureToasts: false,
      forgeActivityToasts: true,
      rogue: true,
    });
    expect(out).toEqual({
      checkpointsEnabled: false,
      terminalFontSize: 16,
      defaultAutofetchMinutes: 10,
      confirmDiscard: false,
      openFavoritesOnLaunch: true,
      useCommitTemplate: false,
      uiScalePercent: 110,
      aiMonthlyTokenBudget: 2000000,
      aiBudgetResetDay: 15,
      locale: "en",
      forgeSnapshotSeconds: 90,
      forgeFailureToasts: false,
      forgeActivityToasts: true,
    });
  });

  it("lifts a sub-floor GitHub snapshot cadence to the 30s floor (0 stays off)", () => {
    expect(parseAppSettings({ forgeSnapshotSeconds: 10 }).forgeSnapshotSeconds).toBe(30);
    expect(parseAppSettings({ forgeSnapshotSeconds: 0 }).forgeSnapshotSeconds).toBe(0);
    expect(parseAppSettings({ forgeSnapshotSeconds: 99999 }).forgeSnapshotSeconds).toBe(3600);
    expect(parseAppSettings({}).forgeSnapshotSeconds).toBe(60);
  });

  it("clamps terminal font size and autofetch minutes", () => {
    expect(parseAppSettings({ terminalFontSize: 2 }).terminalFontSize).toBe(9);
    expect(parseAppSettings({ terminalFontSize: 90 }).terminalFontSize).toBe(28);
    expect(parseAppSettings({ defaultAutofetchMinutes: -5 }).defaultAutofetchMinutes).toBe(0);
    expect(parseAppSettings({ defaultAutofetchMinutes: 99999 }).defaultAutofetchMinutes).toBe(720);
  });

  it("clamps and snaps the ui scale onto the slider grid", () => {
    expect(parseAppSettings({ uiScalePercent: 105 }).uiScalePercent).toBe(105);
    // Off-grid values snap to the nearest 5-step.
    expect(parseAppSettings({ uiScalePercent: 108 }).uiScalePercent).toBe(110);
    expect(parseAppSettings({ uiScalePercent: 3 }).uiScalePercent).toBe(UI_SCALE_MIN);
    expect(parseAppSettings({ uiScalePercent: 500 }).uiScalePercent).toBe(UI_SCALE_MAX);
    expect(parseAppSettings({ uiScalePercent: "x" }).uiScalePercent).toBe(100);
  });

  it("keeps only registered locales (i18n pilot: en)", () => {
    expect(parseAppSettings({ locale: "en" }).locale).toBe("en");
    // Unknown locales re-default until a catalog registers them.
    expect(parseAppSettings({ locale: "de" }).locale).toBe("en");
    expect(parseAppSettings({ locale: 42 }).locale).toBe("en");
  });
});

describe("AppSettingsStore", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("starts from defaults with empty storage", () => {
    const store = new AppSettingsStore(memoryStorage());
    expect(store.value).toEqual(APP_SETTINGS_DEFAULTS);
  });

  it("persists an update through the versioned envelope and reloads", () => {
    const storage = memoryStorage();
    const store = new AppSettingsStore(storage);
    store.update({ checkpointsEnabled: false, terminalFontSize: 16 });
    expect(store.checkpointsEnabled).toBe(false);

    const raw = storage.getItem(APP_SETTINGS_KEY);
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw as string)).toMatchObject({ v: 1 });

    const reloaded = new AppSettingsStore(storage);
    expect(reloaded.checkpointsEnabled).toBe(false);
    expect(reloaded.terminalFontSize).toBe(16);
  });

  it("round-trips the locale and syncs it into the i18n module", () => {
    const storage = memoryStorage();
    let store = new AppSettingsStore(storage);
    expect(store.value.locale).toBe("en");
    expect(getLocale()).toBe("en");

    store.update({ locale: "en" });
    expect(JSON.parse(storage.getItem(APP_SETTINGS_KEY)!)).toMatchObject({
      v: 1,
      data: { locale: "en" },
    });

    // A fresh store (launch restore) reads the persisted locale and feeds
    // t()'s active locale through setLocale.
    setLocale("en");
    store = new AppSettingsStore(storage);
    expect(store.value.locale).toBe("en");
    expect(getLocale()).toBe("en");
  });

  it("re-defaults on a malformed envelope", () => {
    const storage = memoryStorage();
    storage.setItem(APP_SETTINGS_KEY, "{not json");
    const store = new AppSettingsStore(storage);
    expect(store.value).toEqual(APP_SETTINGS_DEFAULTS);
  });

  it("reset restores defaults", () => {
    const storage = memoryStorage();
    const store = new AppSettingsStore(storage);
    store.update({ confirmDiscard: false });
    store.reset();
    const reloaded = new AppSettingsStore(storage);
    expect(reloaded.value).toEqual(APP_SETTINGS_DEFAULTS);
  });
});
