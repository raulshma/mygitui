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
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="scrim" role="presentation" onkeydown={onKeydown}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="ai-settings-title"
      aria-describedby="ai-settings-description"
    >
      <header class="dialog-header">
        <div>
          <h2 id="ai-settings-title" class="title">AI settings</h2>
          <p id="ai-settings-description" class="subtitle">Choose a provider and check its connection.</p>
        </div>
        <button class="icon-button" type="button" aria-label="Close AI settings" onclick={close}>
          ×
        </button>
      </header>

      <form
        class="form"
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <div class="dialog-body">
        <fieldset class="group">
          <legend class="legend">Preferred backend</legend>
          {#each AI_BACKENDS as option (option)}
            <label class:chosen={backendChoice === option} class="backend-option">
              <input
                type="radio"
                name="ai-backend"
                value={option}
                bind:group={backendChoice}
              />
              <span class="backend-copy">
                <strong>{option === "opencode" ? "OpenCode" : "OpenRouter"}</strong>
                <span>{option === "opencode" ? "Local server, managed by mygitui" : "Direct API, requires an API key"}</span>
              </span>
              <span class="backend-status {statuses[option].status}">{statusLine(option)}</span>
            </label>
          {/each}
        </fieldset>

        <fieldset class="group">
          <legend class="legend">OpenCode</legend>
          <p class="status-line" role="status">{installLine()}</p>
          <div class="row">
            <button
              class="link"
              type="button"
              disabled={checkingInstall}
              onclick={() => void refreshInstall()}
            >
              {#if checkingInstall}Checking…{:else}Check OpenCode{/if}
            </button>
            {#if detection?.installed && detection.major !== null && detection.major >= 2}
              <span class="hint">OpenCode 2.x detected — this build currently supports 1.x servers</span>
            {/if}
          </div>
          <label class="radio">
            <input type="checkbox" bind:checked={opencodeEnabled} />
            <span>Enable OpenCode</span>
          </label>
          <label class="field">
            <span class="field-label">Search models</span>
            <input
              class="input"
              type="search"
              placeholder="Filter by model or provider"
              aria-label="Search OpenCode models"
              bind:value={opencodeModelSearch}
              disabled={!opencodeEnabled}
            />
          </label>
          <label class="field">
            <span class="field-label">Default model</span>
            <select class="input" bind:value={opencodeModel} disabled={!opencodeEnabled}>
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
          <p class="hint">
            Used whenever an AI feature is routed to OpenCode. Leave on the server default to
            follow OpenCode’s own model setting.
          </p>
          {#if opencodeModelsLoading}
            <p class="hint" role="status">Loading OpenCode models…</p>
          {/if}
          {#if opencodeModelsError}
            <p class="hint error-text">OpenCode model list unavailable: {opencodeModelsError}</p>
          {/if}
          {#if !opencodeEnabled}
            <p class="hint warn">
              OpenCode is disabled. AI features can use OpenRouter when fallback is enabled.
            </p>
          {/if}

          <label class="radio advanced-toggle">
            <input type="checkbox" bind:checked={showAdvanced} />
            <span>Advanced</span>
          </label>
          {#if showAdvanced}
            <fieldset class="group inner">
              <legend class="legend">Server mode</legend>
              {#each ["managed", "attach"] as modeOption (modeOption)}
                <label class="radio">
                  <input
                    type="radio"
                    name="opencode-mode"
                    value={modeOption}
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
                <label class="field">
                  <span class="field-label">Server URL</span>
                  <input
                    class="input"
                    type="url"
                    placeholder="http://127.0.0.1:4096"
                    spellcheck="false"
                    bind:value={opencodeUrl}
                  />
                </label>
                <p class="hint">
                  Leave empty to check the default server at 127.0.0.1:4096.
                </p>
              {/if}
            </fieldset>
            <label class="field">
              <span class="field-label">Server password (optional)</span>
              <input
                class="input"
                type="password"
                autocomplete="off"
                placeholder="stored in OS keyring (type to change)"
                bind:value={opencodePassword}
              />
            </label>
            <div class="row">
              <button class="link" type="button" onclick={() => void clearOpencodePassword()}>
                Remove stored password
              </button>
            </div>
          {/if}
        </fieldset>

        <fieldset class="group">
          <legend class="legend">OpenRouter</legend>
          <label class="field">
            <span class="field-label">API key</span>
            <input
              class="input"
              type="password"
              autocomplete="off"
              placeholder="stored in OS keyring (type to change)"
              bind:value={openrouterKey}
            />
          </label>
          <div class="row">
            <button class="link" type="button" onclick={() => void clearOpenrouterKey()}>
              Remove stored API key
            </button>
          </div>
          <label class="field">
            <span class="field-label">Default model</span>
            <input
              class="input"
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
            <p class="hint error-text">Model list unavailable: {modelsError}</p>
          {/if}
        </fieldset>

        <fieldset class="group">
          <legend class="legend">Behavior</legend>
          <label class="radio">
            <input type="checkbox" bind:checked={allowFallback} />
            <span>
              Fall back to the other backend when the preferred one is
              unavailable
            </span>
          </label>
          <p class="hint">
            Repositories opt in to AI features individually (asked on first
            use of an AI feature). Secrets live in the OS keyring; the rest
            stays in local app settings.
          </p>
        </fieldset>

        <div class="status" role="status">
          <button class="secondary" type="button" disabled={testing} onclick={() => void testConnection()}>
            {#if testing}Checking…{:else}Check connections{/if}
          </button>
          <ul class="status-list">
            {#each AI_BACKENDS as backend (backend)}
              <li>
                <span class="mono">{backend}</span>: {statusLine(backend)}
              </li>
            {/each}
          </ul>
        </div>
        </div>

        <div class="actions">
          <button class="primary" type="submit" disabled={saving}>
            {#if saving}Saving…{:else}Save{/if}
          </button>
          <button class="secondary" type="button" onclick={close}>Cancel</button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow-y: auto;
    padding: 1rem;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(42rem, 100%);
    max-height: calc(100dvh - 2rem);
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    border-radius: var(--m3-shape-large, 16px);
    padding: 1.25rem;
    box-shadow: 0 8px 32px color-mix(in srgb, black 35%, transparent);
  }

  .dialog-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    flex: none;
    margin-bottom: 1.125rem;
  }

  .title {
    margin: 0;
    font-size: 1.2rem;
    font-weight: 600;
  }

  .subtitle {
    margin: 0.25rem 0 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
  }

  .icon-button {
    display: grid;
    flex: none;
    place-items: center;
    width: 2rem;
    height: 2rem;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 1.35rem;
    line-height: 1;
    cursor: pointer;
  }

  .icon-button:hover {
    background: var(--m3-surface-container-highest, var(--m3-surface));
  }

  .form {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* Only this region scrolls — the header above and the actions row below
     stay pinned regardless of how much settings content overflows. */
  .dialog-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .group {
    min-width: 0;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 12px);
    margin: 0;
    padding: 0.9rem 1rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
    background: color-mix(in srgb, var(--m3-surface-container-low, var(--m3-surface)) 58%, transparent);
  }

  .legend {
    padding: 0 0.35rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .backend-option {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) minmax(8rem, auto);
    align-items: center;
    gap: 0.75rem;
    min-height: 3.25rem;
    padding: 0.65rem 0.75rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    cursor: pointer;
  }

  .backend-option.chosen {
    border-color: var(--m3-primary);
    background: color-mix(in srgb, var(--m3-primary) 7%, transparent);
  }

  .backend-option input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .backend-copy,
  .backend-status {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 0.15rem;
  }

  .backend-copy strong {
    font-size: 0.875rem;
    font-weight: 600;
  }

  .backend-copy span,
  .backend-status {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  .backend-status {
    max-width: 14rem;
    overflow-wrap: anywhere;
    text-align: right;
  }

  .backend-status.ok {
    color: var(--m3-primary);
  }

  .backend-status.down,
  .backend-status.unauthenticated {
    color: var(--m3-error, inherit);
  }

  .radio {
    display: flex;
    align-items: baseline;
    gap: 0.375rem;
    font-size: 0.8125rem;
    cursor: pointer;
  }

  .radio input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .field-label {
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .input {
    font: inherit;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.5rem 0.625rem;
  }

  .input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .hint {
    margin: 0;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .hint.warn {
    color: var(--m3-error, inherit);
  }

  .status-line {
    margin: 0;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
    overflow-wrap: anywhere;
  }

  .group.inner {
    margin: 0.25rem 0 0 0.875rem;
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
    padding-left: 0.625rem;
    border-top: 0;
    border-right: 0;
    border-bottom: 0;
    border-radius: 0;
    background: none;
  }

  .advanced-toggle {
    margin-top: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .error-text {
    color: var(--m3-error, inherit);
  }

  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.75rem;
  }

  .link {
    border: none;
    background: none;
    color: var(--m3-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0;
    cursor: pointer;
    text-decoration: underline;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin: 0.25rem 0 0;
    padding: 0.75rem 0;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .status-list {
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .mono {
    font-family: ui-monospace, Consolas, monospace;
  }

  .actions {
    flex: none;
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding-top: 0.75rem;
  }

  .primary,
  .secondary {
    font: inherit;
    font-size: 0.8125rem;
    padding: 0.35rem 1rem;
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
    border: none;
  }

  .primary {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .primary:disabled,
  .secondary:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .secondary {
    background: none;
    color: var(--m3-primary);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .primary:focus-visible,
  .secondary:focus-visible,
  .link:focus-visible,
  .icon-button:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  @media (max-width: 36rem) {
    .scrim {
      align-items: flex-start;
      padding: 0.5rem;
    }

    .dialog {
      max-height: calc(100dvh - 1rem);
      padding: 1rem;
    }

    .backend-option {
      grid-template-columns: auto minmax(0, 1fr);
    }

    .backend-status {
      grid-column: 2;
      text-align: left;
    }

    .status {
      align-items: stretch;
      flex-direction: column;
    }
  }
</style>
