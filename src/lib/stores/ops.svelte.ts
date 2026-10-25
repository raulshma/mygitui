/**
 * Op + auth stores (Svelte 5 runes) — M2 mutation UI state.
 *
 * `OpStore` mirrors the backend's per-repo serial op queue: `op-progress`
 * events are upserted into a repo→ops map (fresh reference per update so
 * rune readers re-run), dropped when `done`, and failures surface as error
 * toasts. Panels drive spinners / inline progress off `busy()` /
 * `opsFor()`. Raw hook/CLI output arrives on `op-log` and is kept per repo
 * in `logs` (rendered by `OpLogStrip`) so it survives the op finishing.
 *
 * `AuthStore` holds the pending `auth-request` for the modal dialog. The FE
 * flow is: backend pauses a network op and emits `auth-request` → dialog
 * binds to `pending` → user fills it → `answer()` calls `auth_respond` and
 * the backend continues (or `dismiss()` answers empty = cancel). Additional
 * requests queue behind the visible one; `whenAnswered(opId)` lets callers
 * await the dialog result programmatically.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests).
 */

import { authRespond } from "$lib/ipc/client";
import { onAuthRequest, onOpLog, onOpProgress } from "$lib/events/repoEvents";
import type { AuthRequest, OpLog, OpProgress, RepoId } from "$lib/ipc/types";
import { toast } from "$lib/toast";

/** One in-flight (not yet done) backend op. */
export interface ActiveOp {
  op_id: string;
  kind: string;
  message: string;
  pct: number | null;
  done: boolean;
  error: string | null;
}

/** Raw output of the latest logging op in a repo (hooks / CLI fallback). */
export interface OpLogSession {
  op_id: string;
  kind: string;
  lines: string[];
  done: boolean;
  error: string | null;
}

/** Keep the hook log bounded even when a chatty hook floods the pipe. */
const MAX_LOG_LINES = 500;

/** "fetch" → "Fetch" (fallback for kinds without a friendly label). */
function kindLabel(kind: string): string {
  const known: Record<string, string> = {
    stage: "Stage",
    commit: "Commit",
    branch: "Branch",
    fetch: "Fetch",
    pull: "Pull",
    push: "Push",
    clone: "Clone",
  };
  return known[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1);
}

export class OpStore {
  /** Active ops per repo (repos with no ops have no entry). */
  ops: Record<RepoId, ActiveOp[]> = $state({});

  /** Latest op output per repo (hook/CLI lines), kept after completion. */
  logs: Record<RepoId, OpLogSession> = $state({});

  #unregister: Array<() => void> = [];

  /**
   * Subscribes to `op-progress` + `op-log` events (registration is
   * idempotent; the subscription lifecycle lives in
   * `$lib/events/repoEvents`). Safe to call from every RepoView mount.
   */
  startOpsEvents(): void {
    if (this.#unregister.length > 0) return;
    this.#unregister = [
      onOpProgress((event) => this.applyProgress(event)),
      onOpLog((event) => this.applyLog(event)),
    ];
  }

  /**
   * Applies one progress event: upserts the op, drops it when done (error
   * toasts first). Public so tests / future code can inject progress.
   */
  applyProgress(p: OpProgress): void {
    if (p.done) {
      this.#finishLog(p);
      if (p.error) {
        toast(`${kindLabel(p.kind)} failed: ${p.error}`, { kind: "error" });
      }
      const remaining = this.ops[p.repo_id] ?? [];
      const kept = remaining.filter((op) => op.op_id !== p.op_id);
      const ops = { ...this.ops };
      if (kept.length === 0) delete ops[p.repo_id];
      else ops[p.repo_id] = kept;
      this.ops = ops;
      return;
    }

    this.#clearStaleLog(p);
    const list = this.ops[p.repo_id] ?? [];
    const entry: ActiveOp = {
      op_id: p.op_id,
      kind: p.kind,
      message: p.message,
      pct: p.pct,
      done: false,
      error: p.error,
    };
    const idx = list.findIndex((op) => op.op_id === p.op_id);
    const next =
      idx === -1
        ? [...list, entry]
        : list.map((op, i) => (i === idx ? entry : op));
    this.ops = { ...this.ops, [p.repo_id]: next };
  }

  /**
   * Appends one raw output line from a hook/CLI op. A line from a different
   * op starts a fresh session (the strip always shows the newest output).
   */
  applyLog(l: OpLog): void {
    const current = this.logs[l.repo_id];
    const next: OpLogSession =
      current && current.op_id === l.op_id
        ? {
            ...current,
            lines: [...current.lines, l.line].slice(-MAX_LOG_LINES),
          }
        : {
            op_id: l.op_id,
            kind: l.kind,
            lines: [l.line],
            done: false,
            error: null,
          };
    this.logs = { ...this.logs, [l.repo_id]: next };
  }

  /** Latest log session for a repo, or `null` when nothing logged yet. */
  logsFor(repoId: RepoId): OpLogSession | null {
    return this.logs[repoId] ?? null;
  }

  /** Dismiss the repo's log session (the next hook line starts a new one). */
  clearLogs(repoId: RepoId): void {
    if (this.logs[repoId] === undefined) return;
    const logs = { ...this.logs };
    delete logs[repoId];
    this.logs = logs;
  }

