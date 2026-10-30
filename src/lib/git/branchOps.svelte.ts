/**
 * Shared branch-operations controller (Svelte 5 runes) — the mutation flows
 * behind every "do something to a branch" affordance.
 *
 * The branch panel and the header branch switcher offer the same safety UX
 * for the same backend ops, so the flow logic lives here exactly once: a
 * dirty-worktree switch becomes a real choice (stash / force / cancel), a
 * delete previews first (merged → confirm, unmerged → type-the-name +
 * optional undo checkpoint), and every completed mutation reloads the
 * caller's list and notifies the repo view. The controller owns only the
 * three dialogs' state ({@link BranchOpsController.switchChoice},
 * {@link BranchOpsController.confirmDelete},
 * {@link BranchOpsController.previewDelete}) plus flow decisions; it does
 * NOT own the branch list — surfaces feed it via
 * {@link BranchOpsController.setList} (for the HEAD branch a delete counts
 * "into") and {@link BranchOpsController.setReload} (their own reload
 * function) — so both surfaces keep their own rendering, filtering and
 * panels. Render the dialogs with
 * `BranchActionDialogs.svelte`, passing the controller.
 *
 * Error toasts follow the app-wide `${label} failed: ${msg}` convention;
 * success texts are preserved verbatim from the branch panel this was
 * extracted from. Lives in a `.svelte.ts` module because `$state`/`$derived`
 * only compile there. Backend access goes exclusively through
 * `$lib/ipc/client` (mocked with `vi.mock` in tests); dirty-worktree reads
 * come from the tab store.
 */

import {
  branchCheckoutRemote,
  branchCreate,
  branchDelete,
  branchRename,
  branchSwitch,
  guardCheckpoint,
  opsPreview,
  pushRepo,
  remotes,
  stashPush,
} from "$lib/ipc/client";
import type { BranchInfo, PreviewInfo } from "$lib/ipc/types";
import { busy as opBusy } from "$lib/stores/ops.svelte";
import { tabStore } from "$lib/stores/tabs.svelte";
import { toast } from "$lib/toast";
import { isMergedPreview } from "$lib/components/safety/safetyModel";

/**
 * Reactive controller for one repo's branch mutations. Create one per
 * component via {@link branchOpsController} (never `new` it directly — the
 * factory keeps construction uniform for future surfaces).
 */
export class BranchOpsController {
  /** Repo this controller operates on (fixed at construction). */
  readonly #repoId: string;
  /** Owner's post-mutation hook (RepoView refreshStatus), may be absent. */
  readonly #onMutated: (() => void) | null;
  /** Caller-registered reload function (set via {@link setReload}). */
  #reload: (() => void | Promise<void>) | null = null;
  /** Latest branch list, fed by the owner (HEAD lookups, not rendering). */
  #list: BranchInfo[] = [];

  /** Pending dirty-switch choice dialog, or `null` when none. */
  switchChoice = $state<{ name: string } | null>(null);
  /** Pending merged-delete confirmation, or `null` when none. */
  confirmDelete = $state<{ name: string } | null>(null);
  /** Pending unmerged (force-)delete preview, or `null` when none. */
  previewDelete = $state<{ name: string; info: PreviewInfo } | null>(null);

