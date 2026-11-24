/**
 * Durable Issues activity outbox and the shared per-root sync envelope.
 *
 * Cursor, seen-ID, toggle, and outbox changes are one synchronous storage
 * write. Unlike the best-effort preference helpers, `writeIssueSyncState`
 * throws: subscribers must not advance or expose events when durability fails.
 * Persisted events contain only stable IDs and display metadata, never issue
 * or comment bodies and never credentials.
 */

import { loadVersioned, type StorageLike } from "$lib/state/persist";
import { toast, type ToastOptions } from "$lib/toast.svelte";
import {
  commentNodeKey,
  issueNodeKey,
  type IssueActivityKind,
  type IssueIdentity,
} from "./model";
import { normalizeIssueRoot } from "./prefs";

export const ISSUE_SYNC_PREFIX = "mygitui.issues.sync.";
export const ISSUE_SYNC_VERSION = 1;
export const OUTBOX_LIMIT = 200;
export const OUTBOX_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1_000;
export const SEEN_ID_LIMIT = 2_000;

export type IssueSyncPhase =
  | "baseline-pending"
  | "ready"
  | "paused"
  | "storage-error"
  | "auth-error"
  | "offline";

export type IssueSyncError =
  | "storage-error"
  | "auth-error"
  | "offline"
  | "rate-limited"
  | "request-failed"
  | null;

export interface IssueActivityEvent {
  id: string;
  kind: IssueActivityKind;
  issueId: string;
  issueNumber: number;
  commentId: string | null;
  title: string;
  actor: string;
  createdAt: string;
  detectedAt: string;
  url: string;
  readAt: string | null;
}

/** FE-only persisted shape; the envelope adds `{ v, data }` on disk. */
export interface IssueSyncState {
  newIssues: boolean;
  newComments: boolean;
  phase: IssueSyncPhase;
  identity: IssueIdentity | null;
  issueCursor: string | null;
  commentCursor: string | null;
  seenIssueIds: string[];
  seenCommentIds: string[];
  baselineIssues: boolean;
  baselineComments: boolean;
  lastSuccessAt: string | null;
  lastError: IssueSyncError;
  outbox: IssueActivityEvent[];
}

export function defaultIssueSyncState(): IssueSyncState {
  return {
    newIssues: false,
    newComments: false,
    phase: "paused",
    identity: null,
    issueCursor: null,
    commentCursor: null,
    seenIssueIds: [],
    seenCommentIds: [],
    baselineIssues: false,
    baselineComments: false,
    lastSuccessAt: null,
    lastError: null,
    outbox: [],
  };
}

export function issueSyncKey(root: string): string {
  return ISSUE_SYNC_PREFIX + normalizeIssueRoot(root);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown, fallback = "", max = 2_000): string {
  return typeof value === "string" ? value.slice(0, max) : fallback;
}

function timestamp(value: unknown): string | null {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) return null;
  return value;
}

/** Rust activity continuations are opaque, same-endpoint Link paths. */
function cursor(value: unknown): string | null {
  if (
    typeof value === "string" &&
    value.startsWith("/") &&
    value.length <= 4_096 &&
    !/[\u0000-\u0020\u007f]/.test(value)
  ) {
    return value;
  }
  return timestamp(value);
}

function stableIds(value: unknown, prefix: "issue:" | "comment:"): string[] {
  if (!Array.isArray(value)) return [];
  const out: string[] = [];
  const seen = new Set<string>();
  for (const entry of value) {
    if (typeof entry !== "string" || !entry.startsWith(prefix)) continue;
    const id = entry.slice(0, 512);
    if (id.length <= prefix.length || seen.has(id)) continue;
    seen.add(id);
    out.push(id);
    if (out.length === SEEN_ID_LIMIT) break;
  }
  return out;
}

