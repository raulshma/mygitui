<script lang="ts">
  /**
   * CompareBar (B3 lane) — two-ref compare input strip.
   *
   * HOW TO MOUNT (HistoryView already does; for RepoView or elsewhere):
   *   import CompareBar from "$lib/components/panels/CompareBar.svelte";
   *   let compareFiles: FileDiff[] = [];
   *   <CompareBar repoId={tab.id} root={repoRoot} onCompare={...} />
   *   {#if compareFiles.length}
   *     <DiffViewer files={compareFiles} />
   *   {/if}
   *
   * The bar itself owns nothing beyond the two ref inputs: `repoDiff` runs on
   * "Compare" (or Enter), and results are emitted upward via `onCompare` for
   * the parent to mount in a DiffViewer. Ref suggestions come from
   * `repo_refs` (shared `<datalist>`), so any ref name, sha or rev expression
   * the backend accepts can still be typed.
   *
   * Modes (phase 5 item 50): two-dot `A..B` diffs A→B directly; three-dot
   * `A...B` resolves merge-base(A,B) via `repo_merge_base` and diffs
   * base→B (the bar shows the resolved base in the emitted label; a failed
   * resolve — unrelated histories — shows a hint instead of a diff). The
   * target (B) side can also be a symbolic side (worktree / index / HEAD)
   * instead of a typed ref; three-dot against a symbolic side anchors on
   * HEAD (git has no `A...worktree` expression — the working tree's commit
   * IS HEAD).
   *
   * Mode + target-side kind persist per repo root (`compare.ts` prefs,
   * keyed like historyPrefs; repo ids are per-session). Popouts (no root)
   * keep session-only state.
   */
  import { repoDiff, repoMergeBase, repoRefs } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import type { DiffSide, FileDiff } from "$lib/ipc/types";
  import { Button } from "$lib/components/ui/button/index.js";
  import { describeError } from "$lib/errors";
  import {
    loadComparePrefs,
    saveComparePrefs,
    targetKindToSide,
    type CompareMode,
    type ComparePrefs,
    type CompareTargetKind,
  } from "./compare";

  let {
    repoId,
    onCompare,
    root = "",
  }: {
    repoId: string;
    /** Receives the compare result (may be empty) plus the resolved sides. */
    onCompare: (
      files: FileDiff[],
      base: string,
      target: string,
      sides?: { old: DiffSide; new: DiffSide },
    ) => void;
    /** Worktree root (persistence key); empty = session-only (popouts). */
    root?: string;
  } = $props();

  const listId = $props.id();

  let base = $state("");
  let target = $state("");
  let refNames = $state<string[]>([]);
  let loading = $state(false);
  let mode = $state<CompareMode>("twodot");
  let targetKind = $state<CompareTargetKind>("commit");
  /** The failed three-dot resolve (hint line; cleared on any input). */
  let noBase = $state("");

  $effect(() => {
    const id = repoId;
    refNames = [];
    repoRefs(id)
      .then((pairs) => {
        if (id === repoId) refNames = pairs.map(([name]) => name);
      })
      .catch(() => {
        if (id === repoId) refNames = []; // datalist is an enhancement only
      });
  });

  // Persisted choices restore once per mounting root; popouts (root "")
  // keep the defaults. Capturing the initial root is intended: the prop is
  // fixed per mounting site, like HistoryView's own prefs restore.
  // svelte-ignore state_referenced_locally
  {
    const prefs: ComparePrefs = loadComparePrefs(root);
    mode = prefs.mode;
    targetKind = prefs.targetKind;
  }

  function setMode(next: CompareMode): void {
    mode = next;
    noBase = "";
    saveComparePrefs(root, { mode, targetKind });
  }

  function setTargetKind(next: CompareTargetKind): void {
    targetKind = next;
    noBase = "";
    saveComparePrefs(root, { mode, targetKind });
  }

  /** Commit-ish the B side anchors on (symbolic sides diff against HEAD). */
  function targetCommitish(): string {
    return targetKind === "commit" ? target.trim() : "HEAD";
  }

  /** Display label of the B side (what the compare header shows). */
  function targetLabel(): string {
    return targetKind === "commit" ? target.trim() : targetKind;
  }

  async function run(): Promise<void> {
    const b = base.trim();
    if (!b) {
      toast("Enter a base ref to compare");
      return;
    }
    if (targetKind === "commit" && !target.trim()) {
      toast("Enter a target ref to compare");
      return;
    }
    loading = true;
    noBase = "";
    try {
      const t = targetLabel();
      const newSide = targetKindToSide(targetKind, targetCommitish());
      let oldSide: DiffSide = { commit: b };
      let baseLabel = b;
      if (mode === "threedot") {
        const mergeBase = await repoMergeBase(repoId, b, targetCommitish());
        if (!mergeBase) {
          noBase = `no merge base between ${b} and ${t} (unrelated histories?)`;
          return;
        }
        oldSide = { commit: mergeBase };
        baseLabel = `${b}...${t} (base ${mergeBase.slice(0, 8)})`;
      }
      const files = await repoDiff(repoId, oldSide, newSide);
      onCompare(files, baseLabel, t, { old: oldSide, new: newSide });
      if (files.length === 0) toast(`No differences between ${baseLabel} and ${t}`);
    } catch (err) {
      toast(`Compare failed: ${describeError(err)}`, {
        kind: "error",
      });
    } finally {
      loading = false;
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter") {
      event.preventDefault();
      void run();
    }
  }

  function swap(): void {
    const oldBase = base;
    base = target;
    target = oldBase;
  }
