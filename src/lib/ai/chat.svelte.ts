/**
 * Assistant chat store (M13) — the repo-aware Q&A conversation.
 *
 * Conversations are keyed by repo ROOT (not repoId): ids are per-launch,
 * while the root path identifies the repository across restarts — the same
 * insight the actions store reached. Threads persist under the enveloped
 * `mygitui.ai.chat` key, capped at {@link CHAT_MESSAGE_CAP} messages
 * (newest kept), {@link CHAT_MESSAGE_TEXT_CAP} characters per message and
 * {@link CHAT_THREAD_CAP} roots, so a runaway model cannot wedge storage;
 * messages are plain git data + model output, never secrets.
 *
 * A chat turn is its own engine run with its OWN busy slot per root — it
 * does not take the one-feature-at-a-time `AiStore.run` gate — but it
 * respects the same gates: master switch (`enabled`) and per-repo opt-in
 * (`isOptedIn`), both injected by `AiStore` so tests stay hermetic. Every
 * turn also mints an `AiJobTicket` (`feature: "chat"`), so turns are as
 * visible as feature runs: a status-bar row with phase + elapsed + cancel,
 * and an activity-history record afterwards.
 *
 * The advisory contract lives in {@link CHAT_SYSTEM}: the assistant never
 * mutates anything; it explains and suggests, and the user acts. Gathers
 * are best-effort — a chip whose data cannot be read degrades to a note
 * in the prompt instead of failing the turn.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { loadVersioned, writeEnvelope } from "$lib/state/persist";
import type { StorageLike } from "$lib/state/persist";
import type { DiffSide, FileDiff, LogFilter, LogPage } from "$lib/ipc/types";
import { AiError } from "./types";
import type { AiBackend, AiUsage } from "./types";
import type { FeatureRouter } from "./features";
import { budgetedDiffText } from "./diffBudget";
import { formatFileDiff, gatherCommits } from "./features";
import { describeError } from "$lib/errors";
import { AiJobStore } from "./jobs.svelte";

/** localStorage key for the enveloped chat threads. */
export const CHAT_STORAGE_KEY = "mygitui.ai.chat";
/** Stored messages per thread, newest kept (user + assistant together). */
export const CHAT_MESSAGE_CAP = 60;
/** Threads (repo roots) kept, oldest-inserted dropped — storage stays bounded. */
export const CHAT_THREAD_CAP = 20;
/** Character cap per stored message (either role) — mirrors `JOB_TEXT_CAP`
 * so a runaway model cannot wedge the storage envelope. */
export const CHAT_MESSAGE_TEXT_CAP = 20_000;
/** How many trailing messages ride along as the conversation transcript. */
export const CHAT_WINDOW = 12;
/** Character budget for the staged-diff context block. */
export const CHAT_STAGED_CHARS = 24_000;
/** Character budget for a single file's working-copy diff block. */
export const CHAT_FILE_CHARS = 16_000;
/** Recent commits listed by the history context block. */
export const CHAT_COMMIT_COUNT = 10;
/** Deadline for one chat turn (mirrors `runFeature`). */
const CHAT_TIMEOUT_MS = 120_000;

/** The assistant's advisory contract + answer style. */
export const CHAT_SYSTEM = [
  "You are the assistant inside mygitui, a desktop Git client.",
  "You help the developer understand and work with the currently open repository.",
  "You NEVER modify the repository: no commits, no staging, no file writes, no commands executed.",
  "You only explain and suggest — when action is needed, describe exactly what the user could run or click themselves.",
  "Answer in concise GitHub-flavored markdown; cite file paths and commit shas from the context when they matter.",
  "If the provided context does not contain the answer, say so instead of guessing.",
  "Never include secrets, API keys, tokens, passwords or credentials in your output, even if they appear in the provided data.",
].join(" ");

/** One chat bubble. `role: "assistant"` carries serving metadata. */
export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
  /** Epoch ms when the message was created. */
  at: number;
  // -- assistant only -------------------------------------------------------
  backend?: AiBackend | null;
  model?: string | null;
  /** Round-trip duration of the turn. */
  elapsedMs?: number;
  usage?: AiUsage | null;
  /** Safe failure text; `text` may still hold partial streamed output. */
  error?: string | null;
  /** Turn was cancelled by the user (text = what arrived before cancel). */
  cancelled?: boolean;
}

