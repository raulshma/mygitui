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
import type { SavedPermission } from "$lib/ai/opencode";
import type { OpenCodeProvider } from "$lib/ai/opencode";
  import type {
    AiAgenticBudget,
    AiBackend,
    FeatureKind,
    ModelInfo,
    OpencodeMode,
  } from "$lib/ai/types";
  import { AGENTIC_FEATURES, AI_FEATURES, aiFeatureLabel } from "$lib/ai/registry";
import type { AiFeatureMode } from "$lib/ai/types";
  import { customPrompts, CUSTOM_PROMPT_VARS } from "$lib/ai/customPrompts.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import {
    opencodeDetect,
    opencodeServeStatus,
    type OpencodeDetection,
    type OpencodeServeState,
  } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  const iconClose = $derived(appIcon("close")!);
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { describeError } from "$lib/errors";

  let {
    open = $bindable(false),
    onclose = undefined,
  }: {
    open: boolean;
    /** Called after Save (or Close) when the dialog should go away. */
    onclose?: () => void;
  } = $props();

  // --- form state (copies of the live config; applied on Save) ---------------
  let aiEnabled = $state(true);
  let statusBar = $state(true);
  /**
   * Per-feature MODES keyed by tag (chunk 5): agentic-eligible rows carry
   * a tri-state control, the other registry rows a plain on/off checkbox
   * (auto/off). Persisted only for kinds the user changed — see save().
   */
  let featureModes = $state<Record<string, AiFeatureMode>>({});
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
  // Soft monthly AI usage budget (item 58): form copies, applied on Save.
  let aiTokenBudget = $state("0");
  let aiBudgetResetDay = $state("1");

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
  /** Form-row keys: every tag the Features section can persist. */
  type AiFeatureModeKey = import("$lib/ai/registry").AiFeatureTag;

  /** Collapsed-by-default section for mode/URL/password. */
  let showAdvanced = $state(false);

  // -- Saved "always" allow rules (chunk 5): the opencode server persists
  // project-scoped allow rules for approved agentic asks; this section
  // surfaces and revokes them. Server state only — never saved locally.
  let savedRules = $state<SavedPermission[] | null>(null);
  let savedRulesLoading = $state(false);
  let savedRuleError = $state<string | null>(null);
  let removingRule = $state<string | null>(null);

  const savedRulesCapable = $derived.by(() => {
    if (backendChoice !== "opencode" || !opencodeEnabled) return false;
    const provider = ai.supervisor.provider("opencode");
    if (
      typeof (provider as { listSavedPermissions?: unknown }).listSavedPermissions !== "function"
    ) {
      return false;
    }
    // Reachability when the supervisor knows it (a down/unauthenticated
    // server cannot list rules — the quiet note shows); "unknown"/
    // "checking" stay capable-pending until refreshSavedRules resolves.
    const status = statuses.opencode.status;
    return status === "ok" || status === "unknown" || status === "checking";
  });

  async function refreshSavedRules(): Promise<void> {
    const provider = ai.supervisor.provider("opencode");
    if (
      typeof (provider as { listSavedPermissions?: unknown }).listSavedPermissions !== "function"
    ) {
      savedRules = null;
      return;
    }
    savedRulesLoading = true;
    savedRuleError = null;
    try {
      savedRules = await (provider as OpenCodeProvider).listSavedPermissions();
    } catch (err) {
      savedRules = null;
      savedRuleError = describeError(err);
    } finally {
      savedRulesLoading = false;
    }
  }

  // -- Agentic budget (chunk 6): caps for ONE agentic run. Empty fields
  // mean "use default" (resolveAgenticBudget's field-by-field fallback);
  // save() drops values below 1 so the config never carries 0/negatives.
  // Like every other field here, edits land on Save — Reset only clears
  // the inputs (Cancel discards it).
  let budgetWallMinutes = $state("");
  let budgetToolCalls = $state("");

  /**
   * Hard ceiling for the wall-clock cap: `setTimeout` overflows (fires
   * after ~1 ms) beyond 2^31-1 ms, so a larger value would budget-stop
   * every run instantly.
   */
  const BUDGET_WALL_MAX_MINUTES = 24 * 60;

  function resetAgenticBudget(): void {
    budgetWallMinutes = "";
    budgetToolCalls = "";
  }

  /**
   * One budget input → its cap: blank means "use the default", and any
   * value below 1 (the parser would drop 0/negatives) is treated the
   * same instead of being written to the config. `max` clamps the value
   * (the wall clock's `setTimeout` ceiling). Accepts numeric strings
   * from the input (which a number field may hand back as a number).
   */
  function parseBudgetCap(value: string | number, max?: number): number | undefined {
    const text = String(value).trim();
    if (text.length === 0) return undefined;
    const parsed = Number(text);
    if (!Number.isFinite(parsed) || parsed < 1) return undefined;
    const rounded = Math.round(parsed);
    return max !== undefined ? Math.min(rounded, max) : rounded;
  }

  async function removeSavedRule(id: string): Promise<void> {
    const provider = ai.supervisor.provider("opencode");
    removingRule = id;
    try {
      await (provider as OpenCodeProvider).deleteSavedPermission(id);
      savedRules = (savedRules ?? []).filter((rule) => rule.id !== id);
    } catch (err) {
      savedRuleError = describeError(err);
    } finally {
      removingRule = null;
    }
  }

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
      aiEnabled = ai.isEnabled();
      statusBar = ai.isStatusBarEnabled();
      featureModes = Object.fromEntries(
        (
          [...AGENTIC_FEATURES, ...AI_FEATURES.map((def) => def.kind)] as AiFeatureModeKey[]
        ).map((kind) => [kind, ai.featureMode(kind)]),
      );
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
      aiTokenBudget = String(appSettings.aiMonthlyTokenBudget);
      aiBudgetResetDay = String(appSettings.aiBudgetResetDay);
      showAdvanced = false;
      savedRules = null;
      savedRuleError = null;
      budgetWallMinutes =
        ai.config.agenticBudget?.maxWallClockMs !== undefined
          ? String(ai.config.agenticBudget.maxWallClockMs / 60_000)
          : "";
      budgetToolCalls =
        ai.config.agenticBudget?.maxToolCalls !== undefined
          ? String(ai.config.agenticBudget.maxToolCalls)
          : "";
      void refreshInstall();
      if (backendChoice === "opencode" && opencodeEnabled) void refreshSavedRules();
    });
  });

  // The saved-rules section follows the backend radios: switching to the
  // opencode backend mid-dialog (re)loads the rules for it.
  $effect(() => {
    if (!open) return;
    if (backendChoice !== "opencode" || !opencodeEnabled) return;
    void refreshSavedRules();
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
        modelsError = describeError(err);
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
          opencodeModelsError = describeError(err);
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
      ai.setEnabled(aiEnabled);
      ai.setStatusBarEnabled(statusBar);
      // Write only kinds whose form mode differs from the live store: a
      // hand-pinned mode (e.g. single-shot) hydrated into the control
      // survives an unchanged save (never mass-rewritten to auto).
      for (const [kind, mode] of Object.entries(featureModes)) {
        if (mode !== ai.featureMode(kind as AiFeatureModeKey)) {
          ai.setFeatureMode(kind as AiFeatureModeKey, mode);
        }
      }
      // Agentic budget (chunk 6): empty field = default; invalid input is
      // dropped (never 0/negatives in the config — the parser drops those
      // on reload anyway).
      const wallMinutes = parseBudgetCap(budgetWallMinutes, BUDGET_WALL_MAX_MINUTES);
      const toolCalls = parseBudgetCap(budgetToolCalls);
      const agenticBudget: AiAgenticBudget = {};
      if (wallMinutes !== undefined) {
        agenticBudget.maxWallClockMs = wallMinutes * 60_000;
      }
      if (toolCalls !== undefined) {
        agenticBudget.maxToolCalls = toolCalls;
      }
      ai.setAgenticBudget(
        agenticBudget.maxWallClockMs !== undefined || agenticBudget.maxToolCalls !== undefined
          ? agenticBudget
          : undefined,
      );
      ai.setBackend(backendChoice);
      ai.setOpencodeMode(opencodeModeChoice);
      // Kept even in managed mode (attach needs it again after a switch);
      // the resolver only reads it in attach mode.
      ai.setOpencodeUrl(opencodeUrl);
      ai.setOpencodeModel(opencodeModel);
      ai.setOpenrouterModel(openrouterModel);
      ai.setAllowFallback(allowFallback);
      // Soft monthly usage budget (item 58): parsed leniently (blank/junk
      // → 0 = off) and persisted with the app settings envelope.
      const budget = Number(aiTokenBudget.trim());
      const resetDay = Number(aiBudgetResetDay.trim());
      appSettings.update({
        aiMonthlyTokenBudget: Number.isFinite(budget) && budget > 0 ? Math.round(budget) : 0,
        aiBudgetResetDay: Number.isFinite(resetDay) && resetDay >= 1
          ? Math.min(31, Math.max(1, Math.round(resetDay)))
          : 1,
      });
      if (opencodePassword.trim().length > 0) {
        await ai.saveOpencodePassword(opencodePassword);
      }
      if (openrouterKey.trim().length > 0) {
        await ai.saveOpenrouterKey(openrouterKey);
      }
      // Check after writing secrets so the probe uses the newly saved password.
      // With the master switch off nothing is probed or spawned — providers
      // keep their stored config for when AI is turned back on.
      if (aiEnabled) {
        await ai.setOpencodeEnabled(opencodeEnabled);
        await ai.supervisor.check("openrouter").catch(() => {});
        void readInstallState();
      }
      toast("AI settings saved", { kind: "success" });
      onclose?.();
    } catch (err) {
      toast(
        `Saving AI settings failed: ${describeError(err)}`,
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
        `Removing the password failed: ${describeError(err)}`,
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
        `Removing the API key failed: ${describeError(err)}`,
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
      class="dialog flex w-[42rem] max-w-none flex-col overflow-y-auto text-popover-foreground max-h-[calc(100dvh_-_2rem)] wm-stacked:max-h-[calc(100dvh_-_1rem)] sm:max-w-none"
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
          <Dialog.Title id="ai-settings-title" class="title m-0 font-semibold text-foreground">AI settings</Dialog.Title>
          <p id="ai-settings-description" class="subtitle mb-0 mt-1 text-ui-base text-muted-foreground">Choose a provider and check its connection.</p>
        </div>
        <Button variant="ghost" size="icon" class="icon-button flex-none rounded-full text-muted-foreground [&_svg:not([class*='size-'])]:size-4" type="button" aria-label="Close AI settings" onclick={close}>
          <Icon icon={iconClose} size={16} />
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
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">Features</legend>
          <label class="radio flex cursor-pointer items-baseline gap-ui-375 text-ui-base font-medium">
            <Checkbox bind:checked={aiEnabled} />
            <span>
              Enable AI features
              <span class="block text-ui-meta font-normal text-muted-foreground">
                Master switch — off hides every AI affordance and cancels running jobs. Nothing is sent anywhere while off.
              </span>
            </span>
          </label>
          {#if aiEnabled}
            {#each AGENTIC_FEATURES as kind (kind)}
              <div class="feature-mode flex flex-col gap-[3px] pl-4">
                <div class="flex min-h-[1.4rem] items-center justify-between gap-ui-375">
                  <span class="text-ui-base">{aiFeatureLabel(kind)}</span>
                  <span
                    class="mode-control flex flex-none items-center gap-px rounded-full border border-border p-px"
                    role="group"
                    aria-label="{aiFeatureLabel(kind)} mode"
                  >
                    {#each [["auto", "Auto"], ["single-shot", "Single-shot"], ["off", "Off"]] as [mode, label] (mode)}
                      <button
                        class="rounded-full px-2 py-px text-ui-micro transition-colors {featureModes[kind] === mode
                          ? 'bg-primary/15 font-medium text-foreground'
                          : 'text-muted-foreground hover:bg-accent'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                        type="button"
                        aria-pressed={featureModes[kind] === mode}
                        onclick={() => (featureModes[kind] = mode as AiFeatureMode)}
                      >
                        {label}
                      </button>
                    {/each}
                  </span>
                </div>
                <span class="text-ui-meta font-normal text-muted-foreground">
                  {featureModes[kind] === "single-shot"
                    ? "Always gathers context up front and sends one prompt — never uses repo tools."
                    : featureModes[kind] === "off"
                      ? "Turned off."
                      : "Agentic when a 2.x opencode server is connected; falls back to single-shot otherwise."}
                </span>
              </div>
            {/each}
            {#each AI_FEATURES as def (def.kind)}
              {#if !AGENTIC_FEATURES.includes(def.kind)}
                <label class="radio feature-toggle flex cursor-pointer items-baseline gap-ui-375 pl-4 text-ui-base">
                  <Checkbox
                    checked={featureModes[def.kind] !== "off"}
                    onCheckedChange={(checked) => (featureModes[def.kind] = checked === true ? "auto" : "off")}
                  />
                  <span>{def.title}</span>
                </label>
              {/if}
            {/each}
            {#if AGENTIC_FEATURES.length > 0}
              <div class="agentic-budget flex flex-col gap-[3px] border-t border-border pt-[0.625rem] pl-4">
                <span class="text-ui-base">Agentic run budget</span>
                <span class="text-ui-meta font-normal text-muted-foreground">
                  Bounds ONE agentic run. An empty field — or a value below 1 — uses the default
                  (5 minutes / 40 tool calls).
                </span>
                <div class="flex flex-wrap items-center gap-3 pt-px">
                  <label class="field flex items-center gap-ui-375 text-ui-meta">
                    <span class="flex-none text-muted-foreground">Wall clock (minutes)</span>
                    <Input
                      class="w-20 rounded"
                      type="number"
                      min="1"
                      max={BUDGET_WALL_MAX_MINUTES}
                      step="1"
                      bind:value={budgetWallMinutes}
                      aria-label="Agentic run wall clock cap in minutes"
                      placeholder="5"
                    />
                  </label>
                  <label class="field flex items-center gap-ui-375 text-ui-meta">
                    <span class="flex-none text-muted-foreground">Tool calls</span>
                    <Input
                      class="w-20 rounded"
                      type="number"
                      min="1"
                      step="1"
                      bind:value={budgetToolCalls}
                      aria-label="Agentic run tool-call cap"
                      placeholder="40"
                    />
                  </label>
                  <Button
                    variant="link"
                    size="sm"
                    class="link relative h-auto p-0 text-ui-meta underline after:absolute after:-inset-1 after:content-['']"
                    type="button"
                    onclick={resetAgenticBudget}
                  >
                    Reset to defaults
                  </Button>
                </div>
              </div>
            {/if}
            <label class="radio flex cursor-pointer items-baseline gap-ui-375 text-ui-base">
              <Checkbox bind:checked={statusBar} />
              <span>
                Live AI activity in the status bar
                <span class="block text-ui-meta text-muted-foreground">
                  One row per running AI job: phase, elapsed time, Cancel.
                </span>
              </span>
            </label>
          {:else}
            <p class="hint m-0 text-ui-meta text-muted-foreground" role="status">
              AI is off. Provider settings below keep their values for when it is turned back on.
            </p>
          {/if}
        </fieldset>

        {#if aiEnabled && backendChoice === "opencode" && opencodeEnabled}
          <fieldset
            class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]"
          >
            <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">
              Allowed tool rules
            </legend>
            <p class="m-0 text-ui-meta text-muted-foreground">
              Approvals you allowed with "Always in this repo" persist as allow rules on the
              opencode server. Removing one asks again on the next matching request.
            </p>
            {#if !savedRulesCapable}
              <p class="m-0 text-ui-meta text-muted-foreground" role="status">
                Saved rules need a 2.x opencode server.
              </p>
            {:else if savedRulesLoading && savedRules === null}
              <p class="m-0 flex items-center gap-1.5 text-ui-meta text-muted-foreground" role="status">
                <span class="inline-block size-1.5 animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
                Loading saved rules…
              </p>
            {:else if savedRuleError !== null}
              <p class="m-0 text-ui-meta text-destructive" role="alert">
                Saved rules could not be loaded: {savedRuleError}
              </p>
            {:else if savedRules !== null && savedRules.length === 0}
              <p class="m-0 text-ui-meta text-muted-foreground" role="status">
                No saved rules — every agentic ask that needs one asked you first.
              </p>
            {:else if savedRules !== null}
              <ul class="rules m-0 flex list-none flex-col gap-px p-0" aria-label="Saved allow rules">
                {#each savedRules as rule (rule.id)}
                  <li
                    class="rule flex items-baseline gap-2 rounded px-1 py-px text-ui-micro {removingRule === rule.id
                      ? 'opacity-50'
                      : ''}"
                  >
                    <span class="flex-none font-medium text-foreground">{rule.action}</span>
                    <span class="min-w-0 flex-1 truncate font-mono text-muted-foreground" title={rule.resource}>
                      {rule.resource}
                    </span>
                    <span
                      class="flex-none font-mono text-muted-foreground/70"
                      title="Server-side project this rule is scoped to"
                    >
                      {rule.projectID.slice(0, 8)}
                    </span>
                    <Button
                      variant="ghost"
                      size="xs"
                      class="h-auto flex-none whitespace-nowrap rounded-sm px-1.5 py-px font-normal text-destructive"
                      type="button"
                      disabled={removingRule !== null}
                      onclick={() => void removeSavedRule(rule.id)}
                    >
                      Remove
                    </Button>
                  </li>
                {/each}
              </ul>
            {/if}
            {#if savedRulesCapable}
              <Button
                variant="link"
                size="sm"
                class="link relative h-auto self-start p-0 text-ui-meta underline after:absolute after:-inset-1 after:content-['']"
                type="button"
                disabled={savedRulesLoading}
                onclick={() => void refreshSavedRules()}
              >
                {#if savedRulesLoading}Refreshing…{:else}Refresh saved rules{/if}
              </Button>
            {/if}
          </fieldset>
        {/if}

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">Preferred backend</legend>
          {#each AI_BACKENDS as option (option)}
            <label
              class="backend-option grid min-h-[3.25rem] cursor-pointer grid-cols-[auto_minmax(0,1fr)_minmax(8rem,auto)] items-center gap-3 rounded-lg border px-3 py-[0.65rem] wm-stacked:grid-cols-[auto_minmax(0,1fr)] {backendChoice === option
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
              <span class="backend-copy flex min-w-0 flex-col gap-ui-15">
                <strong class="text-sm font-semibold">{option === "opencode" ? "OpenCode" : "OpenRouter"}</strong>
                <span class="text-ui-meta text-muted-foreground">{option === "opencode" ? "Local server, managed by mygitui" : "Direct API, requires an API key"}</span>
              </span>
              <span class="backend-status {statuses[option].status} flex min-w-0 flex-col gap-ui-15 text-right text-ui-meta [overflow-wrap:anywhere] max-w-56 wm-stacked:col-start-2 wm-stacked:text-left {statuses[option].status === 'ok' ? 'text-primary-ink' : statuses[option].status === 'down' || statuses[option].status === 'unauthenticated' ? 'text-destructive' : 'text-muted-foreground'}">{statusLine(option)}</span>
            </label>
          {/each}
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">OpenCode</legend>
          <p class="status-line m-0 text-ui-base text-foreground [overflow-wrap:anywhere]" role="status">{installLine()}</p>
          <div class="row flex flex-wrap items-center gap-3">
            <Button
              variant="link"
              size="sm"
              class="link relative h-auto p-0 text-ui-meta underline after:absolute after:-inset-1 after:content-['']"
              type="button"
              disabled={checkingInstall}
              onclick={() => void refreshInstall()}
            >
              {#if checkingInstall}Checking…{:else}Check OpenCode{/if}
            </Button>
            {#if detection?.installed && detection.major !== null && detection.major >= 2}
              <span class="hint m-0 text-ui-meta text-muted-foreground">OpenCode 2.x detected (v{detection.version}) — managed and attach modes both supported</span>
            {/if}
          </div>
          <label class="radio flex cursor-pointer items-baseline gap-ui-375 text-ui-base">
            <Checkbox bind:checked={opencodeEnabled} />
            <span>Enable OpenCode</span>
          </label>
          <label class="field flex flex-col gap-ui-125">
            <span class="field-label text-ui-meta text-muted-foreground">Search models</span>
            <Input
              class="input rounded"
              type="search"
              placeholder="Filter by model or provider"
              aria-label="Search OpenCode models"
              bind:value={opencodeModelSearch}
              disabled={!opencodeEnabled}
            />
          </label>
          <label class="field flex flex-col gap-ui-125">
            <span class="field-label text-ui-meta text-muted-foreground">Default model</span>
            <select class="input rounded border border-border bg-background text-foreground font-[inherit] focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary" bind:value={opencodeModel} disabled={!opencodeEnabled}>
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
          <p class="hint m-0 text-ui-meta text-muted-foreground">
            Used whenever an AI feature is routed to OpenCode. Leave on the server default to
            follow OpenCode’s own model setting.
          </p>
          {#if opencodeModelsLoading}
            <p class="hint m-0 text-ui-meta text-muted-foreground" role="status">Loading OpenCode models…</p>
          {/if}
          {#if opencodeModelsError}
            <p class="hint error-text m-0 text-ui-meta text-destructive">OpenCode model list unavailable: {opencodeModelsError}</p>
          {/if}
          {#if !opencodeEnabled}
            <p class="hint warn m-0 text-ui-meta text-destructive">
              OpenCode is disabled. AI features can use OpenRouter when fallback is enabled.
            </p>
          {/if}

          <label class="radio advanced-toggle mt-1 flex cursor-pointer items-baseline gap-ui-375 text-ui-base text-muted-foreground">
            <Checkbox bind:checked={showAdvanced} />
            <span>Advanced</span>
          </label>
          {#if showAdvanced}
            <fieldset class="group inner m-0 ml-3.5 mt-0.5 flex min-w-0 flex-col gap-[0.625rem] rounded-none border-0 border-l border-border bg-transparent pb-0 pl-2.5 pr-0 pt-0">
              <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">Server mode</legend>
              {#each ["managed", "attach"] as modeOption (modeOption)}
                <label class="radio flex cursor-pointer items-baseline gap-ui-375 text-ui-base">
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
                <label class="field flex flex-col gap-ui-125">
                  <span class="field-label text-ui-meta text-muted-foreground">Server URL</span>
                  <Input
                    class="input rounded"
                    type="url"
                    placeholder="http://127.0.0.1:4096"
                    spellcheck="false"
                    bind:value={opencodeUrl}
                  />
                </label>
                <p class="hint m-0 text-ui-meta text-muted-foreground">
                  Leave empty to check the default server at 127.0.0.1:4096.
                </p>
              {/if}
            </fieldset>
            <label class="field flex flex-col gap-ui-125">
              <span class="field-label text-ui-meta text-muted-foreground">Server password (optional)</span>
              <Input
                class="input rounded"
                type="password"
                autocomplete="off"
                placeholder="stored in OS keyring (type to change)"
                bind:value={opencodePassword}
              />
            </label>
            <div class="row flex flex-wrap items-center gap-3">
              <Button variant="link" size="sm" class="link relative h-auto p-0 text-ui-meta underline after:absolute after:-inset-1 after:content-['']" type="button" onclick={() => void clearOpencodePassword()}>
                Remove stored password
              </Button>
            </div>
          {/if}
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">OpenRouter</legend>
          <label class="field flex flex-col gap-ui-125">
            <span class="field-label text-ui-meta text-muted-foreground">API key</span>
            <Input
              class="input rounded"
              type="password"
              autocomplete="off"
              placeholder="stored in OS keyring (type to change)"
              bind:value={openrouterKey}
            />
          </label>
          <div class="row flex flex-wrap items-center gap-3">
            <Button variant="link" size="sm" class="link relative h-auto p-0 text-ui-meta underline after:absolute after:-inset-1 after:content-['']" type="button" onclick={() => void clearOpenrouterKey()}>
              Remove stored API key
            </Button>
          </div>
          <label class="field flex flex-col gap-ui-125">
            <span class="field-label text-ui-meta text-muted-foreground">Default model</span>
            <Input
              class="input rounded"
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
            <p class="hint error-text m-0 text-ui-meta text-destructive">Model list unavailable: {modelsError}</p>
          {/if}
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">Behavior</legend>
          <label class="radio flex cursor-pointer items-baseline gap-ui-375 text-ui-base">
            <Checkbox bind:checked={allowFallback} />
            <span>
              Fall back to the other backend when the preferred one is
              unavailable
            </span>
          </label>
          <p class="hint m-0 text-ui-meta text-muted-foreground">
            Repositories opt in to AI features individually (asked on first
            use of an AI feature). Secrets live in the OS keyring; the rest
            stays in local app settings.
          </p>
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">Custom prompts</legend>
          <p class="hint m-0 text-ui-meta text-muted-foreground">
            Reusable templates that appear as AI commands in the command
            palette. Variables:
            {#each CUSTOM_PROMPT_VARS as variable, i (variable)}
              <code class="font-mono text-foreground">{`{{${variable}}}`}</code>{i < CUSTOM_PROMPT_VARS.length - 1 ? ", " : ""}
            {/each}
            — budgeted staged diff, per-file change stats, recent commits.
            Unknown placeholders are left as-is; edits apply immediately.
          </p>
          {#each customPrompts.entries as entry (entry.id)}
            <div class="custom-prompt flex flex-col gap-1.5 rounded-lg border border-border bg-background px-3 py-2">
              <div class="row flex items-center gap-2">
                <Checkbox
                  checked={entry.enabled}
                  onCheckedChange={(checked) => customPrompts.update(entry.id, { enabled: checked === true })}
                />
                <Input
                  class="input rounded"
                  type="text"
                  aria-label="Prompt name"
                  placeholder="Prompt name"
                  value={entry.name}
                  onchange={(event) => customPrompts.update(entry.id, { name: event.currentTarget.value })}
                />
                <Button
                  variant="outline"
                  size="icon"
                  class="icon-button flex-none rounded-full text-muted-foreground [&_svg:not([class*='size-'])]:size-3.5"
                  type="button"
                  aria-label={`Delete ${entry.name}`}
                  onclick={() => customPrompts.remove(entry.id)}
                >
                  <Icon icon={iconClose} size={12} />
                </Button>
              </div>
              <Textarea
                class="min-h-16 resize-y rounded font-mono text-ui-meta"
                aria-label={`Template for ${entry.name}`}
                spellcheck="false"
                value={entry.template}
                onchange={(event) => customPrompts.update(entry.id, { template: event.currentTarget.value })}
              />
            </div>
          {/each}
          <div class="row flex items-center gap-3">
            <Button
              variant="outline"
              size="sm"
              class="rounded-full text-primary-ink"
              type="button"
              onclick={() => customPrompts.add()}
            >
              Add prompt
            </Button>
            <span class="text-ui-micro text-muted-foreground">Enabled entries become “AI: …” palette commands.</span>
          </div>
        </fieldset>

        <fieldset class="group m-0 flex min-w-0 flex-col gap-[0.625rem] rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]">
          <legend class="legend px-ui-35 text-xs font-semibold text-muted-foreground">Usage budget</legend>
          <div class="row flex flex-wrap items-end gap-3">
            <label class="field flex flex-col gap-ui-125">
              <span class="field-label text-ui-meta text-muted-foreground">Monthly token budget (0 = off)</span>
              <Input
                class="input w-44 rounded"
                type="text"
                inputmode="numeric"
                placeholder="e.g. 2000000"
                spellcheck="false"
                bind:value={aiTokenBudget}
              />
            </label>
            <label class="field flex flex-col gap-ui-125">
              <span class="field-label text-ui-meta text-muted-foreground">Reset day (1–31)</span>
              <Input
                class="input w-24 rounded"
                type="text"
                inputmode="numeric"
                spellcheck="false"
                bind:value={aiBudgetResetDay}
              />
            </label>
          </div>
          <p class="hint m-0 text-ui-meta text-muted-foreground">
            Soft budget — runs are never blocked. When a budget is set, the
            status bar shows “% of monthly budget” (a warning from 100% up),
            computed from THIS machine's visible AI activity history (the
            most recent 200 runs) — it is a lower bound, not a bill.
          </p>
        </fieldset>

        <div class="status mb-0 mt-1 flex items-center gap-3 border-b border-t border-border py-3 wm-stacked:flex-col wm-stacked:items-stretch" role="status">
          <Button variant="outline" class="secondary rounded-full text-primary-ink" type="button" disabled={testing} onclick={() => void testConnection()}>
            {#if testing}Checking…{:else}Check connections{/if}
          </Button>
          <ul class="status-list m-0 flex list-none flex-col gap-ui-125 p-0 text-ui-meta text-muted-foreground">
            {#each AI_BACKENDS as backend (backend)}
              <li>
                <span class="mono font-mono">{backend}</span>: {statusLine(backend)}
              </li>
            {/each}
          </ul>
        </div>
        </div>

        <div class="actions flex flex-none justify-end gap-2 pt-3">
          <Button variant="default" class="primary rounded-full" type="submit" disabled={saving}>
            {#if saving}Saving…{:else}Save{/if}
          </Button>
          <Button variant="outline" class="secondary rounded-full text-primary-ink" type="button" onclick={close}>Cancel</Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}
