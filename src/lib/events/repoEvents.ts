/**
 * Backend push-event fan-out — one Tauri subscription per event name.
 *
 * Every backend push event (`repo-changed`, `op-progress`, `op-log`,
 * `auth-request`, `action-output`, `clone-progress`, `pty-output`, `pty-exit`,
 * `hostkey-request`, `device-auth-request`, `device-status`) used to be
 * subscribed by each consumer independently, and every one of them carried
 * its own copy of the idempotent-start flag + `#unlisten` + try/catch
 * lifecycle (tabs, ops ×2, actions, terminal, popouts, history instances).
 * This module owns that lifecycle exactly once per event name:
 *
 *   - The first {@link onRepoChange}/{@link onOpProgress}/… registration
 *     ensures the (single) backend subscription for that event name
 *     ("register-ensures-subscribed" — lazy where today's behavior was
 *     lazy, e.g. pty events only subscribe when a terminal is created).
 *     Once started, a subscription lives for the module's lifetime;
 *     unregistering the last listener only stops the forwarding, it does
 *     not tear the transport down (matches today, where stores subscribe
 *     for the whole session and only test helpers unsubscribe).
 *   - Listeners are forwarded every event whose declared `filter` accepts
 *     it, so each consumer's refresh policy is DECLARED at the
 *     registration site instead of buried in a handler body.
 *   - One bad listener cannot break the others (per-listener catch).
 *
 * Transport stays in `$lib/ipc/client`: its `on*` helpers are the seam
 * tests inject through (mocked `listen` / `isTauri`); this module is a
 * plain fan-out over them. Tests can fully tear the module down with
 * {@link resetRepoEvents}.
 */

import type {
  ActionOutputEvent,
  AuthRequest,
  CloneProgressEvent,
  DeviceAuthRequestEvent,
  DeviceStatusEvent,
  HostKeyRequestEvent,
  OpLog,
  OpProgress,
  PtyExitEvent,
  PtyOutputEvent,
  RepoChangedEvent,
} from "$lib/ipc/types";

/**
 * The client is imported as a namespace and accessed lazily (inside each
 * channel's subscribe closure): test suites that mock `$lib/ipc/client`
 * partially (only the commands they exercise) must be able to import this
 * module — named-import bindings would be resolved — and rejected — at
 * module evaluation, while a namespace property is only read when a channel
 * actually subscribes. In production every export exists, so behavior is
 * identical.
 */
import * as client from "$lib/ipc/client";

// ---------------------------------------------------------------------------
// RepoChange — the typed `repo-changed` view with named kinds
// ---------------------------------------------------------------------------

/**
 * What changed, derived from the watcher payload (src-tauri watcher.rs
 * `RepoChangedEvent`): a batch is workdir/index churn (`paths`), a
 * refs/HEAD move (`head_moved`), or a watcher-forced resync (`full`).
 */
export const REPO_CHANGE_KINDS = ["paths", "head_moved", "full"] as const;

/** One {@link REPO_CHANGE_KINDS} entry. */
export type RepoChangeKind = (typeof REPO_CHANGE_KINDS)[number];

/** One `repo-changed` event: the wire payload plus its derived `kind`. */
export type RepoChange = RepoChangedEvent & { readonly kind: RepoChangeKind };

/** Derives the named kind from the raw watcher payload (`full` wins). */
export function repoChangeKind(change: RepoChangedEvent): RepoChangeKind {
  if (change.full) return "full";
  if (change.head_moved) return "head_moved";
  return "paths";
}

/**
 * Shared "significant change" policy (the HistoryStore rule since 21571ee):
 * index-only churn (stage/unstage) cannot change the commit graph, so only
 * refs/HEAD movement or a watcher-forced full resync counts.
 */
export function isSignificant(change: RepoChange): boolean {
  return change.head_moved || change.full;
}

// ---------------------------------------------------------------------------
// Generic per-event channel (register → forward, subscription owned here)
// ---------------------------------------------------------------------------

