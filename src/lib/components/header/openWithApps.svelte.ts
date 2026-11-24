/**
 * "Open with" plumbing shared by the repo header's overflow menu (the
 * control's Workstream A home) — detection, last-used persistence and
 * launching, extracted verbatim from the old `OpenWithButton.svelte` when
 * the split button was demoted into the header overflow (plan §5.3).
 *
 * The primary segment opens the repository root with the last-used app
 * (persisted `mygitui.openwith.last`, default: the platform file manager);
 * detection lists every detected app with its brand icon (appIcons.ts),
 * then — after a divider — the user-defined custom actions
 * (`$lib/openwith/customApps.svelte`, generic terminal glyph). Detection
 * re-runs on every menu open (cheap stat probes, so newly installed apps
 * appear without a restart); ids resolve backend-side against a fresh
 * probe, so only machine-verified executables are ever spawned for
 * detected apps. Custom ids are namespaced `custom:<uuid>` in the
 * last-used key and spawn the user's own command line via
 * `open_with_custom`.
 */
import { detectEditors, openWith, openWithCustom } from "$lib/ipc/client";
import type { EditorApp } from "$lib/ipc/types";
import { describeError } from "$lib/errors";
import { toast } from "$lib/toast";
import { parseSshUri } from "$lib/stores/tabs.svelte";
import {
  CUSTOM_ID_PREFIX,
  customApps,
  fromMenuId,
  isCustomId,
  type CustomApp,
} from "$lib/openwith/customApps.svelte";

const LAST_KEY = "mygitui.openwith.last";

/** Detection failure is surfaced once per session, then stays quiet. */
let detectionToasted = false;

/** One "open with" menu/launch entry: a detected app or a custom action. */
export interface OpenEntry {
  /** `custom:<uuid>` for custom apps, else the detected id. */
  id: string;
  name: string;
  /** Present for custom entries (drives the spawn + the icon). */
  custom?: CustomApp;
}

/**
 * One detection pass. Called on mount and again on every menu open (the
 * probes are a handful of `stat`s); a failure toasts once per session
 * and resolves as "no apps". Also re-syncs the last-used mirror from
 * storage — another window (a popout) may have persisted a launch since
 * this one last did.
 */
export function refreshApps(): Promise<EditorApp[]> {
  lastId = readLast();
  return detectEditors()
    .then((found) => (Array.isArray(found) ? found : []))
    .catch((err: unknown) => {
      if (!detectionToasted) {
        detectionToasted = true;
        toast(
          `Could not detect editors: ${describeError(err)}`,
          { kind: "error" },
        );
      }
      return [] as EditorApp[];
    });
}

function readLast(): string {
  try {
    return localStorage.getItem(LAST_KEY) ?? "explorer";
  } catch {
    return "explorer";
  }
}

/**
 * The persisted last-used id, mirrored as reactive state: a launch (here or
 * from the picker) must flip the pinned split button's main segment live —
 * a plain localStorage read inside `currentEntry` would only refresh on the
 * next apps probe, leaving the button stale until a menu re-open.
 */
let lastId = $state(readLast());

function persistLast(id: string): void {
  lastId = id;
  try {
    localStorage.setItem(LAST_KEY, id);
  } catch {
    // Persistence is best-effort.
  }
}

/** Custom actions, namespaced so they never collide with detected ids. */
export function customEntries(): OpenEntry[] {
  return customApps.apps.map((app) => ({
    id: `${CUSTOM_ID_PREFIX}${app.id}`,
    name: app.name,
    custom: app,
  }));
}

/** Last-used custom app, when the persisted id points at one that exists. */
export function lastCustomEntry(): OpenEntry | undefined {
  if (!isCustomId(lastId)) return undefined;
  const app = customApps.byId(fromMenuId(lastId));
  return app ? { id: lastId, name: app.name, custom: app } : undefined;
}

/**
 * The app the primary/"open again" action targets: the last-used entry,
 * falling back to the file manager (always present), then the first
 * detected app. `undefined` only when detection found nothing at all.
 * Reads the reactive mirror, so callers' `$derived`s track launches.
 */
export function currentEntry(apps: EditorApp[]): OpenEntry | undefined {
  const entries: OpenEntry[] = [
    ...apps.map((a) => ({ id: a.id, name: a.name })),
    ...customEntries(),
  ];
  const lastCustom = lastCustomEntry();
  return (
    lastCustom ??
    entries.find((e) => e.id === lastId) ??
    entries.find((e) => e.id === "explorer") ??
    entries[0]
  );
}

/**
 * Launches the repository root in `entry`, remembering it as last-used
 * (launch failures toast; the last-used choice still sticks, mirroring the
 * old split button's flow). `deviceId` marks a repo that lives on a device:
 * the backend routes the launch through the editor's SSH-remote integration
 * (VS Code family; unsupported editors toast a named error instead of
 * opening a path that does not exist locally).
 */
export async function openWithEntry(
  root: string,
  entry: OpenEntry,
  deviceId?: string | null,
): Promise<void> {
  persistLast(entry.id);
  const parsed = parseSshUri(root);
  const effectiveDeviceId =
    deviceId !== undefined ? deviceId : (parsed?.deviceId ?? undefined);
  const effectivePath = parsed ? parsed.path : root;
  const pending = entry.custom
    ? openWithCustom(effectivePath, entry.custom)
    : openWith(effectivePath, entry.id, effectiveDeviceId);
  try {
    await pending;
  } catch (err: unknown) {
    toast(
      `Could not open ${entry.name}: ${describeError(err)}`,
      { kind: "error" },
    );
  }
}
