/**
 * Remote-devices store (Svelte 5 runes) — registered SSH machines, their
 * connection status and the pending connect-prompt queue.
 *
 * `load()` mirrors the backend registry into `devices`; `add` / `update` /
 * `remove` / `connect` / `disconnect` keep it in step around the backend
 * commands, and `device-status` events patch statuses the backend changed
 * on its own (poll reconnects, dropped sockets). An event for a device the
 * list doesn't know yet schedules a re-sync instead of being dropped — the
 * backend only emits on change, so a dropped event would pin a stale
 * status until the next reload (the launch "disconnected" bug).
 * `autoConnectOnLaunch()` runs background connect loops for devices the
 * user flagged opt-in. The `device-status`
 * subscription and the prompt queues follow the AuthStore pattern in
 * `$lib/stores/ops.svelte`:
 *
 *   backend pauses a connect and emits `hostkey-request` /
 *   `device-auth-request` → this store surfaces it as the single
 *   {@link RemoteDeviceStore.pendingPrompt} slot (`hostkeyPending` /
 *   `authPending` are derived views of it) → dialog binds to it → the user
 *   answers via `answerHostKey()` / `answerAuth()` (or `dismissHostKey()` /
 *   `dismissAuth()` on Escape), which call the `device_*_respond` commands
 *   and advance to the next queued request. One shared queue + one visible
 *   slot: a host-key and an auth prompt can never stack as two modals.
 *
 * The backend expires an unanswered prompt after 120 s; `pruneStale()`
 * drops queue entries older than that so a timed-out prompt never blocks
 * the dialog forever (App.svelte calls it on an interval; the apply paths
 * also prune on every arrival).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests).
 */

import {
  deviceAdd,
  deviceAuthRespond,
  deviceConnect,
  deviceDisconnect,
  deviceForgetHostKey,
  deviceHostkeyRespond,
  deviceList,
  deviceRemove,
  deviceUpdate,
} from "$lib/ipc/client";
import type {
  DeviceAuthRequestEvent,
  DeviceInfo,
  DeviceInput,
  DeviceProfile,
  DeviceStatusEvent,
  HostKeyRequestEvent,
} from "$lib/ipc/types";
import { onDeviceAuthRequest, onDeviceStatus, onHostKeyRequest } from "$lib/events/repoEvents";
import { toast } from "$lib/toast";
import { describeError } from "$lib/errors";

/**
 * How long the backend waits for a `device_*_respond` answer before it
 * times the prompt out on its own (devices.rs). Queue entries older than
 * this are prunable.
 */
export const PROMPT_TIMEOUT_MS = 120_000;

/**
 * How long a `device-status` event for a device the list doesn't know yet
 * waits before triggering a registry re-sync (trailing debounce — a burst
 * of unknown-id events costs one `device_list`).
 */
export const RESYNC_DEBOUNCE_MS = 100;

/**
 * Delay before auto-connect retry attempt `attempt` (0-based). Mirrors the
 * backend's `reconnect_backoff_secs` for dropped sessions: 1, 2, 5, 10 s,
 * capped at 30 s forever.
 */
export function autoConnectBackoffMs(attempt: number): number {
  const secs = [1, 2, 5, 10][attempt] ?? 30;
  return Math.min(secs, 30) * 1000;
}

const sleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

/**
 * A `DeviceInput` that re-saves `profile` unchanged (edit dialog base /
 * poll-interval updates): secrets are left out, so the backend keeps the
 * stored ones.
 */
export function deviceInputFromProfile(profile: DeviceProfile): DeviceInput {
  return {
    id: profile.id,
    name: profile.name,
    host: profile.host,
    port: profile.port,
    username: profile.username,
    kind: profile.auth.kind,
    key_path: profile.auth.key_path,
    has_passphrase: profile.auth.has_passphrase,
    // Secrets are null = keep whatever is stored in the keyring.
    password: null,
    passphrase: null,
    poll_secs: profile.poll_secs,
    auto_connect: profile.auto_connect,
  };
}

/** A pending connect prompt: the tagged event travels with the slot. */
export type PendingPrompt =
  | { kind: "hostkey"; request: HostKeyRequestEvent }
  | { kind: "auth"; request: DeviceAuthRequestEvent };

export class RemoteDeviceStore {
  /** All registered devices, in registry order. */
  devices: DeviceInfo[] = $state([]);

  /**
   * The connect prompt the dialog is showing, or `null` when none is
   * pending. One slot over one shared queue — the two prompt kinds
   * arbitrate instead of stacking.
   */
  pendingPrompt: PendingPrompt | null = $state(null);