</script>

<div
  class="comparebar flex items-center gap-1.5 border-b border-border bg-card px-2 py-1"
  role="group"
  aria-label="Compare two refs"
  aria-busy={loading}
>
  <div
    class="flex flex-none items-center rounded-sm border border-border"
    role="group"
    aria-label="Compare mode"
  >
    <Button
      class="rounded-r-none border-0 leading-none font-normal text-ui-meta"
      variant={mode === "twodot" ? "secondary" : "ghost"}
      size="xs"
      onclick={() => setMode("twodot")}
      aria-pressed={mode === "twodot"}
      title="Two-dot: diff base..target directly"
      disabled={loading}
    >
      A..B
    </Button>
    <Button
      class="rounded-l-none border-0 leading-none font-normal text-ui-meta"
      variant={mode === "threedot" ? "secondary" : "ghost"}
      size="xs"
      onclick={() => setMode("threedot")}
      aria-pressed={mode === "threedot"}
      title="Three-dot: diff merge-base(base, target)...target"
      disabled={loading}
    >
      A...B
    </Button>
  </div>
  <input
    class="ref min-w-40 flex-1 rounded-sm border border-border bg-background px-2 py-1 font-mono text-xs text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring disabled:opacity-60"
    list={listId}
    placeholder="base (e.g. main)"
    aria-label="Base ref"
    bind:value={base}
    onkeydown={onKeydown}
    disabled={loading}
  />
  <Button
    class="flex-none rounded-sm leading-none font-normal text-ui-meta"
    variant="secondary"
    size="xs"
    onclick={swap}
    aria-label="Swap base and target"
    title="Swap"
    disabled={loading}
  >
    ⇄
  </Button>
  <select
    class="flex-none rounded-sm border border-border bg-background px-1 py-1 text-xs text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring disabled:opacity-60"
    aria-label="Target side"
    bind:value={targetKind}
    onchange={() => setTargetKind(targetKind)}
    disabled={loading}
  >
    <option value="commit">ref</option>
    <option value="worktree">worktree</option>
    <option value="index">index</option>
    <option value="head">HEAD</option>
  </select>
  <input
    class="ref min-w-40 flex-1 rounded-sm border border-border bg-background px-2 py-1 font-mono text-xs text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring disabled:opacity-60"
    list={listId}
    placeholder={targetKind === "commit" ? "target (e.g. HEAD~3)" : "HEAD"}
    aria-label="Target ref"
    bind:value={target}
    onkeydown={onKeydown}
    disabled={loading || targetKind !== "commit"}
  />
  <Button
    class="rounded-sm text-xs"
    size="xs"
    onclick={() => void run()}
    disabled={loading}
  >
    {loading ? "Comparing…" : "Compare"}
  </Button>
  {#if noBase}
    <span class="flex-none text-xs text-muted-foreground" role="status">{noBase}</span>
  {/if}
  <datalist id={listId}>
    {#each refNames as name (name)}
      <option value={name}></option>
    {/each}
  </datalist>
</div>
