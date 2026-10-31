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
   *    chip (running / exit N) and a Cancel button while running. Selecting
   *    a run opens its full log in the center editor (`ActionRunView`),
   *    where it can also be popped out into its own window.
   *
   * Backend access goes through `actionsStore` (which owns the
   * `action-output` subscription); palette integration calls
   * `runAction(repoId, def)` from `$lib/stores/actions.svelte` directly.
   */
  import { runStatusLabel } from "./actionsModel";
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
  import { emitUiEvent } from "$lib/palette/events";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
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
      runDef(def);
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

  /** Opens a run's detail + log in the center editor. */
  function openRun(runId: string): void {
    emitUiEvent("action-run-open", { repoId, runId });
  }

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
    void actionsStore.run(repoId, def).then((runId) => {
      if (runId) openRun(runId);
    });
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
        // Open the run in the editor so first-time users see its output.
        if (runId) openRun(runId);
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

</script>

<section
  class="actions-panel flex flex-col gap-[0.6rem] px-3 py-2 text-[0.8125rem] text-foreground"
  aria-label="Detected projects and custom actions"
>
  <header class="section-head flex flex-wrap items-baseline gap-2">
    <h3 class="section-title m-0 text-[0.9375rem] font-semibold">Detected projects</h3>
    <span class="section-hint text-[0.7rem] text-muted-foreground">Run or watch from the repo root</span>
    <Button
      variant="outline"
      size="xs"
      class="btn ghost ml-auto rounded-full px-[0.6rem] text-[0.7rem] text-muted-foreground"
      type="button"
      disabled={projectsLoading}
      onclick={() => void projectsStore.refresh(repoId)}
    >
      {projectsLoading ? "Scanning…" : "Refresh"}
    </Button>
  </header>

  {#if projectsError}
    <p class="empty error m-0 text-xs text-destructive" role="alert">
      Project detection failed: {projectsError}
    </p>
  {:else if projectsLoading && projects.length === 0}
    <p class="empty m-0 text-xs text-muted-foreground">Detecting projects…</p>
  {:else if projects.length === 0}
    <p class="empty m-0 text-xs text-muted-foreground">
      No project manifests found. Detected: Node, Rust, Python, .NET, Android,
      Go, Ruby, PHP, and Make.
    </p>
  {:else}
    <ul class="project-list m-0 flex list-none flex-col gap-[0.45rem] p-0" role="list">
      {#each projects as project (project.kind + ":" + project.path)}
        <li class="project-card flex flex-col gap-[0.4rem] rounded-lg border border-border bg-card px-[0.55rem] py-[0.45rem]">
          <div class="project-head flex min-w-0 flex-wrap items-baseline gap-[0.4rem]">
            <span class="project-kind flex-none text-[0.625rem] font-bold uppercase tracking-[0.04em] text-primary">{projectKindLabel(project.kind)}</span>
            <span class="project-name min-w-0 truncate text-[0.8rem] font-semibold" title={project.name}>{project.name}</span>
            {#if project.path !== "."}
              <span class="project-path flex-none font-mono text-[0.68rem] text-muted-foreground" title={project.path}>{project.path}</span>
            {/if}
            <span
              class="project-from ml-auto max-w-[55%] truncate text-[0.65rem] text-muted-foreground opacity-75"
              title={`Detected from ${project.detected_from.join(", ")}`}
            >
              {project.detected_from.join(", ")}
            </span>
          </div>
          {#if project.scripts.length === 0}
            <p class="empty m-0 text-xs text-muted-foreground">No scripts detected.</p>
          {:else}
            <div class="script-grid grid grid-cols-[repeat(auto-fill,minmax(13rem,1fr))] gap-[0.35rem]">
              {#each project.scripts as script (`${script.id}:${script.command}`)}
                {@const runName = scriptRunName(project, script)}
                {@const running = runningNames.has(runName)}
                <div class="script min-w-0 overflow-hidden rounded-md border border-border bg-background">
                  <Button
                    variant="ghost"
                    class="script-run flex w-full min-w-0 items-center justify-start gap-[0.35rem] rounded-none px-1 py-[0.3rem] text-left font-normal"
                    type="button"
                    disabled={running}
                    title={`${script.command}\n\n${script.watch ? "Long-running watcher" : "Runs to completion"}`}
                    onclick={() => runScript(project, script)}
                  >
                    <span class="script-label min-w-0 flex-1 truncate text-xs font-medium">{script.label}</span>
                    <Badge
                      variant="outline"
                      class={`script-kind ${script.watch ? "watch" : ""} h-auto flex-none rounded-full border-transparent px-[0.35rem] py-[0.04rem] text-[0.625rem] font-semibold uppercase tracking-[0.03em] ${script.watch ? "bg-secondary text-secondary-foreground" : "bg-muted text-muted-foreground"}`}
                    >
                      {script.watch ? "watch" : scriptKindLabel(script.kind)}
                    </Badge>
                    {#if running}
                      <span class="script-state flex-none text-[0.62rem] text-primary">running…</span>
                    {/if}
                  </Button>
                  <span class="script-command-row flex min-w-0 items-center gap-[0.2rem] border-t border-border pb-[0.15rem] pl-[0.4rem] pr-[0.2rem] pt-[0.15rem]">
                    <code class="script-command min-w-0 flex-1 font-mono text-[0.65rem] leading-[1.35] text-muted-foreground [overflow-wrap:anywhere]" title={script.command}>{script.command}</code>
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      class="script-copy flex-none text-[0.8rem] leading-none text-muted-foreground opacity-55 hover:opacity-100"
                      type="button"
                      aria-label={`Copy command for ${script.label}`}
                      title="Copy command"
                      onclick={() => void copyCommand(script.command)}
                    >
                      ⧉
                    </Button>
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <header class="section-head actions-head mt-[0.4rem] flex flex-wrap items-baseline gap-2">
    <h3 class="section-title m-0 text-[0.9375rem] font-semibold">Custom actions</h3>
    <span class="section-hint text-[0.7rem] text-muted-foreground">Shell commands run in the repository root</span>
  </header>

  {#if defs.length === 0}
    <p class="empty m-0 text-xs text-muted-foreground">No custom actions yet — add one below.</p>
  {:else}
    <ul class="def-list m-0 flex list-none flex-col gap-1 p-0" role="list">
      {#each defs as def (def.id)}
        <li class="def-row flex flex-col gap-[0.3rem] rounded-lg border border-border bg-card px-[0.55rem] py-[0.45rem]">
          <div class="def-fields flex gap-[0.4rem]">
            <Input
              class="def-name h-auto min-w-0 flex-[1_1_9rem] rounded px-[0.4rem] py-1 text-[0.78rem]"
              type="text"
              aria-label="Action name"
              value={def.name}
              onchange={(e) => actionsStore.updateDef(def.id, { name: e.currentTarget.value })}
            />
            <Input
              class="def-command h-auto min-w-0 flex-[2_1_14rem] rounded px-[0.4rem] py-1 font-mono text-[0.78rem]"
              type="text"
              spellcheck="false"
              aria-label="Action command"
              value={def.command}
              onchange={(e) =>
                actionsStore.updateDef(def.id, { command: e.currentTarget.value })}
            />
          </div>
          <div class="def-controls flex items-center gap-[0.4rem]">
            <select
              class="def-scope flex-none rounded border border-border bg-background px-[0.4rem] py-1 text-[0.78rem] text-foreground font-[inherit] focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
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
            <Input
              class="def-shortcut h-auto flex-[0_0_7.5rem] cursor-pointer rounded px-[0.4rem] py-1 font-mono text-[0.7rem] text-primary"
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
            <Button
              variant="default"
              size="xs"
              class="btn run rounded-full px-[0.85rem]"
              type="button"
              disabled={runningNames.has(def.name)}
              onclick={() => runDef(def)}
            >
              {runningNames.has(def.name) ? "Running…" : "Run"}
            </Button>
            <Button
              variant="outline"
              size="xs"
              class="btn remove rounded-full px-2 text-muted-foreground"
              type="button"
              aria-label={`Delete action ${def.name}`}
              onclick={() => removeDef(def.id)}
            >
              ✕
            </Button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  <form class="add-form flex flex-wrap items-center gap-1" onsubmit={submitAdd}>
    <Input
      class="def-name h-auto min-w-0 flex-[1_1_9rem] rounded px-[0.4rem] py-1 text-[0.78rem]"
      type="text"
      placeholder="Name"
      aria-label="New action name"
      bind:value={addName}
    />
    <Input
      class="def-command h-auto min-w-0 flex-[2_1_14rem] rounded px-[0.4rem] py-1 font-mono text-[0.78rem]"
      type="text"
      placeholder="Command (e.g. npm test)"
      aria-label="New action command"
      spellcheck="false"
      bind:value={addCommand}
    />
    <select class="def-scope flex-none rounded border border-border bg-background px-[0.4rem] py-1 text-[0.78rem] text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary" aria-label="New action scope" bind:value={addScope}>
      <option value="repo">this repo</option>
      <option value="global">global</option>
    </select>
    <Button variant="default" size="xs" class="btn add flex-none rounded-full px-[0.85rem]" type="submit" disabled={!addName.trim() || !addCommand.trim()}>
      Add
    </Button>
  </form>

  <header class="section-head runs-head mt-[0.4rem] flex flex-wrap items-baseline gap-2">
    <h3 class="section-title m-0 text-[0.9375rem] font-semibold">Runs</h3>
    {#if runs.length > 0}
      <span class="section-hint text-[0.7rem] text-muted-foreground">{runs.length} recent</span>
    {/if}
  </header>

  {#if runs.length === 0}
    <p class="empty m-0 text-xs text-muted-foreground">No runs yet — hit Run on an action above.</p>
  {:else}
    <ul class="run-list m-0 flex list-none flex-col gap-1 p-0" role="list">
      {#each runs as run (run.run_id)}
        <li class="run-row overflow-hidden rounded-lg border border-border transition-colors duration-100 hover:border-ring/60 hover:bg-muted/60 focus-within:border-ring/60 focus-within:bg-muted/60">
          <div class="run-head flex items-center justify-between gap-1">
            <Button
              variant="ghost"
              class="run-toggle flex min-w-0 flex-1 cursor-pointer items-center justify-start gap-2 rounded-none px-[0.55rem] py-[0.4rem] text-left font-normal"
              type="button"
              title="Show this run's log in the editor"
              onclick={() => openRun(run.run_id)}
            >
              <Badge
                variant="outline"
                class={`chip chip-${run.status} h-auto flex-none rounded-full border-transparent px-2 py-[0.1rem] text-[0.68rem] font-semibold ${run.status === "running" ? "bg-secondary text-secondary-foreground" : run.status === "success" ? "bg-primary/15 text-primary" : "bg-destructive/10 text-destructive"}`}
              >
                {runStatusLabel(run.status, run.exit_code)}
              </Badge>
              <span class="run-name truncate">{run.name}</span>
            </Button>
            {#if !run.done}
              <Button
                variant="destructive"
                size="xs"
                class="btn cancel rounded-full px-[0.85rem]"
                type="button"
                onclick={() => void actionsStore.cancel(run.run_id)}
              >
                Cancel
              </Button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>
