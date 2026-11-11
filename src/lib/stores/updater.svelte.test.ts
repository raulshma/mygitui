/**
 * Unit tests for the app self-update store (`$lib/stores/updater.svelte`).
 * The IPC client is fully mocked (`vi.mock`); storage is in-memory. The
 * real `isTauri()` resolves false under vitest, so `init()` schedules
 * nothing — auto-check gating is exercised through the pure
 * {@link shouldAutoCheck} rule and manual `checkNow` calls.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  updaterCheck,
  updaterDownloadAndInstall,
  updaterRelaunch,
} from "$lib/ipc/client";
import type { UpdateInfo, UpdateProgress } from "$lib/ipc/types";
import { dismissToast, getToasts } from "$lib/toast";
import type { StorageLike } from "$lib/state/persist";
import {
  CHECK_STALE_MS,
  UpdaterStore,
  parseStored,
  shouldAutoCheck,
  UPDATER_STORAGE_KEY,
} from "$lib/stores/updater.svelte";

vi.mock("$lib/ipc/client", () => ({
  updaterCheck: vi.fn(),
  updaterDownloadAndInstall: vi.fn(),
  updaterRelaunch: vi.fn(),
}));

const mockCheck = vi.mocked(updaterCheck);
const mockDownload = vi.mocked(updaterDownloadAndInstall);
const mockRelaunch = vi.mocked(updaterRelaunch);

/** In-memory storage (hermetic; records writes). */
function memStorage(initial: Record<string, string> = {}): StorageLike & {
  dump(): Record<string, string>;
} {
  const data = { ...initial };
  return {
    getItem: (key) => data[key] ?? null,
    setItem: (key, value) => {
      data[key] = value;
    },
    dump: () => data,
  };
}

const INFO: UpdateInfo = {
  version: "0.2.0",
  current_version: "0.1.20",
  notes: "Bug fixes.",
  date: "2026-11-01",
};

function newStore(storage = memStorage()): UpdaterStore {
  return new UpdaterStore({ storage, scheduler: () => () => {} });
}

