<script lang="ts">
  /**
   * Clone-repository dialog (B4 lane).
   *
   * App.svelte owns the `open` flag (bindable) and mounts this component
   * for the whole session, so an in-flight clone keeps reporting even if
   * the dialog is closed — progress events are matched by the run id this
   * dialog mints (URLs repeat across clones; run ids do not, which also
   * keeps two windows' same-URL clones apart) and the success path (toast
   * + `openTab`) still runs.
   *
   * Backend contract: `cloneRepo(url, destination, depth?, runId?)` → Rust
   * `repo_clone`, which emits `clone-progress` events
   * `{url, received, total, objects, run_id?}` (received = bytes;
   * total/objects = object counts — libgit2 has no total-bytes estimate)
   * and resolves with the cloned repository path. The blobless path (git
   * CLI) emits no progress events; the bar stays indeterminate there.
   */
  import { onMount } from "svelte";
  import {
    cloneRepo,
    cloneBlobless,
    pickFolder,
  } from "$lib/ipc/client";
  import type { CloneProgressEvent } from "$lib/ipc/types";
  import { onCloneProgress } from "$lib/events/repoEvents";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { describeError } from "$lib/errors";

  let { open = $bindable(false) }: { open?: boolean } = $props();

  let url = $state("");
  let destination = $state("");
  let shallow = $state(false);
  let blobless = $state(false);
  let cloning = $state(false);
  let error = $state("");
  let progress = $state<CloneProgressEvent | null>(null);
  /** Run id of the clone currently in flight (progress events match it). */
  let activeRunId: string | null = null;
  /** URL of the clone currently in flight (fallback discriminator). */
  let activeUrl: string | null = null;
  /** Wall clock of the last progress event (drives the stall hint). */
  let lastProgressAt = 0;
  let urlInput = $state<HTMLInputElement | null>(null);
  /** Ticks once per interval while cloning so the stall hint can fire. */
  let now = $state(Date.now());

  /** No progress event for this long → hint that the transfer may be stuck. */
  const STALL_AFTER_MS = 45_000;

  let runCounter = 0;
  /** Mints a fresh correlation id per clone (unique across windows). */
  function mintRunId(): string {
    runCounter += 1;
    return typeof crypto !== "undefined" && "randomUUID" in crypto
      ? `clone-${crypto.randomUUID()}`
      : `clone-${Date.now()}-${runCounter}`;
  }
  /** Element focused before the dialog opened (focus returns there on close). */
  let restoreTo: HTMLElement | null = null;

  const canClone = $derived(url.trim() !== "" && destination.trim() !== "" && !cloning);
  const percent = $derived(
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.objects / progress.total) * 100))
      : null,
  );
  /** True when a clone with a live progress stream has gone quiet. */
  const stalled = $derived(
    cloning && progress !== null && now - lastProgressAt > STALL_AFTER_MS,
  );

  // Progress events live for the component's lifetime (not just while the
  // dialog is open) so background clones still complete visibly. Registered
  // through the shared event module; the in-flight URL filter stays here.
  onMount(() => {
    return onCloneProgress((event: CloneProgressEvent) => {
      if (activeRunId === null) return;
      // Strict match on the echoed run id; URL matching only covers
      // events predating the field.
      const mine =
        event.run_id !== null && event.run_id !== undefined
          ? event.run_id === activeRunId
          : event.url === activeUrl;
      if (mine) {
        progress = event;
        lastProgressAt = Date.now();
      }
    });
  });

  // Tick the clock while cloning so the stall hint can fire (the interval
  // only exists for a run with a live progress stream — the blobless path
  // never emits progress and stays "Contacting remote…").
  $effect(() => {
    if (!cloning || progress === null) return;
    const timer = setInterval(() => (now = Date.now()), 5_000);
    return () => clearInterval(timer);
  });

  $effect(() => {
    if (open) {
      restoreTo =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      error = "";
      requestAnimationFrame(() => urlInput?.focus());
    } else if (restoreTo !== null) {
      restoreTo.focus();
      restoreTo = null;
    }
  });

  /**
   * Escape closes through close() so the in-flight-clone guard still runs
   * (bits-ui's own dismissal is disabled below). Tab is looped inside the
   * dialog by bits-ui's focus scope, replacing the shared trapTab helper.
   */
  function onDialogKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }

  function close(): void {
    if (cloning) {
      // Keep the dialog up while a clone runs (no cancel in M1).
      toast("Clone is still running — it continues in the background.", {
        kind: "info",
      });
      open = false;
      return;
    }
    open = false;
  }

  async function browse(): Promise<void> {
    const picked = await pickFolder();
    if (picked) destination = picked;
  }

  async function startClone(): Promise<void> {
    if (!canClone) return;
    const targetUrl = url.trim();
    const destPath = destination.trim();
    cloning = true;
    error = "";
    progress = null;
    const runId = mintRunId();
    activeRunId = runId;
    activeUrl = targetUrl;
    lastProgressAt = Date.now();
    try {
      // Blobless clones route through the git CLI (libgit2 has no --filter).
      const path = blobless
        ? await cloneBlobless(targetUrl, destPath, shallow ? 1 : undefined)
        : await cloneRepo(targetUrl, destPath, shallow ? 1 : undefined, runId);
      toast(`Cloned ${targetUrl} → ${path}`, { kind: "success" });
      open = false;
      try {
        await openTab(path);
      } catch (err) {
        const message = describeError(err);
        toast(`Failed to open ${path}: ${message}`, { kind: "error" });
      }
    } catch (err) {
      const message = describeError(err);
      error = message;
      toast(`Clone failed: ${message}`, { kind: "error" });
    } finally {
      cloning = false;
      activeRunId = null;
      activeUrl = null;
    }
  }

  function fmtBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    const units = ["KB", "MB", "GB", "TB"];
    let value = n;
    let unit = -1;
    do {
      value /= 1024;
      unit += 1;
    } while (value >= 1024 && unit < units.length - 1);
    return `${value.toFixed(1)} ${units[unit]}`;
  }
