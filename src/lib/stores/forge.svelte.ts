/**
 * Forge store (Svelte 5 runes) — M6 gh-assisted GitHub flow.
 *
 * Caches, per open repo: the `gh` status (process-global, fetched once per
 * refresh), the forge context (owner/repo/branch), the PR list and — lazily,
 * per opened PR — its CI checks (keyed `"<repoId>:<number>"`). Refresh
 * actions are the store's whole write surface; stale-response races are
 * dropped with per-target sequence counters.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests); components build on the {@link forgeStore} singleton
 * or construct isolated instances.
 */

import {
  forgeContext,
  forgeStatus,
  prChecks,
  prList,
} from "$lib/ipc/client";
import type {
  CheckInfo,
  ForgeContext,
  ForgeStatus,
  PrInfo,
} from "$lib/ipc/client";
import type { RepoId } from "$lib/ipc/types";

/** Storage key for one PR's checks. */
export function checksKey(repoId: RepoId, number: number): string {
  return `${repoId}:${number}`;
}

export class ForgeStore {
  /** `gh` availability/auth (global, not per repo). */
  status = $state<ForgeStatus | null>(null);
  /** True while the status probe is in flight. */
  statusLoading = $state(false);

  /** Forge context per repo id. */
  contexts = $state<Record<string, ForgeContext>>({});
  /** Context errors per repo id (no origin / non-GitHub / detached HEAD). */
  contextErrors = $state<Record<string, string>>({});

  /** PR list per repo id. */
  prs = $state<Record<string, PrInfo[]>>({});
  /** PR-list refresh in flight per repo id. */
  prsLoading = $state<Record<string, boolean>>({});

  /** CI checks per `"<repoId>:<number>"` (lazy, per opened PR). */
  checks = $state<Record<string, CheckInfo[]>>({});
  /** Checks refresh in flight per `"<repoId>:<number>"`. */
  checksLoading = $state<Record<string, boolean>>({});

  #statusSeq = 0;
  #contextSeqs = new Map<string, number>();
  #prsSeqs = new Map<string, number>();
  #checksSeqs = new Map<string, number>();

  /** Probes `gh` status (once per call; panels call this on mount). */
  async refreshStatus(): Promise<ForgeStatus | null> {
    this.statusLoading = true;
    const seq = ++this.#statusSeq;
    try {
      const status = await forgeStatus();
      if (seq === this.#statusSeq) this.status = status;
      return status;
    } catch (err) {
      if (seq === this.#statusSeq) {
        this.status = { available: false, version: "", authed: false };
        console.warn("[forge] status probe failed:", err);
      }
      return null;
    } finally {
      if (seq === this.#statusSeq) this.statusLoading = false;
    }
  }

  /** Refreshes (or clears) one repo's forge context. */
  async refreshContext(repoId: RepoId): Promise<ForgeContext | null> {
    const seq = (this.#contextSeqs.get(repoId) ?? 0) + 1;
    this.#contextSeqs.set(repoId, seq);
    try {
      const context = await forgeContext(repoId);
      if (seq === this.#contextSeqs.get(repoId)) {
        this.contexts = { ...this.contexts, [repoId]: context };
        const errors = { ...this.contextErrors };
        delete errors[repoId];
        this.contextErrors = errors;
      }
      return context;
    } catch (err) {
      if (seq === this.#contextSeqs.get(repoId)) {
        const message = err instanceof Error ? err.message : String(err);
        this.contextErrors = { ...this.contextErrors, [repoId]: message };
      }
      return null;
    }
  }

  /** Refreshes one repo's PR list (resolves [] on error — panel toasts). */
  async refreshPrs(repoId: RepoId): Promise<PrInfo[]> {
    const seq = (this.#prsSeqs.get(repoId) ?? 0) + 1;
    this.#prsSeqs.set(repoId, seq);
    this.prsLoading = { ...this.prsLoading, [repoId]: true };
    try {
      const list = await prList(repoId);
      if (seq === this.#prsSeqs.get(repoId)) {
        this.prs = { ...this.prs, [repoId]: list };
      }
      return list;
    } catch (err) {
      console.warn("[forge] pr list failed:", err);
      if (seq === this.#prsSeqs.get(repoId)) {
        this.prs = { ...this.prs, [repoId]: [] };
      }
      return [];
    } finally {
      if (seq === this.#prsSeqs.get(repoId)) {
        this.prsLoading = { ...this.prsLoading, [repoId]: false };
      }
    }
  }

  /** Lazily refreshes one PR's checks (keyed per repo+number). */
  async refreshChecks(repoId: RepoId, number: number): Promise<CheckInfo[]> {
    const key = checksKey(repoId, number);
    const seq = (this.#checksSeqs.get(key) ?? 0) + 1;
    this.#checksSeqs.set(key, seq);
    this.checksLoading = { ...this.checksLoading, [key]: true };
    try {
      const list = await prChecks(repoId, number);
      if (seq === this.#checksSeqs.get(key)) {
        this.checks = { ...this.checks, [key]: list };
      }
      return list;
    } catch (err) {
      console.warn("[forge] pr checks failed:", err);
      if (seq === this.#checksSeqs.get(key)) {
        this.checks = { ...this.checks, [key]: [] };
      }
      return [];
    } finally {
      if (seq === this.#checksSeqs.get(key)) {
        this.checksLoading = { ...this.checksLoading, [key]: false };
      }
    }
  }

  /**
   * Panel mount bundle: status (global, skipped when already known),
   * context and PR list for `repoId` — in parallel.
   */
  async ensure(repoId: RepoId): Promise<void> {
    const jobs: Promise<unknown>[] = [this.refreshContext(repoId), this.refreshPrs(repoId)];
    if (!this.status) jobs.push(this.refreshStatus());
    await Promise.all(jobs);
  }

  /** Everything a PR creation should refresh: context + list. */
  async afterCreate(repoId: RepoId): Promise<void> {
    await Promise.all([this.refreshContext(repoId), this.refreshPrs(repoId)]);
  }

  // -- read helpers ---------------------------------------------------------

  contextFor(repoId: RepoId): ForgeContext | null {
    return this.contexts[repoId] ?? null;
  }

  contextErrorFor(repoId: RepoId): string | null {
    return this.contextErrors[repoId] ?? null;
  }

  prsFor(repoId: RepoId): PrInfo[] {
    return this.prs[repoId] ?? [];
  }

  prsLoadingFor(repoId: RepoId): boolean {
    return this.prsLoading[repoId] ?? false;
  }

  checksFor(repoId: RepoId, number: number): CheckInfo[] | null {
    return this.checks[checksKey(repoId, number)] ?? null;
  }

  checksLoadingFor(repoId: RepoId, number: number): boolean {
    return this.checksLoading[checksKey(repoId, number)] ?? false;
  }
}

/** The application-wide forge store. */
export const forgeStore = new ForgeStore();

/**
 * Opens a URL in the system browser via the opener plugin. No-op (resolves
 * `false`) outside a Tauri webview or on failure — never throws.
 */
export async function openExternal(url: string): Promise<boolean> {
  if (!url) return false;
  try {
    const { isTauri } = await import("$lib/entry/dragdrop");
    if (!isTauri()) return false;
    const opener = await import("@tauri-apps/plugin-opener");
    await opener.openUrl(url);
    return true;
  } catch (err) {
    console.warn("[forge] open url failed:", err);
    return false;
  }
}
