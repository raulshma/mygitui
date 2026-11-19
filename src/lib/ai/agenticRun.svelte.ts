/**
 * Agentic-run controller (ADR 0001, chunk 1 of the agentic-runs design).
 *
 * One instance per agentic run, owned by the opencode provider's
 * `runAgentic` path. It owns the two halves of "the agent acts on the
 * repo with the user in the loop":
 *
 *   - **Approvals**: `permission.asked` events land here. Asks matching
 *     the app-side deny list (`evaluateAsk`) are auto-rejected without
 *     surfacing (attach-mode users may sit behind a looser server
 *     policy); everything else becomes a pending approval with a
 *     {@link AGENTIC_ASK_TIMEOUT_MS} auto-reject timer per ask (an
 *     unanswered ask must never wedge the run). `pending` is reactive
 *     `$state` so chunk 2's approval cards can bind to it directly.
 *
 *   - **Budget**: a wall-clock cap and a tool-call cap per run (constants
 *     in `types.ts`, overridable via `AiConfig.agenticBudget`). A breach
 *     fires the session interrupt and settles the run as budget-stopped —
 *     the caller keeps the partial text.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Timers and the clock are injectable; tests run with fake timers.
 */

import { AGENTIC_TOOL_CALLS_MAX, AGENTIC_WALL_CLOCK_MS } from "./types";
import type { AgenticBudget } from "./types";
import { AGENTIC_DENY_MESSAGE, evaluateAsk } from "./toolPolicy";

/** A decision for one pending approval (opencode reply vocabulary). */
export type AgenticDecision = "once" | "always" | "reject";

/** One ask waiting for the user (rendered as an approval card in chunk 2). */
export interface AgenticApproval {
  /** opencode permission request id (`permission.asked` data `id`). */
  requestID: string;
  /** Tool action that asked ("shell", "edit", …). */
  action: string;
  /** Resource patterns the ask names (shell command, path, …). */
  resources: string[];
  /** Epoch ms when the ask arrived. */
  askedAt: number;
}

/** Which budget cap fired (both settle the run as budget-stopped). */
export type AgenticBudgetReason = "wall-clock" | "tool-calls";

/** Running totals over one run's approval asks (result summary). */
export interface AgenticApprovalSummary {
  /** Total asks seen (auto-rejected, approved, rejected and expired). */
  asked: number;
  /** Deny-list asks rejected without surfacing. */
  autoRejected: number;
  /** Asks the user allowed ("once" or "always"). */
  approved: number;
  /** Asks the user explicitly rejected. */
  rejected: number;
  /** Asks that hit the {@link AGENTIC_ASK_TIMEOUT_MS} auto-reject. */
  expired: number;
}

function freshApprovalSummary(): AgenticApprovalSummary {
  return { asked: 0, autoRejected: 0, approved: 0, rejected: 0, expired: 0 };
}

/** One observed tool call (`session.tool.called` → result events). */
export interface AgenticStep {
  /** Tool-call part id (also the success/failed event's `id`). */
  id: string;
  /** Raw tool input (shell command, file path, …; shape is tool-specific). */
  input: Record<string, unknown>;
  /** Whether the server executed it (`false` while blocked on approval). */
  executed: boolean;
  /** Outcome once the matching success/failed event arrived. */
  status: "ok" | "failed" | null;
}

/** Injectable timer surface (tests; defaults to the globals). */
export interface AgenticTimers {
  /** Arms `handler` after `ms`; returns an opaque handle. */
  set(handler: () => void, ms: number): unknown;
  /** Cancels a handle from {@link AgenticTimers.set}. */
  clear(handle: unknown): void;
}