  /** True while a "branch"-kind backend op runs for this repo. */
  readonly busy: boolean = $derived(this.#branchBusy());

  /**
   * The `busy` derived reads through this method because `#repoId` is
   * assigned in the constructor (after field initializers run) — a direct
   * reference in the initializer trips TS's use-before-initialization
   * check. Deriveds evaluate lazily, so by the first read `#repoId` is set;
   * dependency tracking happens when the method body runs.
   */
  #branchBusy(): boolean {
    return opBusy(this.#repoId, "branch");
  }

  constructor(repoId: string, onMutated?: () => void) {
    this.#repoId = repoId;
    this.#onMutated = onMutated ?? null;
  }

  /**
   * Registers the owner's reload function, called after every completed
   * mutation (the controller has no list of its own to refresh). Call once
   * right after construction; passing `undefined` keeps the last one.
   */
  setReload(reload: () => void | Promise<void>): void {
    this.#reload = reload;
  }

  /** Feeds the owner's branch list (call after every reload). */
  setList(list: BranchInfo[]): void {
    this.#list = list;
  }

  /** The HEAD branch from the fed list, or `null` (detached/unloaded). */
  headBranch(): BranchInfo | null {
    return this.#list.find((b) => b.is_head) ?? null;
  }

  // -- create / remote checkout ----------------------------------------------

  /**
   * Creates `name` from HEAD and checks it out (the fixed variant the
   * switcher uses; the panel's create form calls `branchCreate` directly
   * because it exposes from/checkout options). Resolves `true` on success,
   * `false` on failure (toasted internally) or an empty name (silent).
   */
  async create(name: string): Promise<boolean> {
    const trimmed = name.trim();
    if (!trimmed) return false;
    try {
      await branchCreate(this.#repoId, trimmed, true);
      toast(`Created branch ${trimmed}`, { kind: "success" });
      this.#afterMutation();
      return true;
    } catch (err) {
      toast(
        `Create branch ${trimmed} failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
        { kind: "error" },
      );
      return false;
    }
  }

  /**
   * Creates + checks out a local branch tracking `<remote>/<name>`.
   * Resolves the local branch name, or `null` on failure (toasted
   * internally).
   */
  async checkoutRemote(remote: string, name: string): Promise<string | null> {
    try {
      const local = await branchCheckoutRemote(this.#repoId, remote, name);
      toast(`Checked out ${local} (tracking ${remote}/${name})`, {
        kind: "success",
      });
      this.#afterMutation();
      return local;
    } catch (err) {
      toast(
        `Checkout ${name} failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
        { kind: "error" },
      );
      return null;
    }
  }

  // -- switch ------------------------------------------------------------------

  /**
   * Requests switching to `name`: a dirty worktree opens the choice dialog
   * (the user decides stash vs. discard), a clean one switches directly.
   */
  async requestSwitch(name: string): Promise<void> {
    if (this.#dirtyWorktree()) {
      // Real choice replaces the old "auto-stash" hint.
      this.switchChoice = { name };
      return;
    }
    await this.#doSwitch(name, false);
  }

  /** "Stash changes": stash_push → clean switch. */
  async chooseStash(): Promise<void> {
    if (!this.switchChoice) return;
    const name = this.switchChoice.name;
    this.switchChoice = null;
    await this.#run(`Switch to ${name}`, async () => {
      await stashPush(this.#repoId, `pre-switch stash (switching to ${name})`);
      await branchSwitch(this.#repoId, name, false);
    });
  }

  /** "Proceed anyway": forced checkout (discards local modifications). */
  async chooseForce(): Promise<void> {
    if (!this.switchChoice) return;
    const name = this.switchChoice.name;
    await this.#doSwitch(name, true);
  }

  /** Closes the switch-choice dialog without switching. */
  cancelChoice(): void {
    this.switchChoice = null;
  }

  async #doSwitch(name: string, force: boolean): Promise<void> {
    this.switchChoice = null;
    await this.#run(`Switch to ${name}`, () =>
      branchSwitch(this.#repoId, name, force),
    );
  }

  // -- delete ------------------------------------------------------------------

  /**
   * Starts a delete: preview first. Merged → confirm dialog (safe delete);
   * unmerged → preview dialog with type-the-name + undo checkpoint.
   */
  async requestDelete(branch: BranchInfo): Promise<void> {
    const into = this.headBranch()?.name ?? "HEAD";
    let info: PreviewInfo;
    try {
      info = await opsPreview(this.#repoId, "branch_delete", {
        name: branch.name,
        into,
      });
    } catch (err) {
      toast(
        `Cannot preview delete of ${branch.name}: ${
          err instanceof Error ? err.message : String(err)
        }`,
        { kind: "error" },
      );
      return;
    }
    if (isMergedPreview(info)) {
      this.confirmDelete = { name: branch.name };
      return;
    }
    this.previewDelete = { name: branch.name, info };
  }

