<script lang="ts">
  /**
   * Actions panel (M4) — user-defined shell actions with streamed output.
   *
   * Two sections:
   *  - Config: the merged action-def view for this repo (global + own,
   *    from the localStorage-backed `actionsStore`), each row editable
   *    inline (name, command, scope, optional shortcut) with Run and
   *    Delete; an add form appends new defs (persisted across sessions).
   *  - Runs: recent runs of this repo (newest first), each with a status
   *    chip (running / exit N), an expandable terminal-style output view
   *    (monospace, auto-scrolled to the bottom, display-capped) and a
   *    Cancel button while running.
   *
   * Backend access goes through `actionsStore` (which owns the
   * `action-output` subscription); palette integration calls
   * `runAction(repoId, def)` from `$lib/stores/actions.svelte` directly.
   */
  import {
    outputDisplayLines,
    runStatusLabel,
    truncateLines,
    RUN_LINE_CAP,
  } from "./actionsModel";
  import {
    actionsStore,
    startActionsEvents,
    type ActionDef,
  } from "$lib/stores/actions.svelte";

  let { repoId }: { repoId: string } = $props();

  // The panel is the primary mount of the store's event subscription
  // (idempotent; other consumers get the same stream).
  $effect(() => {
    void repoId;
    startActionsEvents();
  });

  // Merged config view for the active repo (fresh array per store change).
  const defs = $derived(actionsStore.defsFor(repoId));
  const runs = $derived(actionsStore.runsFor(repoId));
  /** Def names with a live run (their Run buttons show a spinner state). */
  const runningNames = $derived(
    new Set(runs.filter((run) => !run.done).map((run) => run.name)),
  );

  // Add form
  let addName = $state("");
  let addCommand = $state("");
  let addScope = $state<"global" | "repo">("repo");

  // Expand state: which run's output is open (one at a time).
  let expandedRunId = $state<string | null>(null);
  let outputBody: HTMLDivElement | undefined = $state();

  function submitAdd(event: SubmitEvent): void {
    event.preventDefault();
    const def = actionsStore.addDef({
      name: addName,
      command: addCommand,
      scope: addScope,
      ...(addScope === "repo" ? { repoId } : {}),
    });
    if (def) {
      addName = "";
      addCommand = "";
    }
  }

  function runDef(def: ActionDef): void {
    void actionsStore.run(repoId, def);
  }

  function removeDef(id: string): void {
    actionsStore.removeDef(id);
  }

  function toggleRun(runId: string): void {
    expandedRunId = expandedRunId === runId ? null : runId;
  }

  // Display lines for a run: display-cap re-applied over the store buffer,
  // with the store's cross-buffer dropped count folded into the notice.
  function displayLines(run: (typeof runs)[number]): string[] {
    const kept = truncateLines(run.lines, RUN_LINE_CAP);
    return outputDisplayLines(kept.lines, run.dropped + kept.dropped, RUN_LINE_CAP);
  }

  // Auto-scroll the open output to the bottom as lines land.
  $effect(() => {
    if (!expandedRunId) return;
    const run = runs.find((r) => r.run_id === expandedRunId);
    void run?.lines.length;
    void run?.done;
    requestAnimationFrame(() => {
      if (outputBody) outputBody.scrollTop = outputBody.scrollHeight;
    });
  });
</script>