/** Which context chips the user enabled for one turn. */
export interface ChatContextSelection {
  /** Staged diff (budgeted per file). */
  staged: boolean;
  /** Recent commit history (first log page, capped). */
  commits: boolean;
  /** Working-copy diff for this specific path (the center pane's file). */
  path?: string | null;
  /** Free-text paste box content. */
  pasted?: string | null;
}

/** One send request. */
export interface ChatSendOptions {
  repoId: string;
  /** Repo root — the thread key (stable across launches). */
  root: string;
  repoName?: string | null;
  question: string;
  context: ChatContextSelection;
}

/** Git data providers chat needs (subset of `$lib/ipc/client`). */
export interface ChatGitClient {
  repoDiff(repoId: string, oldSide: DiffSide, newSide: DiffSide): Promise<FileDiff[]>;
  streamLog(
    repoId: string,
    filter: LogFilter,
    onPage: (page: LogPage) => void,
  ): Promise<void>;
}

/** Injectable dependencies (tests). */
export interface ChatStoreDeps {
  /** Storage for the thread envelope; `null` disables persistence. */
  storage?: StorageLike | null;
  /** Job store the turns report into (defaults to an isolated store). */
  jobs?: AiJobStore;
  /** Request routing (defaults to an unroutable stub; tests inject). */
  router?: FeatureRouter;
  /** Git data provider (defaults to the real IPC client, lazily). */
  git?: ChatGitClient;
  /** Master-switch gate; `false` refuses sends (default: on). */
  enabled?: () => boolean;
  /** Per-repo opt-in gate (default: opted in — tests wire the real one). */
  isOptedIn?: (repoId: string) => boolean;
  /** Epoch-ms clock (tests). */
  now?: () => number;
  /** Message id minting (tests). */
  id?: () => string;
}

/** Parse-and-drop validation for persisted threads. */
export function parseChatThreads(data: unknown): Record<string, ChatMessage[]> | null {
  if (typeof data !== "object" || data === null) return null;
  const out: Record<string, ChatMessage[]> = {};
  for (const [root, raw] of Object.entries(data as Record<string, unknown>)) {
    if (typeof root !== "string" || root === "" || !Array.isArray(raw)) continue;
    const messages: ChatMessage[] = [];
    for (const item of raw) {
      if (typeof item !== "object" || item === null) continue;
      const m = item as Record<string, unknown>;
      if (
        typeof m.id !== "string" ||
        (m.role !== "user" && m.role !== "assistant") ||
        typeof m.text !== "string" ||
        typeof m.at !== "number"
      ) {
        continue;
      }
      messages.push({
        id: m.id,
        role: m.role,
        // Capped at parse too: a payload written before the write-side cap
        // must not wedge the envelope on load.
        text: capSafe(m.text, CHAT_MESSAGE_TEXT_CAP),
        at: m.at,
        backend: m.backend === "opencode" || m.backend === "openrouter" ? m.backend : null,
        model: typeof m.model === "string" ? m.model : null,
        elapsedMs: typeof m.elapsedMs === "number" ? m.elapsedMs : undefined,
        usage: typeof m.usage === "object" && m.usage !== null ? (m.usage as AiUsage) : undefined,
        error: typeof m.error === "string" ? m.error : undefined,
        cancelled: m.cancelled === true ? true : undefined,
      });
    }
    if (messages.length > 0) out[root] = messages.slice(-CHAT_MESSAGE_CAP);
  }
  const roots = Object.keys(out);
  for (const stale of roots.slice(0, Math.max(0, roots.length - CHAT_THREAD_CAP))) {
    delete out[stale];
  }
  return Object.keys(out).length > 0 ? out : null;
}

function emptyThreads(): Record<string, ChatMessage[]> {
  return {};
}

/** The assistant chat store — owned by `AiStore`; construct for tests. */
export class ChatStore {
  /** Threads by repo root (deep-reactive; persisted on every change). */
  readonly threads = $state<Record<string, ChatMessage[]>>({});
  /** One in-flight turn per root (own slot, separate from feature runs). */
  readonly busy = $state<Record<string, boolean>>({});

