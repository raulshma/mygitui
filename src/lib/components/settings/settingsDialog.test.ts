/**
 * SettingsDialog tests: section navigation, cross-section search, live
 * store-backed writes (the injected stores persist to the real jsdom
 * localStorage, so the envelope payloads are asserted directly), and the
 * Advanced inspector + diagnostics + confirmed danger reset. Choice settings
 * render as segmented `role="radio"` pills; switches keep their
 * aria-labelledby labels and whole rows are clickable. The `$lib/toast`
 * module is mocked like CustomAppsSection's suite; nothing invokes Tauri
 * IPC at render time.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { toast } from "$lib/toast";
import SettingsDialog from "./SettingsDialog.svelte";
import { COMMANDS } from "$lib/palette/commands";
import { DEFAULT_BINDINGS } from "$lib/palette/keybinds";
import {
  AppSettingsStore,
  APP_SETTINGS_DEFAULTS,
  APP_SETTINGS_KEY,
} from "$lib/state/appSettings.svelte";
import { CustomAppsStore, CUSTOM_APPS_KEY } from "$lib/openwith/customApps.svelte";
import { autofetch, AUTOFETCH_STORAGE_KEY } from "$lib/stores/autofetch.svelte";
import { updater, UPDATER_STORAGE_KEY } from "$lib/stores/updater.svelte";
import {
  terminalThemePref,
  TERMINAL_THEME_STORAGE_KEY,
} from "$lib/terminal/terminalThemes.svelte";
import { editorPrefs } from "$lib/components/files/editorPrefs.svelte";
import { fileTreePrefs } from "$lib/components/panels/fileTreePrefs.svelte";
import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";

function envelopeData(key: string): unknown {
  const raw = localStorage.getItem(key);
  expect(raw).not.toBeNull();
  return JSON.parse(raw as string);
}

/** bits-ui Switch renders a `role="switch"` BUTTON — state lives in aria. */
function switchState(el: Element): boolean {
  return el.getAttribute("aria-checked") === "true";
}