</script>

{#if open}
  <Dialog.Root
    bind:open
    onOpenChange={(next) => {
      if (!next) close();
    }}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay
      class="scrim bg-black/50"
      role="presentation"
      onclick={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    />
    <Dialog.Content
      class="dialog w-[min(32rem,calc(100vw-2rem))] max-w-none sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="clone-title"
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onOpenAutoFocus={(event) => event.preventDefault()}
      onCloseAutoFocus={(event) => event.preventDefault()}
      onkeydown={onDialogKeydown}
    >
      <Dialog.Header class="title-row">
        <Dialog.Title id="clone-title" class="title flex-1">Clone repository</Dialog.Title>
      </Dialog.Header>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void startClone();
        }}
      >
        <Label class="field flex-col items-stretch gap-1">
          <span class="field-label text-xs font-medium text-muted-foreground">Repository URL</span>
          <Input
            class="input flex-1 min-w-0 font-mono"
            type="text"
            placeholder="https://github.com/user/repo.git"
            autocomplete="off"
            spellcheck="false"
            bind:value={url}
            bind:ref={urlInput}
            disabled={cloning}
          />
        </Label>

        <Label class="field flex-col items-stretch gap-1">
          <span class="field-label text-xs font-medium text-muted-foreground">Destination folder</span>
          <span class="dest-row flex gap-2">
            <Input
              class="input flex-1 min-w-0 font-mono"
              type="text"
              placeholder="e.g. C:\Users\you\Projects\repo"
              autocomplete="off"
              spellcheck="false"
              bind:value={destination}
              disabled={cloning}
            />
            <Button
              class="secondary shrink-0"
              variant="outline"
              type="button"
              disabled={cloning}
              onclick={() => void browse()}
            >
              Browse…
            </Button>
          </span>
        </Label>

        <Label class="check flex cursor-pointer items-center gap-2 text-sm font-normal">
          <Checkbox bind:checked={shallow} disabled={cloning} />
          <span>Shallow clone (<code class="font-mono text-muted-foreground">--depth 1</code>)</span>
        </Label>

        <Label class="check flex cursor-pointer items-center gap-2 text-sm font-normal">
          <Checkbox bind:checked={blobless} disabled={cloning} />
          <span>Blobless clone (<code class="font-mono text-muted-foreground">--filter=blob:none</code>, fetches file contents on demand)</span>
        </Label>

        {#if error}
          <p class="error text-destructive text-sm" role="alert">{error}</p>
        {/if}

        {#if cloning && progress}
          <div
            class="progress h-2 rounded-full bg-muted overflow-hidden"
            role="progressbar"
            aria-label="Clone progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent ?? undefined}
          >
            <div
              class="bar h-full rounded-full bg-primary transition-[width] duration-150 {percent === null ? 'animate-[slide_1.1s_ease-in-out_infinite]' : ''}"
              style:width={percent !== null ? `${percent}%` : "100%"}
              class:indeterminate={percent === null}
            ></div>
          </div>
          <p class="stats text-xs text-muted-foreground tabular-nums">
            {percent !== null ? `${percent}% · ` : ""}{progress.objects}/{progress.total} objects · {fmtBytes(progress.received)}
          </p>
          {#if stalled}
            <p class="stats text-xs text-muted-foreground" role="status">
              No progress update for {Math.round(STALL_AFTER_MS / 1000)} s — the remote may be slow or the transfer stalled.
            </p>
          {/if}
        {:else if cloning}
          <p class="stats text-xs text-muted-foreground tabular-nums">Contacting remote…</p>
        {/if}

        <div class="actions flex justify-end gap-2">
          <Button class="secondary" variant="outline" type="button" onclick={close}>
            {cloning ? "Hide" : "Cancel"}
          </Button>
          <Button class="primary" type="submit" disabled={!canClone}>
            {cloning ? "Cloning…" : "Clone"}
          </Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}