  /** The visible host-key prompt, or `null` (derived from {@link pendingPrompt}). */
  get hostkeyPending(): HostKeyRequestEvent | null {
    return this.pendingPrompt?.kind === "hostkey"
      ? this.pendingPrompt.request
      : null;
  }

  /** The visible auth prompt, or `null` (derived from {@link pendingPrompt}). */
  get authPending(): DeviceAuthRequestEvent | null {
    return this.pendingPrompt?.kind === "auth" ? this.pendingPrompt.request : null;
  }

  #promptQueue: PendingPrompt[] = [];
  /** request_id → queue arrival (ms), for {@link pruneStale}. */
  #queuedAt = new Map<string, number>();
  #unregister: Array<() => void> = [];
  /**
   * Device ids with a launch auto-connect loop running — also the loop's
   * cancellation set (user disconnect / removal / prompt clears the entry).
   */
  #autoConnecting = new Set<string>();
  /** Pending {@link RESYNC_DEBOUNCE_MS} re-sync from an unknown-id event. */
  #resyncTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * Subscribes to the device prompt/status events (registration is
   * idempotent; the subscription lifecycle lives in
   * `$lib/events/repoEvents`). Safe to call from the App mount.
   */
  startEvents(): void {
    if (this.#unregister.length > 0) return;
    this.#unregister = [
      onHostKeyRequest((event) => this.applyHostKeyRequest(event)),
      onDeviceAuthRequest((event) => this.applyAuthRequest(event)),
      onDeviceStatus((event) => this.applyStatus(event)),
    ];
  }

  /** The device entry for `deviceId`, or `null` when unknown. */
  byId(deviceId: string): DeviceInfo | null {
    return this.devices.find((device) => device.profile.id === deviceId) ?? null;
  }

  /** Connection status of `deviceId` (`"disconnected"` when unknown). */
  statusOf(deviceId: string): DeviceInfo["status"] {
    return this.byId(deviceId)?.status ?? "disconnected";
  }

  /**
   * Refreshes the device registry from the backend. Never throws — a
   * failure leaves the current list and logs a warning (the home view
   * renders whatever is known).
   */
  async load(): Promise<void> {
    try {
      const list = await deviceList();
      this.devices = Array.isArray(list) ? list : [];
    } catch (err) {
      console.warn("[devices] device_list failed:", err);
    }
  }

  /** Registers a device and adds it to the list. Rejects on backend error. */
  async add(input: DeviceInput): Promise<DeviceInfo> {
    const device = await deviceAdd(input);
    this.devices = [...this.devices, device];
    return device;
  }

  /** Saves changes to a device and swaps the fresh entry into the list. */
  async update(input: DeviceInput): Promise<DeviceInfo> {
    const device = await deviceUpdate(input);
    const id = device.profile.id;
    this.devices = this.devices.map((entry) =>
      entry.profile.id === id ? device : entry,
    );
    return device;
  }

  /** Deletes a device and drops it (plus its prompts) from the store. */
  async remove(deviceId: string): Promise<void> {
    this.#autoConnecting.delete(deviceId);
    await deviceRemove(deviceId);
    this.devices = this.devices.filter(
      (device) => device.profile.id !== deviceId,
    );
    this.#dropPromptsFor(deviceId);
  }

  /**
   * Connects: marks the device `connecting`, then lets the backend answer
   * (resolve / `device-status` event / error). Never throws — failures are
   * toasted and the device falls back to `disconnected`.
   */
  async connect(deviceId: string): Promise<void> {
    this.setStatus(deviceId, "connecting");
    try {
      await deviceConnect(deviceId);
      // An authoritative `device-status` event that arrived mid-await wins;
      // only complete the optimistic transition if it is still showing.
      if (this.statusOf(deviceId) === "connecting") {
        this.setStatus(deviceId, "connected");
      }
    } catch (err) {
      if (this.statusOf(deviceId) === "connecting") {
        this.setStatus(deviceId, "disconnected");
      }
      toast(
        `Connect failed: ${describeError(err)}`,
        { kind: "error" },
      );
    }
  }

  /**
   * Disconnects. Never throws — the status is set optimistically (the
   * authoritative one arrives via the `device-status` event) and a failed
   * command is toasted. The optimistic patch yields to an in-flight
   * connect: while the backend is dialing, the connect's own completion /
   * `device-status` events own the status, so disconnecting mid-connect
   * cannot clobber `connecting` and leave the chip lying in either
   * direction.
   */
  async disconnect(deviceId: string): Promise<void> {
    // A manual disconnect cancels the launch auto-connect loop: the user
    // just said this device is not wanted right now.
    this.#autoConnecting.delete(deviceId);
    if (this.statusOf(deviceId) !== "connecting") {
      this.setStatus(deviceId, "disconnected");
    }
    try {
      await deviceDisconnect(deviceId);
    } catch (err) {
      toast(
        `Disconnect failed: ${describeError(err)}`,
        { kind: "error" },
      );
    }
  }

  /**
   * Removes the device's pinned host key. Resolves `true` when a key was
   * pinned; never throws (the caller toasts the outcome).
   */
  async forgetHostKey(deviceId: string): Promise<boolean> {
    try {
      return await deviceForgetHostKey(deviceId);
    } catch (err) {
      toast(
        `Forget host key failed: ${describeError(err)}`,
        { kind: "error" },
      );
      return false;
    }
  }

  // -- launch auto-connect ----------------------------------------------------

  /**
   * Starts background connect loops for every device flagged
   * `auto_connect` (opt-in, off by default). Called once the initial
   * registry load has landed, at app launch; the backend's watchdog takes
   * over reconnection for sessions that drop later.
   */
  autoConnectOnLaunch(): void {
    for (const device of this.devices) {
      if (!device.profile.auto_connect) continue;
      if (this.statusOf(device.profile.id) === "connected") continue;
      this.#startAutoConnect(device.profile.id);
    }
  }

  /** Starts one auto-connect loop (single-flighted per device). */
  #startAutoConnect(deviceId: string): void {
    if (this.#autoConnecting.has(deviceId)) return;
    this.#autoConnecting.add(deviceId);
    void this.#autoConnectRun(deviceId);
  }

  /**
   * The loop: dial, and on failure retry with the backend's backoff
   * schedule until connected or cancelled. Deliberately does not reuse
   * {@link connect} — that one toasts per failure, which a background
   * loop would spam; the chip shows the state instead. A device that is
   * already dialing (e.g. a restored tab's open) is fine: `device_connect`
   * serializes behind the in-flight dial at the backend handle mutex and
   * returns its result.
   */
  async #autoConnectRun(deviceId: string): Promise<void> {
    try {
      for (let attempt = 0; ; attempt++) {
        if (!this.#autoConnecting.has(deviceId)) return;
        if (this.statusOf(deviceId) === "connected") return;
        if (this.byId(deviceId) === null) return; // removed mid-flight
        this.setStatus(deviceId, "connecting");
        try {
          await deviceConnect(deviceId);
          // An authoritative `device-status` event that arrived mid-await
          // wins; only complete the optimistic transition if it is still
          // showing.
          if (this.statusOf(deviceId) === "connecting") {
            this.setStatus(deviceId, "connected");
          }
          return;
        } catch {
          if (this.statusOf(deviceId) === "connecting") {
            this.setStatus(deviceId, "disconnected");
          }
        }
        await sleep(autoConnectBackoffMs(attempt));
      }
    } finally {
      this.#autoConnecting.delete(deviceId);
    }
  }

  /** Debounced registry re-sync ({@link applyStatus}'s unknown-device path). */
  #scheduleResync(): void {
    if (this.#resyncTimer !== null) return;
    this.#resyncTimer = setTimeout(() => {
      this.#resyncTimer = null;
      void this.load();
    }, RESYNC_DEBOUNCE_MS);
  }

  // -- prompt queue ----------------------------------------------------------

  /** Registers an incoming host-key prompt (public for tests). */
  applyHostKeyRequest(event: HostKeyRequestEvent): void {
    this.applyPrompt({ kind: "hostkey", request: event });
  }

  /** Registers an incoming device-auth prompt (public for tests). */
  applyAuthRequest(event: DeviceAuthRequestEvent): void {
    this.applyPrompt({ kind: "auth", request: event });
  }

  /**
   * Registers an incoming prompt (public for tests): prunes stale entries,
   * queues it and shows it as soon as no other prompt is on screen.
   */
  applyPrompt(prompt: PendingPrompt): void {
    // A prompt means the dial is waiting on the user: the auto-connect
    // loop must stop retrying under them (and re-prompting) — they own
    // the connect now.
    this.#autoConnecting.delete(prompt.request.device_id);
    this.pruneStale();
    this.#queuedAt.set(prompt.request.request_id, Date.now());
    this.#promptQueue.push(prompt);
    if (this.pendingPrompt === null) {
      this.pendingPrompt = this.#promptQueue.shift() ?? null;
    }
  }

  /**
   * Applies a `device-status` event: patches the device's status. An event
   * for a device the list doesn't know yet (boot race: the dial started
   * before `load()` landed; a device registered from another window) is not
   * dropped — it schedules a debounced registry re-sync, because the
   * backend only emits on status *change* and this event would otherwise
   * be lost for good (launch pinned "disconnected" until a reload).
   */
  applyStatus(event: DeviceStatusEvent): void {
    if (this.byId(event.device_id) === null) {
      this.#scheduleResync();
      return;
    }
    this.setStatus(event.device_id, event.status);
  }

  /**
   * Dialog submit for the visible host-key prompt: answers the backend,
   * advances the queue. Never throws.
   */
  async answerHostKey(accept: boolean): Promise<void> {
    const request = this.hostkeyPending;
    if (!request) return;
    this.#advance();
    try {
      await deviceHostkeyRespond(request.request_id, accept);
    } catch (err) {
      toast(
        `Host key answer failed: ${describeError(err)}`,
        { kind: "error" },
      );
    }
  }

  /** Dialog reject (button or Escape): denies the key. */
  dismissHostKey(): Promise<void> {
    return this.answerHostKey(false);
  }

  /**
   * Dialog submit for the visible auth prompt (`password: null` = cancel),
   * advancing the queue. Never throws.
   */
  async answerAuth(password: string | null, store: boolean): Promise<void> {
    const request = this.authPending;
    if (!request) return;
    this.#advance();
    try {
      await deviceAuthRespond(request.request_id, password, store);
    } catch (err) {
      toast(
        `Device auth answer failed: ${describeError(err)}`,
        { kind: "error" },
      );
    }
  }

  /** Dialog cancel: answers with no password so the backend aborts. */
  dismissAuth(): Promise<void> {
    return this.answerAuth(null, false);
  }

  /**
   * Drops prompts older than {@link PROMPT_TIMEOUT_MS} (the backend timed
   * them out already). Called on every prompt arrival; App.svelte also
   * polls it so a timed-out visible prompt clears without new traffic.
   */
  pruneStale(now: number = Date.now()): void {
    this.#pruneQueue(this.#promptQueue, now);
    const pending = this.pendingPrompt;
    if (
      pending !== null &&
      now - (this.#queuedAt.get(pending.request.request_id) ?? now) >
        PROMPT_TIMEOUT_MS
    ) {
      this.#advance();
    }
  }

  /** Deregisters and clears everything (test helper / teardown). */
  stop(): void {
    for (const unregister of this.#unregister) unregister();
    this.#unregister = [];
    this.devices = [];
    this.#promptQueue = [];
    this.#queuedAt.clear();
    this.pendingPrompt = null;
    this.#autoConnecting.clear();
    if (this.#resyncTimer !== null) {
      clearTimeout(this.#resyncTimer);
      this.#resyncTimer = null;
    }
  }

  // -- internals ---------------------------------------------------------------

  /** Patches one device's status (fresh reference → rune reactivity). */
  setStatus(deviceId: string, status: DeviceInfo["status"]): void {
    this.devices = this.devices.map((device) =>
      device.profile.id === deviceId ? { ...device, status } : device,
    );
  }

  /** Shows the next queued prompt (or closes the slot). */
  #advance(): void {
    this.pendingPrompt = this.#promptQueue.shift() ?? null;
  }

  #pruneQueue(queue: PendingPrompt[], now: number): void {
    for (let i = queue.length - 1; i >= 0; i--) {
      const requestId = queue[i]!.request.request_id;
      const arrivedAt = this.#queuedAt.get(requestId);
      if (arrivedAt !== undefined && now - arrivedAt > PROMPT_TIMEOUT_MS) {
        this.#queuedAt.delete(requestId);
        queue.splice(i, 1);
      }
    }
  }

  /** Drops every pending/queued prompt belonging to `deviceId`. */
  #dropPromptsFor(deviceId: string): void {
    this.#promptQueue = this.#promptQueue.filter(
      (prompt) => prompt.request.device_id !== deviceId,
    );
    if (this.pendingPrompt?.request.device_id === deviceId) this.#advance();
  }
}

/** The application-wide remote-devices store. */
export const remoteDevices = new RemoteDeviceStore();

// Standalone function API over the singleton (what components import). --

export function startRemoteDeviceEvents(): void {
  remoteDevices.startEvents();
}