/** Flushes pending microtasks (check → optional auto-download chains). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  mockCheck.mockReset();
  mockDownload.mockReset();
  mockRelaunch.mockReset();
  mockCheck.mockResolvedValue(null);
  mockDownload.mockResolvedValue();
  mockRelaunch.mockResolvedValue();
  // Toast state is module-global; previous tests' rows must not leak in.
  for (const item of getToasts()) dismissToast(item.id);
});

describe("shouldAutoCheck (pure)", () => {
  const base = { isTauri: true, autoCheck: true, lastCheck: null, now: 1_000 };

  it("checks when enabled and never checked", () => {
    expect(shouldAutoCheck(base)).toBe(true);
  });

  it("checks when the last poll is stale (≥12h old)", () => {
    expect(
      shouldAutoCheck({ ...base, lastCheck: 1_000 - CHECK_STALE_MS }),
    ).toBe(true);
  });

  it("skips when the last poll is fresh (<12h old)", () => {
    expect(
      shouldAutoCheck({ ...base, lastCheck: 1_000 - CHECK_STALE_MS + 1 }),
    ).toBe(false);
  });

  it("skips when polling is disabled or outside Tauri", () => {
    expect(shouldAutoCheck({ ...base, autoCheck: false })).toBe(false);
    expect(shouldAutoCheck({ ...base, isTauri: false })).toBe(false);
  });
});

describe("parseStored (pure)", () => {
  it("drops malformed payloads back to defaults", () => {
    expect(parseStored(null)).toEqual(parseStored({}));
    expect(parseStored({ autoCheck: "yes" }).autoCheck).toBe(true);
    expect(parseStored({ lastCheck: "soon" }).lastCheck).toBeNull();
    expect(parseStored({ dismissedVersion: 3 }).dismissedVersion).toBeNull();
  });

  it("keeps valid fields", () => {
    const parsed = parseStored({
      autoCheck: false,
      autoDownload: false,
      lastCheck: 42,
      dismissedVersion: "0.2.0",
    });
    expect(parsed).toEqual({
      autoCheck: false,
      autoDownload: false,
      lastCheck: 42,
      dismissedVersion: "0.2.0",
    });
  });
});

describe("UpdaterStore check cycle", () => {
  it("manual up-to-date check lands in uptodate with a toast, chip hidden", async () => {
    const store = newStore();
    await store.checkNow({ manual: true });

    expect(store.status).toBe("uptodate");
    expect(store.lastCheck).not.toBeNull();
    expect(store.chipVisible).toBe(false);
    expect(getToasts().some((t) => t.message.includes("latest version"))).toBe(true);
  });

  it("an available update toasts once with a View action", async () => {
    mockCheck.mockResolvedValue(INFO);
    const store = newStore();
    store.setAutoDownload(false); // observe "available" before any download

    await store.checkNow({ manual: false });
    expect(store.status).toBe("available");
    expect(store.info?.version).toBe("0.2.0");
    expect(store.chipVisible).toBe(true);

    const available = getToasts().filter((t) => t.message.includes("Update available"));
    expect(available).toHaveLength(1);
    expect(available[0]?.action?.label).toBe("View");
    expect(mockDownload).not.toHaveBeenCalled();
  });

  it("autoDownload starts the chain inside the check and lands ready", async () => {
    mockCheck.mockResolvedValue(INFO);
    // Hold the download mid-flight; released below.
    let releaseDownload: () => void = () => {};
    mockDownload.mockImplementation(
      () => new Promise<void>((resolve) => { releaseDownload = resolve; }),
    );
    const store = newStore(); // autoDownload defaults on

    await store.checkNow({ manual: false });
    // The download starts synchronously with the check's tail — the
    // "available" moment is never observable with autoDownload on.
    expect(store.status).toBe("downloading");
    expect(mockDownload).toHaveBeenCalledTimes(1);
    expect(store.chipVisible).toBe(true);

    releaseDownload();
    await flush();
    expect(store.status).toBe("ready");
    expect(
      getToasts().some(
        (t) =>
          t.message.includes("ready") &&
          t.message.includes("0.2.0") &&
          t.action?.label === "Restart",
      ),
    ).toBe(true);
  });

  it("a second check of the same version does not re-toast", async () => {
    mockCheck.mockResolvedValue(INFO);
    const store = newStore();
    store.setAutoDownload(false); // keep re-checks allowed (no in-flight download)

    await store.checkNow({ manual: false });
    const afterFirst = getToasts().filter((t) => t.message.includes("Update available"));
    expect(afterFirst).toHaveLength(1);

    await store.checkNow({ manual: false });
    const afterSecond = getToasts().filter((t) => t.message.includes("Update available"));
    expect(afterSecond.length).toBe(afterFirst.length);
  });

  it("a skipped version: no toast, no auto-download, chip hidden until newer", async () => {
    const storage = memStorage({
      [UPDATER_STORAGE_KEY]: JSON.stringify({
        v: 1,
        data: { dismissedVersion: "0.2.0" },
      }),
    });
    mockCheck.mockResolvedValue(INFO);
    const store = new UpdaterStore({ storage, scheduler: () => () => {} });
    expect(store.dismissedVersion).toBe("0.2.0");

    await store.checkNow({ manual: false });
    expect(store.status).toBe("available");
    expect(store.chipVisible).toBe(false);
    expect(getToasts().filter((t) => t.message.includes("Update available"))).toHaveLength(0);
    // A skip means "don't fetch this one either", even with autoDownload on.
    await flush();
    expect(mockDownload).not.toHaveBeenCalled();

    // A newer release overrides the skip.
    const newer = { ...INFO, version: "0.3.0" };
    mockCheck.mockResolvedValue(newer);
    await store.checkNow({ manual: false });
    expect(store.chipVisible).toBe(true);
  });

  it("download progress lands in the store and resets on ready", async () => {
    mockCheck.mockResolvedValue(INFO);
    mockDownload.mockImplementation((onProgress: (p: UpdateProgress) => void) => {
      onProgress({ downloaded: 60, total: 120 });
      onProgress({ downloaded: 120, total: 120 });
      return Promise.resolve();
    });
    const store = newStore();
    await store.checkNow({ manual: false });
    await flush();

    expect(store.status).toBe("ready");
    expect(store.progress).toBeNull();
    expect(store.percent).toBeNull(); // progress reset once ready
  });

  it("check errors land in error status; only manual checks toast them", async () => {
    mockCheck.mockRejectedValue(new Error("no network"));
    const store = newStore();

    await store.checkNow({ manual: false });
    expect(store.status).toBe("error");
    expect(store.error).toBe("no network");
    const duringAuto = getToasts().filter((t) => t.kind === "error").length;

    await store.checkNow({ manual: true });
    expect(getToasts().filter((t) => t.kind === "error").length).toBe(duringAuto + 1);
    expect(store.chipVisible).toBe(true); // muted warning dot
  });

  it("download errors land in error status and manual downloads toast", async () => {
    mockCheck.mockResolvedValue(INFO);
    mockDownload.mockRejectedValue(new Error("signature mismatch"));
    const store = newStore();

    await store.checkNow({ manual: false });
    await flush();
    expect(store.status).toBe("error");
    expect(store.error).toBe("signature mismatch");

    // Retry path: a manual download attempt surfaces a toast.
    const duringAuto = getToasts().filter((t) => t.kind === "error").length;
    await store.download({ manual: true });
    await flush();
    expect(getToasts().filter((t) => t.kind === "error").length).toBe(duringAuto + 1);
  });

  it("relaunch goes through the IPC layer", () => {
    const store = newStore();
    store.relaunch();
    expect(mockRelaunch).toHaveBeenCalledTimes(1);
  });
});

describe("UpdaterStore persistence", () => {
  it("settings changes write the versioned envelope", () => {
    const storage = memStorage();
    const store = new UpdaterStore({ storage, scheduler: () => () => {} });

    store.setAutoCheck(false);
    store.setAutoDownload(false);
    const raw = storage.dump()[UPDATER_STORAGE_KEY]!;
    expect(JSON.parse(raw)).toEqual({
      v: 1,
      data: {
        autoCheck: false,
        autoDownload: false,
        lastCheck: null,
        dismissedVersion: null,
      },
    });
  });

  it("skipVersion persists and survives a reload", async () => {
    mockCheck.mockResolvedValue(INFO);
    const storage = memStorage();
    const store = new UpdaterStore({ storage, scheduler: () => () => {} });
    store.setAutoDownload(false); // stay in "available" (a download skips past it)
    await store.checkNow({ manual: false });
    expect(store.chipVisible).toBe(true);
    store.skipVersion();
    expect(store.dismissedVersion).toBe("0.2.0");
    expect(store.chipVisible).toBe(false);

    const reloaded = new UpdaterStore({ storage, scheduler: () => () => {} });
    expect(reloaded.dismissedVersion).toBe("0.2.0");
  });

  it("skip cannot hide an update that is already downloaded (ready wins)", async () => {
    mockCheck.mockResolvedValue(INFO);
    const store = newStore();
    await store.checkNow({ manual: false });
    await flush(); // download completes → ready
    store.skipVersion();
    expect(store.dismissedVersion).toBe("0.2.0");
    // A pending restart must stay actionable — no silent hiding.
    expect(store.chipVisible).toBe(true);
  });

  it("checkNow records the poll time (the 12h gate reads it)", async () => {
    const storage = memStorage();
    const store = new UpdaterStore({ storage, scheduler: () => () => {} });
    await store.checkNow({ manual: true });

    expect(store.lastCheck).not.toBeNull();
    const persisted = JSON.parse(storage.dump()[UPDATER_STORAGE_KEY]!) as {
      data: { lastCheck: number | null };
    };
    expect(persisted.data.lastCheck).toBe(store.lastCheck);
  });
});