<section class="actions-panel" aria-label="Custom actions">
  <header class="section-head">
    <h3 class="section-title">Actions</h3>
    <span class="section-hint">Shell commands run in the repository root</span>
  </header>

  {#if defs.length === 0}
    <p class="empty">No actions configured — add one below.</p>
  {:else}
    <ul class="def-list" role="list">
      {#each defs as def (def.id)}
        <li class="def-row">
          <div class="def-fields">
            <input
              class="def-name"
              type="text"
              aria-label="Action name"
              value={def.name}
              onchange={(e) => actionsStore.updateDef(def.id, { name: e.currentTarget.value })}
            />
            <input
              class="def-command"
              type="text"
              spellcheck="false"
              aria-label="Action command"
              value={def.command}
              onchange={(e) =>
                actionsStore.updateDef(def.id, { command: e.currentTarget.value })}
            />
          </div>
          <div class="def-controls">
            <select
              class="def-scope"
              aria-label="Action scope"
              value={def.scope}
              onchange={(e) => {
                const scope = e.currentTarget.value as "global" | "repo";
                actionsStore.updateDef(def.id, {
                  scope,
                  ...(scope === "repo" ? { repoId } : { repoId: undefined }),
                });
              }}
            >
              <option value="global">global</option>
              <option value="repo">this repo</option>
            </select>
            <button
              class="btn run"
              type="button"
              disabled={runningNames.has(def.name)}
              onclick={() => runDef(def)}
            >
              {runningNames.has(def.name) ? "Running…" : "Run"}
            </button>
            <button
              class="btn remove"
              type="button"
              aria-label={`Delete action ${def.name}`}
              onclick={() => removeDef(def.id)}
            >
              ✕
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  <form class="add-form" onsubmit={submitAdd}>
    <input
      class="def-name"
      type="text"
      placeholder="Name"
      aria-label="New action name"
      bind:value={addName}
    />
    <input
      class="def-command"
      type="text"
      placeholder="Command (e.g. npm test)"
      aria-label="New action command"
      spellcheck="false"
      bind:value={addCommand}
    />
    <select class="def-scope" aria-label="New action scope" bind:value={addScope}>
      <option value="repo">this repo</option>
      <option value="global">global</option>
    </select>
    <button class="btn add" type="submit" disabled={!addName.trim() || !addCommand.trim()}>
      Add
    </button>
  </form>

  <header class="section-head runs-head">
    <h3 class="section-title">Runs</h3>
    {#if runs.length > 0}
      <span class="section-hint">{runs.length} recent</span>
    {/if}
  </header>

  {#if runs.length === 0}
    <p class="empty">No runs yet — hit Run on an action above.</p>
  {:else}
    <ul class="run-list" role="list">
      {#each runs as run (run.run_id)}
        <li class="run-row" class:open={expandedRunId === run.run_id}>
          <div class="run-head">
            <button
              class="run-toggle"
              type="button"
              aria-expanded={expandedRunId === run.run_id}
              onclick={() => toggleRun(run.run_id)}
            >
              <span class={`chip chip-${run.status}`}>
                {runStatusLabel(run.status, run.exit_code)}
              </span>
              <span class="run-name">{run.name}</span>
            </button>
            {#if !run.done}
              <button
                class="btn cancel"
                type="button"
                onclick={() => void actionsStore.cancel(run.run_id)}
              >
                Cancel
              </button>
            {/if}
          </div>
          {#if expandedRunId === run.run_id}
            <div class="term" bind:this={outputBody} aria-label={`Output of ${run.name}`}>
              {#each displayLines(run) as line, i (i)}
                <div class="term-line">{line}</div>
              {/each}
              {#if !run.done}
                <div class="term-line term-live">▊</div>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .actions-panel {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
  }

  .section-head {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
  }

  .section-title {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 600;
  }

  .section-hint {
    color: var(--m3-on-surface-variant);
    font-size: 0.7rem;
  }

  .runs-head {
    margin-top: 0.4rem;
  }

  .empty {
    margin: 0;
    color: var(--m3-on-surface-variant);
    font-size: 0.75rem;
  }

  .def-list,
  .run-list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .def-row {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.45rem 0.55rem;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 8px;
    background: var(--m3-surface-container-low, transparent);
  }

  .def-fields {
    display: flex;
    gap: 0.4rem;
  }

  input[type="text"],
  select {
    font: inherit;
    font-size: 0.78rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, transparent);
    border: 1px solid var(--m3-outline-variant);
    border-radius: 4px;
    padding: 0.25rem 0.4rem;
    min-width: 0;
  }

  input[type="text"]:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .def-name {
    flex: 1 1 9rem;
  }

  .def-command {
    flex: 2 1 14rem;
    font-family: ui-monospace, Consolas, monospace;
  }

  .def-controls {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .def-scope {
    flex: 0 0 auto;
  }

  .btn {
    font: inherit;
    font-size: 0.75rem;
    font-weight: 500;
    border: none;
    border-radius: 9999px;
    padding: 0.28rem 0.85rem;
    cursor: pointer;
  }

  .btn:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }

  .btn.run {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .btn.add {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    flex: 0 0 auto;
  }

  .btn.remove {
    background: none;
    border: 1px solid var(--m3-outline-variant);
    color: var(--m3-on-surface-variant);
    padding: 0.2rem 0.5rem;
  }

  .btn.cancel {
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
  }

  .add-form {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
  }

  .run-row {
    border: 1px solid var(--m3-outline-variant);
    border-radius: 8px;
    overflow: hidden;
  }

  .run-row.open {
    border-color: var(--m3-outline);
  }

  .run-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem;
  }

  .run-toggle {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex: 1 1 auto;
    min-width: 0;
    background: none;
    border: none;
    font: inherit;
    color: inherit;
    padding: 0.4rem 0.55rem;
    cursor: pointer;
    text-align: left;
  }

  .run-toggle:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .run-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip {
    flex: 0 0 auto;
    font-size: 0.68rem;
    font-weight: 600;
    border-radius: 9999px;
    padding: 0.1rem 0.5rem;
  }

  .chip-running {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }

  .chip-success {
    background: var(--m3-primary-container);
    color: var(--m3-on-primary-container);
  }

  .chip-failed {
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
  }

  .term {
    margin: 0 0.55rem 0.55rem;
    padding: 0.5rem;
    max-height: 14rem;
    overflow-y: auto;
    background: var(--m3-surface-container-lowest, transparent);
    border: 1px solid var(--m3-outline-variant);
    border-radius: 4px;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .term-line {
    color: var(--m3-on-surface);
  }

  .term-live {
    color: var(--m3-on-surface-variant);
  }
</style>