describe("SettingsDialog", () => {
  let settings: AppSettingsStore;
  let apps: CustomAppsStore;

  /** Clears every persisted key + resets the singleton stores' state. */
  function resetPersistedState(): void {
    const doomed: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key !== null) doomed.push(key);
    }
    for (const key of doomed) localStorage.removeItem(key);
    // Singleton stores keep in-memory state across tests — set known values.
    updater.setAutoCheck(true);
    updater.setAutoDownload(true);
    editorPrefs.setWrap(false);
    editorPrefs.setMdView("split");
    fileTreePrefs.setShowIgnored(false);
    fileIconPreferences.setTheme("colorful");
    terminalThemePref.set("system");
    for (const root of Object.keys(autofetch.config)) autofetch.setInterval(root, 0);
  }

  beforeEach(() => {
    vi.clearAllMocks();
    resetPersistedState();
    // Fresh stores over the real jsdom localStorage, so envelope writes can
    // be asserted directly.
    settings = new AppSettingsStore(localStorage);
    apps = new CustomAppsStore(localStorage);
  });

  afterEach(() => {
    cleanup();
  });

  it("opens on General and switches content when the nav is used", async () => {
    render(SettingsDialog, { open: true, settings, apps });

    // General is the landing section: theme select + About.
    expect(screen.getByLabelText("Theme")).toBeTruthy();
    expect(screen.getByRole("button", { name: "About" })).toBeTruthy();
    expect(screen.queryByLabelText("Checkpoints for git changes")).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Git" }));
    expect(screen.getByLabelText("Checkpoints for git changes")).toBeTruthy();
    expect(screen.queryByLabelText("Theme")).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Editor & Diffs" }));
    expect(screen.getByLabelText("Diff layout")).toBeTruthy();
  });

  it("writes theme preference through the owning store (persisted)", async () => {
    render(SettingsDialog, { open: true, settings, apps });

    expect(screen.getByRole("radio", { name: "Match system" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    await fireEvent.click(screen.getByRole("radio", { name: "Dark" }));

    expect(localStorage.getItem("mygitui.theme")).toBe("dark");
    expect(screen.getByRole("radio", { name: "Dark" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("radio", { name: "Match system" }).getAttribute("aria-checked")).toBe(
      "false",
    );
  });

  it("toggles the git switches and persists the app-settings envelope", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Git" }));

    const checkpoints = screen.getByLabelText("Checkpoints for git changes");
    expect(switchState(checkpoints)).toBe(true); // APP_SETTINGS_DEFAULTS
    await fireEvent.click(checkpoints);

    const confirmDiscard = screen.getByLabelText("Confirm before discard");
    expect(switchState(confirmDiscard)).toBe(true);
    await fireEvent.click(confirmDiscard);

    // Both toggles are visible in the store and its persisted envelope.
    expect(settings.value.checkpointsEnabled).toBe(false);
    expect(settings.value.confirmDiscard).toBe(false);
    const data = envelopeData(APP_SETTINGS_KEY) as { data: Record<string, unknown> };
    expect(data.data.checkpointsEnabled).toBe(false);
    expect(data.data.confirmDiscard).toBe(false);
  });

  it("edits the default auto-fetch cadence and clears per-repo overrides", async () => {
    autofetch.setInterval("/repos/demo", 30);

    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Git" }));

    // Per-repo override listed with a clear button.
    expect(screen.getByText("/repos/demo")).toBeTruthy();
    expect(screen.getByText("30 min")).toBeTruthy();
    await fireEvent.click(
      screen.getByRole("button", { name: "Clear auto-fetch override for /repos/demo" }),
    );
    expect(autofetch.getInterval("/repos/demo")).toBe(0);
    expect(JSON.parse(localStorage.getItem(AUTOFETCH_STORAGE_KEY) as string)).toEqual({});

    // Default cadence: typed minutes land (clamped) in the store + envelope.
    const input = screen.getByLabelText(
      "Default auto-fetch interval in minutes",
    ) as HTMLInputElement;
    await fireEvent.change(input, { target: { value: "45" } });
    expect(settings.value.defaultAutofetchMinutes).toBe(45);
    const data = envelopeData(APP_SETTINGS_KEY) as { data: Record<string, unknown> };
    expect(data.data.defaultAutofetchMinutes).toBe(45);
  });

  it("writes the diff/editor/file-tree prefs through their own stores", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Editor & Diffs" }));

    // Diff wrap defaults ON; turning it off persists `false`.
    const diffWrap = screen.getByLabelText("Wrap long diff lines");
    expect(switchState(diffWrap)).toBe(true);
    await fireEvent.click(diffWrap);
    expect(localStorage.getItem("mygitui.diff.wrap")).toBe("false");

    // Diff layout segmented pill persists JSON ("split" | "unified").
    await fireEvent.click(screen.getByRole("radio", { name: "Unified" }));
    expect(localStorage.getItem("mygitui.diff.mode")).toBe('"unified"');

    // Editor wrap defaults OFF (unlike the diff viewer); toggling persists "1".
    const editorWrap = screen.getByLabelText("Wrap lines in the editor");
    expect(switchState(editorWrap)).toBe(false);
    await fireEvent.click(editorWrap);
    expect(localStorage.getItem("mygitui.files.wrap")).toBe("1");

    await fireEvent.click(screen.getByRole("radio", { name: "Preview" }));
    expect(localStorage.getItem("mygitui.files.md-view")).toBe("preview");
    expect(editorPrefs.mdView).toBe("preview");

    // File-tree "show ignored" toggle writes through the prefs store.
    const showIgnored = screen.getByLabelText("Show ignored files");
    expect(switchState(showIgnored)).toBe(false);
    await fireEvent.click(showIgnored);
    expect(fileTreePrefs.showIgnored).toBe(true);
    expect(localStorage.getItem("mygitui.files.show-ignored")).toBe("1");
  });

  it("toggles a setting by clicking anywhere on its row", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Editor & Diffs" }));

    expect(editorPrefs.wrap).toBe(false);
    await fireEvent.click(screen.getByText("Wrap lines in the editor"));
    expect(editorPrefs.wrap).toBe(true);
    expect(localStorage.getItem("mygitui.files.wrap")).toBe("1");
  });

  it("changes the terminal theme and font size (font clamped to bounds)", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Terminal" }));

    await fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    expect(terminalThemePref.preference).toBe("dark");
    expect(localStorage.getItem(TERMINAL_THEME_STORAGE_KEY)).toBe("dark");

    const font = screen.getByLabelText(
      "Terminal font size in pixels",
    ) as HTMLInputElement;
    await fireEvent.change(font, { target: { value: "15" } });
    expect(settings.value.terminalFontSize).toBe(15);
    // Out-of-range values clamp to the shared bounds.
    await fireEvent.change(font, { target: { value: "999" } });
    expect(settings.value.terminalFontSize).toBe(28);

    // The companion slider carries its own label and mirrors the value.
    expect(screen.getByLabelText("Terminal font size in pixels slider")).toBeTruthy();
  });

  it("searches across sections and returns to the section view on clear", async () => {
    render(SettingsDialog, { open: true, settings, apps });

    const input = screen.getByLabelText("Search settings") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "wrap" } });

    // Only matching rows show, grouped under their section heading.
    expect(screen.getByText("Wrap long diff lines")).toBeTruthy();
    expect(screen.getByText("Wrap lines in the editor")).toBeTruthy();
    expect(screen.getByRole("region", { name: "Editor & Diffs" })).toBeTruthy();
    expect(screen.queryByText("Checkpoints for git changes")).toBeNull();
    expect(screen.queryByRole("radio", { name: "Match system" })).toBeNull();

    // Clearing leaves search mode: General's content is back.
    await fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    expect(screen.queryByText("Wrap long diff lines")).toBeNull();
    expect(screen.getByRole("radio", { name: "Match system" })).toBeTruthy();
  });

  it("shows an empty state when the search matches nothing", async () => {
    render(SettingsDialog, { open: true, settings, apps });

    await fireEvent.input(screen.getByLabelText("Search settings"), {
      target: { value: "zzz-nothing-matches" },
    });
    expect(screen.getByText(/Nothing matches/)).toBeTruthy();
    expect(screen.getByText(/Try “wrap”/)).toBeTruthy();
  });

  it("embeds the custom open-with apps manager", async () => {
    apps.add({ name: "Neovim (kitty)", command: "kitty {path}" });

    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Open With" }));

    expect(screen.getByText("Neovim (kitty)")).toBeTruthy();
    expect(screen.getByText("kitty {path}")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add custom app" })).toBeTruthy();
    expect(localStorage.getItem(CUSTOM_APPS_KEY)).not.toBeNull();
  });

  it("links to the AI settings dialog without duplicating it", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "AI" }));

    const open = screen.getByRole("button", { name: "AI settings…" });
    expect(open).toBeTruthy();
    // The typed UI bus carries the request (App.svelte owns the dialog).
    const listener = vi.fn();
    document.addEventListener("open-ai-settings", listener);
    await fireEvent.click(open);
    document.removeEventListener("open-ai-settings", listener);
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("reflects and persists the updater switches", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Updates" }));

    expect(screen.getByText("Never checked")).toBeTruthy();
    const autoCheck = screen.getByLabelText("Check for updates automatically");
    expect(switchState(autoCheck)).toBe(true);
    await fireEvent.click(autoCheck);

    expect(updater.autoCheck).toBe(false);
    const data = envelopeData(UPDATER_STORAGE_KEY) as { data: Record<string, unknown> };
    expect(data.data.autoCheck).toBe(false);

    const checkButton = screen.getByRole("button", { name: "Check for updates" });
    expect(checkButton).toBeTruthy();
  });

  it("inspects seeded mygitui.* keys and copies diagnostics JSON", async () => {
    localStorage.setItem("mygitui.test.seed", JSON.stringify({ a: 1 }));

    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(globalThis.navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Advanced" }));

    // The inspector lists the seeded key (rescan runs on section entry).
    await waitFor(() => expect(screen.getByText("mygitui.test.seed")).toBeTruthy());

    await fireEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const payload = JSON.parse(writeText.mock.calls[0][0] as string);
    expect(payload["mygitui.test.seed"]).toEqual({ a: 1 });
    expect(toast).toHaveBeenCalledWith("App settings copied as JSON", { kind: "success" });
  });

  it("resets keybind overrides through the keybinds store", async () => {
    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Advanced" }));

    await fireEvent.click(screen.getByRole("button", { name: "Reset all keybinds" }));
    expect(toast).toHaveBeenCalledWith("Keyboard shortcuts reset to defaults", {
      kind: "success",
    });
    // resetBindings persists an empty override map (not a removed key).
    expect(localStorage.getItem("mygitui.keybinds")).toBe("{}");
  });

  it("resets all app settings after the confirm dialog", async () => {
    localStorage.setItem("mygitui.test.seed", "gone-soon");
    localStorage.setItem("mygitui.diff.wrap", "false");

    render(SettingsDialog, { open: true, settings, apps });
    await fireEvent.click(screen.getByRole("button", { name: "Advanced" }));

    await fireEvent.click(screen.getByRole("button", { name: "Reset settings…" }));

    // ConfirmDialog gates the destructive action.
    expect(screen.getByText("Reset all app settings?")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Reset settings" }));

    // Every seeded mygitui.* key is gone; the global settings re-default
    // (re-persisted envelope) and the copy mentions the relaunch semantics.
    expect(localStorage.getItem("mygitui.test.seed")).toBeNull();
    expect(localStorage.getItem("mygitui.diff.wrap")).toBeNull();
    const data = envelopeData(APP_SETTINGS_KEY) as { data: Record<string, unknown> };
    expect(data.data).toMatchObject(APP_SETTINGS_DEFAULTS);
    expect(toast).toHaveBeenCalledWith("App settings reset to defaults", { kind: "info" });
  });

  it("does not render anything while closed", () => {
    render(SettingsDialog, { open: false, settings, apps });
    expect(screen.queryByText("Settings")).toBeNull();
  });
});

describe("settings registry wiring", () => {
  it("registers app.settings near the other app.* commands with the Ctrl+, keybind", () => {
    const command = COMMANDS.find((entry) => entry.id === "app.settings");
    expect(command).toBeDefined();
    expect(command?.section).toBe("App");
    expect(command?.shortcut).toBe("ctrl+comma");
    expect(DEFAULT_BINDINGS["app.settings"]).toBe("ctrl+comma");
  });

  it("opens the dialog through the typed UI bus when the command runs", () => {
    const command = COMMANDS.find((entry) => entry.id === "app.settings");
    expect(command).toBeDefined();
    const listener = vi.fn();
    document.addEventListener("open-settings", listener);
    try {
      command?.run({ repoId: null, root: null });
    } finally {
      document.removeEventListener("open-settings", listener);
    }
    expect(listener).toHaveBeenCalledTimes(1);
  });
});
