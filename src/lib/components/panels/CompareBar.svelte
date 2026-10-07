<script lang="ts">
  /**
   * CompareBar (B3 lane) — two-ref compare input strip.
   *
   * HOW TO MOUNT (HistoryView already does; for RepoView or elsewhere):
   *   import CompareBar from "$lib/components/panels/CompareBar.svelte";
   *   let compareFiles: FileDiff[] = [];
   *   <CompareBar
   *     repoId={tab.id}
   *     onCompare={(files, base, target) => (compareFiles = files)} />
   *   {#if compareFiles.length}
   *     <DiffViewer files={compareFiles} />
   *   {/if}
   *
   * The bar itself owns nothing beyond the two ref inputs: `repoDiff` runs on
   * "Compare" (or Enter), and results are emitted upward via `onCompare` for
   * the parent to mount in a DiffViewer. Ref suggestions come from
   * `repo_refs` (shared `<datalist>`), so any ref name, sha or rev expression
   * the backend accepts can still be typed.
   */
  import { repoDiff, repoRefs } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import type { FileDiff } from "$lib/ipc/types";

  let {
    repoId,
    onCompare,
  }: {
    repoId: string;
    /** Receives the compare result (may be empty) plus the resolved sides. */
    onCompare: (files: FileDiff[], base: string, target: string) => void;
  } = $props();

  const listId = $props.id();

  let base = $state("");
  let target = $state("");
  let refNames = $state<string[]>([]);
  let loading = $state(false);

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

  async function run(): Promise<void> {
    const b = base.trim();
    const t = target.trim();
    if (!b || !t) {
      toast("Enter two refs to compare");
      return;
    }
    loading = true;
    try {
      const files = await repoDiff(repoId, { commit: b }, { commit: t });
      onCompare(files, b, t);
      if (files.length === 0) toast(`No differences between ${b} and ${t}`);
    } catch (err) {
      toast(`Compare failed: ${err instanceof Error ? err.message : String(err)}`, {
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

<div class="comparebar" role="group" aria-label="Compare two refs" aria-busy={loading}>
  <input
    class="ref"
    list={listId}
    placeholder="base (e.g. main)"
    aria-label="Base ref"
    bind:value={base}
    onkeydown={onKeydown}
    disabled={loading}
  />
  <button class="swap" onclick={swap} aria-label="Swap base and target" title="Swap" disabled={loading}>
    ⇄
  </button>
  <input
    class="ref"
    list={listId}
    placeholder="target (e.g. HEAD~3)"
    aria-label="Target ref"
    bind:value={target}
    onkeydown={onKeydown}
    disabled={loading}
  />
  <button class="go" onclick={() => void run()} disabled={loading}>
    {loading ? "Comparing…" : "Compare"}
  </button>
  <datalist id={listId}>
    {#each refNames as name (name)}
      <option value={name}></option>
    {/each}
  </datalist>
</div>

<style>
  .comparebar {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  input.ref {
    min-width: 10rem;
    flex: 1;
    padding: 0.25rem 0.5rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface);
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  input.ref:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  input.ref:disabled {
    opacity: 0.6;
  }

  button {
    padding: 0.25rem 0.625rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-high, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
  }

  button:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  button:disabled {
    opacity: 0.6;
    cursor: default;
  }

  button.go {
    color: var(--m3-on-primary);
    background: var(--m3-primary);
    border-color: var(--m3-primary);
    font-weight: 500;
  }

  button.swap {
    flex: none;
    font-size: 0.875rem;
    line-height: 1;
  }
</style>