/** A registration: an optional declared filter plus the handler. */
interface ChannelListener<E> {
  filter?: (event: E) => boolean;
  handle: (event: E) => void;
}

/**
 * One backend event name's fan-out hub: at most one live subscription,
 * created on the first registration, never re-created (except after
 * {@link resetRepoEvents}).
 */
class EventChannel<E> {
  readonly #name: string;
  readonly #subscribe: (forward: (event: E) => void) => Promise<() => void>;
  readonly #listeners = new Set<ChannelListener<E>>();
  #unlisten: (() => void) | null = null;
  #pending = false;
  #generation = 0;

  constructor(
    name: string,
    subscribe: (forward: (event: E) => void) => Promise<() => void>,
  ) {
    this.#name = name;
    this.#subscribe = subscribe;
  }

  /** Registers a listener and ensures the backend subscription. */
  register(listener: ChannelListener<E>): () => void {
    this.#listeners.add(listener);
    this.#ensureSubscribed();
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Forwards one event to every listener its filter accepts. */
  #forward(event: E): void {
    for (const listener of this.#listeners) {
      try {
        if (listener.filter && !listener.filter(event)) continue;
        listener.handle(event);
      } catch (err) {
        // One bad listener must not starve the others (or the backend
        // callback): report and keep fanning out.
        console.error(`[events] ${this.#name} listener failed:`, err);
      }
    }
  }

