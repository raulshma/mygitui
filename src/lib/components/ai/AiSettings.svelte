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
  let openrouterModel = $state("");
  let allowFallback = $state(true);
  /** Write-only secret inputs (never rendered back, never persisted here). */
  let opencodePassword = $state("");
  let openrouterKey = $state("");

  let saving = $state(false);
  let testing = $state(false);
  let models = $state<ModelInfo[]>([]);
  let modelsError = $state<string | null>(null);
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

  /** Probes for the local CLI + managed server (best effort, never throws). */
  async function refreshInstall(): Promise<void> {
    checkingInstall = true;
    try {
      [detection, serveState] = await Promise.all([
        opencodeDetect().catch(() => null),
        opencodeServeStatus().catch(() => null),
      ]);
    } finally {
      checkingInstall = false;
    }
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
      openrouterModel = ai.config.openrouterModel ?? "";
      allowFallback = ai.config.allowFallback !== false;
      opencodePassword = "";
      openrouterKey = "";
      showAdvanced = false;
      void refreshInstall();
    });
  });

  // Load the model list for the selected backend (datalist suggestions).
  $effect(() => {
    if (!open) return;
    void backendChoice;
    const provider = ai.supervisor.provider(backendChoice);
    models = [];
    modelsError = null;
    provider
      .listModels()
      .then((list) => {
        models = list;
      })
      .catch((err: unknown) => {
        models = [];
        modelsError = err instanceof Error ? err.message : String(err);
      });
  });

  const backendLabels: Record<AiBackend, string> = {
    opencode: "opencode — local, auto-managed (recommended)",
    openrouter: "OpenRouter — direct API (needs an API key)",
  };

  /** One-line summary of the local opencode install + managed server. */
  function installLine(): string {
    if (detection === null) return "checking for a local opencode install…";
    if (!detection.installed) {
      return "not found on PATH — install opencode, then Re-check";
    }
    const version = detection.version ?? "unknown version";
    const at = detection.path ? ` (${detection.path})` : "";
    if (serveState?.running && serveState.url) {
      return `v${version} — managed server running at ${serveState.url}`;
    }
    if (opencodeModeChoice === "attach") {
      return `v${version}${at} — attach mode: using a server you run`;
    }
    if (opencodeEnabled) {
      return `v${version}${at} — will start automatically (managed)`;
    }
    return `v${version}${at} — disabled`;
  }

  /** Per-backend status line for the display under "Test connection". */
  function statusLine(backend: AiBackend): string {
    const status = statuses[backend];
    if (status.status === "checking") return "checking…";
    if (status.status === "unknown") return "not tested";
    if (status.status === "ok") return `ok (${status.latencyMs ?? "?"} ms)`;
    if (status.status === "unauthenticated") return "reachable but unauthorized";
    return status.error ? `down — ${status.error}` : "down";
  }

  async function testConnection(): Promise<void> {
    testing = true;
    try {
      await ai.supervisor.checkAll();
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
      ai.setOpenrouterModel(openrouterModel);
      ai.setAllowFallback(allowFallback);
      await ai.setOpencodeEnabled(opencodeEnabled);
      if (opencodePassword.trim().length > 0) {
        await ai.saveOpencodePassword(opencodePassword);
      }
      if (openrouterKey.trim().length > 0) {
        await ai.saveOpenrouterKey(openrouterKey);
      }
      // Reflect the new config immediately (managed mode spawns here).
      await ai.supervisor.checkAll().catch(() => {});
      void refreshInstall();
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
    >
      <h2 id="ai-settings-title" class="title">AI settings</h2>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <fieldset class="group">
          <legend class="legend">Backend</legend>
          {#each AI_BACKENDS as option (option)}
            <label class="radio">
              <input
                type="radio"
                name="ai-backend"
                value={option}
                bind:group={backendChoice}
              />
              <span>{backendLabels[option]}</span>
            </label>
          {/each}
        </fieldset>

        <fieldset class="group">
          <legend class="legend">opencode</legend>
          <p class="status-line" role="status">{installLine()}</p>
          <div class="row">
            <button
              class="link"
              type="button"
              disabled={checkingInstall}
              onclick={() => void refreshInstall()}
            >
              {#if checkingInstall}Checking…{:else}Re-check{/if}
            </button>
            {#if detection?.installed && detection.major !== null && detection.major >= 2}
              <span class="hint">opencode 2.x detected — this build targets 1.x servers</span>
            {/if}
          </div>
          <label class="radio">
            <input type="checkbox" bind:checked={opencodeEnabled} />
            <span>Enable opencode (the app starts and manages the server)</span>
          </label>
          {#if !opencodeEnabled}
            <p class="hint warn">
              opencode is off — nothing is probed or spawned and AI features
              use OpenRouter only.
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
                      ? "Managed — mygitui spawns opencode serve itself (default)"
                      : "Attach — connect to a server I run"}
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
                  Leave empty to autodiscover the default (127.0.0.1:4096).
                </p>
              {/if}
            </fieldset>
            <label class="field">
              <span class="field-label">Password (basic auth, optional)</span>
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
            {#if testing}Testing…{:else}Test connection{/if}
          </button>
          <ul class="status-list">
            {#each AI_BACKENDS as backend (backend)}
              <li>
                <span class="mono">{backend}</span>: {statusLine(backend)}
              </li>
            {/each}
          </ul>
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
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(30rem, calc(100vw - 2rem));
    max-height: calc(100vh - 4rem);
    overflow-y: auto;
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    border-radius: var(--m3-shape-large, 16px);
    padding: 1rem 1.25rem;
    box-shadow: 0 8px 32px color-mix(in srgb, black 35%, transparent);
  }

  .title {
    margin: 0 0 0.5rem;
    font-size: 1rem;
    font-weight: 600;
  }

  .group {
    border: none;
    margin: 0 0 0.75rem;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .legend {
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 0;
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
    padding: 0.3rem 0.5rem;
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
  }

  .group.inner {
    margin: 0.25rem 0 0 0.875rem;
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
    padding-left: 0.625rem;
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
    align-items: flex-start;
    gap: 0.75rem;
    margin: 0.25rem 0 0.75rem;
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
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
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
  .link:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
