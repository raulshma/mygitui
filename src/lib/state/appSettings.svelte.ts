/**
 * Global app settings (Svelte 5 runes).
 *
 * The coarse switches the Settings dialog manages that no feature store
 * owns by itself — checkpointing before destructive git mutations, the
 * terminal's font size, the default auto-fetch cadence for newly opened
 * repos. Persisted under `mygitui.settings` with the versioned envelope
 * (`$lib/state/persist`), parse-and-drop like every other store: a
 * malformed payload re-defaults, never throws.
 *
 * The exported {@link appSettings} singleton is the single read/write
 * surface; components bind to it, tests inject `localStorage`.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";
import { setLocale, type Locale } from "$lib/i18n";

/** localStorage key for the global settings envelope. */
export const APP_SETTINGS_KEY = "mygitui.settings";

/** What persists across launches. */
export interface AppSettingsData {
  /**
   * Create a restorable checkpoint before destructive working-copy
   * mutations (discard, clean). Off = mutate directly, nothing to undo
   * from the checkpoints panel.
   */
  checkpointsEnabled: boolean;
  /** Terminal font size in px (clamped to a readable range). */
  terminalFontSize: number;
  /**
   * Auto-fetch cadence (minutes) applied when a repo has no per-repo
   * override; `0` disables auto-fetch by default.
   */
  defaultAutofetchMinutes: number;
  /**
   * Ask for confirmation before discarding working-copy changes.
   * (The per-action confirm dialog already exists; this is the master
   * switch for users who find it noisy.)
   */
  confirmDiscard: boolean;
  /**
   * Open up to five favorite repositories (see
   * `$lib/stores/favoriteRepos.svelte`) at launch, in favorites order,
   * when they are not already part of the restored session.
   */
  openFavoritesOnLaunch: boolean;
  /**
   * Prefill the commit message box from the repo's `.gitmessage` template
   * (config `commit.template` is not readable through the current backend,
   * so the workdir-root file is the template source) when the box opens
   * empty.
   */
  useCommitTemplate: boolean;
  /**
   * Root font-size multiplier in percent (85–125, step 5, 100 = default).
   * Applied as `html { font-size: calc(100% * m/100) }` by the appearance
   * store's managed style element.
   */
  uiScalePercent: number;
  /**
   * Soft monthly AI token budget (item 58). 0 = no budget. Never blocks a
   * run — the status bar's usage chip only turns into a warning at 100%.
   * Aggregation covers the LOCAL visible job history (the 200-record
   * activity ring), so the percentage is a lower bound, not a bill.
   */
  aiMonthlyTokenBudget: number;
  /**
   * Day of month (1–31; clamped to each month's real length) the AI
   * budget window resets on. Default 1st.
   */
  aiBudgetResetDay: number;
  /**
   * Interface locale (i18n pilot, item 69). "en" is the only registered
   * locale; unknown values re-default on parse. The store syncs the value
   * into `$lib/i18n`'s `setLocale` on load and on every update, so `t()`
   * always reads the persisted choice.
   */
  locale: Locale;
  /**
   * GitHub snapshot cadence (E1, plan §9.3) for the ACTIVE repo, in seconds.
   * 0 disables background polling (manual refresh + popovers still work).
   * Clamped to a 30 s floor (`FORGE_SNAPSHOT_SECONDS_MIN`) — the plan's
   * floor for user-configurable cadences; inactive-open repos always poll
   * at the fixed 5-minute rate.
   */
  forgeSnapshotSeconds: number;
  /**
   * Toast when a repo's GitHub state turns failing (checks/run failed).
   * Default ON — failures are the one thing the footer may interrupt for
   * (§9.2); everything else is opt-in.
   */
  forgeFailureToasts: boolean;
  /**
   * Toast on GitHub activity transitions (run started/finished, PR opened).
   * Default OFF (opt-in; §9.2 toasts default-on for failures only).
   */
  forgeActivityToasts: boolean;
  /**
   * Master switch for every GitHub feature. Off = no `gh` process is
   * spawned anywhere (the frontend IPC gate rejects before invoking and the
   * Rust runner refuses to launch gh as a second line of defense).
   */
  githubEnabled: boolean;
  /** Pull requests, their checks, diffs, review threads and mutations. */
  githubPullRequests: boolean;
  /** GitHub Actions runs, logs, reruns and workflow dispatch. */
  githubActions: boolean;
  /** GitHub Issues list/detail/comments/activity polling. */
  githubIssues: boolean;
  /** GitHub notification threads. */
  githubNotifications: boolean;
  /** Footer PR/checks/run chips and their background snapshot polling. */
  githubStatusChips: boolean;
}

