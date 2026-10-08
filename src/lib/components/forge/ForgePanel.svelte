<script lang="ts">
  /**
   * Forge panel (M6) — gh-assisted GitHub flow.
   *
   * Banner first: gh missing → install link, gh unauthenticated → copyable
   * `gh auth login`, non-GitHub origin → explanation. When ready, the panel
   * shows the current branch's PR card (state chips, checks with
   * pass/fail/pending icons + refresh, "Open on GitHub" through the opener
   * plugin) and the repo's open PRs. "Create PR" opens {@link
   * CreatePrDialog}; a successful create refreshes context + list here.
   *
   * MOUNTING CONTRACT (docs/contracts.md "M6 FE contracts"): this component
   * takes `{ repoId }` and needs a `"forge"` PanelId registry entry to be
   * placeable — layout files were read-only for this lane, the orchestrator
   * adds the id + `PANEL_META` + the RepoView snippet + `TREE_PANELS`.
   */
  import type { PrInfo } from "$lib/ipc/client";
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { toast } from "$lib/toast";
  import CreatePrDialog from "./CreatePrDialog.svelte";
  import {
    GH_AUTH_COMMAND,
    GH_INSTALL_DOCS_URL,
    checkIcon,
    checkStateClass,
    checkSummaryLabel,
    isPrOpen,
    prAgeLabel,
    prForBranch,
    prStateClass,
    prStateLabel,
    statusBanner,
    summarizeChecks,
    worstChipClass,
  } from "./forgeModel";

  let { repoId }: { repoId: string } = $props();

  let dialogOpen = $state(false);
  /** Ensure() runs once per repo (status flips would otherwise re-trigger). */
  let ensuredRepo = $state<string | null>(null);
  /** Currently expanded open-PR row (its checks render inline). */
  let expandedNumber = $state<number | null>(null);

  $effect(() => {
    if (ensuredRepo === repoId) return;
    ensuredRepo = repoId;
    void forgeStore.ensure(repoId);
  });

  const banner = $derived(statusBanner(forgeStore.status));
  const context = $derived(forgeStore.contextFor(repoId));
  const contextError = $derived(forgeStore.contextErrorFor(repoId));
  const prs = $derived(forgeStore.prsFor(repoId));
  const prsLoading = $derived(forgeStore.prsLoadingFor(repoId));
  const branchPr = $derived(prForBranch(prs, context?.branch ?? ""));
  const openPrs = $derived(
    prs
      .filter((pr) => isPrOpen(pr.state) && pr.number !== branchPr?.number)
      .sort((a, b) => b.number - a.number),
  );
  const branchAge = $derived(branchPr ? prAgeLabel(branchPr.created_at) : null);

  // Checks load lazily for the current branch's open PR.
  $effect(() => {
    if (!branchPr || !isPrOpen(branchPr.state)) return;
    void forgeStore.refreshChecks(repoId, branchPr.number);
  });

  const canCreate = $derived(banner.kind === "ready" && !!context?.branch);

  /** Opens a URL in the system browser (toast on failure). */
  async function openUrl(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }

  async function copyAuthCommand(): Promise<void> {
    try {
      await navigator.clipboard.writeText(GH_AUTH_COMMAND);
      toast(`Copied "${GH_AUTH_COMMAND}" to the clipboard`, { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  function refresh(): void {
    void forgeStore.refreshPrs(repoId);
    void forgeStore.refreshContext(repoId);
  }

  /** Expands/collapses one open-PR row (expanding lazily loads its checks). */
  function toggleExpanded(number: number): void {
    expandedNumber = expandedNumber === number ? null : number;
    if (expandedNumber !== null) void forgeStore.refreshChecks(repoId, number);
  }

  /** Called by the dialog after a successful create (dialog stays open). */
  function onCreated(): void {
    void forgeStore.afterCreate(repoId);
  }
</script>

<aside class="forge-panel" aria-label="Pull requests">
  {#snippet checksBlock(pr: PrInfo)}
    {@const checks = forgeStore.checksFor(repoId, pr.number)}
    {@const loading = forgeStore.checksLoadingFor(repoId, pr.number)}
    {@const summary = summarizeChecks(checks ?? [])}
    <div class="checks">
      <div class="checks-head">
        <span class="chip {worstChipClass(summary.worst)}">
          {checkSummaryLabel(summary)}
        </span>
        <button
          class="tb"
          type="button"
          disabled={loading}
          onclick={() => void forgeStore.refreshChecks(repoId, pr.number)}
        >
          {loading ? "Loading…" : "Refresh checks"}
        </button>
      </div>
      {#if checks === null}
        <p class="muted" aria-live="polite">Loading checks…</p>
      {:else if checks.length === 0}
        <p class="muted">No checks reported for this PR.</p>
      {:else}
        <ul class="check-list" role="list" aria-label="CI checks">
          {#each checks as check (check.name)}
            <li class="check">
              <span class="check-icon {checkStateClass(check.state)}" aria-hidden="true">
                {checkIcon(check.state)}
              </span>
              <span class="check-name" title={check.name}>{check.name}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/snippet}

  <div class="toolbar">
    <span class="title">Pull requests</span>
    {#if forgeStore.status?.available}
      <span class="gh-meta" title="GitHub CLI version">gh {forgeStore.status.version}</span>
    {/if}
    <span class="spacer"></span>
    <button
      class="tb"
      type="button"
      onclick={() => void forgeStore.refreshStatus()}
      disabled={forgeStore.statusLoading}
    >
      {forgeStore.statusLoading ? "Checking…" : "Re-check gh"}
    </button>
    <button class="tb" type="button" onclick={refresh} disabled={prsLoading}>
      {prsLoading ? "Loading…" : "Refresh"}
    </button>
    <button class="act" type="button" disabled={!canCreate} onclick={() => (dialogOpen = true)}>
      Create PR
    </button>
  </div>

  {#if banner.kind !== "ready"}
    <div class="banner {banner.kind}" role="status">
      <p class="banner-text">{banner.text}</p>
      {#if banner.kind === "missing"}
        <button class="tb" type="button" onclick={() => void openUrl(GH_INSTALL_DOCS_URL)}>
          Install guide
        </button>
      {:else if banner.kind === "unauthed"}
        <code class="cmd">{GH_AUTH_COMMAND}</code>
        <button class="tb" type="button" onclick={() => void copyAuthCommand()}>Copy</button>
      {/if}
    </div>
  {:else if contextError}
    <div class="banner missing" role="status">
      <p class="banner-text">
        The gh flow is unavailable for this repository: {contextError}
      </p>
    </div>
  {:else if context}
    {#if branchPr}
      <div class="card current">
        <div class="card-head">
          <span class="chip {prStateClass(branchPr.state)}">{prStateLabel(branchPr.state)}</span>
          {#if branchPr.is_draft}
            <span class="chip chip-draft">Draft</span>
          {/if}
          <span class="pr-title" title={branchPr.title}>{branchPr.title}</span>
          <span class="pr-no">#{branchPr.number}</span>
        </div>
        <div class="card-sub">
          <span class="refs">{branchPr.head_ref_name} → {branchPr.base_ref_name}</span>
          {#if branchAge}
            <span class="age">{branchAge}</span>
          {/if}
          <span class="spacer"></span>
          <button class="tb" type="button" onclick={() => void openUrl(branchPr.url)}>
            Open on GitHub
          </button>
        </div>
        {#if isPrOpen(branchPr.state)}
          {@render checksBlock(branchPr)}
        {/if}
      </div>
    {:else}
      <div class="card empty">
        <p>
          No pull request for branch <code>{context.branch}</code> yet.
        </p>
        <button class="act" type="button" onclick={() => (dialogOpen = true)}>Create PR</button>
      </div>
    {/if}

    <div class="section">
      <h3 class="section-title">Open pull requests</h3>
      {#if openPrs.length === 0}
        <p class="muted">
          {prsLoading ? "Loading pull requests…" : "No other open pull requests."}
        </p>
      {:else}
        <ul class="prs" role="list" aria-label="Open pull requests">
          {#each openPrs as pr (pr.number)}
            <li>
              <div class="pr-line">
                <button
                  class="pr-row"
                  type="button"
                  aria-expanded={expandedNumber === pr.number}
                  title="Show checks"
                  onclick={() => toggleExpanded(pr.number)}
                >
                  <span class="expand-icon" aria-hidden="true">
                    {expandedNumber === pr.number ? "▾" : "▸"}
                  </span>
                  <span class="chip {prStateClass(pr.state)}">{prStateLabel(pr.state)}</span>
                  {#if pr.is_draft}
                    <span class="chip chip-draft">Draft</span>
                  {/if}
                  <span class="pr-title" title={pr.title}>{pr.title}</span>
                  <span class="pr-no">#{pr.number}</span>
                  <span class="refs">{pr.head_ref_name} → {pr.base_ref_name}</span>
                </button>
                <button
                  class="tb"
                  type="button"
                  title="Open on GitHub"
                  onclick={() => void openUrl(pr.url)}
                >
                  Open
                </button>
              </div>
              {#if expandedNumber === pr.number}
                <div class="expand">
                  {@render checksBlock(pr)}
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <CreatePrDialog {repoId} open={dialogOpen} onClose={() => (dialogOpen = false)} onCreated={onCreated} />
</aside>

<style>
  .forge-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.375rem 0.5rem;
    flex: none;
    position: sticky;
    top: 0;
    background: var(--m3-surface, inherit);
    z-index: 1;
  }

  .title {
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .gh-meta {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .spacer {
    margin-left: auto;
  }

  .banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin: 0.25rem 0.5rem;
    padding: 0.5rem 0.625rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .banner-text {
    margin: 0;
    flex: 1;
    min-width: 12rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .cmd {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    padding: 0.1rem 0.4rem;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface);
  }

  .card {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    padding: 0.5rem 0.625rem;
    margin: 0.25rem 0.5rem;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
    min-width: 0;
  }

  .pr-title {
    font-weight: 600;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .pr-no {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
  }

  .card-sub {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.375rem;
    flex-wrap: wrap;
  }

  .refs {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
    font-family: ui-monospace, Consolas, monospace;
  }

  .age {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .checks {
    margin-top: 0.5rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    padding-top: 0.375rem;
  }

  .checks-head {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .check-list {
    margin: 0.25rem 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
  }

  .check-icon {
    flex: none;
    width: 1em;
    text-align: center;
  }

  .check-pass {
    color: var(--m3-primary, inherit);
  }

  .check-fail {
    color: var(--m3-error, inherit);
    font-weight: 700;
  }

  .check-pending {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .check-skipping {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .check-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
  }

  .chip {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.05rem 0.5rem;
    font-size: 0.6563rem;
    font-weight: 600;
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface);
  }

  .chip-pass {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
  }

  .chip-fail {
    background: var(--m3-error-container, var(--m3-error));
    color: var(--m3-on-error-container, var(--m3-on-error));
  }

  .chip-pending,
  .chip-skipping {
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .chip-none {
    background: var(--m3-surface-container, var(--m3-surface-container-low));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .chip-pr-open {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
  }

  .chip-pr-merged {
    background: var(--m3-tertiary-container, var(--m3-surface-container-high));
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
  }

  .chip-pr-closed {
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .chip-draft {
    background: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .section {
    margin: 0.5rem 0.5rem 0.75rem;
  }

  .section-title {
    margin: 0 0 0.25rem;
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .prs {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .pr-row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    width: 100%;
    text-align: left;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.7813rem;
    padding: 0.375rem 0.5rem;
    cursor: pointer;
    min-width: 0;
  }

  .pr-line {
    display: flex;
    align-items: flex-start;
    gap: 0.25rem;
    min-width: 0;
  }

  .pr-line .pr-row {
    flex: 1;
  }

  .pr-line .tb {
    margin-top: 0.375rem;
  }

  .expand {
    margin: 0.25rem 0 0.25rem 1.5rem;
  }

  .expand-icon {
    flex: none;
    width: 1em;
    text-align: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .pr-row:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .pr-row .pr-title {
    flex: 0 1 auto;
  }

  .pr-row .refs {
    margin-left: auto;
    flex: none;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card.empty {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .card.empty p {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .muted {
    margin: 0.25rem 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .tb {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .act {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .tb:focus-visible,
  .act:focus-visible,
  .pr-row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .tb:disabled,
  .act:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
</style>
