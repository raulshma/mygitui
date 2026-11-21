<script lang="ts">
  /**
   * Agent-mode PR list — a dense, read-only view of the repo's open PRs.
   * Same data path as ForgePanel: `forgeStore.ensure(repoId)` on mount,
   * `refreshPrs` + `refreshContext` on retry; a row click opens the PR in
   * the system browser. No merge/checkout actions here — that stays in the
   * full ForgePanel.
   */
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { toast } from "$lib/toast";
  import { Button } from "$lib/components/ui/button/index.js";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import {
    GH_AUTH_COMMAND,
    GH_INSTALL_DOCS_URL,
    statusBanner,
  } from "$lib/components/forge/forgeModel";

  let { repoId }: { repoId: string } = $props();

  const iconApproved = $derived(appIcon("check-circle")!);
  const iconChanges = $derived(appIcon("triangle-alert")!);

  /** Render cap: a dense overview, not an archive. */
  const PR_RENDER_CAP = 100;

  /** Ensure() runs once per repo (status flips would otherwise re-trigger). */
  let ensuredRepo = $state<string | null>(null);

  $effect(() => {
    if (ensuredRepo === repoId) return;
    ensuredRepo = repoId;
    void forgeStore.ensure(repoId);
  });

  const banner = $derived(statusBanner(forgeStore.status));
  const contextError = $derived(forgeStore.contextErrorFor(repoId));
  const prs = $derived(forgeStore.prsFor(repoId));
  const prsLoading = $derived(forgeStore.prsLoadingFor(repoId));
  const prsError = $derived(forgeStore.prsErrorFor(repoId));
  const openPrs = $derived(
    prs
      .filter((pr) => pr.state === "OPEN")
      .sort((a, b) => b.number - a.number)
      .slice(0, PR_RENDER_CAP),
  );

  /** Opens a PR in the system browser (toast on failure). */
  async function openUrl(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }

  /** Same pair ForgePanel's refresh runs: PR list (its filter) + context. */
  function refresh(): void {
    void forgeStore.refreshPrs(repoId, forgeStore.filterFor(repoId));
    void forgeStore.refreshContext(repoId);
  }

  async function copyAuthCommand(): Promise<void> {
    try {
      await navigator.clipboard.writeText(GH_AUTH_COMMAND);
      toast(`Copied "${GH_AUTH_COMMAND}" to the clipboard`, { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }
</script>

<section
  class="flex min-h-0 flex-1 flex-col overflow-hidden bg-background text-xs text-foreground"
  aria-label="Agent pull requests"
>
  {#if banner.kind === "missing"}
    <EmptyState title="GitHub CLI (gh) not found." hint={banner.text}>
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-2 py-px text-ui-meta font-normal"
        variant="secondary"
        type="button"
        onclick={() => void openUrl(GH_INSTALL_DOCS_URL)}
      >
        Install guide
      </Button>
    </EmptyState>
  {:else if banner.kind === "unauthed"}
    <EmptyState title="GitHub CLI (gh) is not authenticated." hint={banner.text}>
      <div class="flex items-center gap-2">
        <code class="rounded-sm bg-muted px-2 py-px font-mono text-xs text-foreground">{GH_AUTH_COMMAND}</code>
        <Button
          class="h-auto whitespace-nowrap rounded-sm px-2 py-px text-ui-meta font-normal"
          variant="secondary"
          type="button"
          onclick={() => void copyAuthCommand()}
        >
          Copy
        </Button>
      </div>
    </EmptyState>
  {:else if contextError}
    <EmptyState title="The gh flow is unavailable for this repository." hint={contextError} />
  {:else if banner.kind === "loading"}
    <div class="flex min-h-0 flex-1 flex-col gap-1 p-2" aria-hidden="true">
      {#each Array.from({ length: 4 }) as _, i (i)}
        <div class="h-6 animate-pulse rounded bg-muted"></div>
      {/each}
    </div>
  {:else}
    <div class="min-h-0 flex-1 overflow-y-auto p-2">
      {#if prsError}
        <div class="mb-2 flex items-center gap-2">
          <p class="m-0 min-w-0 flex-1 text-destructive [overflow-wrap:anywhere]" role="alert">{prsError}</p>
          <Button
            class="h-auto flex-none whitespace-nowrap rounded-sm px-2 py-px text-ui-meta font-normal"
            variant="outline"
            type="button"
            onclick={refresh}
          >
            Retry
          </Button>
        </div>
      {/if}
      {#if prsLoading && openPrs.length === 0}
        <div class="flex flex-col gap-1" aria-hidden="true">
          {#each Array.from({ length: 4 }) as _, i (i)}
            <div class="h-6 animate-pulse rounded bg-muted"></div>
          {/each}
        </div>
      {:else if openPrs.length === 0}
        {#if !prsError}
          <EmptyState title="No open PRs" />
        {/if}
      {:else}
        <ul class="m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Open pull requests">
          {#each openPrs as pr (pr.number)}
            <li>
              <button
                class="flex w-full min-w-0 cursor-pointer items-center gap-2 rounded-md border border-border bg-card px-2 py-1 text-left text-ui-meta text-foreground hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                type="button"
                title={pr.title}
                onclick={() => void openUrl(pr.url)}
              >
                <span class="flex-none font-mono text-muted-foreground">#{pr.number}</span>
                <span class="min-w-0 flex-1 truncate font-medium">{pr.title}</span>
                {#if pr.is_draft}
                  <span class="flex-none rounded-full border border-border px-1.5 text-ui-micro font-semibold text-muted-foreground">Draft</span>
                {/if}
                {#if pr.review_decision === "APPROVED"}
                  <span class="flex-none text-primary" title="Approved">
                    <Icon icon={iconApproved} size={12} />
                  </span>
                {:else if pr.review_decision === "CHANGES_REQUESTED"}
                  <span class="flex-none text-destructive" title="Changes requested">
                    <Icon icon={iconChanges} size={12} />
                  </span>
                {/if}
                <span class="flex-none font-mono tabular-nums text-muted-foreground">+{pr.additions}/-{pr.deletions}</span>
                {#if pr.author}
                  <span class="max-w-[8rem] flex-none truncate text-muted-foreground">{pr.author}</span>
                {/if}
                <span class="max-w-[30%] flex-none truncate font-mono text-muted-foreground">{pr.head_ref_name}→{pr.base_ref_name}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</section>
