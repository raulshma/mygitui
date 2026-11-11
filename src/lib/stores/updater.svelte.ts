/**
 * App self-update store (Svelte 5 runes).
 *
 * Owns the update lifecycle the backend commands implement (see
 * `src-tauri/src/updater.rs`): poll → (auto)download → install → user-clicked
 * relaunch.
 *
 * States: `idle` (never checked) → `checking` → `uptodate` (or `available`)
 * → `downloading` → `ready` → relaunch; any step can land in `error` with a
 * message the chip surfaces as a muted warning dot (retry = another check).
 *
 * Behavior knobs live in localStorage under `mygitui.updater`
 * (`{ autoCheck, autoDownload, lastCheck, dismissedVersion }`, versioned
 * envelope): startup polls at most every ~12h, 15s after launch, and only
 * when `autoCheck`; a discovered update with `autoDownload` starts
 * downloading immediately. `dismissedVersion` ("Skip this version") hides
 * the chip until a *newer* release appears. Restarting is deliberately
 * never automatic — killing the user's session unannounced is worse than
 * one extra click.
 *
 * Backend access goes exclusively through `$lib/ipc/client` (mocked in
 * tests via `vi.mock`); components bind to the exported {@link updater}
 * singleton.
 */

import { isTauri } from "$lib/entry/dragdrop";
import {
  updaterCheck,
  updaterDownloadAndInstall,
  updaterRelaunch,
} from "$lib/ipc/client";
import type { UpdateInfo, UpdateProgress } from "$lib/ipc/types";
import { emitUiEvent } from "$lib/palette/events";
import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";
import { toast } from "$lib/toast";

/** Lifecycle state driving the chip, popover and toasts. */
export type UpdaterStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "uptodate"
  | "error";

/** localStorage key: settings + last poll + skipped version. */
export const UPDATER_STORAGE_KEY = "mygitui.updater";

/** Startup auto-checks only when the last poll is older than this. */
export const CHECK_STALE_MS = 12 * 60 * 60 * 1000;

/** Idle delay before the startup auto-check (let launch settle first). */
export const STARTUP_DELAY_MS = 15_000;

/** What persists across launches. */
interface StoredUpdater {
  autoCheck: boolean;
  autoDownload: boolean;
  /** Millisecond timestamp of the last completed poll (`null` = never). */
  lastCheck: number | null;
  /** Version the user chose "Skip this version" on (`null` = none). */
  dismissedVersion: string | null;
}

const DEFAULTS: StoredUpdater = {
  autoCheck: true,
  autoDownload: true,
  lastCheck: null,
  dismissedVersion: null,
};

/** Parse-and-drop loader (envelope version 1; malformed → defaults). */
export function parseStored(data: unknown): StoredUpdater {
  if (typeof data !== "object" || data === null) return { ...DEFAULTS };
  const raw = data as Partial<StoredUpdater>;
  const out: StoredUpdater = { ...DEFAULTS };
  if (typeof raw.autoCheck === "boolean") out.autoCheck = raw.autoCheck;
  if (typeof raw.autoDownload === "boolean") out.autoDownload = raw.autoDownload;
  if (typeof raw.lastCheck === "number" && Number.isFinite(raw.lastCheck)) {
    out.lastCheck = raw.lastCheck;
  }
  if (typeof raw.dismissedVersion === "string") {
    out.dismissedVersion = raw.dismissedVersion;
  }
  return out;
}

/**
 * Pure eligibility rule for the startup auto-check: only when running in
 * Tauri, polling is enabled, and the last poll is stale (or never ran).
 */
export function shouldAutoCheck(opts: {
  isTauri: boolean;
  autoCheck: boolean;
  lastCheck: number | null;
  now: number;
}): boolean {
  if (!opts.isTauri || !opts.autoCheck) return false;
  return opts.lastCheck === null || opts.now - opts.lastCheck >= CHECK_STALE_MS;
}

/** Delay/scheduler injection for tests (defaults to `setTimeout`). */
type Scheduler = (cb: () => void, ms: number) => () => void;

function realScheduler(cb: () => void, ms: number): () => void {
  const timer = setTimeout(cb, ms);
  return () => clearTimeout(timer);
}