function parseIdentity(value: unknown): IssueIdentity | null {
  if (!isRecord(value)) return null;
  const executor = value.executor === "local" || value.executor === "device" ? value.executor : null;
  const deviceId = value.device_id === null ? null : text(value.device_id, "", 512) || null;
  const host = text(value.host, "", 512);
  const owner = text(value.owner, "", 512);
  const repo = text(value.repo, "", 512);
  const remote = text(value.remote, "", 512);
  const userId = text(value.user_id, "", 512);
  const userLogin = text(value.user_login, "", 512);
  if (executor === null || host === "" || owner === "" || repo === "" || remote === "" || userId === "" || userLogin === "") {
    return null;
  }
  if (executor === "device" && deviceId === null) return null;
  return {
    repo_id: text(value.repo_id, "", 512),
    executor,
    device_id: deviceId,
    host,
    owner,
    repo,
    remote,
    user_id: userId,
    user_login: userLogin,
  };
}

function parseEvent(value: unknown): IssueActivityEvent | null {
  if (!isRecord(value)) return null;
  const kind = value.kind === "issue" || value.kind === "comment" ? value.kind : null;
  const createdAt = timestamp(value.createdAt);
  const detectedAt = timestamp(value.detectedAt);
  const issueNumber = value.issueNumber;
  const commentId = value.commentId === null ? null : text(value.commentId, "", 512) || null;
  if (kind === null || createdAt === null || detectedAt === null || typeof issueNumber !== "number" || !Number.isInteger(issueNumber) || issueNumber < 1) {
    return null;
  }
  if (kind === "comment" && commentId === null) return null;
  const issueId = text(value.issueId, "", 512);
  const id = text(value.id, "", 512);
  if (issueId === "" || id === "") return null;
  const expected = kind === "issue" ? issueNodeKey(issueId) : commentNodeKey(commentId!);
  if (id !== expected) return null;
  return {
    id,
    kind,
    issueId,
    issueNumber,
    commentId,
    title: text(value.title, "", 1_000),
    actor: text(value.actor, "", 512),
    createdAt,
    detectedAt,
    url: text(value.url, "", 4_000),
    readAt: timestamp(value.readAt),
  };
}

const PHASES = new Set<IssueSyncPhase>([
  "baseline-pending",
  "ready",
  "paused",
  "storage-error",
  "auth-error",
  "offline",
]);
const SYNC_ERRORS = new Set<IssueSyncError>([
  null,
  "storage-error",
  "auth-error",
  "offline",
  "rate-limited",
  "request-failed",
]);

/** Unknown fields (including bodies/tokens) are discarded, never copied. */
export function parseIssueSyncState(data: unknown): IssueSyncState | null {
  if (!isRecord(data)) return null;
  const phase = PHASES.has(data.phase as IssueSyncPhase)
    ? (data.phase as IssueSyncPhase)
    : "paused";
  const lastError = SYNC_ERRORS.has(data.lastError as IssueSyncError)
    ? (data.lastError as IssueSyncError)
    : null;
  const outbox: IssueActivityEvent[] = [];
  const eventIds = new Set<string>();
  if (Array.isArray(data.outbox)) {
    for (const raw of data.outbox) {
      const event = parseEvent(raw);
      if (event === null || eventIds.has(event.id)) continue;
      eventIds.add(event.id);
      outbox.push(event);
    }
  }
  outbox.sort((a, b) => b.detectedAt.localeCompare(a.detectedAt) || a.id.localeCompare(b.id));
  return {
    newIssues: data.newIssues === true,
    newComments: data.newComments === true,
    phase,
    identity: parseIdentity(data.identity),
    issueCursor: cursor(data.issueCursor),
    commentCursor: cursor(data.commentCursor),
    seenIssueIds: stableIds(data.seenIssueIds, "issue:"),
    seenCommentIds: stableIds(data.seenCommentIds, "comment:"),
    baselineIssues: data.baselineIssues === true,
    baselineComments: data.baselineComments === true,
    lastSuccessAt: timestamp(data.lastSuccessAt),
    lastError,
    outbox,
  };
}

export function defaultSyncStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function readIssueSyncState(
  storage: StorageLike | null,
  root: string,
): IssueSyncState {
  const key = normalizeIssueRoot(root);
  if (key === "") return defaultIssueSyncState();
  return (
    loadVersioned<IssueSyncState>({
      storage,
      key: issueSyncKey(key),
      toVersion: ISSUE_SYNC_VERSION,
      parse: parseIssueSyncState,
    }) ?? defaultIssueSyncState()
  );
}

