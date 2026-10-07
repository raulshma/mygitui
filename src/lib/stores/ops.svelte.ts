/**
 * Op + auth stores (Svelte 5 runes) — M2 mutation UI state.
 *
 * `OpStore` mirrors the backend's per-repo serial op queue: `op-progress`
 * events are upserted into a repo→ops map (fresh reference per update so
 * rune readers re-run), dropped when `done`, and failures surface as error
 * toasts. Panels drive spinners / inline progress off `busy()` /
 * `opsFor()`.
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

import { authRespond, onAuthRequest, onOpProgress } from "$lib/ipc/client";
import type { AuthRequest, OpProgress, RepoId } from "$lib/ipc/types";
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

  #started = false;
  #unlisten: (() => void) | null = null;

  /**
   * Subscribes to `op-progress` events. Idempotent: safe to call from every
   * RepoView mount.
   */
  startOpsEvents(): void {
    if (this.#started) return;
    this.#started = true;
    onOpProgress((event) => this.applyProgress(event))
      .then((unlisten) => {
        this.#unlisten = unlisten;
      })
      .catch((err: unknown) =>
        console.error("[ops] op-progress subscription failed:", err),
      );
  }

  /**
   * Applies one progress event: upserts the op, drops it when done (error
   * toasts first). Public so tests / future code can inject progress.
   */
  applyProgress(p: OpProgress): void {
    const list = this.ops[p.repo_id] ?? [];
    if (p.done) {
      if (p.error) {
        toast(`${kindLabel(p.kind)} failed: ${p.error}`, { kind: "error" });
      }
      const remaining = list.filter((op) => op.op_id !== p.op_id);
      const ops = { ...this.ops };
      if (remaining.length === 0) delete ops[p.repo_id];
      else ops[p.repo_id] = remaining;
      this.ops = ops;
      return;
    }
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

  /** Unsubscribes and clears (test helper / teardown). */
  stop(): void {
    try {
      this.#unlisten?.();
    } catch {
      // Ignore unlisten failures.
    }
    this.#unlisten = null;
    this.#started = false;
    this.ops = {};
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
  #started = false;
  #unlisten: (() => void) | null = null;

  /**
   * Subscribes to `auth-request` events. Idempotent: safe to call from
   * every RepoView mount.
   */
  startAuthEvents(): void {
    if (this.#started) return;
    this.#started = true;
    onAuthRequest((request) => this.handleRequest(request))
      .then((unlisten) => {
        this.#unlisten = unlisten;
      })
      .catch((err: unknown) =>
        console.error("[auth] auth-request subscription failed:", err),
      );
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

  /** Unsubscribes and clears everything (test helper / teardown). */
  stop(): void {
    try {
      this.#unlisten?.();
    } catch {
      // Ignore unlisten failures.
    }
    this.#unlisten = null;
    this.#started = false;
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