/** Returns `localStorage` when available, else `null` (never throws). */
function safeStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export class UpdaterStore {
  /** Where the cycle currently is. */
  status: UpdaterStatus = $state("idle");
  /** The discovered update (`null` until a poll found one). */
  info: UpdateInfo | null = $state(null);
  /** Latest throttled download progress (`null` before the first chunk). */
  progress: UpdateProgress | null = $state(null);
  /** Why the last step failed (`error` status only). */
  error: string | null = $state(null);

  /** Poll on startup every ~12h. Persisted. */
  autoCheck: boolean = $state(true);
  /** Start downloading as soon as an update is discovered. Persisted. */
  autoDownload: boolean = $state(true);
  /** Last completed poll's timestamp (persisted; drives the 12h gate). */
  lastCheck: number | null = $state(null);
  /** Version skipped by the user (persisted; hides the chip until a newer release). */
  dismissedVersion: string | null = $state(null);

  #storage: StorageLike | null;
  #scheduler: Scheduler;
  /** Version the "update available" toast already fired for (session-only dedupe). */
  #toastedVersion: string | null = null;
  /** Startup timer handle (cleared when the store is disposed / re-init). */
  #startupTimer: (() => void) | null = null;

  constructor(opts?: {
    storage?: StorageLike | null;
    scheduler?: Scheduler;
  }) {
    this.#storage = opts?.storage !== undefined ? opts.storage : safeStorage();
    this.#scheduler = opts?.scheduler ?? realScheduler;
    const stored = loadVersioned<StoredUpdater>({
      storage: this.#storage,
      key: UPDATER_STORAGE_KEY,
      parse: parseStored,
    });
    if (stored !== null) {
      this.autoCheck = stored.autoCheck;
      this.autoDownload = stored.autoDownload;
      this.lastCheck = stored.lastCheck;
      this.dismissedVersion = stored.dismissedVersion;
    }
  }

  /** The discovered version, when one is known. */
  get version(): string | null {
    return this.info?.version ?? null;
  }

  /** Download percentage (0–100), `null` while unknown/indeterminate. */
  get percent(): number | null {
    const progress = this.progress;
    if (progress === null || progress.total === null || progress.total <= 0) {
      return null;
    }
    return Math.min(100, Math.floor((progress.downloaded / progress.total) * 100));
  }

  /**
   * Chip visibility: hidden entirely for idle/uptodate (no visual noise)
   * and while the user skipped the discovered version — but a download in
   * flight or an installed update stay visible regardless (actions pending).
   */
  get chipVisible(): boolean {
    if (this.status === "idle" || this.status === "uptodate") return false;
    if (
      this.status === "available" &&
      this.info !== null &&
      this.info.version === this.dismissedVersion
    ) {
      return false;
    }
    return true;
  }

  /** Persists the knobs + last poll + skipped version (best-effort). */
  #persist(): void {
    writeEnvelope(this.#storage, UPDATER_STORAGE_KEY, {
      autoCheck: this.autoCheck,
      autoDownload: this.autoDownload,
      lastCheck: this.lastCheck,
      dismissedVersion: this.dismissedVersion,
    } satisfies StoredUpdater);
  }

  /**
   * Startup wiring (main window only — App.svelte skips popouts): loads
   * settings and schedules the gated auto-check 15s out.
   */
  init(): void {
    this.#startupTimer?.();
    if (!isTauri()) return;
    this.#startupTimer = this.#scheduler(() => {
      this.#startupTimer = null;
      if (
        shouldAutoCheck({
          isTauri: isTauri(),
          autoCheck: this.autoCheck,
          lastCheck: this.lastCheck,
          now: Date.now(),
        })
      ) {
        void this.checkNow({ manual: false });
      }
    }, STARTUP_DELAY_MS);
  }

  /**
   * One poll of the update endpoint. Manual (`manual: true`) checks always
   * run and announce failures as toasts; the automatic one stays quiet on
   * failure (the chip's error dot is the signal). No-ops while a check or
   * download is already in flight.
   */
  async checkNow(opts?: { manual?: boolean }): Promise<void> {
    const manual = opts?.manual ?? true;
    if (this.status === "checking" || this.status === "downloading") return;
    this.status = "checking";
    this.error = null;
    try {
      const info = await updaterCheck();
      this.lastCheck = Date.now();
      this.#persist();
      if (info === null) {
        this.info = null;
        this.progress = null;
        this.status = "uptodate";
        if (manual) toast("You're on the latest version of mygitui", { kind: "success" });
        return;
      }
      this.info = info;
      this.status = "available";
      // One toast per discovered update, never for a skipped version.
      if (info.version !== this.#toastedVersion) {
        this.#toastedVersion = info.version;
        if (info.version !== this.dismissedVersion) {
          toast(`Update available: mygitui ${info.version}`, {
            action: {
              label: "View",
              run: () => emitUiEvent("open-updates"),
            },
          });
        }
      }
      if (this.autoDownload && info.version !== this.dismissedVersion) {
        void this.download({ manual: false });
      }
    } catch (err) {
      this.status = "error";
      this.error = describe(err);
      if (manual) toast(`Update check failed: ${describe(err)}`, { kind: "error" });
    }
  }

  /**
   * Downloads the discovered update and runs the platform install. With
   * `manual` (user clicked Download) failures toast; an auto-download
   * failure just lands in `error` (chip warning dot, popover shows retry).
   * On success the toast carries the Restart action — relaunching stays a
   * user click, never automatic.
   */
  async download(opts?: { manual?: boolean }): Promise<void> {
    const manual = opts?.manual ?? true;
    if (this.status === "downloading" || this.status === "ready") return;
    if (this.info === null) return;
    this.status = "downloading";
    this.progress = null;
    this.error = null;
    const version = this.info.version;
    try {
      await updaterDownloadAndInstall((progress) => {
        this.progress = progress;
      });
      this.status = "ready";
      this.progress = null;
      toast(`Update to v${version} ready — restart to apply`, {
        kind: "success",
        action: {
          label: "Restart",
          run: () => this.relaunch(),
        },
      });
    } catch (err) {
      this.status = "error";
      this.error = describe(err);
      this.progress = null;
      if (manual) {
        toast(`Update download failed: ${describe(err)}`, { kind: "error" });
      }
    }
  }

  /** Applies the installed update by restarting the app (user-clicked). */
  relaunch(): void {
    updaterRelaunch().catch(() => {
      // The process usually exits before the invoke resolves; if it does
      // come back with an error there is nothing actionable to show.
    });
  }

  /** "Skip this version": hides the chip until a newer release appears. */
  skipVersion(): void {
    if (this.info === null) return;
    this.dismissedVersion = this.info.version;
    this.#persist();
  }

  setAutoCheck(value: boolean): void {
    this.autoCheck = value;
    this.#persist();
  }

  setAutoDownload(value: boolean): void {
    this.autoDownload = value;
    this.#persist();
  }
}

function describe(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Shared app-wide instance (App.svelte wires the startup check). */
export const updater = new UpdaterStore();