/** Bounded, sanitized snapshot used for every atomic write. */
export function boundIssueSyncState(state: IssueSyncState, now = Date.now()): IssueSyncState {
  const parsed = parseIssueSyncState(state) ?? defaultIssueSyncState();
  const cutoff = now - OUTBOX_MAX_AGE_MS;
  const outbox = parsed.outbox
    .filter((event) => Date.parse(event.detectedAt) >= cutoff)
    .slice(0, OUTBOX_LIMIT);
  return { ...parsed, outbox };
}

/** Atomic and synchronous. Failure is intentionally observable by callers. */
export function writeIssueSyncState(
  storage: StorageLike | null,
  root: string,
  state: IssueSyncState,
  now = Date.now(),
): void {
  if (storage === null) throw new Error("Issue activity storage is unavailable");
  const key = normalizeIssueRoot(root);
  if (key === "") throw new Error("Issue activity root is empty");
  storage.setItem(
    issueSyncKey(key),
    JSON.stringify({ v: ISSUE_SYNC_VERSION, data: boundIssueSyncState(state, now) }),
  );
}

function pushSeen(ids: readonly string[], id: string): string[] {
  return [id, ...ids.filter((entry) => entry !== id)].slice(0, SEEN_ID_LIMIT);
}

/** Marks a successful local mutation seen without producing an unread event. */
export function markLocalActivitySeen(
  storage: StorageLike | null,
  root: string,
  kind: IssueActivityKind,
  id: string,
): boolean {
  if (root === "" || id === "") return false;
  const state = readIssueSyncState(storage, root);
  if (state.identity === null) return false;
  const next: IssueSyncState =
    kind === "issue"
      ? { ...state, seenIssueIds: pushSeen(state.seenIssueIds, issueNodeKey(id)) }
      : { ...state, seenCommentIds: pushSeen(state.seenCommentIds, commentNodeKey(id)) };
  writeIssueSyncState(storage, root, next);
  return true;
}

/** Routes an already-read event without searching any unrelated repository. */
export function routeIssueActivityEvent(
  event: IssueActivityEvent,
  repoId: string | undefined,
  requestNonce = Date.now(),
): void {
  if (repoId !== undefined && typeof document !== "undefined") {
    document.dispatchEvent(
      new CustomEvent("issues-open", {
        detail: {
          repoId,
          issueNumber: event.issueNumber,
          commentId: event.commentId,
          requestNonce,
        },
      }),
    );
    return;
  }
  if (event.url !== "") {
    void import("@tauri-apps/plugin-opener")
      .then(({ openUrl }) => openUrl(event.url))
      .catch(() => {});
  }
}

export interface ActivityPresentation {
  repoId: string;
  repoName: string;
  onOpen: (event: IssueActivityEvent) => void;
}

export interface ActivityStoreOptions {
  storage?: StorageLike | null;
  now?: () => number;
  showToast?: (message: string, options?: ToastOptions) => void;
  onRetry?: (root: string) => void;
}

/** In-memory reactive index over the durable, root-keyed outboxes. */
export class IssueActivityStore {
  #storage: StorageLike | null;
  #now: () => number;
  #showToast: (message: string, options?: ToastOptions) => void;
  #onRetry?: (root: string) => void;
  #outboxes = $state<Record<string, IssueActivityEvent[]>>({});
  #warned = new Set<string>();

  constructor(options: ActivityStoreOptions = {}) {
    this.#storage = "storage" in options ? (options.storage ?? null) : defaultSyncStorage();
    this.#now = options.now ?? Date.now;
    this.#showToast = options.showToast ?? toast;
    this.#onRetry = options.onRetry;
  }

