<script lang="ts">
  /**
   * Clone-repository dialog (B4 lane).
   *
   * App.svelte owns the `open` flag (bindable) and mounts this component
   * for the whole session, so an in-flight clone keeps reporting even if
   * the dialog is closed — progress events are matched by URL and the
   * success path (toast + `openTab`) still runs.
   *
   * Backend contract: `cloneRepo(url, destination, depth?)` → Rust
   * `repo_clone`, which emits `clone-progress` events
   * `{url, received, total, objects}` (received = bytes; total/objects =
   * object counts — libgit2 has no total-bytes estimate) and resolves with
   * the cloned repository path.
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

  let { open = $bindable(false) }: { open?: boolean } = $props();

  let url = $state("");
  let destination = $state("");
  let shallow = $state(false);
  let blobless = $state(false);
  let cloning = $state(false);
  let error = $state("");
  let progress = $state<CloneProgressEvent | null>(null);
  /** URL of the clone currently in flight (progress events are matched on it). */
  let activeUrl: string | null = null;
  let urlInput = $state<HTMLInputElement | null>(null);
  /** Element focused before the dialog opened (focus returns there on close). */
  let restoreTo: HTMLElement | null = null;

  const canClone = $derived(url.trim() !== "" && destination.trim() !== "" && !cloning);
  const percent = $derived(
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.objects / progress.total) * 100))
      : null,
  );

  // Progress events live for the component's lifetime (not just while the
  // dialog is open) so background clones still complete visibly. Registered
  // through the shared event module; the in-flight URL filter stays here.
  onMount(() => {
    return onCloneProgress((event: CloneProgressEvent) => {
      if (activeUrl !== null && event.url === activeUrl) progress = event;
    });
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
    activeUrl = targetUrl;
    try {
      // Blobless clones route through the git CLI (libgit2 has no --filter).
      const path = blobless
        ? await cloneBlobless(targetUrl, destPath, shallow ? 1 : undefined)
        : await cloneRepo(targetUrl, destPath, shallow ? 1 : undefined);
      toast(`Cloned ${targetUrl} → ${path}`, { kind: "success" });
      open = false;
      try {
        await openTab(path);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        toast(`Failed to open ${path}: ${message}`, { kind: "error" });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      toast(`Clone failed: ${message}`, { kind: "error" });
    } finally {
      cloning = false;
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
  <Dialog.Root bind:open>
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
      class="dialog w-[min(32rem,calc(100vw-2rem))] max-w-none sm:max-w-none left-1/2 top-[12vh] -translate-x-1/2 translate-y-0"
      role="dialog"
      aria-modal="true"
      aria-labelledby="clone-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onOpenAutoFocus={(event) => event.preventDefault()}
      onCloseAutoFocus={(event) => event.preventDefault()}
      onkeydown={onDialogKeydown}
    >
      <Dialog.Header class="title-row flex-row items-center gap-2">
        <Dialog.Title id="clone-title" class="title flex-1">Clone repository</Dialog.Title>
        <Button
          class="close rounded-full text-lg leading-none text-muted-foreground hover:text-foreground"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Close clone dialog"
          title="Close"
          onclick={close}
        >
          ×
        </Button>
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
            class="input flex-1 min-w-0 h-auto py-2.5 px-3 font-mono text-sm md:text-sm"
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
              class="input flex-1 min-w-0 h-auto py-2.5 px-3 font-mono text-sm md:text-sm"
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

