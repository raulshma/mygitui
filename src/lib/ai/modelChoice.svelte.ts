/**
 * Per-run model choice (Svelte 5 runes) — the per-repo manual model
 * override behind the run UIs' compact model picker (item 55).
 *
 * The launchers (AiFeatureButton's picker slot, ExplainHunkPanel's header,
 * AssistantPanel-adjacent run sheets) render {@link AiModelPicker}, which
 * binds here. The selected id rides the existing job-start call:
 * `AiStore.run` merges `AiRunOptions.model` over `ctx.model`, and
 * `runFeature` already forwards `ctx.model` into the provider request
 * (`AiGenerateRequest.model`) — opencode resolves `req.model` over its
 * configured default (`opencode.ts`), OpenRouter likewise
 * (`openrouter.ts`), so no client-layer change was needed.
 *
 * Options offered by the picker: "Default" (settings-configured model) +
 * the last 3 models that actually served a successful run for the current
 * backend, derived from the job history ring (`recentModels`), + a free
 * "Custom…" text entry. The last manual choice persists per repo id under
 * `mygitui.ai.modelChoice` in the versioned envelope
 * (`$lib/state/persist`), parse-and-drop like every other store; repo ids
 * are the backend's stable repo ids (tab ids), so the choice survives
 * restarts.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { loadVersioned, writeEnvelope } from "$lib/state/persist";
import type { StorageLike } from "$lib/state/persist";
import type { AiBackend } from "./types";
import type { AiJobRecord } from "./jobs.svelte";

/** localStorage key for the per-repo model override envelope. */
export const MODEL_CHOICE_KEY = "mygitui.ai.modelChoice";

/** What persists: one manual model id per repo (absent = default). */
export interface ModelChoiceData {
  byRepo: Record<string, string>;
}

/** Parse-and-drop loader: junk rows dropped, empty ids dropped. */
export function parseModelChoice(data: unknown): ModelChoiceData | null {
  if (typeof data !== "object" || data === null) return null;
  const raw = (data as { byRepo?: unknown }).byRepo;
  if (typeof raw !== "object" || raw === null) return null;
  const byRepo: Record<string, string> = {};
  for (const [repoId, model] of Object.entries(raw as Record<string, unknown>)) {
    if (repoId.length > 0 && typeof model === "string" && model.length > 0) {
      byRepo[repoId] = model;
    }
  }
  return { byRepo };
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * The per-repo model override store — construct for tests; the app uses
 * the module-level {@link modelChoice} singleton.
 */
export class ModelChoiceStore {
  #data = $state<ModelChoiceData>({ byRepo: {} });
  readonly #storage: StorageLike | null;

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<ModelChoiceData>({
      storage,
      key: MODEL_CHOICE_KEY,
      parse: parseModelChoice,
    });
    if (loaded !== null) this.#data = loaded;
  }

  /** The manual override for `repoId` (`null` = run on the default). */
  overrideFor(repoId: string): string | null {
    return this.#data.byRepo[repoId] ?? null;
  }

  /**
   * Sets or clears the override (`null`/empty = back to the settings
   * default). Persisted on every change; storage failures keep the
   * in-memory choice usable for the session.
   */
  setOverride(repoId: string, model: string | null | undefined): void {
    const next = { byRepo: { ...this.#data.byRepo } };
    const trimmed = model?.trim();
    if (trimmed !== undefined && trimmed.length > 0 && repoId.length > 0) {
      next.byRepo[repoId] = trimmed;
    } else {
      delete next.byRepo[repoId];
    }
    this.#data = next;
    writeEnvelope(this.#storage, MODEL_CHOICE_KEY, next);
  }

  /** Clears every override (test/settings reset helper). */
  clearAll(): void {
    this.#data = { byRepo: {} };
    writeEnvelope(this.#storage, MODEL_CHOICE_KEY, this.#data);
  }
}

/** The app-wide per-repo model choice singleton. */
export const modelChoice = new ModelChoiceStore();

/** The minimal history shape {@link recentModels} needs. */
export type RecentModelRecord = Pick<AiJobRecord, "backend" | "model" | "status">;

/**
 * The models a picker offers besides "Default": the most recent DISTINCT
 * models that actually served a SUCCESSFUL run on `backend`, newest first
 * (history is newest-first), capped at `limit` (default 3). Cancelled and
 * failed runs never advertise a model — "used successfully" is the
 * contract; a model the backend rejects must not be suggested again.
 */
export function recentModels(
  history: readonly RecentModelRecord[],
  backend: AiBackend,
  limit = 3,
): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const record of history) {
    if (record.status !== "done") continue;
    if (record.backend !== backend) continue;
    const model = record.model;
    if (model === null || model.length === 0 || seen.has(model)) continue;
    seen.add(model);
    out.push(model);
    if (out.length >= limit) break;
  }
  return out;
}