  #ensureSubscribed(): void {
    if (this.#unlisten !== null || this.#pending) return;
    const generation = this.#generation;
    this.#pending = true;
    // The client's on* helpers never throw and resolve with a no-op
    // unlisten outside Tauri; `Promise.resolve` additionally tolerates
    // bare `vi.fn()` mocks in tests.
    Promise.resolve(this.#subscribe((event) => this.#forward(event)))
      .then((unlisten) => {
        if (generation !== this.#generation) {
          // resetRepoEvents() ran while the subscription was in flight —
          // drop the transport registration this promise brought back.
          try {
            unlisten();
          } catch {
            // Ignore.
          }
          return;
        }
        this.#unlisten = unlisten;
        this.#pending = false;
      })
      .catch((err: unknown) => {
        console.error(`[events] ${this.#name} subscription failed:`, err);
        if (generation === this.#generation) this.#pending = false;
      });
  }

  /** Test teardown: drops every listener and unsubscribes the backend. */
  reset(): void {
    this.#generation += 1;
    this.#listeners.clear();
    try {
      this.#unlisten?.();
    } catch {
      // Ignore.
    }
    this.#unlisten = null;
    this.#pending = false;
  }
}

const repoChannel = new EventChannel<RepoChange>(
  "repo-changed",
  (forward) =>
    client.onRepoChanged((event) =>
      forward({ ...event, kind: repoChangeKind(event) }),
    ),
);
const opProgressChannel = new EventChannel<OpProgress>(
  "op-progress",
  (forward) => client.onOpProgress(forward),
);
const opLogChannel = new EventChannel<OpLog>(
  "op-log",
  (forward) => client.onOpLog(forward),
);
const authRequestChannel = new EventChannel<AuthRequest>(
  "auth-request",
  (forward) => client.onAuthRequest(forward),
);
const actionOutputChannel = new EventChannel<ActionOutputEvent>(
  "action-output",
  (forward) => client.onActionOutput(forward),
);
const cloneProgressChannel = new EventChannel<CloneProgressEvent>(
  "clone-progress",
  (forward) => client.onCloneProgress(forward),
);
const ptyOutputChannel = new EventChannel<PtyOutputEvent>(
  "pty-output",
  (forward) => client.onPtyOutput(forward),
);
const ptyExitChannel = new EventChannel<PtyExitEvent>(
  "pty-exit",
  (forward) => client.onPtyExit(forward),
);
const hostkeyRequestChannel = new EventChannel<HostKeyRequestEvent>(
  "hostkey-request",
  (forward) => client.onHostkeyRequest(forward),
);
const deviceAuthRequestChannel = new EventChannel<DeviceAuthRequestEvent>(
  "device-auth-request",
  (forward) => client.onDeviceAuthRequest(forward),
);
const deviceStatusChannel = new EventChannel<DeviceStatusEvent>(
  "device-status",
  (forward) => client.onDeviceStatus(forward),
);

// ---------------------------------------------------------------------------
// Public registrations (consumers import these; unsubscribe is synchronous)
// ---------------------------------------------------------------------------

/** A `repo-changed` registration: `filter` declares which changes matter. */
export interface RepoChangeListener {
  /**
   * Which changes this consumer reacts to. Declare the policy here —
   * e.g. {@link isSignificant} for graph consumers, a same-repo predicate
   * for popouts, `() => true` when every event matters.
   */
  filter?: (change: RepoChange) => boolean;
  /** Called for every change the filter accepted. */
  handle: (change: RepoChange) => void;
}

/** Subscribes to `repo-changed`; returns the synchronous unsubscribe. */
export function onRepoChange(listener: RepoChangeListener): () => void {
  return repoChannel.register(listener);
}

/** Subscribes to `op-progress`; returns the synchronous unsubscribe. */
export function onOpProgress(handle: (event: OpProgress) => void): () => void {
  return opProgressChannel.register({ handle });
}

/** Subscribes to `op-log` (raw hook/CLI output); returns the unsubscribe. */
export function onOpLog(handle: (event: OpLog) => void): () => void {
  return opLogChannel.register({ handle });
}

/** Subscribes to `auth-request`; returns the synchronous unsubscribe. */
export function onAuthRequest(handle: (event: AuthRequest) => void): () => void {
  return authRequestChannel.register({ handle });
}

/** Subscribes to `action-output`; returns the synchronous unsubscribe. */
export function onActionOutput(
  handle: (event: ActionOutputEvent) => void,
): () => void {
  return actionOutputChannel.register({ handle });
}

/** Subscribes to `clone-progress`; returns the synchronous unsubscribe. */
export function onCloneProgress(
  handle: (event: CloneProgressEvent) => void,
): () => void {
  return cloneProgressChannel.register({ handle });
}

/** Subscribes to `pty-output`; returns the synchronous unsubscribe. */
export function onPtyOutput(
  handle: (event: PtyOutputEvent) => void,
): () => void {
  return ptyOutputChannel.register({ handle });
}

/** Subscribes to `pty-exit`; returns the synchronous unsubscribe. */
export function onPtyExit(handle: (event: PtyExitEvent) => void): () => void {
  return ptyExitChannel.register({ handle });
}

/** Subscribes to `hostkey-request`; returns the synchronous unsubscribe. */
export function onHostKeyRequest(
  handle: (event: HostKeyRequestEvent) => void,
): () => void {
  return hostkeyRequestChannel.register({ handle });
}

/** Subscribes to `device-auth-request`; returns the synchronous unsubscribe. */
export function onDeviceAuthRequest(
  handle: (event: DeviceAuthRequestEvent) => void,
): () => void {
  return deviceAuthRequestChannel.register({ handle });
}

/** Subscribes to `device-status`; returns the synchronous unsubscribe. */
export function onDeviceStatus(
  handle: (event: DeviceStatusEvent) => void,
): () => void {
  return deviceStatusChannel.register({ handle });
}

/**
 * Test helper: drops every listener and unsubscribes every backend event,
 * so the next registration starts from a clean module (each event name
 * re-subscribes through the client seam).
 */
export function resetRepoEvents(): void {
  repoChannel.reset();
  opProgressChannel.reset();
  opLogChannel.reset();
  authRequestChannel.reset();
  actionOutputChannel.reset();
  cloneProgressChannel.reset();
  ptyOutputChannel.reset();
  ptyExitChannel.reset();
  hostkeyRequestChannel.reset();
  deviceAuthRequestChannel.reset();
  deviceStatusChannel.reset();
}