export const APP_SETTINGS_DEFAULTS: AppSettingsData = {
  checkpointsEnabled: true,
  terminalFontSize: 13,
  defaultAutofetchMinutes: 0,
  confirmDiscard: true,
  openFavoritesOnLaunch: false,
  useCommitTemplate: true,
  uiScalePercent: 100,
  aiMonthlyTokenBudget: 0,
  aiBudgetResetDay: 1,
  locale: "en",
  forgeSnapshotSeconds: 60,
  forgeFailureToasts: true,
  forgeActivityToasts: false,
  githubEnabled: true,
  githubPullRequests: true,
  githubActions: true,
  githubIssues: true,
  githubNotifications: true,
  githubStatusChips: true,
};

/** Terminal font size bounds shared by the setting and the terminal store. */
export const TERMINAL_FONT_MIN = 9;
export const TERMINAL_FONT_MAX = 28;

/** Auto-fetch cadence bounds (minutes; 0 = off). */
export const AUTOFETCH_MINUTES_MIN = 0;
export const AUTOFETCH_MINUTES_MAX = 720;

/** Root font-size multiplier bounds (percent). */
export const UI_SCALE_MIN = 85;
export const UI_SCALE_MAX = 125;
/** Slider step; parsing snaps stored values onto the step grid. */
export const UI_SCALE_STEP = 5;

