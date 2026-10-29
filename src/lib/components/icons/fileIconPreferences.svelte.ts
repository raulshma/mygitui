/**
 * Global file-icon theme preference, shared by every mounted source-control
 * panel and persisted for the next app launch.
 */
import type { FileIconTheme } from "./fileIcons";

const STORAGE_KEY = "mygitui.file-icons.theme";
const DEFAULT_THEME: FileIconTheme = "colorful";
const VALID_THEMES = new Set<FileIconTheme>([
  "colorful",
  "material",
  "minimal",
]);

function readPreference(): FileIconTheme {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    return value && VALID_THEMES.has(value as FileIconTheme)
      ? (value as FileIconTheme)
      : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

let theme = $state<FileIconTheme>(readPreference());

function setTheme(value: FileIconTheme): void {
  theme = value;
  try {
    localStorage.setItem(STORAGE_KEY, value);
  } catch {
    // Keep the in-memory preference if storage is unavailable.
  }
}

export const fileIconPreferences = {
  get theme(): FileIconTheme {
    return theme;
  },
  setTheme,
};