  /** Merged-delete confirmation (safe `branch_delete`). */
  async confirmMergedDelete(): Promise<void> {
    const job = this.confirmDelete;
    if (!job) return;
    this.confirmDelete = null;
    await this.#run(`Delete ${job.name}`, () =>
      branchDelete(this.#repoId, job.name, false),
    );
  }

  /**
   * Preview dialog confirmed: optional guard checkpoint, then force delete
   * (the checkpoint toast doubles as the undo affordance's anchor).
   */
  async confirmForceDelete(createCheckpoint: boolean): Promise<void> {
    const job = this.previewDelete;
    if (!job) return;
    this.previewDelete = null;
    await this.#run(`Force delete ${job.name}`, async () => {
      if (createCheckpoint) {
        const cp = await guardCheckpoint(this.#repoId, "pre-branch-delete");
        await branchDelete(this.#repoId, job.name, true);
        toast(
          `Force deleted ${job.name} — undo available (checkpoint ${cp.id})`,
          { kind: "success" },
        );
      } else {
        await branchDelete(this.#repoId, job.name, true);
      }
    });
  }

  /** Closes both delete dialogs without deleting. */
  cancelDelete(): void {
    this.confirmDelete = null;
    this.previewDelete = null;
  }

  // -- rename / push -----------------------------------------------------------

  /**
   * Renames `oldName` → `newName`. No-op (no IPC, no toast) when the new
   * name is empty or unchanged — callers close their editor either way.
   */
  async rename(oldName: string, newName: string): Promise<void> {
    const value = newName.trim();
    if (!value || value === oldName) return;
    await this.#run(`Rename ${oldName} → ${value}`, () =>
      branchRename(this.#repoId, oldName, value),
    );
  }

  /** Pushes one branch to its default remote (optionally setting upstream). */
  async push(branch: BranchInfo, setUpstream: boolean): Promise<void> {
    await this.#run(`Push ${branch.name}`, async () => {
      const remoteList = await remotes(this.#repoId);
      const remote =
        remoteList.find((r) => r.name === "origin") ?? remoteList[0];
      if (!remote) {
        toast("No remote configured — cannot push", { kind: "error" });
        return;
      }
      const stats = await pushRepo(this.#repoId, {
        remote: remote.name,
        branch: branch.name,
        force: false,
        force_with_lease: false,
        set_upstream: setUpstream,
        refs: [],
        tags: false,
        delete: false,
      });
      toast(
        `Pushed ${branch.name} (${stats.updated_refs.length} ref${
          stats.updated_refs.length === 1 ? "" : "s"
        } updated)`,
        { kind: "success" },
      );
    });
  }

  // -- internals ---------------------------------------------------------------

  /** True when the repo's working copy has entries (dirty). */
  #dirtyWorktree(): boolean {
    const tab = tabStore.tabs.find((t) => t.id === this.#repoId);
    return Boolean(tab?.status && tab.status.entries.length > 0);
  }

  /** Reload hook + owner notification (call after a completed mutation). */
  #afterMutation(): void {
    void this.#reload?.();
    this.#onMutated?.();
  }

  /** Runs one mutation: reload + notify on success, `${label} failed` on error. */
  async #run(label: string, run: () => Promise<void>): Promise<void> {
    try {
      await run();
      this.#afterMutation();
    } catch (err) {
      toast(`${label} failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }
}

/**
 * Creates the branch-operations controller for one repo. `onMutated` (the
 * RepoView refresh hook) fires after every completed mutation; the owner's
 * list wiring is separate — call `setReload(reload)` once and
 * `setList(list)` after each reload.
 */
export function branchOpsController(
  repoId: string,
  onMutated?: () => void,
): BranchOpsController {
  return new BranchOpsController(repoId, onMutated);
}
