<script lang="ts">
  /**
   * UpdateChip — the app self-update affordance (store:
   * `$lib/stores/updater.svelte.ts`, backend: `src-tauri/src/updater.rs`).
   *
   * The chip IS the badge: hidden entirely while idle / up to date (no
   * visual noise), then
   *
   *   checking      subtle spinner
   *   available     download icon + pulsing dot (the header badge)
   *   downloading   download icon + live percentage
   *   ready         highlighted "Restart to update" chip
   *   error         muted warning dot, tooltip carries the message
   *
   * Mounted next to ThemeToggle/AiHealthChip in the tab-strip trailing area
   * AND in the home header, so the badge is visible with or without open
   * repositories. Clicking toggles a portaled popover (BranchSwitcher idiom:
   * `floating` action, outside-click + Escape to close) with the version
   * transition, release notes, download progress and the actions: Download
   * (when not auto-downloading), Restart to update, Skip this version, and
   * a manual "Check for updates" plus the autoCheck/autoDownload toggles.
   *
   * The "update available" toast's View action rides the typed UI bus
   * (`open-updates`) to open this popover from anywhere.
   */
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Progress } from "$lib/components/ui/progress/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { floating } from "$lib/components/overlay/floating";
  import { updater } from "$lib/stores/updater.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import iconDownload from "@ktibow/iconset-material-symbols/download";
  import iconRestart from "@ktibow/iconset-material-symbols/restart-alt";
  import iconError from "@ktibow/iconset-material-symbols/error";

  let { compact = false }: { compact?: boolean } = $props();

  let open = $state(false);
  let triggerEl = $state<HTMLElement | null>(null);
  let popoverEl = $state<HTMLElement | null>(null);

  // Store shortcuts (runes read through the singleton stay reactive in the
  // template).
  const status = $derived(updater.status);
  const info = $derived(updater.info);
  const percent = $derived(updater.percent);

  const chipVisible = $derived(updater.chipVisible);

  /** Tooltip text per state (the error state carries the message). */
  const chipLabel = $derived.by(() => {
    switch (status) {
      case "checking":
        return "Checking for updates…";
      case "available":
        return `Update to v${info?.version ?? "?"} available`;
      case "downloading": {
        const p = percent;
        return p === null
          ? "Downloading update…"
          : `Downloading update — ${p}%`;
      }
      case "ready":
        return `Update to v${info?.version ?? "?"} ready — restart to apply`;
      case "error":
        return `Update failed: ${updater.error ?? "unknown error"}`;
      default:
        return "Updates";
    }
  });

  function toggle(): void {
    if (open) close();
    else open = true;
  }

  function close(): void {
    open = false;
  }

  // The panel is portaled to <body>, so outside-click detection lives on
  // document; presses on the trigger or inside the panel are ignored
  // (BranchSwitcher idiom).
  $effect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (popoverEl?.contains(target) || triggerEl?.contains(target)) return;
      close();
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      close();
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  });

  // Closing the chip (up-to-date again / version skipped) must not leave a
  // dangling popover.
  $effect(() => {
    if (!chipVisible) close();
  });

  // "View" action on the update-available toast (and palette routes) opens
  // the popover even when the click happened far from the header.
  $effect(() => {
    return onUiEvent("open-updates", () => {
      if (chipVisible) open = true;
    });
  });
</script>