  readonly #storage: StorageLike | null;
  readonly #jobs: AiJobStore;
  readonly #router: FeatureRouter;
  readonly #git: ChatGitClient | null;
  readonly #enabled: () => boolean;
  readonly #isOptedIn: (repoId: string) => boolean;
  readonly #now: () => number;
  readonly #mintId: () => string;
  /** root → in-flight job id (cancel + streaming lookup). */
  readonly #active = new Map<string, string>();

  constructor(deps: ChatStoreDeps = {}) {
    this.#storage = deps.storage === undefined ? defaultStorage() : deps.storage;
    this.#jobs = deps.jobs ?? defaultJobs();
    this.#router = deps.router ?? unroutableRouter();
    this.#git = deps.git ?? null;
    this.#enabled = deps.enabled ?? (() => true);
    this.#isOptedIn = deps.isOptedIn ?? (() => true);
    this.#now = deps.now ?? Date.now;
    this.#mintId =
      deps.id ??
      (() => `chat-${Math.random().toString(36).slice(2, 10)}-${this.#now().toString(36)}`);
    const loaded =
      loadVersioned<Record<string, ChatMessage[]>>({
        storage: this.#storage,
        key: CHAT_STORAGE_KEY,
        parse: parseChatThreads,
      }) ?? emptyThreads();
    this.threads = loaded;
  }

  /** The thread for `root` (`[]` when none yet — safe for templates). */
  conversationFor(root: string): ChatMessage[] {
    return this.threads[root] ?? [];
  }

  /** The in-flight job id for `root` (status-bar/panel cancel lookup). */
  activeJobId(root: string): string | null {
    return this.#active.get(root) ?? null;
  }