  entries(root: string): IssueActivityEvent[] {
    const key = normalizeIssueRoot(root);
    if (key === "") return [];
    const cached = this.#outboxes[key];
    if (cached !== undefined) return cached;
    const loaded = boundIssueSyncState(readIssueSyncState(this.#storage, key), this.#now()).outbox;
    this.#outboxes = { ...this.#outboxes, [key]: loaded };
    return loaded;
  }

  unreadCount(root: string): number {
    return this.entries(root).filter((event) => event.readAt === null).length;
  }

  unreadCounts(): Record<string, number> {
    const counts: Record<string, number> = {};
    for (const root of Object.keys(this.#outboxes)) counts[root] = this.unreadCount(root);
    return counts;
  }

  /** Applies an outbox only after its enclosing sync envelope was persisted. */
  applyPersisted(root: string, state: IssueSyncState): void {
    const key = normalizeIssueRoot(root);
    const outbox = boundIssueSyncState(state, this.#now()).outbox;
    this.#outboxes = { ...this.#outboxes, [key]: outbox };
    this.#warned.delete(key);
  }

  /** At most one toast per repository/scan; durable entries already exist. */
  present(root: string, events: readonly IssueActivityEvent[], presentation: ActivityPresentation): void {
    if (events.length === 0) return;
    const issues = events.filter((event) => event.kind === "issue").length;
    const comments = events.length - issues;
    const parts: string[] = [];
    if (issues > 0) parts.push(`${issues} new ${issues === 1 ? "issue" : "issues"}`);
    if (comments > 0) parts.push(`${comments} ${comments === 1 ? "comment" : "comments"}`);
    const first = events[0]!;
    this.#showToast(`${parts.join(" and ")} in ${presentation.repoName}`, {
      kind: "info",
      action: {
        label: "Open",
        run: () => {
          void this.open(root, first.id, presentation.repoId, presentation.onOpen);
        },
      },
    });
  }

  markRead(root: string, eventId: string, now = this.#now()): boolean {
    const key = normalizeIssueRoot(root);
    const state = readIssueSyncState(this.#storage, key);
    let changed = false;
    const outbox = state.outbox.map((event) => {
      if (event.id !== eventId || event.readAt !== null) return event;
      changed = true;
      return { ...event, readAt: new Date(now).toISOString() };
    });
    if (!changed) {
      this.#outboxes = { ...this.#outboxes, [key]: outbox };
      return true;
    }
    return this.#commit(key, { ...state, outbox });
  }

  markAllRead(root: string, now = this.#now()): boolean {
    const key = normalizeIssueRoot(root);
    const state = readIssueSyncState(this.#storage, key);
    const at = new Date(now).toISOString();
    const outbox = state.outbox.map((event) =>
      event.readAt === null ? { ...event, readAt: at } : event,
    );
    return this.#commit(key, { ...state, outbox });
  }

  /** Marks read, then routes. A storage failure warns but never blocks opening. */
  open(
    root: string,
    eventId: string,
    repoId?: string,
    onOpen?: (event: IssueActivityEvent) => void,
  ): void {
    const event = this.entries(root).find((entry) => entry.id === eventId);
    if (event === undefined) return;
    this.markRead(root, eventId);
    if (onOpen !== undefined) {
      onOpen(event);
      return;
    }
    routeIssueActivityEvent(event, repoId, this.#now());
  }

  warnStorage(root: string): void {
    const key = normalizeIssueRoot(root);
    if (this.#warned.has(key)) return;
    this.#warned.add(key);
    this.#showToast("Issue activity could not be saved. Automatic syncing is paused.", {
      kind: "error",
      action: {
        label: "Retry",
        run: () => {
          if (this.#onRetry !== undefined) this.#onRetry(key);
          else if (typeof document !== "undefined") {
            document.dispatchEvent(new CustomEvent("issues-activity-retry", { detail: { root: key } }));
          }
        },
      },
    });
  }

  /** Test/teardown helper; durable data is intentionally untouched. */
  clear(): void {
    this.#outboxes = {};
    this.#warned.clear();
  }

  #commit(root: string, state: IssueSyncState): boolean {
    try {
      writeIssueSyncState(this.#storage, root, state, this.#now());
      this.applyPersisted(root, state);
      return true;
    } catch {
      this.warnStorage(root);
      return false;
    }
  }
}

export const issueActivity = new IssueActivityStore();