{#if chipVisible}
  <Hint label={chipLabel} side="bottom">
    {#snippet child({ props })}
      {#if status === "ready"}
        <Button
          bind:ref={triggerEl}
          class="update-chip update-ready relative gap-1.5 rounded-full px-2.5 font-medium"
          variant="default"
          size="sm"
          type="button"
          {...props}
          aria-label={chipLabel}
          aria-haspopup="dialog"
          aria-expanded={open}
          onclick={toggle}
        >
          <Icon icon={iconRestart} size={14} />
          {#if !compact}<span class="whitespace-nowrap">Restart to update</span>{/if}
        </Button>
      {:else}
        <Button
          bind:ref={triggerEl}
          class="update-chip relative gap-1 rounded-full {status === 'error'
            ? 'text-muted-foreground'
            : ''}"
          variant="outline"
          size="icon-sm"
          type="button"
          {...props}
          aria-label={chipLabel}
          aria-haspopup="dialog"
          aria-expanded={open}
          onclick={toggle}
        >
          {#if status === "checking"}
            <Spinner class="size-3.5" aria-label="Checking for updates" />
          {:else if status === "error"}
            <span
              class="dot size-2 rounded-full bg-muted-foreground/70"
              aria-hidden="true"
            ></span>
          {:else}
            <Icon icon={iconDownload} size={14} />
          {/if}
          {#if status === "available"}
            <!-- The badge: pulsing dot on the header icon. -->
            <span
              class="badge absolute -right-0.5 -top-0.5 size-1.5 animate-pulse rounded-full bg-primary"
              aria-hidden="true"
            ></span>
          {:else if status === "downloading"}
            <span
              class="pct absolute -right-1 -top-1.5 rounded-full bg-primary px-1 text-[0.55rem] font-semibold leading-[1.1] text-primary-foreground tabular-nums"
              aria-hidden="true"
            >
              {percent === null ? "…" : `${percent}%`}
            </span>
          {/if}
        </Button>
      {/if}
    {/snippet}
  </Hint>

  {#if open}
    <div
      bind:this={popoverEl}
      class="popover fixed z-60 flex max-h-[min(28rem,calc(100vh-1.5rem))] w-[21rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-lg border border-border bg-popover text-ui-base text-popover-foreground shadow-md"
      role="dialog"
      aria-label="Software update"
      use:floating={{ anchor: triggerEl ?? undefined, gap: 6 }}
    >
      <div
        class="head flex flex-none items-center justify-between gap-2 border-b border-border px-3 py-2.5"
      >
        <span class="title text-sm font-medium">Software update</span>
        {#if info}
          <span
            class="versions font-mono text-ui-micro text-muted-foreground tabular-nums"
          >
            v{info.current_version} → v{info.version}
          </span>
        {:else}
          <span class="versions font-mono text-ui-micro text-muted-foreground">v—</span>
        {/if}
      </div>

      <div class="body flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-3 py-2.5">
        {#if status === "error"}
          <p class="error m-0 text-xs text-destructive" role="alert">
            {updater.error ?? "Something went wrong."}
          </p>
        {/if}

        {#if status === "downloading"}
          <div class="dl flex flex-col gap-1.5">
            <div class="flex items-center justify-between text-xs text-muted-foreground">
              <span>Downloading…</span>
              <span class="tabular-nums">{percent === null ? "" : `${percent}%`}</span>
            </div>
            <!-- Unknown total (no Content-Length): the bar stays at 0 and
                 the % label stays empty — the spinner chip communicates
                 motion; known totals are exact. -->
            <Progress value={percent ?? 0} class="h-1.5" />
          </div>
        {/if}

        {#if info?.notes}
          <!-- Release notes may be markdown upstream; plain pre-wrap keeps
               this popover dependency-light (the file-preview markdown
               chunk stays out of the header path). -->
          <div
            class="notes whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground"
          >
            {info.notes}
          </div>
        {:else if info && status !== "downloading"}
          <p class="m-0 text-xs text-muted-foreground">No release notes.</p>
        {/if}

        {#if status === "uptodate"}
          <p class="m-0 text-xs text-muted-foreground">
            You're on the latest version of mygitui.
          </p>
        {/if}
      </div>

      <div
        class="actions flex flex-none flex-wrap items-center gap-1.5 border-t border-border px-3 py-2.5"
      >
        {#if status === "available"}
          <Button
            class="dl-btn h-7 px-2.5 text-xs"
            size="sm"
            type="button"
            onclick={() => void updater.download({ manual: true })}
          >
            <Icon icon={iconDownload} size={13} />
            Download update
          </Button>
          <Button
            class="skip-btn h-7 px-2.5 text-xs"
            variant="ghost"
            size="sm"
            type="button"
            onclick={() => {
              updater.skipVersion();
              close();
            }}
          >
            Skip this version
          </Button>
        {:else if status === "ready"}
          <Button
            class="restart-btn h-7 px-2.5 text-xs"
            size="sm"
            type="button"
            onclick={() => updater.relaunch()}
          >
            <Icon icon={iconRestart} size={13} />
            Restart to update
          </Button>
        {/if}
        {#if status !== "checking" && status !== "downloading"}
          <Button
            class="check-btn h-7 px-2.5 text-xs"
            variant="outline"
            size="sm"
            type="button"
            onclick={() => void updater.checkNow({ manual: true })}
          >
            Check for updates
          </Button>
        {/if}
      </div>

      <div
        class="prefs flex flex-none items-center justify-between gap-3 border-t border-border px-3 py-2 text-xs text-muted-foreground"
      >
        <label class="pref flex cursor-pointer items-center gap-1.5">
          <Checkbox
            checked={updater.autoCheck}
            onCheckedChange={(checked) => updater.setAutoCheck(checked === true)}
          />
          Check automatically
        </label>
        <label class="pref flex cursor-pointer items-center gap-1.5">
          <Checkbox
            checked={updater.autoDownload}
            onCheckedChange={(checked) => updater.setAutoDownload(checked === true)}
          />
          Download automatically
        </label>
      </div>
    </div>
  {/if}
{/if}
