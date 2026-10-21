/**
 * Detected projects/scripts store (Svelte 5 runes).
 *
 * `project_detect` is a cheap, bounded scan (repo root + immediate children),
 * but the Actions panel can mount/unmount with its layout group and should
 * not re-scan on every mount. This store caches the result per repository id,
 * dedupes concurrent refreshes, and exposes loading/error state for the
 * "Detected projects" section.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { projectDetect } from "$lib/ipc/client";
import type { ProjectInfo, RepoId } from "$lib/ipc/types";

export class ProjectsStore {
  /** Detection result per repository id (empty array = scanned, none found). */
  byRepo = $state<Record<string, ProjectInfo[]>>({});
  /** Refresh in flight per repository id. */
  loading = $state<Record<string, boolean>>({});
  /** Last detection error per repository id (cleared on success). */
  errors = $state<Record<string, string>>({});

  #seqs = new Map<string, number>();

  projectsFor(repoId: RepoId): ProjectInfo[] {
    return this.byRepo[repoId] ?? [];
  }

  /** True once a detection completed for this repo (even with no projects). */
  hasDetected(repoId: RepoId): boolean {
    return Object.hasOwn(this.byRepo, repoId);
  }

  loadingFor(repoId: RepoId): boolean {
    return this.loading[repoId] ?? false;
  }

  errorFor(repoId: RepoId): string | null {
    const error = this.errors[repoId];
    return error ? error : null;
  }

  /** Re-scans one repo; stale responses are ignored. */
  async refresh(repoId: RepoId): Promise<ProjectInfo[]> {
    const seq = (this.#seqs.get(repoId) ?? 0) + 1;
    this.#seqs.set(repoId, seq);
    this.loading = { ...this.loading, [repoId]: true };
    if (this.errors[repoId]) {
      const errors = { ...this.errors };
      delete errors[repoId];
      this.errors = errors;
    }
    try {
      const projects = await projectDetect(repoId);
      if (this.#seqs.get(repoId) === seq) {
        this.byRepo = { ...this.byRepo, [repoId]: projects };
      }
      return projects;
    } catch (err) {
      if (this.#seqs.get(repoId) === seq) {
        this.errors = {
          ...this.errors,
          [repoId]: err instanceof Error ? err.message : String(err),
        };
      }
      return [];
    } finally {
      if (this.#seqs.get(repoId) === seq) {
        this.loading = { ...this.loading, [repoId]: false };
      }
    }
  }

  /**
   * Scans once per repo: no-op when cached, already loading, or the last
   * attempt failed (the panel's Refresh button calls `refresh` explicitly).
   */
  async ensure(repoId: RepoId): Promise<void> {
    if (
      this.hasDetected(repoId) ||
      this.loadingFor(repoId) ||
      this.errorFor(repoId)
    ) {
      return;
    }
    await this.refresh(repoId);
  }

  /** Drops cached results (tests / explicit invalidation). */
  clear(repoId?: RepoId): void {
    if (repoId === undefined) {
      this.byRepo = {};
      this.errors = {};
      this.loading = {};
      this.#seqs.clear();
      return;
    }
    const byRepo = { ...this.byRepo };
    const errors = { ...this.errors };
    const loading = { ...this.loading };
    delete byRepo[repoId];
    delete errors[repoId];
    delete loading[repoId];
    this.byRepo = byRepo;
    this.errors = errors;
    this.loading = loading;
    this.#seqs.delete(repoId);
  }
}

/** The application-wide detected-projects store. */
export const projectsStore = new ProjectsStore();