/** GitHub snapshot cadence bounds (seconds; 0 = off). */
export const FORGE_SNAPSHOT_SECONDS_MIN = 30;
export const FORGE_SNAPSHOT_SECONDS_MAX = 3600;
/** Fixed cadence of inactive-open repos' snapshot lanes (plan §9.3). */
export const FORGE_INACTIVE_SNAPSHOT_MS = 5 * 60_000;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Parse-and-drop loader: unknown/malformed fields re-default. */
export function parseAppSettings(data: unknown): AppSettingsData {
  if (typeof data !== "object" || data === null) return { ...APP_SETTINGS_DEFAULTS };
  const raw = data as Partial<AppSettingsData>;
  const out: AppSettingsData = { ...APP_SETTINGS_DEFAULTS };
  if (typeof raw.checkpointsEnabled === "boolean") out.checkpointsEnabled = raw.checkpointsEnabled;
  if (typeof raw.terminalFontSize === "number" && Number.isFinite(raw.terminalFontSize)) {
    out.terminalFontSize = Math.round(
      clamp(raw.terminalFontSize, TERMINAL_FONT_MIN, TERMINAL_FONT_MAX),
    );
  }
  if (
    typeof raw.defaultAutofetchMinutes === "number" &&
    Number.isFinite(raw.defaultAutofetchMinutes)
  ) {
    out.defaultAutofetchMinutes = Math.round(
      clamp(raw.defaultAutofetchMinutes, AUTOFETCH_MINUTES_MIN, AUTOFETCH_MINUTES_MAX),
    );
  }
  if (typeof raw.confirmDiscard === "boolean") out.confirmDiscard = raw.confirmDiscard;
  if (typeof raw.openFavoritesOnLaunch === "boolean") {
    out.openFavoritesOnLaunch = raw.openFavoritesOnLaunch;
  }
  if (typeof raw.useCommitTemplate === "boolean") {
    out.useCommitTemplate = raw.useCommitTemplate;
  }
  if (typeof raw.uiScalePercent === "number" && Number.isFinite(raw.uiScalePercent)) {
    // Snap onto the 5-point slider grid (85, 90, …, 125) so a hand-edited or
    // imported value cannot leave the slider stuck between steps.
    const snapped = Math.round(raw.uiScalePercent / UI_SCALE_STEP) * UI_SCALE_STEP;
    out.uiScalePercent = Math.round(clamp(snapped, UI_SCALE_MIN, UI_SCALE_MAX));
  }
  if (typeof raw.aiMonthlyTokenBudget === "number" && Number.isFinite(raw.aiMonthlyTokenBudget)) {
    // Non-negative whole tokens; 0 stays "no budget".
    out.aiMonthlyTokenBudget = Math.max(0, Math.round(raw.aiMonthlyTokenBudget));
  }
  if (typeof raw.aiBudgetResetDay === "number" && Number.isFinite(raw.aiBudgetResetDay)) {
    out.aiBudgetResetDay = Math.round(clamp(raw.aiBudgetResetDay, 1, 31));
  }
  // Only registered locales pass; anything else re-defaults (the i18n
  // catalog itself falls back to English, then the key).
  if (raw.locale === "en") out.locale = raw.locale;
  if (
    typeof raw.forgeSnapshotSeconds === "number" &&
    Number.isFinite(raw.forgeSnapshotSeconds)
  ) {
    out.forgeSnapshotSeconds = Math.round(
      clamp(raw.forgeSnapshotSeconds, 0, FORGE_SNAPSHOT_SECONDS_MAX),
    );
    // A non-zero value below the floor is lifted to the floor (the plan's
    // 30 s minimum for user-configurable cadences); 0 stays "off".
    if (
      out.forgeSnapshotSeconds > 0 &&
      out.forgeSnapshotSeconds < FORGE_SNAPSHOT_SECONDS_MIN
    ) {
      out.forgeSnapshotSeconds = FORGE_SNAPSHOT_SECONDS_MIN;
    }
  }
  if (typeof raw.forgeFailureToasts === "boolean") {
    out.forgeFailureToasts = raw.forgeFailureToasts;
  }
  if (typeof raw.forgeActivityToasts === "boolean") {
    out.forgeActivityToasts = raw.forgeActivityToasts;
  }
  if (typeof raw.githubEnabled === "boolean") out.githubEnabled = raw.githubEnabled;
  if (typeof raw.githubPullRequests === "boolean") {
    out.githubPullRequests = raw.githubPullRequests;
  }
  if (typeof raw.githubActions === "boolean") out.githubActions = raw.githubActions;
  if (typeof raw.githubIssues === "boolean") out.githubIssues = raw.githubIssues;
  if (typeof raw.githubNotifications === "boolean") {
    out.githubNotifications = raw.githubNotifications;
  }
  if (typeof raw.githubStatusChips === "boolean") {
    out.githubStatusChips = raw.githubStatusChips;
  }
  return out;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function safeStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export class AppSettingsStore {
  #storage: StorageLike | null;
  #value = $state<AppSettingsData>({ ...APP_SETTINGS_DEFAULTS });

  constructor(storage: StorageLike | null = safeStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<AppSettingsData>({
      storage,
      key: APP_SETTINGS_KEY,
      parse: parseAppSettings,
    });
    if (loaded !== null) this.#value = loaded;
    // The i18n pilot's locale path: t() reads the module-level active
    // locale, synced from the persisted setting here and on every update.
    setLocale(this.#value.locale);
  }

  /** Current settings (reactive). */
  get value(): AppSettingsData {
    return this.#value;
  }

  get checkpointsEnabled(): boolean {
    return this.#value.checkpointsEnabled;
  }

  get terminalFontSize(): number {
    return this.#value.terminalFontSize;
  }

  get defaultAutofetchMinutes(): number {
    return this.#value.defaultAutofetchMinutes;
  }

  get confirmDiscard(): boolean {
    return this.#value.confirmDiscard;
  }

  get openFavoritesOnLaunch(): boolean {
    return this.#value.openFavoritesOnLaunch;
  }

  get useCommitTemplate(): boolean {
    return this.#value.useCommitTemplate;
  }

  get uiScalePercent(): number {
    return this.#value.uiScalePercent;
  }

  /** Soft monthly AI token budget (0 = off). */
  get aiMonthlyTokenBudget(): number {
    return this.#value.aiMonthlyTokenBudget;
  }

  /** Day of month the AI budget window resets on (1–31). */
  get aiBudgetResetDay(): number {
    return this.#value.aiBudgetResetDay;
  }

  /** Active-repo GitHub snapshot cadence in seconds (0 = off). */
  get forgeSnapshotSeconds(): number {
    return this.#value.forgeSnapshotSeconds;
  }

  /** Toast on GitHub failures (default on). */
  get forgeFailureToasts(): boolean {
    return this.#value.forgeFailureToasts;
  }

  /** Toast on GitHub activity transitions (default off). */
  get forgeActivityToasts(): boolean {
    return this.#value.forgeActivityToasts;
  }

  /** Master switch for every GitHub feature (off = no gh requests). */
  get githubEnabled(): boolean {
    return this.#value.githubEnabled;
  }

  /** Pull requests, checks, diffs, review threads and mutations. */
  get githubPullRequests(): boolean {
    return this.#value.githubPullRequests;
  }

  /** GitHub Actions runs, logs, reruns and dispatch. */
  get githubActions(): boolean {
    return this.#value.githubActions;
  }

  /** GitHub Issues workspace and activity polling. */
  get githubIssues(): boolean {
    return this.#value.githubIssues;
  }

  /** GitHub notification threads. */
  get githubNotifications(): boolean {
    return this.#value.githubNotifications;
  }

  /** Footer GitHub chips and their background snapshot polling. */
  get githubStatusChips(): boolean {
    return this.#value.githubStatusChips;
  }

  /** Patch settings and persist (partial update, validated on write). */
  update(patch: Partial<AppSettingsData>): void {
    const next = parseAppSettings({ ...this.#value, ...patch });
    this.#value = next;
    writeEnvelope(this.#storage, APP_SETTINGS_KEY, next);
    setLocale(next.locale);
  }

  /** Reset to defaults and persist. */
  reset(): void {
    this.update(APP_SETTINGS_DEFAULTS);
  }
}

/** App-wide settings singleton. */
export const appSettings = new AppSettingsStore();