  /**
   * Sends one user turn: appends the bubble, gathers enabled context
   * (best-effort), builds the transcript prompt, and streams the reply
   * through the router. Resolves with the assistant message. Throws
   * `AiError` on gate refusal or request failure — the user bubble stays
   * so the question is never lost, and a failed turn appends an error
   * assistant bubble the panel can offer a retry on.
   */
  async send(opts: ChatSendOptions): Promise<ChatMessage> {
    if (!this.#enabled()) {
      throw new AiError("opt-in", "AI features are turned off in AI settings", {});
    }
    if (!this.#isOptedIn(opts.repoId)) {
      throw new AiError(
        "opt-in",
        "AI features are not enabled for this repository — allow them when prompted or in AI settings",
        {},
      );
    }
    const question = opts.question.trim();
    if (question.length === 0) {
      throw new AiError("unknown", "the question is empty", {});
    }
    if (this.busy[opts.root]) {
      throw new AiError("unknown", "an assistant request is already running", {});
    }

    // The user bubble lands before anything can fail, so the question
    // survives a gate/gather/generate error for a one-click retry.
    this.#append(opts.root, {
      id: this.#mintId(),
      role: "user",
      text: question,
      at: this.#now(),
    });
    this.busy[opts.root] = true;

    const ticket = this.#jobs.begin({
      repoId: opts.repoId,
      repoName: opts.repoName ?? null,
      feature: "chat",
    });
    this.#active.set(opts.root, ticket.id);
    const startedAt = this.#now();

    // Deadline + external cancel share one controller (runFeature's
    // pattern): whichever fires first aborts, and the catch tells a user
    // cancel apart from the timeout. Declared before the try so EVERY
    // failure below — context gather included — reaches the same terminal
    // transition; a ticket left in `active` would pin its status-bar row
    // forever.
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let streamed = "";
    try {
      ticket.phase("gathering", "collecting context");
      const context = await this.#gatherContext(opts.repoId, opts.context);

      ticket.phase("prompting", "building prompt");
      const history = this.conversationFor(opts.root).slice(-CHAT_WINDOW - 1, -1);
      const prompt = chatPrompt(opts.root, history, context, question);
      ticket.promptPreview(`${CHAT_SYSTEM}\n\n${prompt}`);

      timer = setTimeout(() => controller.abort(), CHAT_TIMEOUT_MS);
      const onExternalAbort = () => controller.abort();
      if (ticket.signal.aborted) controller.abort();
      else ticket.signal.addEventListener("abort", onExternalAbort, { once: true });

      ticket.phase("generating");
      const onDelta = (delta: string): void => {
        streamed += delta;
        ticket.delta(delta);
      };
      const result =
        this.#router.generateStream !== undefined
          ? await this.#router.generateStream(
              {
                system: CHAT_SYSTEM,
                prompt,
                sessionKey: opts.repoId,
                signal: controller.signal,
                onServing: (backend, model) => ticket.serving(backend, model),
              },
              onDelta,
            )
          : await this.#router.generate({
              system: CHAT_SYSTEM,
              prompt,
              sessionKey: opts.repoId,
              signal: controller.signal,
              onServing: (backend, model) => ticket.serving(backend, model),
            });

      const elapsedMs = this.#now() - startedAt;
      const reply: ChatMessage = {
        id: this.#mintId(),
        role: "assistant",
        text: result.text || streamed,
        at: this.#now(),
        backend: result.backend,
        model: result.model,
        elapsedMs,
        usage: result.usage ?? null,
      };
      ticket.done({
        backend: result.backend,
        model: result.model,
        elapsedMs,
        usage: result.usage ?? null,
        text: reply.text,
      });
      this.#append(opts.root, reply);
      return reply;
    } catch (err) {
      // Partial output still matters: keep what streamed before the
      // abort so a cancelled long answer is not simply lost.
      const partial = streamed.trim();
      const userCancelled =
        this.#jobs.cancelRequested(ticket.id) ||
        (isAbort(err) && !controller.signal.aborted);
      if (userCancelled) {
        const cancelled: ChatMessage = {
          id: this.#mintId(),
          role: "assistant",
          text: partial,
          at: this.#now(),
          cancelled: true,
        };
        ticket.cancelled();
        this.#append(opts.root, cancelled);
        throw new AiError("cancelled", "assistant request cancelled", {});
      }
      // The deadline aborts the same controller a user cancel does, but
      // nobody requested a cancel — report the timeout honestly
      // (runFeature's rule) instead of the provider's "AI request
      // cancelled", which would read as if the user had stopped it.
      const deadline = controller.signal.aborted;
      const message = deadline
        ? "the AI backend did not finish in time"
        : describeError(err);
      const failed: ChatMessage = {
        id: this.#mintId(),
        role: "assistant",
        text: partial,
        at: this.#now(),
        error: message,
      };
      ticket.failed(message);
      this.#append(opts.root, failed);
      // The deadline rethrows the honest timeout (runFeature's rule) — the
      // provider's own rejection reads "AI request cancelled", which would
      // blame the user's cancel in the panel's toast.
      throw deadline
        ? new AiError("unavailable", "the AI backend did not finish in time", {})
        : err;
    } finally {
      if (timer !== undefined) clearTimeout(timer);
      this.busy[opts.root] = false;
      this.#active.delete(opts.root);
    }
  }

  /** Cancels the in-flight turn for `root` (no-op when idle). */
  stop(root: string): void {
    const jobId = this.#active.get(root);
    if (jobId !== undefined) this.#jobs.cancel(jobId);
  }

  /** Drops the thread for `root` (memory + storage). */
  clear(root: string): void {
    delete this.threads[root];
    this.#persist();
  }

  // -- internals ---------------------------------------------------------------

  #append(root: string, message: ChatMessage): void {
    // Per-message text cap (either role): a runaway model answer or a
    // giant pasted question must not wedge the storage envelope — the
    // stored bubble truncates, the caller's copy stays whole.
    const stored =
      message.text.length > CHAT_MESSAGE_TEXT_CAP
        ? { ...message, text: capSafe(message.text, CHAT_MESSAGE_TEXT_CAP) }
        : message;
    const next = [...(this.threads[root] ?? []), stored].slice(-CHAT_MESSAGE_CAP);
    this.threads[root] = next;
    // Cap the thread count too (oldest-inserted roots drop first), so
    // opening many repositories cannot grow storage without bound.
    const roots = Object.keys(this.threads);
    for (const stale of roots.slice(0, Math.max(0, roots.length - CHAT_THREAD_CAP))) {
      delete this.threads[stale];
    }
    this.#persist();
  }

  #persist(): void {
    writeEnvelope(this.#storage, CHAT_STORAGE_KEY, this.threads);
  }

  /**
   * Reads the enabled context chips. Best-effort by design: each block is
   * independent and a read failure downgrades that block to a note rather
   * than failing the whole turn — the assistant still answers from the
   * transcript and the other chips.
   */
  async #gatherContext(
    repoId: string,
    selection: ChatContextSelection,
  ): Promise<string> {
    const blocks: string[] = [];
    const git = this.#git ?? (await defaultGitClient());

    if (selection.staged) {
      try {
        const files = await git.repoDiff(repoId, "head", "index");
        const text = budgetedDiffText(files, { maxChars: CHAT_STAGED_CHARS, perFileCap: 6_000 });
        blocks.push(`### Staged changes\n\n\`\`\`diff\n${text}\n\`\`\``);
      } catch {
        blocks.push("### Staged changes\n\n(not available right now)");
      }
    }

    if (selection.commits) {
      try {
        const commits = (await gatherCommits(git, repoId)).slice(0, CHAT_COMMIT_COUNT);
        const lines =
          commits.length > 0
            ? commits.map((c) => `- \`${c.sha.slice(0, 8)}\` ${c.summary}`).join("\n")
            : "(no commits)";
        blocks.push(`### Recent commits (newest first)\n\n${lines}`);
      } catch {
        blocks.push("### Recent commits\n\n(not available right now)");
      }
    }

    if (selection.path) {
      try {
        const files = await git.repoDiff(repoId, "head", "worktree");
        const file = files.find((f) => f.path === selection.path || f.old_path === selection.path);
        if (file === undefined) {
          blocks.push(`### Current file: \`${selection.path}\`\n\n(no working-copy changes)`);
        } else {
          const text = capSafe(formatFileDiff(file), CHAT_FILE_CHARS);
          blocks.push(`### Current file: \`${selection.path}\` (working-copy diff)\n\n\`\`\`diff\n${text}\n\`\`\``);
        }
      } catch {
        blocks.push(`### Current file: \`${selection.path}\`\n\n(not available right now)`);
      }
    }

    const pasted = selection.pasted?.trim();
    if (pasted) {
      blocks.push(`### Pasted notes\n\n${capSafe(pasted, CHAT_FILE_CHARS)}`);
    }

    return blocks.length > 0 ? blocks.join("\n\n") : "(no extra context selected)";
  }
}

