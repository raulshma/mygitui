<script lang="ts">
  /**
   * Preview dialog (M3 safety) — generic "what will this do?" modal.
   *
   * Renders an `ops_preview` result (summary + scrollable file list) and
   * drives the confirm flow the caller performs:
   *   - `danger` rows add an error-colored confirm button and a
   *     "Create undo checkpoint" checkbox (default ON; when left checked the
   *     caller runs `guard_checkpoint` before the op, unchecked = skip);
   *   - `confirmPhrase` (type-the-name) gates the confirm button;
   *   - Escape and Cancel invoke `onCancel`.
   *
   * The dialog never performs the operation itself: `onConfirm` receives
   * whether a checkpoint was requested and the caller executes guard + op.
   * Render it inside `{#if open}` — mounting IS opening (same contract as
   * AuthDialog's conditional render).
   */
  import type { PreviewInfo } from "$lib/ipc/types";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import {
    classifyDanger,
    previewSummaryLines,
  } from "./safetyModel";

  let {
    info,
    danger,
    confirmPhrase = undefined,
    title = undefined,
    onConfirm,
    onCancel,
  }: {
    info: PreviewInfo;
    danger: boolean;
    /** When set, the confirm button stays disabled until the user types this. */
    confirmPhrase?: string;
    /** Dialog heading; derived from the danger classification when omitted. */
    title?: string;
    onConfirm: (createCheckpoint: boolean) => void;
    onCancel: () => void;
  } = $props();

  const dangerInfo = $derived(
    danger
      ? classifyDanger(info.kind)
      : { level: "warn" as const, verb: "Apply", noun: info.kind },
  );

  const lines = $derived(previewSummaryLines(info));
  let phrase = $state("");
  let checkpoint = $state(true);
  let phraseInput = $state<HTMLInputElement | null>(null);
  let confirmButton = $state<HTMLElement | null>(null);

  const phraseOk = $derived(
    confirmPhrase === undefined || phrase === confirmPhrase,
  );

  // Fresh dialog state on every mount, focus the first control.
  $effect(() => {
    phrase = "";
    checkpoint = true;
    requestAnimationFrame(() => {
      if (confirmPhrase !== undefined) phraseInput?.focus();
      else confirmButton?.focus();
    });
  });

  function confirm(): void {
    if (!phraseOk) return;
    onConfirm(danger && checkpoint);
  }
</script>

<Dialog.Root open={true}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
  <Dialog.Content
    class="dialog flex max-h-[calc(100vh-4rem)] w-[min(30rem,calc(100vw-2rem))] max-w-none flex-col gap-0 rounded-lg bg-card px-6 py-5 text-sm text-popover-foreground shadow-xl sm:max-w-none"
    role="dialog"
    aria-modal="true"
    aria-labelledby="preview-title"
    aria-describedby="preview-summary"
    showCloseButton={false}
    escapeKeydownBehavior="ignore"
    interactOutsideBehavior="ignore"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
      }
    }}
  >
    <Dialog.Title
      id="preview-title"
      class={"mb-2 mt-0 text-lg font-medium " +
        (dangerInfo.level === "danger" ? "danger text-destructive" : "")}
    >
      {title ?? `${dangerInfo.verb} — preview`}
    </Dialog.Title>

    <p id="preview-summary" class="mb-2 mt-0 whitespace-pre-line text-foreground [overflow-wrap:anywhere]">{info.summary}</p>

    {#if lines.length > 1}
      <ul class="mb-3 mt-0 max-h-48 list-none overflow-y-auto rounded-sm border border-border bg-background px-[0.6rem] py-[0.35rem] font-mono text-[0.72rem] text-muted-foreground" role="list" aria-label="Affected files">
        {#each lines.slice(1) as line, i (i)}
          <li class="py-[0.1rem] [overflow-wrap:anywhere]">{line}</li>
        {/each}
      </ul>
    {/if}

    {#if danger}
      <label class="check mb-3 flex cursor-pointer items-center gap-2 text-muted-foreground">
        <Checkbox bind:checked={checkpoint} />
        <span>Create undo checkpoint (snapshot first)</span>
      </label>
    {/if}

    {#if confirmPhrase !== undefined}
      <label class="phrase mb-3 flex flex-col gap-1">
        <span class="phrase-label text-xs text-muted-foreground">
          Type <code class="font-mono font-semibold text-destructive">{confirmPhrase}</code> to confirm
        </span>
        <Input
          type="text"
          spellcheck="false"
          autocomplete="off"
          aria-label={`Type ${confirmPhrase} to confirm`}
          placeholder={confirmPhrase}
          class="h-auto rounded-sm px-[0.6rem] py-[0.45rem] font-mono text-sm focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-ring"
          bind:value={phrase}
          bind:ref={phraseInput}
        />
      </label>
    {/if}

    <div class="actions mt-1 flex justify-end gap-2">
      <Button
        variant={dangerInfo.level === "danger" ? "destructive" : "default"}
        type="button"
        disabled={!phraseOk}
        bind:ref={confirmButton}
        onclick={confirm}
      >
        {dangerInfo.verb}
      </Button>
      <Button
        variant="outline"
        class="text-muted-foreground"
        type="button"
        onclick={onCancel}
      >Cancel</Button>
    </div>
  </Dialog.Content>
</Dialog.Root>
