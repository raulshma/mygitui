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
    comboFromKeyboardEvent,
    defForEvent,
    formatShortcut,
    shortcutHolders,
  } from "./shortcutModel";
  import { detectPlatform } from "$lib/palette/keybinds";
  import { toast } from "$lib/toast";
  import {
    actionsStore,
    startActionsEvents,
    type ActionDef,
  } from "$lib/stores/actions.svelte";
  import { projectsStore } from "$lib/stores/projects.svelte";
  import type { ProjectInfo, ProjectScript } from "$lib/ipc/types";

  let { repoId }: { repoId: string } = $props();

  // The panel is the primary mount of the store's event subscription
  // (idempotent; other consumers get the same stream).
  $effect(() => {
    void repoId;
    startActionsEvents();
  });

  // Detected projects/scripts: cached per repo; re-scans only on refresh or
  // first mount of a repo.
  $effect(() => {
    void repoId;
    void projectsStore.ensure(repoId);
  });

  const projects = $derived(projectsStore.projectsFor(repoId));
  const projectsLoading = $derived(projectsStore.loadingFor(repoId));
  const projectsError = $derived(projectsStore.errorFor(repoId));

  // M12: global shortcut runner — ONE window keydown listener matching any
  // stored shortcut of this repo's actions (global + repo scopes) and
  // running the hit through the same `actionsStore.run` path as the Run
  // button. Capture-phase like the palette keybinds; the pure match rules
  // (typing protection, capture-field exemption) live in shortcutModel.
  $effect(() => {
    void repoId;
    const platform = detectPlatform();
    const handler = (event: KeyboardEvent): void => {
      const def = defForEvent(event, actionsStore.defsFor(repoId), platform);
      if (!def) return;
      event.preventDefault();
      event.stopPropagation();
      void actionsStore.run(repoId, def);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
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

  // -- detected projects/scripts ---------------------------------------------

  const PROJECT_KINDS: Record<string, string> = {
    node: "JS/TS",
    rust: "Rust",
    python: "Python",
    dotnet: ".NET",
    android: "Android",
    go: "Go",
    ruby: "Ruby",
    php: "PHP",
    make: "Make",
  };

  const SCRIPT_KINDS: Record<string, string> = {
    run: "run",
    watch: "watch",
    test: "test",
    build: "build",
    lint: "lint",
    install: "install",
  };

  function projectKindLabel(kind: string): string {
    return PROJECT_KINDS[kind] ?? kind;
  }

  function scriptKindLabel(kind: string): string {
    return SCRIPT_KINDS[kind] ?? kind;
  }

  /** Run-list name for a detected script (also used for the running state). */
  function scriptRunName(project: ProjectInfo, script: ProjectScript): string {
    const scope = project.path === "." ? "" : ` (${project.path})`;
    return `${project.name}${scope}: ${script.label}`;
  }

  /** Synthetic, non-persisted def fed to the existing action runner. */
  function scriptDef(project: ProjectInfo, script: ProjectScript): ActionDef {
    return {
      id: `project:${project.kind}:${project.path}:${script.id}`,
      name: scriptRunName(project, script),
      command: script.command,
      scope: "repo",
      repoId,
    };
  }

  function runScript(project: ProjectInfo, script: ProjectScript): void {
    void actionsStore
      .run(repoId, scriptDef(project, script))
      .then((runId) => {
        // Auto-open the output so first-time users see what the script does.
        if (runId) expandedRunId = runId;
      });
  }

  async function copyCommand(command: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(command);
      toast("Command copied to the clipboard", { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  // -- M12: shortcut capture -------------------------------------------------
  // The shortcut field is a readonly input; focusing it arms capture and the
  // NEXT keydown carrying a modifier is recorded (Escape clears, blur
  // cancels). Duplicates are rejected with a toast.

  /** Def id currently capturing keys (null = none). */
  let capturingId = $state<string | null>(null);
  /** Live preview of the in-progress combo while capturing. */
  let capturePreview = $state("");

  function onCaptureFocus(def: ActionDef): void {
    capturingId = def.id;
    capturePreview = "";
  }

  function onCaptureBlur(): void {
    capturingId = null;
    capturePreview = "";
  }

  function onCaptureKeydown(event: KeyboardEvent, def: ActionDef): void {
    if (capturingId !== def.id) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      actionsStore.updateDef(def.id, { shortcut: "" });
      capturingId = null;
      capturePreview = "";
      return;
    }
    const combo = comboFromKeyboardEvent(event, detectPlatform());
    if (combo === null) {
      capturePreview = formatShortcut(event.key.toLowerCase());
      return; // bare modifier — wait for the full combo
    }
    event.preventDefault();
    event.stopPropagation();
    const holders = shortcutHolders(combo, actionsStore.defs, def.id);
    if (holders.length > 0) {
      toast(`Shortcut already used by “${holders[0]!.name}”`, { kind: "error" });
      return;
    }
    actionsStore.updateDef(def.id, { shortcut: combo });
    capturingId = null;
    capturePreview = "";
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

<section class="actions-panel" aria-label="Detected projects and custom actions">
  <header class="section-head">
    <h3 class="section-title">Detected projects</h3>
    <span class="section-hint">Run or watch from the repo root</span>
    <button
      class="btn ghost"
      type="button"
      disabled={projectsLoading}
      onclick={() => void projectsStore.refresh(repoId)}
    >
      {projectsLoading ? "Scanning…" : "Refresh"}
    </button>
  </header>

  {#if projectsError}
    <p class="empty error" role="alert">
      Project detection failed: {projectsError}
    </p>
  {:else if projectsLoading && projects.length === 0}
    <p class="empty">Detecting projects…</p>
  {:else if projects.length === 0}
    <p class="empty">
      No project manifests found. Detected: Node, Rust, Python, .NET, Android,
      Go, Ruby, PHP, and Make.
    </p>
  {:else}
    <ul class="project-list" role="list">
      {#each projects as project (project.kind + ":" + project.path)}
        <li class="project-card">
          <div class="project-head">
            <span class="project-kind">{projectKindLabel(project.kind)}</span>
            <span class="project-name" title={project.name}>{project.name}</span>
            {#if project.path !== "."}
              <span class="project-path" title={project.path}>{project.path}</span>
            {/if}
            <span
              class="project-from"
              title={`Detected from ${project.detected_from.join(", ")}`}
            >
              {project.detected_from.join(", ")}
            </span>
          </div>
          {#if project.scripts.length === 0}
            <p class="empty">No scripts detected.</p>
          {:else}
            <div class="script-grid">
              {#each project.scripts as script (`${script.id}:${script.command}`)}
                {@const runName = scriptRunName(project, script)}
                {@const running = runningNames.has(runName)}
                <div class="script">
                  <button
                    class="script-run"
                    type="button"
                    disabled={running}
                    title={`${script.command}\n\n${script.watch ? "Long-running watcher" : "Runs to completion"}`}
                    onclick={() => runScript(project, script)}
                  >
                    <span class="script-label">{script.label}</span>
                    <span class="script-kind" class:watch={script.watch}>
                      {script.watch ? "watch" : scriptKindLabel(script.kind)}
                    </span>
                    {#if running}
                      <span class="script-state">running…</span>
                    {/if}
                  </button>
                  <span class="script-command-row">
                    <code class="script-command" title={script.command}>{script.command}</code>
                    <button
                      class="script-copy"
                      type="button"
                      aria-label={`Copy command for ${script.label}`}
                      title="Copy command"
                      onclick={() => void copyCommand(script.command)}
                    >
                      ⧉
                    </button>
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <header class="section-head actions-head">
    <h3 class="section-title">Custom actions</h3>
    <span class="section-hint">Shell commands run in the repository root</span>
  </header>

  {#if defs.length === 0}
    <p class="empty">No custom actions yet — add one below.</p>
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
            <!-- M12: shortcut capture — focus, press e.g. Ctrl+Shift+R;
                 Escape clears, blur cancels. -->
            <input
              class="def-shortcut"
              type="text"
              readonly
              data-shortcut-capture
              placeholder="shortcut"
              aria-label={`Shortcut for action ${def.name}`}
              title="Focus and press a key combination (e.g. Ctrl+Shift+R). Escape clears."
              value={capturingId === def.id
                ? capturePreview || "press keys…"
                : def.shortcut
                  ? formatShortcut(def.shortcut)
                  : ""}
              onfocus={() => onCaptureFocus(def)}
              onblur={onCaptureBlur}
              onkeydown={(e) => onCaptureKeydown(e, def)}
            />
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
    padding: 0.5rem 0.75rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
  }

  .section-head {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .section-head .btn {
    margin-left: auto;
  }

  .actions-head {
    margin-top: 0.4rem;
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

  .empty.error {
    color: var(--m3-error);
  }

  /* detected projects/scripts */
  .project-list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }

  .project-card {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    padding: 0.45rem 0.55rem;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 8px;
    background: var(--m3-surface-container-low, transparent);
  }

  .project-head {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    min-width: 0;
    flex-wrap: wrap;
  }

  .project-kind {
    flex: none;
    font-size: 0.625rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--m3-primary);
  }

  .project-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.8rem;
    font-weight: 600;
  }

  .project-path {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant);
  }

  .project-from {
    margin-left: auto;
    max-width: 55%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.65rem;
    color: var(--m3-on-surface-variant);
    opacity: 0.75;
  }

  .script-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(13rem, 1fr));
    gap: 0.35rem;
  }

  .script {
    min-width: 0;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 6px;
    overflow: hidden;
    background: var(--m3-surface-container-lowest, transparent);
  }

  .script-run {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    padding: 0.3rem 0.4rem;
    cursor: pointer;
    text-align: left;
  }

  .script-run:hover:not(:disabled) {
    background: var(--m3-surface-container-high);
  }

  .script-run:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .script-run:disabled {
    cursor: progress;
    opacity: 0.75;
  }

  .script-label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.75rem;
    font-weight: 500;
  }

  .script-kind {
    flex: none;
    border-radius: 9999px;
    padding: 0.04rem 0.35rem;
    background: var(--m3-surface-container-high);
    color: var(--m3-on-surface-variant);
    font-size: 0.625rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .script-kind.watch {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }

  .script-state {
    flex: none;
    font-size: 0.62rem;
    color: var(--m3-primary);
  }

  .script-command-row {
    display: flex;
    align-items: center;
    gap: 0.2rem;
    min-width: 0;
    padding: 0.15rem 0.2rem 0.15rem 0.4rem;
    border-top: 1px solid var(--m3-outline-variant);
  }

  .script-command {
    flex: 1 1 auto;
    min-width: 0;
    white-space: normal;
    overflow-wrap: anywhere;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.65rem;
    line-height: 1.35;
    color: var(--m3-on-surface-variant);
  }

  .script-copy {
    flex: none;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant);
    font-size: 0.8rem;
    line-height: 1;
    padding: 0.1rem 0.2rem;
    border-radius: 4px;
    cursor: pointer;
    opacity: 0.55;
  }

  .script-copy:hover,
  .script-copy:focus-visible {
    opacity: 1;
    background: var(--m3-surface-container-high);
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

  /* M12: shortcut capture field (monospace, captures the next combo). */
  .def-shortcut {
    flex: 0 0 7.5rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.7rem;
    color: var(--m3-primary);
    cursor: pointer;
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

  .btn.ghost {
    background: none;
    border: 1px solid var(--m3-outline-variant);
    color: var(--m3-on-surface-variant);
    padding: 0.18rem 0.6rem;
    font-size: 0.7rem;
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