/** Defaults read the globals at CALL time so fake timers apply. */
const defaultTimers: AgenticTimers = {
  set: (handler, ms) => setTimeout(handler, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Auto-reject timeout for one pending approval (120 s). */
export const AGENTIC_ASK_TIMEOUT_MS = 120_000;

/** Reply message used when an approval times out unanswered. */
export const AGENTIC_TIMEOUT_MESSAGE =
  "Approval timed out after 120 seconds; the ask was rejected.";

/** Reply message used when the run ended with an ask still undecided. */
export const AGENTIC_ENDED_MESSAGE =
  "The run ended before this approval was decided; the ask was rejected.";

/** Injectable dependencies (tests). */
export interface AgenticRunDeps {
  /** The run's opencode session id (used as the observable key). */
  sessionID: string;
  /** POSTs the reply decision for one permission request. */
  reply(requestID: string, decision: AgenticDecision, message?: string | null): Promise<void>;
  /** Interrupts the session (budget breach). */
  interrupt(): Promise<void>;
  /** Budget caps; defaults come from `types.ts` constants. */
  budget?: Partial<AgenticBudget>;
  /** Epoch-ms clock (tests). */
  now?: () => number;
  /** Injectable timers (tests); defaults to `setTimeout`/`clearTimeout`. */
  timers?: AgenticTimers;
  /** Notified when a budget cap fires and the interrupt is issued. */
  onBudgetStop?: (reason: AgenticBudgetReason) => void;
}

/**
 * The per-run approval + budget controller. Reactive state (`pending`,
 * `toolCalls`, `steps`, `budgetStopped`, `summary`) is what chunk 2
 * renders; `handlePermissionAsk` / `handleToolCalled` /
 * `handleToolFinished` are the event entry points the provider's event
 * loop feeds.
 */
export class AgenticRunController {
  readonly sessionID: string;
  /** Pending asks, oldest first (approval cards render these). */
  pending = $state<AgenticApproval[]>([]);
  /** Tool calls observed so far (budget counter + step log). */
  readonly steps = $state<AgenticStep[]>([]);
  /** True once a budget cap fired and the interrupt went out. */
  budgetStopped = $state(false);
  /** Which cap fired (`null` until then). */
  budgetReason = $state<AgenticBudgetReason | null>(null);
  /** Epoch ms when the run started (0 until {@link start}). */
  startedAt = $state(0);
  /** Approval running totals (result summary). */
  readonly summary = $state<AgenticApprovalSummary>(freshApprovalSummary());

  readonly #budget: AgenticBudget;
  readonly #now: () => number;
  readonly #timers: AgenticTimers;
  readonly #reply: AgenticRunDeps["reply"];
  readonly #interrupt: AgenticRunDeps["interrupt"];
  readonly #onBudgetStop: AgenticRunDeps["onBudgetStop"];
  /** Per-ask auto-reject timer handles, keyed by request id. */
  readonly #askTimers = new Map<string, unknown>();
  /** Every ask id seen (duplicate deliveries are ignored, not re-queued). */
  readonly #seenAsks = new Set<string>();
  /** The run-level wall-clock timer handle. */
  #wallClockTimer: unknown = null;
  #disposed = false;

  constructor(deps: AgenticRunDeps) {
    this.sessionID = deps.sessionID;
    // Clamp to the defaults like `resolveAgenticBudget` does: zero or
    // negative caps would either never fire (0/NaN) or fire instantly.
    this.#budget = {
      maxWallClockMs:
        typeof deps.budget?.maxWallClockMs === "number" && deps.budget.maxWallClockMs > 0
          ? deps.budget.maxWallClockMs
          : AGENTIC_WALL_CLOCK_MS,
      maxToolCalls:
        typeof deps.budget?.maxToolCalls === "number" && deps.budget.maxToolCalls > 0
          ? deps.budget.maxToolCalls
          : AGENTIC_TOOL_CALLS_MAX,
    };
    this.#now = deps.now ?? Date.now;
    this.#timers = deps.timers ?? defaultTimers;
    this.#reply = deps.reply;
    this.#interrupt = deps.interrupt;
    this.#onBudgetStop = deps.onBudgetStop;
  }

  /** The resolved caps this run enforces. */
  get budget(): AgenticBudget {
    return { ...this.#budget };
  }

  /**
   * Arms the run-level wall-clock cap. Called once the prompt is about to
   * go out; a breach fires {@link interrupt} and settles budget-stopped.
   */
  start(): void {
    if (this.#wallClockTimer !== null) return;
    this.startedAt = this.#now();
    this.#wallClockTimer = this.#timers.set(() => {
      this.#stopForBudget("wall-clock");
    }, this.#budget.maxWallClockMs);
  }

  /**
   * Entry point for `permission.asked` events. Deny-list asks are
   * auto-rejected WITHOUT enqueueing (bypassing the queue entirely);
   * everything else joins `pending` with its own 120 s auto-reject timer.
   * Once budget-stopped, late asks are rejected immediately — the run is
   * over and nothing new should block on the user.
   */
  handlePermissionAsk(data: { id: string; action: string; resources: string[] }): void {
    // Duplicate deliveries of the same request id (redelivered SSE events)
    // must not double-count or overwrite the id's ask timer.
    if (this.#seenAsks.has(data.id)) return;
    this.#seenAsks.add(data.id);
    this.summary.asked += 1;
    if (evaluateAsk(data.action, data.resources) === "auto-reject") {
      this.summary.autoRejected += 1;
      void this.#reply(data.id, "reject", AGENTIC_DENY_MESSAGE).catch(() => {});
      return;
    }
    if (this.budgetStopped || this.#disposed) {
      // Run already settling: reject so the server is never left hanging.
      void this.#reply(data.id, "reject", AGENTIC_ENDED_MESSAGE).catch(() => {});
      return;
    }
    const approval: AgenticApproval = {
      requestID: data.id,
      action: data.action,
      resources: data.resources,
      askedAt: this.#now(),
    };
    this.pending.push(approval);
    this.#askTimers.set(
      data.id,
      this.#timers.set(() => {
        void this.#expire(data.id);
      }, AGENTIC_ASK_TIMEOUT_MS),
    );
  }

  /**
   * Applies the user's decision to one pending ask: clears its timer,
   * removes it from `pending`, POSTs the reply (which resolves the
   * server's awaiting execution), and records the outcome. Returns
   * `false` for unknown/expired requests (the UI only renders pending
   * cards, so this is a stale-click guard) — no reply is sent then.
   */
  async respond(
    requestID: string,
    decision: AgenticDecision,
    message?: string | null,
  ): Promise<boolean> {
    const index = this.pending.findIndex((approval) => approval.requestID === requestID);
    if (index === -1) return false;
    const handle = this.#askTimers.get(requestID);
    if (handle !== undefined) {
      this.#timers.clear(handle);
      this.#askTimers.delete(requestID);
    }
    this.pending.splice(index, 1);
    if (decision === "reject") this.summary.rejected += 1;
    else this.summary.approved += 1;
    await this.#reply(requestID, decision, message ?? null);
    return true;
  }

  /** Entry point for `session.tool.called` events (budget counter + log). */
  handleToolCalled(data: { id: string; input: Record<string, unknown>; executed: boolean }): void {
    if (this.steps.some((step) => step.id === data.id)) return;
    this.steps.push({
      id: data.id,
      input: data.input,
      executed: data.executed,
      status: null,
    });
    if (this.steps.length >= this.#budget.maxToolCalls) {
      this.#stopForBudget("tool-calls");
    }
  }

  /** Entry point for `session.tool.success` / `session.tool.failed`. */
  handleToolFinished(id: string, ok: boolean): void {
    const step = this.steps.find((candidate) => candidate.id === id);
    if (step === undefined) return;
    step.status = ok ? "ok" : "failed";
  }

  /**
   * Tears the controller down at the end of the run: clears every timer
   * (no leaks) and rejects anything still pending so the server is never
   * left hanging on a dead run. Late asks after disposal are ignored.
   */
  dispose(): void {
    this.#disposed = true;
    if (this.#wallClockTimer !== null) {
      this.#timers.clear(this.#wallClockTimer);
      this.#wallClockTimer = null;
    }
    while (this.pending.length > 0) {
      const approval = this.pending[0]!;
      const handle = this.#askTimers.get(approval.requestID);
      if (handle !== undefined) {
        this.#timers.clear(handle);
        this.#askTimers.delete(approval.requestID);
      }
      this.pending.splice(0, 1);
      this.summary.expired += 1;
      void this.#reply(approval.requestID, "reject", AGENTIC_ENDED_MESSAGE).catch(() => {});
    }
  }

  // -- internals ----------------------------------------------------------------

  /** Auto-rejects one ask whose 120 s timer fired. */
  async #expire(requestID: string): Promise<void> {
    const index = this.pending.findIndex((approval) => approval.requestID === requestID);
    if (index === -1) return;
    this.#askTimers.delete(requestID);
    this.pending.splice(index, 1);
    this.summary.expired += 1;
    await this.#reply(requestID, "reject", AGENTIC_TIMEOUT_MESSAGE).catch(() => {});
  }

  /**
   * Budget breach: settle as budget-stopped, fire the interrupt (the
   * server answers with `session.execution.interrupted`, which the
   * provider maps to the partial-text result), and clear the queue so no
   * ask outlives the run.
   */
  #stopForBudget(reason: AgenticBudgetReason): void {
    if (this.budgetStopped) return;
    this.budgetStopped = true;
    this.budgetReason = reason;
    // The run is over: the wall-clock cap must not linger (dispose later
    // would clear it too, but budget-stop can outlive that path's caller).
    if (this.#wallClockTimer !== null) {
      this.#timers.clear(this.#wallClockTimer);
      this.#wallClockTimer = null;
    }
    while (this.pending.length > 0) {
      const approval = this.pending[0]!;
      const handle = this.#askTimers.get(approval.requestID);
      if (handle !== undefined) {
        this.#timers.clear(handle);
        this.#askTimers.delete(approval.requestID);
      }
      this.pending.splice(0, 1);
      this.summary.expired += 1;
      void this.#reply(approval.requestID, "reject", AGENTIC_ENDED_MESSAGE).catch(() => {});
    }
    void this.#interrupt().catch(() => {});
    this.#onBudgetStop?.(reason);
  }
}