  /** A new op of the same kind discards the previous run's stale output. */
  #clearStaleLog(p: OpProgress): void {
    const current = this.logs[p.repo_id];
    if (!current || current.kind !== p.kind || current.op_id === p.op_id) return;
    this.clearLogs(p.repo_id);
  }

  /** Marks the log session finished when its op completes. */
  #finishLog(p: OpProgress): void {
    const current = this.logs[p.repo_id];
    if (!current || current.op_id !== p.op_id) return;
    this.logs = {
      ...this.logs,
      [p.repo_id]: { ...current, done: true, error: p.error },
    };
  }

  /** Active ops for a repo (empty array when none). */
  opsFor(repoId: RepoId): ActiveOp[] {
    return this.ops[repoId] ?? [];
  }

  /** True while any op (of `kind` when given) runs for the repo. */
  busy(repoId: RepoId, kind?: string): boolean {
    const ops = this.opsFor(repoId);
    return kind === undefined
      ? ops.length > 0
      : ops.some((op) => op.kind === kind);
  }

  /** Deregisters and clears (test helper / teardown). */
  stop(): void {
    for (const unregister of this.#unregister) unregister();
    this.#unregister = [];
    this.ops = {};
    this.logs = {};
  }
}

/** The application-wide op store. */
export const opStore = new OpStore();

// Standalone function API over the singleton (what components import). --

export function startOpsEvents(): void {
  opStore.startOpsEvents();
}

export function opsFor(repoId: RepoId): ActiveOp[] {
  return opStore.opsFor(repoId);
}

export function busy(repoId: RepoId, kind?: string): boolean {
  return opStore.busy(repoId, kind);
}

export function opLogsFor(repoId: RepoId): OpLogSession | null {
  return opStore.logsFor(repoId);
}

export function clearOpLogs(repoId: RepoId): void {
  opStore.clearLogs(repoId);
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/** Credentials handed back to the backend by the auth dialog. */
export interface AuthAnswerPayload {
  username?: string;
  password?: string;
  /** Persist the credentials in the keyring (default false). */
  store?: boolean;
}

export class AuthStore {
  /** The auth request the dialog is showing, or `null` when none pending. */
  pending: AuthRequest | null = $state(null);

  #queue: AuthRequest[] = [];
  #waiters = new Map<string, Array<(answer: AuthAnswerPayload | null) => void>>();
  #unregister: (() => void) | null = null;

  /**
   * Subscribes to `auth-request` events (registration is idempotent; the
   * subscription lifecycle lives in `$lib/events/repoEvents`). Safe to call
   * from every RepoView mount.
   */
  startAuthEvents(): void {
    if (this.#unregister !== null) return;
    this.#unregister = onAuthRequest((request) => this.handleRequest(request));
  }

  /**
   * Registers an incoming request (public for tests): queues it and shows
   * it as soon as no other request is on screen.
   */
  handleRequest(request: AuthRequest): void {
    this.#queue.push(request);
    if (this.pending === null) this.pending = this.#queue.shift() ?? null;
  }

  /**
   * Resolves when this op's dialog is answered (with the payload) or
   * cancelled (with `null`). Multiple waiters are fine.
   */
  whenAnswered(opId: string): Promise<AuthAnswerPayload | null> {
    return new Promise((resolve) => {
      const list = this.#waiters.get(opId) ?? [];
      list.push(resolve);
      this.#waiters.set(opId, list);
    });
  }

  /**
   * Dialog submit: sends the credentials via `auth_respond` (backend
   * continues the op), resolves waiters, advances to the next queued
   * request. Never throws.
   */
  async answer(payload: AuthAnswerPayload): Promise<void> {
    const request = this.pending;
    if (!request) return;
    this.#advance();
    await this.#respond(request, payload);
    this.#resolveWaiters(request.op_id, payload);
  }

  /**
   * Dialog cancel: answers with empty credentials so the backend aborts
   * the op, resolves waiters with `null`, advances the queue. Never throws.
   */
  async dismiss(): Promise<void> {
    const request = this.pending;
    if (!request) return;
    this.#advance();
    await this.#respond(request, null);
    this.#resolveWaiters(request.op_id, null);
  }

  /** Deregisters and clears everything (test helper / teardown). */
  stop(): void {
    this.#unregister?.();
    this.#unregister = null;
    this.#queue = [];
    this.pending = null;
    this.#resolveWaitersAll(null);
  }

  #advance(): void {
    this.pending = this.#queue.shift() ?? null;
  }

  async #respond(request: AuthRequest, payload: AuthAnswerPayload | null): Promise<void> {
    try {
      await authRespond(
        request.op_id,
        payload?.username,
        payload?.password,
        payload?.store ?? false,
      );
    } catch (err: unknown) {
      toast(
        `Auth failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  #resolveWaiters(opId: string, value: AuthAnswerPayload | null): void {
    const list = this.#waiters.get(opId);
    if (!list) return;
    this.#waiters.delete(opId);
    for (const resolve of list) resolve(value);
  }

  #resolveWaitersAll(value: AuthAnswerPayload | null): void {
    for (const [opId, list] of this.#waiters) {
      this.#waiters.delete(opId);
      for (const resolve of list) resolve(value);
    }
  }
}

/** The application-wide auth store. */
export const authStore = new AuthStore();

export function startAuthEvents(): void {
  authStore.startAuthEvents();
}
