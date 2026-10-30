/**
 * Remote-devices store (Svelte 5 runes) — registered SSH machines, their
 * connection status and the pending connect-prompt queue.
 *
 * `load()` mirrors the backend registry into `devices`; `add` / `update` /
 * `remove` / `connect` / `disconnect` keep it in step around the backend
 * commands, and `device-status` events patch statuses the backend changed
 * on its own (poll reconnects, dropped sockets). The `device-status`
 * subscription and the prompt queues follow the AuthStore pattern in
 * `$lib/stores/ops.svelte`:
 *
 *   backend pauses a connect and emits `hostkey-request` /
 *   `device-auth-request` → this store surfaces it as `hostkeyPending` /
 *   `authPending` → dialog binds to it → the user answers via
 *   `answerHostKey()` / `answerAuth()` (or `dismissHostKey()` /
 *   `dismissAuth()` on Escape), which call the `device_*_respond` commands
 *   and advance to the next queued request.
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

/**
 * How long the backend waits for a `device_*_respond` answer before it
 * times the prompt out on its own (devices.rs). Queue entries older than
 * this are prunable.
 */
export const PROMPT_TIMEOUT_MS = 120_000;

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
  };
}

export class RemoteDeviceStore {
  /** All registered devices, in registry order. */
  devices: DeviceInfo[] = $state([]);

  /** The connect prompt the dialog is showing, or `null` when none pending. */
  hostkeyPending: HostKeyRequestEvent | null = $state(null);
  /** The auth prompt the dialog is showing, or `null` when none pending. */
  authPending: DeviceAuthRequestEvent | null = $state(null);

  #hostkeyQueue: HostKeyRequestEvent[] = [];
  #authQueue: DeviceAuthRequestEvent[] = [];
  /** request_id → queue arrival (ms), for {@link pruneStale}. */
  #queuedAt = new Map<string, number>();
  #unregister: Array<() => void> = [];

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
      this.devices = await deviceList();
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
      this.setStatus(deviceId, "connected");
    } catch (err) {
      this.setStatus(deviceId, "disconnected");
      toast(
        `Connect failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  /**
   * Disconnects. Never throws — the status is set optimistically (the
   * authoritative one arrives via the `device-status` event) and a failed
   * command is toasted.
   */
  async disconnect(deviceId: string): Promise<void> {
    this.setStatus(deviceId, "disconnected");
    try {
      await deviceDisconnect(deviceId);
    } catch (err) {
      toast(
        `Disconnect failed: ${err instanceof Error ? err.message : String(err)}`,
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
        `Forget host key failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
      return false;
    }
  }

  // -- prompt queue ----------------------------------------------------------

  /**
   * Registers an incoming host-key prompt (public for tests): prunes stale
   * entries, queues it and shows it as soon as no other prompt is on
   * screen.
   */
  applyHostKeyRequest(event: HostKeyRequestEvent): void {
    this.pruneStale();
    this.#queuedAt.set(event.request_id, Date.now());
    this.#hostkeyQueue.push(event);
    if (this.hostkeyPending === null) {
      this.hostkeyPending = this.#hostkeyQueue.shift() ?? null;
    }
  }

  /** Registers an incoming device-auth prompt (same policy as above). */
  applyAuthRequest(event: DeviceAuthRequestEvent): void {
    this.pruneStale();
    this.#queuedAt.set(event.request_id, Date.now());
    this.#authQueue.push(event);
    if (this.authPending === null) {
      this.authPending = this.#authQueue.shift() ?? null;
    }
  }

  /**
   * Applies a `device-status` event: patches the device's status (unknown
   * devices are ignored — the registry list is the source of truth for
   * which devices exist).
   */
  applyStatus(event: DeviceStatusEvent): void {
    this.setStatus(event.device_id, event.status);
  }

  /**
   * Dialog submit for the visible host-key prompt: answers the backend,
   * advances the queue. Never throws.
   */
  async answerHostKey(accept: boolean): Promise<void> {
    const request = this.hostkeyPending;
    if (!request) return;
    this.#advanceHostKey();
    try {
      await deviceHostkeyRespond(request.request_id, accept);
    } catch (err) {
      toast(
        `Host key answer failed: ${err instanceof Error ? err.message : String(err)}`,
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
    this.#advanceAuth();
    try {
      await deviceAuthRespond(request.request_id, password, store);
    } catch (err) {
      toast(
        `Device auth answer failed: ${err instanceof Error ? err.message : String(err)}`,
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
    this.#pruneQueue(this.#hostkeyQueue, now);
    this.#pruneQueue(this.#authQueue, now);
    if (
      this.hostkeyPending !== null &&
      now - (this.#queuedAt.get(this.hostkeyPending.request_id) ?? now) >
        PROMPT_TIMEOUT_MS
    ) {
      this.#advanceHostKey();
    }
    if (
      this.authPending !== null &&
      now - (this.#queuedAt.get(this.authPending.request_id) ?? now) >
        PROMPT_TIMEOUT_MS
    ) {
      this.#advanceAuth();
    }
  }

  /** Deregisters and clears everything (test helper / teardown). */
  stop(): void {
    for (const unregister of this.#unregister) unregister();
    this.#unregister = [];
    this.devices = [];
    this.#hostkeyQueue = [];
    this.#authQueue = [];
    this.#queuedAt.clear();
    this.hostkeyPending = null;
    this.authPending = null;
  }

  // -- internals ---------------------------------------------------------------

  /** Patches one device's status (fresh reference → rune reactivity). */
  setStatus(deviceId: string, status: DeviceInfo["status"]): void {
    this.devices = this.devices.map((device) =>
      device.profile.id === deviceId ? { ...device, status } : device,
    );
  }

  #advanceHostKey(): void {
    this.hostkeyPending = this.#hostkeyQueue.shift() ?? null;
  }

  #advanceAuth(): void {
    this.authPending = this.#authQueue.shift() ?? null;
  }

  #pruneQueue(
    queue: Array<HostKeyRequestEvent | DeviceAuthRequestEvent>,
    now: number,
  ): void {
    for (let i = queue.length - 1; i >= 0; i--) {
      const arrivedAt = this.#queuedAt.get(queue[i]!.request_id);
      if (arrivedAt !== undefined && now - arrivedAt > PROMPT_TIMEOUT_MS) {
        this.#queuedAt.delete(queue[i]!.request_id);
        queue.splice(i, 1);
      }
    }
  }

  /** Drops every pending/queued prompt belonging to `deviceId`. */
  #dropPromptsFor(deviceId: string): void {
    this.#hostkeyQueue = this.#hostkeyQueue.filter(
      (request) => request.device_id !== deviceId,
    );
    if (this.hostkeyPending?.device_id === deviceId) this.#advanceHostKey();
    this.#authQueue = this.#authQueue.filter(
      (request) => request.device_id !== deviceId,
    );
    if (this.authPending?.device_id === deviceId) this.#advanceAuth();
  }
}

/** The application-wide remote-devices store. */
export const remoteDevices = new RemoteDeviceStore();

// Standalone function API over the singleton (what components import). --

export function startRemoteDeviceEvents(): void {
  remoteDevices.startEvents();
}