// ---------------------------------------------------------------------------
// Prompt assembly (pure)
// ---------------------------------------------------------------------------

/** Renders one prior message for the transcript. */
function transcriptLine(message: ChatMessage): string | null {
  if (message.role === "user") return `user: ${message.text}`;
  // Error/cancelled assistant turns contribute noise, not context — skip
  // them so a failed exchange does not confuse the next answer.
  if (message.error || message.cancelled || message.text.length === 0) return null;
  return `assistant: ${message.text}`;
}

/** Builds the user prompt: repo header, context blocks, transcript, question. */
export function chatPrompt(
  root: string,
  history: ChatMessage[],
  context: string,
  question: string,
): string {
  const parts: string[] = [`Repository: ${root}`];
  if (context) parts.push(`Context from the app:\n\n${context}`);
  const transcript = history
    .map(transcriptLine)
    .filter((line): line is string => line !== null)
    .join("\n\n");
  if (transcript) parts.push(`Conversation so far:\n\n${transcript}`);
  parts.push(`Question: ${question}`);
  return parts.join("\n\n");
}

/** Cap helper that never throws on non-string input. */
function capSafe(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max)}\n[truncated]`;
}

/** True when `err` is an abort (user cancel or deadline), best-effort. */
function isAbort(err: unknown): boolean {
  return err instanceof DOMException && err.name === "AbortError";
}

/** Lazily wires the real git client (dynamic import keeps tests hermetic). */
async function defaultGitClient(): Promise<ChatGitClient> {
  const client = await import("$lib/ipc/client");
  return {
    repoDiff: (repoId, oldSide, newSide) => client.repoDiff(repoId, oldSide, newSide),
    streamLog: (repoId, filter, onPage) => client.streamLog(repoId, filter, onPage),
  };
}

/** Fallback job store when none is injected (isolated, no persistence). */
function defaultJobs(): AiJobStore {
  return new AiJobStore({ storage: null });
}

/** Fallback router that fails loudly — chat without routing is a bug. */
function unroutableRouter(): FeatureRouter {
  return {
    async generate() {
      throw new AiError("unavailable", "no AI backend is configured", {});
    },
  };
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}
