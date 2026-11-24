<script lang="ts">
  /**
   * AiSettingsSection — comprehensive, embeddable AI configuration section.
   *
   * Designed to be directly embedded within the SettingsDialog or anywhere in
   * the app without requiring a dialog chrome, modal overlay, or modal save button.
   *
   * All settings persist reactively through the underlying stores:
   * - Core AI config: `ai` store (`mygitui.ai` in localStorage)
   * - Secrets: OS keyring via Tauri IPC (`secretsSet`, `secretsDelete`)
   * - Usage budget: `appSettings` store (`mygitui.settings`)
   * - Custom prompt templates: `customPrompts` store (`mygitui.ai.customPrompts`)
   */
  import { onMount } from "svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import { AI_BACKENDS } from "$lib/ai/types";
  import type {
    AiAgenticBudget,
    AiBackend,
    AiFeatureMode,
    ModelInfo,
    OpencodeMode,
  } from "$lib/ai/types";
  import {
    AGENTIC_FEATURES,
    AI_FEATURES,
    aiFeatureLabel,
    type AiFeatureTag,
  } from "$lib/ai/registry";
  import type { SavedPermission, OpenCodeProvider } from "$lib/ai/opencode";
  import { customPrompts, CUSTOM_PROMPT_VARS } from "$lib/ai/customPrompts.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import {
    opencodeDetect,
    opencodeServeStatus,
    type OpencodeDetection,
    type OpencodeServeState,
  } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";

  // Icons
  const iconBolt = $derived(appIcon("bolt")!);
  const iconCheck = $derived(appIcon("check")!);
  const iconRefresh = $derived(appIcon("refresh")!);
  const iconSearch = $derived(appIcon("search")!);
  const iconLock = $derived(appIcon("lock")!);
  const iconDelete = $derived(appIcon("delete")!);
  const iconClose = $derived(appIcon("close")!);
  const iconChevronDown = $derived(appIcon("chevron-down")!);
  const iconChevronRight = $derived(appIcon("chevron-right")!);
  const iconInfo = $derived(appIcon("info")!);

  // --- Supervisor status rune tracking ---
  let statuses = $state(ai.supervisor.statuses);
  $effect(() => ai.supervisor.subscribe((next) => (statuses = next)));

  // --- OpenCode detection & server live state ---
  let detection = $state<OpencodeDetection | null>(null);
  let serveState = $state<OpencodeServeState | null>(null);
  let checkingOpencode = $state(false);

  // --- Model lists ---
  let openrouterModels = $state<ModelInfo[]>([]);
  let openrouterModelsLoading = $state(false);
  let openrouterModelsError = $state<string | null>(null);

  let opencodeModels = $state<ModelInfo[]>([]);
  let opencodeModelsLoading = $state(false);
  let opencodeModelsError = $state<string | null>(null);
  let opencodeModelSearch = $state("");

  // Grouped OpenCode models for dropdown
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

  // --- Write-only secret inputs (OS Keyring) ---
  let openrouterKeyInput = $state("");
  let savingOpenrouterKey = $state(false);
  let testingOpenrouter = $state(false);

  let opencodePasswordInput = $state("");
  let savingOpencodePassword = $state(false);

  // --- Agentic Budget Inputs ---
  const BUDGET_WALL_MAX_MINUTES = 24 * 60;
  let budgetWallMinutes = $state(
    ai.config.agenticBudget?.maxWallClockMs !== undefined
      ? String(ai.config.agenticBudget.maxWallClockMs / 60_000)
      : "",
  );
  let budgetToolCalls = $state(
    ai.config.agenticBudget?.maxToolCalls !== undefined
      ? String(ai.config.agenticBudget.maxToolCalls)
      : "",
  );

  // --- Monthly Usage Budget Inputs ---
  let aiTokenBudget = $state(String(appSettings.aiMonthlyTokenBudget));
  let aiBudgetResetDay = $state(String(appSettings.aiBudgetResetDay));

  // --- Saved Permissions (OpenCode server) ---
  let savedRules = $state<SavedPermission[] | null>(null);
  let savedRulesLoading = $state(false);
  let savedRuleError = $state<string | null>(null);
  let removingRule = $state<string | null>(null);

  const savedRulesCapable = $derived.by(() => {
    if (ai.config.backend !== "opencode" || ai.config.opencodeEnabled === false) return false;
    const provider = ai.supervisor.provider("opencode");
    if (
      typeof (provider as { listSavedPermissions?: unknown }).listSavedPermissions !== "function"
    ) {
      return false;
    }
    const status = statuses.opencode.status;
    return status === "ok" || status === "unknown" || status === "checking";
  });

  // --- Collapsible sections toggle state ---
  let showCustomPrompts = $state(true);
  let showAdvancedOpenCode = $state(false);

  // --- Helper methods for budget parsing ---
  function parseBudgetCap(value: string | number, max?: number): number | undefined {
    const text = String(value).trim();
    if (text.length === 0) return undefined;
    const parsed = Number(text);
    if (!Number.isFinite(parsed) || parsed < 1) return undefined;
    const rounded = Math.round(parsed);
    return max !== undefined ? Math.min(rounded, max) : rounded;
  }

  function commitAgenticBudget(): void {
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
  }

  function resetAgenticBudget(): void {
    budgetWallMinutes = "";
    budgetToolCalls = "";
    ai.setAgenticBudget(undefined);
    toast("Agentic budget reset to defaults (5 min / 40 tool calls)", { kind: "success" });
  }

  function commitUsageBudget(): void {
    const budget = Number(aiTokenBudget.trim());
    const resetDay = Number(aiBudgetResetDay.trim());
    appSettings.update({
      aiMonthlyTokenBudget: Number.isFinite(budget) && budget > 0 ? Math.round(budget) : 0,
      aiBudgetResetDay:
        Number.isFinite(resetDay) && resetDay >= 1
          ? Math.min(31, Math.max(1, Math.round(resetDay)))
          : 1,
    });
  }

  // --- Secret actions ---
  async function saveOpenrouterKey(): Promise<void> {
    const key = openrouterKeyInput.trim();
    if (!key) return;
    savingOpenrouterKey = true;
    try {
      await ai.saveOpenrouterKey(key);
      openrouterKeyInput = "";
      toast("OpenRouter API key saved to OS keyring", { kind: "success" });
      await ai.supervisor.check("openrouter").catch(() => {});
      void refreshOpenrouterModels();
    } catch (err) {
      toast(`Failed to save OpenRouter key: ${describeError(err)}`, { kind: "error" });
    } finally {
      savingOpenrouterKey = false;
    }
  }

  async function removeOpenrouterKey(): Promise<void> {
    openrouterKeyInput = "";
    try {
      await ai.clearOpenrouterKey();
      await ai.supervisor.check("openrouter").catch(() => {});
      toast("OpenRouter API key removed from keyring", { kind: "success" });
    } catch (err) {
      toast(`Failed to remove OpenRouter key: ${describeError(err)}`, { kind: "error" });
    }
  }

  async function testOpenrouterConnection(): Promise<void> {
    testingOpenrouter = true;
    try {
      await ai.supervisor.check("openrouter");
      const st = statuses.openrouter;
      if (st.status === "ok") {
        toast(`OpenRouter connected (${st.latencyMs ?? "?"} ms)`, { kind: "success" });
      } else if (st.status === "unauthenticated") {
        toast("OpenRouter requires a valid API key", { kind: "error" });
      } else {
        toast(st.error ? `OpenRouter unavailable: ${st.error}` : "OpenRouter unavailable", {
          kind: "error",
        });
      }
    } catch (err) {
      toast(`OpenRouter check failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      testingOpenrouter = false;
    }
  }

  async function saveOpencodePassword(): Promise<void> {
    const pass = opencodePasswordInput.trim();
    if (!pass) return;
    savingOpencodePassword = true;
    try {
      await ai.saveOpencodePassword(pass);
      opencodePasswordInput = "";
      toast("OpenCode password saved to OS keyring", { kind: "success" });
      await ai.supervisor.check("opencode").catch(() => {});
    } catch (err) {
      toast(`Failed to save OpenCode password: ${describeError(err)}`, { kind: "error" });
    } finally {
      savingOpencodePassword = false;
    }
  }

  async function removeOpencodePassword(): Promise<void> {
    opencodePasswordInput = "";
    try {
      await ai.clearOpencodePassword();
      await ai.supervisor.check("opencode").catch(() => {});
      toast("OpenCode password removed from keyring", { kind: "success" });
    } catch (err) {
      toast(`Failed to remove OpenCode password: ${describeError(err)}`, { kind: "error" });
    }
  }

  // --- OpenCode install & models ---
  async function readInstallState(): Promise<void> {
    if (ai.config.opencodeEnabled === false || (ai.config.opencodeMode ?? "managed") !== "managed") {
      detection = null;
      serveState = null;
      return;
    }
    [detection, serveState] = await Promise.all([
      opencodeDetect().catch(() => null),
      opencodeServeStatus().catch(() => null),
    ]);
  }

  async function refreshOpencode(): Promise<void> {
    checkingOpencode = true;
    try {
      await ai.supervisor.check("opencode");
      await readInstallState();
      if (ai.config.backend === "opencode" && ai.config.opencodeEnabled !== false) {
        void refreshSavedRules();
        void refreshOpencodeModels();
      }
      const st = statuses.opencode;
      if (st.status === "ok") {
        toast(`OpenCode connected (${st.latencyMs ?? "?"} ms)`, { kind: "success" });
      }
    } catch (err) {
      toast(`OpenCode check failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      checkingOpencode = false;
    }
  }

  async function refreshOpenrouterModels(): Promise<void> {
    openrouterModelsLoading = true;
    openrouterModelsError = null;
    try {
      openrouterModels = await ai.supervisor.provider("openrouter").listModels();
    } catch (err) {
      openrouterModels = [];
      openrouterModelsError = describeError(err);
    } finally {
      openrouterModelsLoading = false;
    }
  }

  async function refreshOpencodeModels(): Promise<void> {
    if (ai.config.opencodeEnabled === false) return;
    opencodeModelsLoading = true;
    opencodeModelsError = null;
    try {
      opencodeModels = await ai.supervisor.provider("opencode").listModels();
    } catch (err) {
      opencodeModels = [];
      opencodeModelsError = describeError(err);
    } finally {
      opencodeModelsLoading = false;
    }
  }

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

  async function removeSavedRule(id: string): Promise<void> {
    const provider = ai.supervisor.provider("opencode");
    removingRule = id;
    try {
      await (provider as OpenCodeProvider).deleteSavedPermission(id);
      savedRules = (savedRules ?? []).filter((rule) => rule.id !== id);
      toast("Saved permission rule removed", { kind: "success" });
    } catch (err) {
      savedRuleError = describeError(err);
      toast(`Failed to remove rule: ${describeError(err)}`, { kind: "error" });
    } finally {
      removingRule = null;
    }
  }

  // --- Initial load & effect hooks ---
  onMount(() => {
    void refreshInstallState();
  });

  async function refreshInstallState(): Promise<void> {
    await Promise.all([
      readInstallState(),
      refreshOpenrouterModels(),
      refreshOpencodeModels(),
    ]);
    if (savedRulesCapable) {
      void refreshSavedRules();
    }
  }

  // Summary lines
  function installLine(): string {
    if (ai.config.opencodeEnabled === false) return "OpenCode is disabled.";
    const mode = ai.config.opencodeMode ?? (ai.config.opencodeUrl ? "attach" : "managed");
    if (mode === "attach") {
      const target = ai.config.opencodeUrl?.trim() || "http://127.0.0.1:4096";
      const status = statuses.opencode;
      if (status.status === "ok") return `Connected to ${target}`;
      if (status.status === "checking") return `Checking ${target}…`;
      if (status.status === "unauthenticated") return `The server at ${target} requires a password.`;
      if (status.status === "down") {
        return status.error ? `Could not reach ${target}: ${status.error}` : `Could not reach ${target}.`;
      }
      return `Ready to connect to ${target}.`;
    }
    if (detection === null) return "Checking for OpenCode install…";
    if (!detection.installed) return "OpenCode CLI was not found on PATH. Install it or attach to a remote server.";
    const version = detection.version ? `v${detection.version}` : "version unavailable";
    const at = detection.path ? ` (${detection.path})` : "";
    if (serveState?.running && serveState.url) {
      return `${version} — managed server running at ${serveState.url}`;
    }
    return `${version}${at} — mygitui starts the server when needed`;
  }

  function getStatusInfo(backend: AiBackend) {
    const st = statuses[backend];
    if (st.status === "checking") {
      return { text: "Checking…", dot: "bg-primary animate-ping", badgeClass: "border-border text-muted-foreground" };
    }
    if (st.status === "ok") {
      const latency = st.latencyMs !== null && st.latencyMs !== undefined ? ` · ${st.latencyMs} ms` : "";
      return { text: `Connected${latency}`, dot: "bg-emerald-500", badgeClass: "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400" };
    }
    if (st.status === "unauthenticated") {
      return {
        text: backend === "opencode" ? "Password required" : "API key required",
        dot: "bg-amber-500",
        badgeClass: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400",
      };
    }
    if (st.status === "down") {
      return {
        text: st.error ? `Unavailable: ${st.error}` : "Unavailable",
        dot: "bg-destructive",
        badgeClass: "border-destructive/30 bg-destructive/10 text-destructive",
      };
    }
    return { text: "Not checked", dot: "bg-muted-foreground/50", badgeClass: "border-border text-muted-foreground" };
  }

  const activeStatusInfo = $derived.by(() => {
    if (!ai.isEnabled()) {
      return { text: "AI Disabled", dot: "bg-muted-foreground/40", badgeClass: "border-border text-muted-foreground" };
    }
    return getStatusInfo(ai.config.backend);
  });
</script>

<div class="ai-settings-section flex min-w-0 flex-col gap-4 text-foreground" aria-label="AI Settings">
  <!-- =========================================================================
       1. Master AI & Activity
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex items-center justify-between gap-3">
      <div class="flex items-center gap-2">
        <span class="flex size-7 flex-none items-center justify-center rounded-lg bg-primary/10 text-primary-ink">
          <Icon icon={iconBolt} size={15} />
        </span>
        <div class="flex flex-col">
          <h4 class="m-0 text-sm font-semibold text-foreground">AI Features & Activity</h4>
          <span class="text-ui-meta text-muted-foreground">Master control and background status bar indicators</span>
        </div>
      </div>
      <Badge variant="outline" class="flex items-center gap-1.5 px-2.5 py-0.5 font-normal {activeStatusInfo.badgeClass}">
        <span class="size-1.5 rounded-full {activeStatusInfo.dot}"></span>
        <span>{activeStatusInfo.text}</span>
      </Badge>
    </div>

    <div class="flex flex-col gap-2 pt-1">
      <!-- Master switch row -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="flex cursor-pointer items-center justify-between gap-4 rounded-lg border border-border bg-background/50 px-3.5 py-2.5 transition-colors hover:border-ring/50 hover:bg-muted/30"
        onclick={() => ai.setEnabled(!ai.isEnabled())}
      >
        <span class="flex min-w-0 flex-col gap-0.5">
          <span id="ai-master-toggle-label" class="text-sm font-medium text-foreground">Enable AI features</span>
          <span class="text-ui-meta text-muted-foreground">
            Master switch. Off hides every AI affordance (buttons, assistant, health chip) and cancels running AI jobs.
          </span>
        </span>
        <Switch
          class="flex-none"
          aria-labelledby="ai-master-toggle-label"
          checked={ai.isEnabled()}
          onclick={(e) => e.stopPropagation()}
          onCheckedChange={(checked) => ai.setEnabled(checked)}
        />
      </div>

      <!-- Status bar activity toggle row -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="flex items-center justify-between gap-4 rounded-lg border border-border bg-background/50 px-3.5 py-2.5 transition-colors {ai.isEnabled() ? 'cursor-pointer hover:border-ring/50 hover:bg-muted/30' : 'opacity-60 cursor-not-allowed'}"
        onclick={() => {
          if (ai.isEnabled()) ai.setStatusBarEnabled(!ai.isStatusBarEnabled());
        }}
      >
        <span class="flex min-w-0 flex-col gap-0.5">
          <span id="ai-statusbar-toggle-label" class="text-sm font-medium text-foreground">Live AI activity in the status bar</span>
          <span class="text-ui-meta text-muted-foreground">
            Shows one row per running AI job with phase, elapsed time and quick cancel action.
          </span>
        </span>
        <Switch
          class="flex-none"
          aria-labelledby="ai-statusbar-toggle-label"
          checked={ai.isStatusBarEnabled()}
          disabled={!ai.isEnabled()}
          onclick={(e) => e.stopPropagation()}
          onCheckedChange={(checked) => ai.setStatusBarEnabled(checked)}
        />
      </div>
    </div>
  </section>

  <!-- =========================================================================
       2. Provider & Backend Selection
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex flex-col gap-0.5">
      <h4 class="m-0 text-sm font-semibold text-foreground">Preferred Provider</h4>
      <span class="text-ui-meta text-muted-foreground">
        Choose your primary backend. AI features route to this provider by default.
      </span>
    </div>

    <!-- Segmented Provider Switch -->
    <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-2" role="radiogroup" aria-label="Preferred AI Provider">
      {#each AI_BACKENDS as option (option)}
        {@const st = getStatusInfo(option)}
        {@const selected = ai.config.backend === option}
        <button
          type="button"
          role="radio"
          aria-checked={selected}
          class="flex flex-col gap-2 rounded-xl border p-3.5 text-left transition-all {selected
            ? 'border-primary bg-primary/[0.08] shadow-sm'
            : 'border-border bg-background/50 hover:border-border hover:bg-muted/30'}"
          onclick={() => ai.setBackend(option)}
        >
          <div class="flex items-center justify-between gap-2">
            <span class="text-sm font-semibold text-foreground">
              {option === "opencode" ? "OpenCode" : "OpenRouter"}
            </span>
            <span class="flex items-center gap-1.5 rounded-full px-2 py-0.5 text-ui-micro {st.badgeClass}">
              <span class="size-1.5 rounded-full {st.dot}"></span>
              <span>{st.text}</span>
            </span>
          </div>
          <span class="text-ui-meta text-muted-foreground">
            {option === "opencode"
              ? "Local server on this machine, managed automatically by mygitui."
              : "Direct cloud API with model routing. Requires an OpenRouter API key."}
          </span>
        </button>
      {/each}
    </div>

    <!-- Fallback switch row -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="mt-1 flex cursor-pointer items-center justify-between gap-4 rounded-lg border border-border bg-background/50 px-3.5 py-2.5 transition-colors hover:border-ring/50 hover:bg-muted/30"
      onclick={() => ai.setAllowFallback(ai.config.allowFallback === false)}
    >
      <span class="flex min-w-0 flex-col gap-0.5">
        <span id="ai-fallback-label" class="text-sm font-medium text-foreground">
          Fall back to alternate backend
        </span>
        <span class="text-ui-meta text-muted-foreground">
          If your preferred provider is unavailable or down, automatically attempt the alternate provider.
        </span>
      </span>
      <Switch
        class="flex-none"
        aria-labelledby="ai-fallback-label"
        checked={ai.config.allowFallback !== false}
        onclick={(e) => e.stopPropagation()}
        onCheckedChange={(checked) => ai.setAllowFallback(checked)}
      />
    </div>
  </section>

  <!-- =========================================================================
       3. OpenCode Setup Card
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex items-center justify-between gap-3">
      <div class="flex flex-col gap-0.5">
        <div class="flex items-center gap-2">
          <h4 class="m-0 text-sm font-semibold text-foreground">OpenCode Configuration</h4>
          {#if detection?.installed && detection.major !== null && detection.major >= 2}
            <Badge variant="outline" class="border-primary/30 bg-primary/10 text-primary-ink text-ui-micro font-normal">
              v{detection.version}
            </Badge>
          {/if}
        </div>
        <span class="text-ui-meta text-muted-foreground">Local runtime detection, connection mode, and server defaults</span>
      </div>

      <Button
        variant="outline"
        size="sm"
        class="flex items-center gap-1.5 rounded-full text-ui-meta"
        type="button"
        disabled={checkingOpencode}
        onclick={refreshOpencode}
      >
        <span class={checkingOpencode ? "animate-spin" : ""}>
          <Icon icon={iconRefresh} size={14} />
        </span>
        <span>{checkingOpencode ? "Checking…" : "Check OpenCode"}</span>
      </Button>
    </div>

    <!-- Detection summary pill box -->
    <div class="flex flex-col gap-1.5 rounded-lg border border-border bg-muted/30 px-3.5 py-2.5">
      <div class="flex items-center justify-between gap-2">
        <span class="text-ui-meta font-medium text-foreground">Runtime Status</span>
        {#if serveState?.running}
          <span class="flex items-center gap-1 text-ui-micro text-emerald-600 dark:text-emerald-400">
            <span class="size-1.5 rounded-full bg-emerald-500"></span>
            Managed server running
          </span>
        {/if}
      </div>
      <p class="m-0 text-ui-meta text-muted-foreground [overflow-wrap:anywhere]" role="status">
        {installLine()}
      </p>
    </div>

    <!-- OpenCode enable switch -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="flex cursor-pointer items-center justify-between gap-4 rounded-lg border border-border bg-background/50 px-3.5 py-2.5 transition-colors hover:border-ring/50 hover:bg-muted/30"
      onclick={() => void ai.setOpencodeEnabled(ai.config.opencodeEnabled === false)}
    >
      <span class="flex min-w-0 flex-col gap-0.5">
        <span id="opencode-enable-label" class="text-sm font-medium text-foreground">Enable OpenCode</span>
        <span class="text-ui-meta text-muted-foreground">
          Allows AI operations and agentic jobs to dispatch to the local OpenCode server.
        </span>
      </span>
      <Switch
        class="flex-none"
        aria-labelledby="opencode-enable-label"
        checked={ai.config.opencodeEnabled !== false}
        onclick={(e) => e.stopPropagation()}
        onCheckedChange={(checked) => void ai.setOpencodeEnabled(checked)}
      />
    </div>

    <!-- Model picker row -->
    <div class="flex flex-col gap-1.5 rounded-lg border border-border bg-background/50 p-3.5">
      <div class="flex items-center justify-between gap-2">
        <label for="opencode-default-model-select" class="text-sm font-medium text-foreground">
          Default Model
        </label>
        <Button
          variant="ghost"
          size="xs"
          class="h-auto p-1 text-ui-micro text-muted-foreground hover:text-foreground"
          type="button"
          disabled={opencodeModelsLoading || ai.config.opencodeEnabled === false}
          onclick={refreshOpencodeModels}
        >
          <span class={opencodeModelsLoading ? "animate-spin" : ""}>
            <Icon icon={iconRefresh} size={12} />
          </span>
          <span class="ml-1">{opencodeModelsLoading ? "Loading…" : "Refresh models"}</span>
        </Button>
      </div>

      <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <!-- Model filter input -->
        <div class="relative">
          <Input
            class="h-9 rounded pl-8 text-ui-meta"
            type="search"
            placeholder="Filter models or providers…"
            aria-label="Filter OpenCode models"
            bind:value={opencodeModelSearch}
            disabled={ai.config.opencodeEnabled === false}
          />
          <span class="pointer-events-none absolute left-2.5 top-2.5 text-muted-foreground">
            <Icon icon={iconSearch} size={14} />
          </span>
        </div>

        <!-- Model select dropdown -->
        <select
          id="opencode-default-model-select"
          class="h-9 rounded border border-border bg-background px-3 text-ui-meta text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-50"
          value={ai.config.opencodeModel ?? ""}
          disabled={ai.config.opencodeEnabled === false}
          onchange={(e) => ai.setOpencodeModel((e.currentTarget as HTMLSelectElement).value || undefined)}
        >
          <option value="">Use OpenCode server default</option>
          {#if ai.config.opencodeModel && !opencodeModelGroups.some((g) => g.models.some((m) => m.id === ai.config.opencodeModel))}
            <option value={ai.config.opencodeModel}>
              Saved · {ai.config.opencodeModel}
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
      </div>

      {#if opencodeModelsError}
        <p class="m-0 pt-1 text-ui-meta text-destructive" role="alert">
          Failed to load model list: {opencodeModelsError}
        </p>
      {/if}
    </div>

    <!-- Advanced mode toggle & fields -->
    <div class="flex flex-col gap-2 pt-1">
      <button
        type="button"
        class="flex items-center gap-1.5 self-start text-ui-meta font-medium text-muted-foreground hover:text-foreground focus-visible:outline-none"
        onclick={() => (showAdvancedOpenCode = !showAdvancedOpenCode)}
      >
        <Icon icon={showAdvancedOpenCode ? iconChevronDown : iconChevronRight} size={14} />
        <span>Advanced server mode & authentication</span>
      </button>

      {#if showAdvancedOpenCode}
        <div class="flex flex-col gap-3 rounded-lg border border-border bg-muted/20 p-3.5">
          <!-- Server mode choice -->
          <div class="flex flex-col gap-1.5">
            <span class="text-ui-meta font-medium text-foreground">Server Mode</span>
            <div class="flex flex-wrap items-center gap-2">
              {#each ["managed", "attach"] as modeOption (modeOption)}
                {@const active = (ai.config.opencodeMode ?? "managed") === modeOption}
                <button
                  type="button"
                  class="rounded-lg border px-3 py-1.5 text-ui-meta transition-colors {active
                    ? 'border-primary bg-primary/10 font-medium text-primary-ink'
                    : 'border-border bg-background text-muted-foreground hover:bg-muted'}"
                  onclick={() => ai.setOpencodeMode(modeOption as OpencodeMode)}
                >
                  {modeOption === "managed"
                    ? "Managed (automatic spawn)"
                    : "Attach (custom server URL)"}
                </button>
              {/each}
            </div>
          </div>

          {#if (ai.config.opencodeMode ?? "managed") === "attach"}
            <!-- Attach Server URL -->
            <label class="flex flex-col gap-1">
              <span class="text-ui-meta font-medium text-foreground">Server URL</span>
              <Input
                class="rounded font-mono text-ui-meta"
                type="url"
                placeholder="http://127.0.0.1:4096"
                spellcheck="false"
                value={ai.config.opencodeUrl ?? ""}
                onchange={(e) => ai.setOpencodeUrl((e.currentTarget as HTMLInputElement).value)}
              />
              <span class="text-ui-micro text-muted-foreground">
                Leave blank to connect to the default address at 127.0.0.1:4096.
              </span>
            </label>
          {/if}

          <!-- Server Password (Keyring) -->
          <div class="flex flex-col gap-1.5">
            <span class="text-ui-meta font-medium text-foreground">Server Password (Optional)</span>
            <div class="flex gap-2">
              <Input
                class="rounded font-mono text-ui-meta"
                type="password"
                autocomplete="off"
                placeholder="stored in OS keyring (type to update)"
                bind:value={opencodePasswordInput}
                onkeydown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void saveOpencodePassword();
                  }
                }}
              />
              <Button
                variant="outline"
                type="button"
                disabled={savingOpencodePassword || !opencodePasswordInput.trim()}
                onclick={saveOpencodePassword}
              >
                {savingOpencodePassword ? "Saving…" : "Save password"}
              </Button>
            </div>
            <div class="flex items-center justify-between gap-2 pt-0.5">
              <span class="text-ui-micro text-muted-foreground">
                Write-only: stored in your OS keyring, never saved in plain text.
              </span>
              <Button
                variant="link"
                size="sm"
                class="h-auto p-0 text-ui-micro text-muted-foreground underline hover:text-destructive"
                type="button"
                onclick={removeOpencodePassword}
              >
                Remove stored password
              </Button>
            </div>
          </div>
        </div>
      {/if}
    </div>
  </section>

  <!-- =========================================================================
       4. OpenRouter Setup Card
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex items-center justify-between gap-3">
      <div class="flex flex-col gap-0.5">
        <h4 class="m-0 text-sm font-semibold text-foreground">OpenRouter Setup</h4>
        <span class="text-ui-meta text-muted-foreground">
          Direct API integration, secure keyring storage, and default model selection
        </span>
      </div>

      <Button
        variant="outline"
        size="sm"
        class="flex items-center gap-1.5 rounded-full text-ui-meta"
        type="button"
        disabled={testingOpenrouter}
        onclick={testOpenrouterConnection}
      >
        <span class={testingOpenrouter ? "animate-spin" : ""}>
          <Icon icon={iconRefresh} size={14} />
        </span>
        <span>{testingOpenrouter ? "Testing…" : "Test connection"}</span>
      </Button>
    </div>

    <!-- API Key Input -->
    <div class="flex flex-col gap-1.5 rounded-lg border border-border bg-background/50 p-3.5">
      <label for="openrouter-api-key-input" class="text-sm font-medium text-foreground">
        OpenRouter API Key
      </label>
      <div class="flex gap-2">
        <Input
          id="openrouter-api-key-input"
          class="rounded font-mono text-ui-meta"
          type="password"
          autocomplete="off"
          placeholder="stored in OS keyring (type to update)"
          bind:value={openrouterKeyInput}
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void saveOpenrouterKey();
            }
          }}
        />
        <Button
          variant="default"
          type="button"
          disabled={savingOpenrouterKey || !openrouterKeyInput.trim()}
          onclick={saveOpenrouterKey}
        >
          {savingOpenrouterKey ? "Saving…" : "Save key"}
        </Button>
      </div>

      <div class="flex flex-wrap items-center justify-between gap-2 pt-0.5">
        <span class="text-ui-micro text-muted-foreground">
          Write-only OS keyring integration. Values are never written to disk or localStorage.
        </span>
        <Button
          variant="link"
          size="sm"
          class="h-auto p-0 text-ui-micro text-muted-foreground underline hover:text-destructive"
          type="button"
          onclick={removeOpenrouterKey}
        >
          Remove stored API key
        </Button>
      </div>
    </div>

    <!-- OpenRouter Default Model Selection -->
    <div class="flex flex-col gap-1.5 rounded-lg border border-border bg-background/50 p-3.5">
      <div class="flex items-center justify-between gap-2">
        <label for="openrouter-model-input" class="text-sm font-medium text-foreground">
          Default Model
        </label>
        <Button
          variant="ghost"
          size="xs"
          class="h-auto p-1 text-ui-micro text-muted-foreground hover:text-foreground"
          type="button"
          disabled={openrouterModelsLoading}
          onclick={refreshOpenrouterModels}
        >
          <span class={openrouterModelsLoading ? "animate-spin" : ""}>
            <Icon icon={iconRefresh} size={12} />
          </span>
          <span class="ml-1">{openrouterModelsLoading ? "Loading…" : "Refresh models"}</span>
        </Button>
      </div>

      <Input
        id="openrouter-model-input"
        class="rounded font-mono text-ui-meta"
        type="text"
        list="ai-openrouter-models-list"
        placeholder="anthropic/claude-sonnet-4.5"
        spellcheck="false"
        value={ai.config.openrouterModel ?? ""}
        onchange={(e) => ai.setOpenrouterModel((e.currentTarget as HTMLInputElement).value || undefined)}
      />
      <datalist id="ai-openrouter-models-list">
        {#each openrouterModels as model (model.id)}
          <option value={model.id}>{model.label ?? model.id}</option>
        {/each}
      </datalist>

      {#if openrouterModelsError}
        <p class="m-0 text-ui-meta text-destructive" role="alert">
          Model list unavailable: {openrouterModelsError}
        </p>
      {:else}
        <span class="text-ui-micro text-muted-foreground">
          Type or select a model slug. Fallback default if left blank is anthropic/claude-sonnet-4.5.
        </span>
      {/if}
    </div>
  </section>

  <!-- =========================================================================
       5. Feature Capabilities & Agentic Budget
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex flex-col gap-0.5">
      <h4 class="m-0 text-sm font-semibold text-foreground">AI Capabilities & Modes</h4>
      <span class="text-ui-meta text-muted-foreground">
        Configure modes for agentic features, toggle individual tools, and set execution budgets.
      </span>
    </div>

    <!-- Agentic Eligible Features -->
    <div class="flex flex-col gap-2 pt-1">
      <span class="text-ui-meta font-semibold uppercase tracking-wider text-muted-foreground">
        Agentic Features
      </span>
      {#each AGENTIC_FEATURES as kind (kind)}
        {@const currentMode = ai.featureMode(kind)}
        <div class="flex flex-col gap-1.5 rounded-lg border border-border bg-background/50 p-3">
          <div class="flex items-center justify-between gap-3">
            <span class="text-sm font-medium text-foreground">{aiFeatureLabel(kind)}</span>
            <div
              class="flex flex-none items-center rounded-lg border border-border bg-muted/60 p-0.5"
              role="radiogroup"
              aria-label="{aiFeatureLabel(kind)} mode"
            >
              {#each [["auto", "Auto"], ["single-shot", "Single-shot"], ["off", "Off"]] as [modeVal, modeLabel] (modeVal)}
                <button
                  type="button"
                  role="radio"
                  aria-checked={currentMode === modeVal}
                  class="rounded-md px-2.5 py-1 text-ui-meta transition-colors {currentMode === modeVal
                    ? 'bg-background font-medium text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                  onclick={() => ai.setFeatureMode(kind, modeVal as AiFeatureMode)}
                >
                  {modeLabel}
                </button>
              {/each}
            </div>
          </div>
          <span class="text-ui-micro text-muted-foreground">
            {currentMode === "auto"
              ? "Agentic when a 2.x OpenCode server is connected; falls back to single-shot otherwise."
              : currentMode === "single-shot"
                ? "Always gathers context up front and sends one prompt — never executes repository tools."
                : "Completely turned off."}
          </span>
        </div>
      {/each}
    </div>

    <!-- Agentic Run Budget Box -->
    <div class="flex flex-col gap-2 rounded-lg border border-border bg-muted/20 p-3.5">
      <div class="flex items-center justify-between gap-2">
        <span class="text-sm font-medium text-foreground">Agentic Run Budget</span>
        <Button
          variant="link"
          size="sm"
          class="h-auto p-0 text-ui-micro text-muted-foreground underline hover:text-foreground"
          type="button"
          onclick={resetAgenticBudget}
        >
          Reset to defaults
        </Button>
      </div>
      <span class="text-ui-micro text-muted-foreground">
        Bounds ONE agentic run. Values below 1 or empty fields use the standard default (5 min / 40 tool calls).
      </span>

      <div class="flex flex-wrap items-center gap-4 pt-1">
        <label class="flex items-center gap-2">
          <span class="text-ui-meta text-muted-foreground">Wall clock:</span>
          <div class="flex items-center gap-1">
            <Input
              class="h-8 w-20 rounded text-center text-ui-meta"
              type="number"
              min="1"
              max={BUDGET_WALL_MAX_MINUTES}
              placeholder="5"
              bind:value={budgetWallMinutes}
              onchange={commitAgenticBudget}
            />
            <span class="text-ui-micro text-muted-foreground">min</span>
          </div>
        </label>

        <label class="flex items-center gap-2">
          <span class="text-ui-meta text-muted-foreground">Tool calls:</span>
          <Input
            class="h-8 w-20 rounded text-center text-ui-meta"
            type="number"
            min="1"
            placeholder="40"
            bind:value={budgetToolCalls}
            onchange={commitAgenticBudget}
          />
        </label>
      </div>
    </div>

    <!-- Standard AI Capabilities (Clean toggle list) -->
    <div class="flex flex-col gap-2 pt-2">
      <span class="text-ui-meta font-semibold uppercase tracking-wider text-muted-foreground">
        Standard Capabilities
      </span>
      <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
        {#each AI_FEATURES as def (def.kind)}
          {#if !AGENTIC_FEATURES.includes(def.kind)}
            {@const isEnabled = ai.featureMode(def.kind) !== "off"}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="flex cursor-pointer items-center justify-between gap-3 rounded-lg border border-border bg-background/50 px-3 py-2 transition-colors hover:border-ring/50 hover:bg-muted/30"
              onclick={() => ai.setFeatureMode(def.kind, isEnabled ? "off" : "auto")}
            >
              <div class="flex min-w-0 flex-col">
                <span class="truncate text-sm font-medium text-foreground">{def.title}</span>
                <span class="truncate text-ui-micro text-muted-foreground">{def.buttonLabel}</span>
              </div>
              <Switch
                class="flex-none"
                aria-label={def.title}
                checked={isEnabled}
                onclick={(e) => e.stopPropagation()}
                onCheckedChange={(checked) => ai.setFeatureMode(def.kind, checked ? "auto" : "off")}
              />
            </div>
          {/if}
        {/each}
      </div>
    </div>
  </section>

  <!-- =========================================================================
       6. Usage Budget Card
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex flex-col gap-0.5">
      <h4 class="m-0 text-sm font-semibold text-foreground">Monthly Usage Budget</h4>
      <span class="text-ui-meta text-muted-foreground">
        Soft budget tracking computed from this machine's visible run history. Runs are never blocked.
      </span>
    </div>

    <div class="flex flex-wrap items-end gap-3 pt-1">
      <label class="flex flex-col gap-1">
        <span class="text-ui-meta font-medium text-foreground">Monthly Token Budget (0 = off)</span>
        <Input
          class="h-9 w-44 rounded text-ui-meta"
          type="text"
          inputmode="numeric"
          placeholder="e.g. 2000000"
          spellcheck="false"
          bind:value={aiTokenBudget}
          onchange={commitUsageBudget}
        />
      </label>

      <label class="flex flex-col gap-1">
        <span class="text-ui-meta font-medium text-foreground">Reset Day (1–31)</span>
        <Input
          class="h-9 w-24 rounded text-center text-ui-meta"
          type="text"
          inputmode="numeric"
          spellcheck="false"
          bind:value={aiBudgetResetDay}
          onchange={commitUsageBudget}
        />
      </label>
    </div>

    <span class="text-ui-micro text-muted-foreground">
      When set, the status bar displays your percentage of monthly budget (warning at ≥ 100%),
      calculated from your most recent 200 runs.
    </span>
  </section>

  <!-- =========================================================================
       7. Custom Prompts Card
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex items-center justify-between gap-3">
      <button
        type="button"
        class="flex items-center gap-2 text-left focus-visible:outline-none"
        onclick={() => (showCustomPrompts = !showCustomPrompts)}
      >
        <Icon icon={showCustomPrompts ? iconChevronDown : iconChevronRight} size={15} />
        <div class="flex flex-col">
          <div class="flex items-center gap-2">
            <h4 class="m-0 text-sm font-semibold text-foreground">Custom Prompts</h4>
            <Badge variant="secondary" class="text-ui-micro font-normal">
              {customPrompts.entries.length}
            </Badge>
          </div>
          <span class="text-ui-meta text-muted-foreground">
            Reusable prompt templates accessible via palette commands
          </span>
        </div>
      </button>

      <Button
        variant="outline"
        size="sm"
        class="rounded-full text-ui-meta"
        type="button"
        onclick={() => customPrompts.add()}
      >
        Add prompt
      </Button>
    </div>

    {#if showCustomPrompts}
      <div class="flex flex-col gap-3 pt-1">
        <div class="flex flex-wrap items-center gap-1.5 text-ui-micro text-muted-foreground">
          <span>Available variables:</span>
          {#each CUSTOM_PROMPT_VARS as variable (variable)}
            <code class="rounded border border-border bg-muted/50 px-1.5 py-0.5 font-mono text-foreground">
              {`{{${variable}}}`}
            </code>
          {/each}
          <span>(diff, file stats, and recent commits).</span>
        </div>

        {#if customPrompts.entries.length === 0}
          <div class="rounded-lg border border-dashed border-border p-6 text-center text-ui-meta text-muted-foreground">
            No custom prompt templates yet. Click “Add prompt” above to create one.
          </div>
        {:else}
          <div class="flex flex-col gap-2.5">
            {#each customPrompts.entries as entry (entry.id)}
              <div class="flex flex-col gap-2 rounded-lg border border-border bg-background/50 p-3">
                <div class="flex items-center gap-2">
                  <Switch
                    checked={entry.enabled}
                    aria-label={`Enable ${entry.name || "prompt"}`}
                    onCheckedChange={(checked) => customPrompts.update(entry.id, { enabled: checked })}
                  />
                  <Input
                    class="h-8 flex-1 rounded text-sm font-medium"
                    type="text"
                    aria-label="Prompt name"
                    placeholder="Prompt title"
                    value={entry.name}
                    oninput={(e) =>
                      customPrompts.update(entry.id, { name: (e.currentTarget as HTMLInputElement).value })}
                  />
                  <Button
                    variant="ghost"
                    size="icon"
                    class="size-8 rounded-full text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                    type="button"
                    aria-label={`Delete ${entry.name || "prompt"}`}
                    onclick={() => customPrompts.remove(entry.id)}
                  >
                    <Icon icon={iconClose} size={14} />
                  </Button>
                </div>
                <Textarea
                  class="min-h-16 resize-y rounded font-mono text-ui-meta"
                  aria-label={`Template for ${entry.name || "prompt"}`}
                  spellcheck="false"
                  placeholder={"Enter prompt instructions with {{diff}} placeholders…"}
                  value={entry.template}
                  oninput={(e) =>
                    customPrompts.update(entry.id, { template: (e.currentTarget as HTMLTextAreaElement).value })}
                />
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </section>

  <!-- =========================================================================
       8. Saved Permissions Card
       ========================================================================= -->
  <section class="flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4 transition-colors">
    <div class="flex items-center justify-between gap-3">
      <div class="flex flex-col gap-0.5">
        <h4 class="m-0 text-sm font-semibold text-foreground">Saved Tool Permissions</h4>
        <span class="text-ui-meta text-muted-foreground">
          Review and revoke permissions previously granted with “Always in this repo”.
        </span>
      </div>

      {#if savedRulesCapable}
        <Button
          variant="outline"
          size="sm"
          class="flex items-center gap-1.5 rounded-full text-ui-meta"
          type="button"
          disabled={savedRulesLoading}
          onclick={refreshSavedRules}
        >
          <span class={savedRulesLoading ? "animate-spin" : ""}>
            <Icon icon={iconRefresh} size={13} />
          </span>
          <span>{savedRulesLoading ? "Refreshing…" : "Refresh rules"}</span>
        </Button>
      {/if}
    </div>

    {#if !savedRulesCapable}
      <p class="m-0 text-ui-meta text-muted-foreground">
        Saved permissions require an active 2.x OpenCode server. Connect to OpenCode to review rules.
      </p>
    {:else if savedRulesLoading && savedRules === null}
      <p class="m-0 flex items-center gap-2 text-ui-meta text-muted-foreground" role="status">
        <span class="size-1.5 animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
        Loading server permissions…
      </p>
    {:else if savedRuleError !== null}
      <p class="m-0 text-ui-meta text-destructive" role="alert">
        Failed to load saved rules: {savedRuleError}
      </p>
    {:else if savedRules !== null && savedRules.length === 0}
      <p class="m-0 text-ui-meta text-muted-foreground">
        No saved permissions — every tool execution requiring approval has prompted first.
      </p>
    {:else if savedRules !== null}
      <ul class="m-0 flex list-none flex-col gap-1.5 p-0" aria-label="Saved permissions">
        {#each savedRules as rule (rule.id)}
          <li
            class="flex items-center justify-between gap-3 rounded-lg border border-border bg-background/50 px-3 py-2 text-ui-meta {removingRule === rule.id
              ? 'opacity-50'
              : ''}"
          >
            <div class="flex min-w-0 items-center gap-2">
              <span class="font-medium text-foreground">{rule.action}</span>
              <span class="truncate font-mono text-muted-foreground" title={rule.resource}>
                {rule.resource}
              </span>
              <span class="font-mono text-ui-micro text-muted-foreground/70" title="Project ID">
                ({rule.projectID.slice(0, 8)})
              </span>
            </div>
            <Button
              variant="ghost"
              size="xs"
              class="h-auto px-2 py-1 text-ui-micro text-destructive hover:bg-destructive/10 hover:text-destructive"
              type="button"
              disabled={removingRule !== null}
              onclick={() => void removeSavedRule(rule.id)}
            >
              Revoke
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>
