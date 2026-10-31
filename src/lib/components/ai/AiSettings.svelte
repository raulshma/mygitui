<script lang="ts">
  /**
   * AI settings dialog (M6, lane H1; M12 redesign).
   *
   * opencode is status-first: the dialog probes for a local opencode
   * install (PATH scan + version) and shows the managed server's state.
   * The default path needs nothing from the user — managed mode detects
   * the CLI and spawns `opencode serve` automatically; the visible knobs
   * are a single Enable switch, a Re-check button, and (under Advanced)
   * the attach mode for users who run their own server (manual URL +
   * optional basic-auth password).
   *
   * OpenRouter keeps its direct-API fields (key in the OS keyring, model
   * datalist fed by the backend's model list), plus the "Test connection"
   * button with per-backend status + latency.
   *
   * Secret fields are write-only: type to change, blank = keep what is
   * stored. Values are saved to the OS keyring (`secrets_set`), NEVER to
   * localStorage; the non-secret config persists under `mygitui.ai`.
   *
   * Mount anywhere (`<AiSettings bind:open />`); RepoView wiring is the
   * orchestrator's lane. The commit-message button opens its own instance
   * from the first-use prompt's "Configure" action.
   */
  import { untrack } from "svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import { AI_BACKENDS } from "$lib/ai/types";
  import type { AiBackend, ModelInfo, OpencodeMode } from "$lib/ai/types";
  import {
    opencodeDetect,
    opencodeServeStatus,
    type OpencodeDetection,
    type OpencodeServeState,
  } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";

  let {
    open = $bindable(false),
    onclose = undefined,
  }: {
    open: boolean;
    /** Called after Save (or Close) when the dialog should go away. */
    onclose?: () => void;
  } = $props();

  // --- form state (copies of the live config; applied on Save) ---------------
  let backendChoice = $state<AiBackend>("opencode");
  let opencodeEnabled = $state(true);
  let opencodeModeChoice = $state<OpencodeMode>("managed");
  let opencodeUrl = $state("");
  let opencodeModel = $state("");
  let opencodeModelSearch = $state("");
  let openrouterModel = $state("");
  let allowFallback = $state(true);
  /** Write-only secret inputs (never rendered back, never persisted here). */
  let opencodePassword = $state("");
  let openrouterKey = $state("");

  let saving = $state(false);
  let testing = $state(false);
  let models = $state<ModelInfo[]>([]);
  let modelsError = $state<string | null>(null);
  let opencodeModels = $state<ModelInfo[]>([]);
  let opencodeModelsError = $state<string | null>(null);
  let opencodeModelsLoading = $state(false);
  let opencodeModelGroups = $derived.by(() => {
    const groups = new Map<string, ModelInfo[]>();
    const query = opencodeModelSearch.trim().toLowerCase();
    for (const model of opencodeModels) {
      const provider = model.provider ?? "Other";
      const searchText = `${provider} ${model.label ?? model.id} ${model.id}`.toLowerCase();
      if (query && !searchText.includes(query)) continue;
      const group = groups.get(provider) ?? [];
      group.push(model);
      groups.set(provider, group);
    }
    return [...groups].map(([provider, models]) => ({ provider, models }));
  });
  /** Collapsed-by-default section for mode/URL/password. */
  let showAdvanced = $state(false);

  // Live install/server state (not form state — refreshed, never saved).
  let detection = $state<OpencodeDetection | null>(null);
  let serveState = $state<OpencodeServeState | null>(null);
  let checkingInstall = $state(false);

  // Supervisor status, mirrored into runes (the supervisor is a plain
  // Svelte-store-contract observable, not a runes store).
  let statuses = $state(ai.supervisor.statuses);
  $effect(() => ai.supervisor.subscribe((next) => (statuses = next)));

  /** Checks the connection, then refreshes local runtime details. */
  async function refreshInstall(): Promise<void> {
    checkingInstall = true;
    try {
      await ai.supervisor.check("opencode");
      await readInstallState();
    } finally {
      checkingInstall = false;
    }
  }

  async function readInstallState(): Promise<void> {
    if (!opencodeEnabled || opencodeModeChoice !== "managed") {
      detection = null;
      serveState = null;
      return;
    }
    [detection, serveState] = await Promise.all([
      opencodeDetect().catch(() => null),
      opencodeServeStatus().catch(() => null),
    ]);
  }

  // Hydrate the form whenever the dialog opens. Reads are untracked: the
  // effect must fire on `open` transitions ONLY — the config mutations in
  // save() would otherwise re-run this mid-save and wipe the form state
  // (including the not-yet-persisted secret inputs) between awaits.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      backendChoice = ai.config.backend;
      opencodeEnabled = ai.config.opencodeEnabled !== false;
      opencodeModeChoice =
        ai.config.opencodeMode ?? (ai.config.opencodeUrl ? "attach" : "managed");
      opencodeUrl = ai.config.opencodeUrl ?? "";
      opencodeModel = ai.config.opencodeModel ?? "";
      opencodeModelSearch = "";
      openrouterModel = ai.config.openrouterModel ?? "";
      allowFallback = ai.config.allowFallback !== false;
      opencodePassword = "";
      openrouterKey = "";
      showAdvanced = false;
      void refreshInstall();
    });
  });

  // Refresh both model lists whenever the settings dialog opens.
  $effect(() => {
    if (!open) return;
    models = [];
    modelsError = null;
    opencodeModels = [];
    opencodeModelsError = null;
    let current = true;
    ai.supervisor
      .provider("openrouter")
      .listModels()
      .then((list) => {
        if (!current) return;
        models = list;
      })
      .catch((err: unknown) => {
        if (!current) return;
        models = [];
        modelsError = err instanceof Error ? err.message : String(err);
      });
    if (untrack(() => ai.config.opencodeEnabled !== false)) {
      opencodeModelsLoading = true;
      ai.supervisor
        .provider("opencode")
        .listModels()
        .then((list) => {
          if (!current) return;
          opencodeModels = list;
          opencodeModelsLoading = false;
        })
        .catch((err: unknown) => {
          if (!current) return;
          opencodeModels = [];
          opencodeModelsError = err instanceof Error ? err.message : String(err);
          opencodeModelsLoading = false;
        });
    } else {
      opencodeModelsLoading = false;
    }
    return () => {
      current = false;
    };
  });

  /** One-line summary of the local OpenCode install and server. */
  function installLine(): string {
    if (!opencodeEnabled) return "OpenCode is disabled.";
    if (opencodeModeChoice === "attach") {
      const target = opencodeUrl.trim() || "http://127.0.0.1:4096";
      const status = statuses.opencode;
      if (status.status === "ok") return `Connected to ${target}`;
      if (status.status === "checking") return `Checking ${target}…`;
      if (status.status === "unauthenticated") return `The server at ${target} requires a password.`;
      if (status.status === "down") {
        return status.error ? `Could not reach ${target}: ${status.error}` : `Could not reach ${target}.`;
      }
      return `Ready to connect to ${target}.`;
    }
    if (detection === null) return "Checking for an OpenCode install…";
    if (!detection.installed) return "OpenCode was not found on PATH. Install it, then check again.";
    const version = detection.version ? `v${detection.version}` : "version unavailable";
    const at = detection.path ? ` (${detection.path})` : "";
    if (serveState?.running && serveState.url) {
      return `${version} — managed server running at ${serveState.url}`;
    }
    return `${version}${at} — mygitui starts the server when needed`;
  }

  /** Short status copy shared by the provider cards and connection list. */
  function statusLine(backend: AiBackend): string {
    const status = statuses[backend];
    if (status.status === "checking") return "Checking…";
    if (status.status === "unknown") return "Not checked yet";
    if (status.status === "ok") return `Connected · ${status.latencyMs ?? "?"} ms`;
    if (status.status === "unauthenticated") {
      return backend === "opencode" ? "Server password required" : "API key required";
    }
    return status.error ? `Unavailable · ${status.error}` : "Unavailable";
  }

  async function testConnection(): Promise<void> {
    testing = true;
    try {
      await ai.supervisor.checkAll();
      await readInstallState();
    } finally {
      testing = false;
    }
  }

  async function save(): Promise<void> {
    saving = true;
    try {
      ai.setBackend(backendChoice);
      ai.setOpencodeMode(opencodeModeChoice);
      // Kept even in managed mode (attach needs it again after a switch);
      // the resolver only reads it in attach mode.
      ai.setOpencodeUrl(opencodeUrl);
      ai.setOpencodeModel(opencodeModel);
      ai.setOpenrouterModel(openrouterModel);
      ai.setAllowFallback(allowFallback);
      if (opencodePassword.trim().length > 0) {
        await ai.saveOpencodePassword(opencodePassword);
      }
      if (openrouterKey.trim().length > 0) {
        await ai.saveOpenrouterKey(openrouterKey);
      }
      // Check after writing secrets so the probe uses the newly saved password.
      await ai.setOpencodeEnabled(opencodeEnabled);
      await ai.supervisor.check("openrouter").catch(() => {});
      void readInstallState();
      toast("AI settings saved", { kind: "success" });
      onclose?.();
    } catch (err) {
      toast(
        `Saving AI settings failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      saving = false;
    }
  }

  async function clearOpencodePassword(): Promise<void> {
    opencodePassword = "";
    try {
      await ai.clearOpencodePassword();
      await ai.supervisor.check("opencode");
      toast("opencode password removed from the keyring");
    } catch (err) {
      toast(
        `Removing the password failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  async function clearOpenrouterKey(): Promise<void> {
    openrouterKey = "";
    try {
      await ai.clearOpenrouterKey();
      await ai.supervisor.check("openrouter");
      toast("OpenRouter API key removed from the keyring");
    } catch (err) {
      toast(
        `Removing the API key failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  function close(): void {
    // `open` is bindable and the app shell passes no `onclose` — clearing
    // it here is what actually dismisses the dialog (Cancel/Escape).
    open = false;
    onclose?.();
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }
</script>

{#if open}
  <Dialog.Root bind:open>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class="dialog flex w-[42rem] max-w-none flex-col gap-0 overflow-y-auto rounded-2xl p-5 text-popover-foreground shadow-[0_8px_32px_rgba(0,0,0,0.35)] max-h-[calc(100dvh_-_2rem)] max-[36rem]:max-h-[calc(100dvh_-_1rem)] max-[36rem]:p-4 sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="ai-settings-title"
      aria-describedby="ai-settings-description"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <header class="dialog-header mb-[1.125rem] flex flex-none items-start justify-between gap-4">
        <div>
          <Dialog.Title id="ai-settings-title" class="title m-0 text-[1.2rem] font-semibold text-foreground">AI settings</Dialog.Title>
          <p id="ai-settings-description" class="subtitle mb-0 mt-1 text-[0.8125rem] text-muted-foreground">Choose a provider and check its connection.</p>
        </div>
        <Button variant="ghost" size="icon" class="icon-button flex-none rounded-full text-[1.35rem] leading-none text-muted-foreground" type="button" aria-label="Close AI settings" onclick={close}>
          ×
        </Button>
      </header>

      <form
        class="form flex min-h-0 flex-1 flex-col"
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <div class="dialog-body flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-[0.35rem] text-xs font-semibold text-muted-foreground">Preferred backend</legend>
          {#each AI_BACKENDS as option (option)}
            <label
              class="backend-option grid min-h-[3.25rem] cursor-pointer grid-cols-[auto_minmax(0,1fr)_minmax(8rem,auto)] items-center gap-3 rounded-lg border px-3 py-[0.65rem] max-[36rem]:grid-cols-[auto_minmax(0,1fr)] {backendChoice === option
                ? 'chosen border-primary bg-primary/[0.07]'
                : 'border-border'}"
            >
              <input
                type="radio"
                name="ai-backend"
                value={option}
                class="m-0 accent-primary"
                bind:group={backendChoice}
              />
              <span class="backend-copy flex min-w-0 flex-col gap-[0.15rem]">
                <strong class="text-sm font-semibold">{option === "opencode" ? "OpenCode" : "OpenRouter"}</strong>
                <span class="text-[0.72rem] text-muted-foreground">{option === "opencode" ? "Local server, managed by mygitui" : "Direct API, requires an API key"}</span>
              </span>
              <span class="backend-status {statuses[option].status} flex min-w-0 flex-col gap-[0.15rem] text-right text-[0.72rem] [overflow-wrap:anywhere] max-w-56 max-[36rem]:col-start-2 max-[36rem]:text-left {statuses[option].status === 'ok' ? 'text-primary' : statuses[option].status === 'down' || statuses[option].status === 'unauthenticated' ? 'text-destructive' : 'text-muted-foreground'}">{statusLine(option)}</span>
            </label>
          {/each}
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-[0.35rem] text-xs font-semibold text-muted-foreground">OpenCode</legend>
          <p class="status-line m-0 text-[0.8125rem] text-foreground [overflow-wrap:anywhere]" role="status">{installLine()}</p>
          <div class="row flex flex-wrap items-center gap-3">
            <Button
              variant="link"
              size="sm"
              class="link h-auto p-0 text-[0.72rem] underline"
              type="button"
              disabled={checkingInstall}
              onclick={() => void refreshInstall()}
            >
              {#if checkingInstall}Checking…{:else}Check OpenCode{/if}
            </Button>
            {#if detection?.installed && detection.major !== null && detection.major >= 2}
              <span class="hint m-0 text-[0.72rem] text-muted-foreground">OpenCode 2.x detected (v{detection.version}) — managed and attach modes both supported</span>
            {/if}
          </div>
          <label class="radio flex cursor-pointer items-baseline gap-[0.375rem] text-[0.8125rem]">
            <Checkbox bind:checked={opencodeEnabled} />
            <span>Enable OpenCode</span>
          </label>
          <label class="field flex flex-col gap-[0.125rem]">
            <span class="field-label text-[0.72rem] text-muted-foreground">Search models</span>
            <Input
              class="input h-auto rounded px-[0.625rem] py-2 text-[0.8125rem]"
              type="search"
              placeholder="Filter by model or provider"
              aria-label="Search OpenCode models"
              bind:value={opencodeModelSearch}
              disabled={!opencodeEnabled}
            />
          </label>
          <label class="field flex flex-col gap-[0.125rem]">
            <span class="field-label text-[0.72rem] text-muted-foreground">Default model</span>
            <select class="input rounded border border-border bg-background px-[0.625rem] py-2 text-[0.8125rem] text-foreground font-[inherit] focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary" bind:value={opencodeModel} disabled={!opencodeEnabled}>
              <option value="">Use OpenCode server default</option>
              {#if opencodeModel && !opencodeModelGroups.some(
                  (group) => group.models.some((model) => model.id === opencodeModel),
                )}
                <option value={opencodeModel}>
                  {opencodeModelsLoading
                    ? `Saved · ${opencodeModel}`
                    : opencodeModels.some((model) => model.id === opencodeModel)
                      ? `Selected · ${opencodeModel}`
                      : `Unavailable · ${opencodeModel}`}
                </option>
              {/if}
              {#each opencodeModelGroups as group (group.provider)}
                <optgroup label={group.provider}>
                  {#each group.models as model (model.id)}
                    <option value={model.id}>{model.label ?? model.id}</option>
                  {/each}
                </optgroup>
              {/each}
            </select>
          </label>
          <p class="hint m-0 text-[0.72rem] text-muted-foreground">
            Used whenever an AI feature is routed to OpenCode. Leave on the server default to
            follow OpenCode’s own model setting.
          </p>
          {#if opencodeModelsLoading}
            <p class="hint m-0 text-[0.72rem] text-muted-foreground" role="status">Loading OpenCode models…</p>
          {/if}
          {#if opencodeModelsError}
            <p class="hint error-text m-0 text-[0.72rem] text-destructive">OpenCode model list unavailable: {opencodeModelsError}</p>
          {/if}
          {#if !opencodeEnabled}
            <p class="hint warn m-0 text-[0.72rem] text-destructive">
              OpenCode is disabled. AI features can use OpenRouter when fallback is enabled.
            </p>
          {/if}

          <label class="radio advanced-toggle mt-1 flex cursor-pointer items-baseline gap-[0.375rem] text-[0.8125rem] text-muted-foreground">
            <Checkbox bind:checked={showAdvanced} />
            <span>Advanced</span>
          </label>
          {#if showAdvanced}
            <fieldset class="group inner m-0 ml-3.5 mt-0.5 flex min-w-0 flex-col gap-[0.625rem] rounded-none border-0 border-l border-border bg-transparent pb-0 pl-2.5 pr-0 pt-0">
              <legend class="legend px-[0.35rem] text-xs font-semibold text-muted-foreground">Server mode</legend>
              {#each ["managed", "attach"] as modeOption (modeOption)}
                <label class="radio flex cursor-pointer items-baseline gap-[0.375rem] text-[0.8125rem]">
                  <input
                    type="radio"
                    name="opencode-mode"
                    value={modeOption}
                    class="m-0 accent-primary"
                    bind:group={opencodeModeChoice}
                  />
                  <span>
                    {modeOption === "managed"
                      ? "Managed — mygitui starts OpenCode when needed"
                      : "Attach — connect to an OpenCode server I run"}
                  </span>
                </label>
              {/each}
              {#if opencodeModeChoice === "attach"}
                <label class="field flex flex-col gap-[0.125rem]">
                  <span class="field-label text-[0.72rem] text-muted-foreground">Server URL</span>
                  <Input
                    class="input h-auto rounded px-[0.625rem] py-2 text-[0.8125rem]"
                    type="url"
                    placeholder="http://127.0.0.1:4096"
                    spellcheck="false"
                    bind:value={opencodeUrl}
                  />
                </label>
                <p class="hint m-0 text-[0.72rem] text-muted-foreground">
                  Leave empty to check the default server at 127.0.0.1:4096.
                </p>
              {/if}
            </fieldset>
            <label class="field flex flex-col gap-[0.125rem]">
              <span class="field-label text-[0.72rem] text-muted-foreground">Server password (optional)</span>
              <Input
                class="input h-auto rounded px-[0.625rem] py-2 text-[0.8125rem]"
                type="password"
                autocomplete="off"
                placeholder="stored in OS keyring (type to change)"
                bind:value={opencodePassword}
              />
            </label>
            <div class="row flex flex-wrap items-center gap-3">
              <Button variant="link" size="sm" class="link h-auto p-0 text-[0.72rem] underline" type="button" onclick={() => void clearOpencodePassword()}>
                Remove stored password
              </Button>
            </div>
          {/if}
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-[0.35rem] text-xs font-semibold text-muted-foreground">OpenRouter</legend>
          <label class="field flex flex-col gap-[0.125rem]">
            <span class="field-label text-[0.72rem] text-muted-foreground">API key</span>
            <Input
              class="input h-auto rounded px-[0.625rem] py-2 text-[0.8125rem]"
              type="password"
              autocomplete="off"
              placeholder="stored in OS keyring (type to change)"
              bind:value={openrouterKey}
            />
          </label>
          <div class="row flex flex-wrap items-center gap-3">
            <Button variant="link" size="sm" class="link h-auto p-0 text-[0.72rem] underline" type="button" onclick={() => void clearOpenrouterKey()}>
              Remove stored API key
            </Button>
          </div>
          <label class="field flex flex-col gap-[0.125rem]">
            <span class="field-label text-[0.72rem] text-muted-foreground">Default model</span>
            <Input
              class="input h-auto rounded px-[0.625rem] py-2 text-[0.8125rem]"
              type="text"
              list="ai-openrouter-models"
              placeholder="anthropic/claude-sonnet-4.5"
              spellcheck="false"
              bind:value={openrouterModel}
            />
          </label>
          <datalist id="ai-openrouter-models">
            {#each models as model (model.id)}
              <option value={model.id}>{model.label ?? model.id}</option>
            {/each}
          </datalist>
          {#if modelsError}
            <p class="hint error-text m-0 text-[0.72rem] text-destructive">Model list unavailable: {modelsError}</p>
          {/if}
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-[0.35rem] text-xs font-semibold text-muted-foreground">Behavior</legend>
          <label class="radio flex cursor-pointer items-baseline gap-[0.375rem] text-[0.8125rem]">
            <Checkbox bind:checked={allowFallback} />
            <span>
              Fall back to the other backend when the preferred one is
              unavailable
            </span>
          </label>
          <p class="hint m-0 text-[0.72rem] text-muted-foreground">
            Repositories opt in to AI features individually (asked on first
            use of an AI feature). Secrets live in the OS keyring; the rest
            stays in local app settings.
          </p>
        </fieldset>

        <div class="status mb-0 mt-1 flex items-center gap-3 border-b border-t border-border py-3 max-[36rem]:flex-col max-[36rem]:items-stretch" role="status">
          <Button variant="outline" size="sm" class="secondary rounded-full px-4 text-primary" type="button" disabled={testing} onclick={() => void testConnection()}>
            {#if testing}Checking…{:else}Check connections{/if}
          </Button>
          <ul class="status-list m-0 flex list-none flex-col gap-[0.125rem] p-0 text-[0.72rem] text-muted-foreground">
            {#each AI_BACKENDS as backend (backend)}
              <li>
                <span class="mono font-mono">{backend}</span>: {statusLine(backend)}
              </li>
            {/each}
          </ul>
        </div>
        </div>

        <div class="actions flex flex-none justify-end gap-2 pt-3">
          <Button variant="default" size="sm" class="primary rounded-full px-4" type="submit" disabled={saving}>
            {#if saving}Saving…{:else}Save{/if}
          </Button>
          <Button variant="outline" size="sm" class="secondary rounded-full px-4 text-primary" type="button" onclick={close}>Cancel</Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}
