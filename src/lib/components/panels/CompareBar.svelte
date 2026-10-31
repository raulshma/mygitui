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
  import { Button } from "$lib/components/ui/button/index.js";

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

<div
  class="comparebar flex items-center gap-1.5 border-b border-border bg-card px-2 py-1"
  role="group"
  aria-label="Compare two refs"
  aria-busy={loading}
>
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
    class="flex-none rounded-sm px-2.5 py-1 text-sm leading-none font-normal"
    variant="secondary"
    onclick={swap}
    aria-label="Swap base and target"
    title="Swap"
    disabled={loading}
  >
    ⇄
  </Button>
  <input
    class="ref min-w-40 flex-1 rounded-sm border border-border bg-background px-2 py-1 font-mono text-xs text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring disabled:opacity-60"
    list={listId}
    placeholder="target (e.g. HEAD~3)"
    aria-label="Target ref"
    bind:value={target}
    onkeydown={onKeydown}
    disabled={loading}
  />
  <Button
    class="rounded-sm px-2.5 py-1 text-xs"
    onclick={() => void run()}
    disabled={loading}
  >
    {loading ? "Comparing…" : "Compare"}
  </Button>
  <datalist id={listId}>
    {#each refNames as name (name)}
      <option value={name}></option>
    {/each}
  </datalist>
</div>
