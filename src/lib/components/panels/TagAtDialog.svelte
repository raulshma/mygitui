<script lang="ts">
  /**
   * TagAtDialog (M12, Lane D) — small modal to create a tag at a specific
   * commit (HistoryView "Tag here…" context-menu action).
   *
   * Fields: tag name, "annotated" checkbox (enables the message input —
   * the backend makes a tag annotated by passing a message), message.
   * Enter submits, Escape cancels; the dialog never calls IPC itself —
   * the owner performs `tagCreate` in `onSubmit(name, annotated, message)`
   * and closes by clearing its state.
   */
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

  let {
    sha,
    onSubmit,
    onCancel = undefined,
  }: {
    /** Commit the tag points at (title context only). */
    sha: string;
    /** Submit with the trimmed name, annotated flag and message. */
    onSubmit: (name: string, annotated: boolean, message: string) => void;
    onCancel?: () => void;
  } = $props();

  let name = $state("");
  let annotated = $state(false);
  let message = $state("");

  let nameEl = $state<HTMLInputElement | null>(null);
  $effect(() => {
    nameEl?.focus();
  });

  function submit(): void {
    const trimmed = name.trim();
    if (!trimmed || (annotated && !message.trim())) return;
    onSubmit(trimmed, annotated, message.trim());
  }
</script>

<Dialog.Root
  open
  onOpenChange={(o) => {
    if (!o) onCancel?.();
  }}
>
  <Dialog.Content class="w-96 sm:max-w-none">
    <form
      class="grid gap-3"
      onsubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <Dialog.Title id="tag-at-title" class="tagat-title m-0 text-lg font-medium text-foreground">
        New tag at {sha.slice(0, 7)}
      </Dialog.Title>
      <Input
        class="rounded-lg"
        placeholder="Tag name"
        aria-label="Tag name"
        bind:value={name}
        bind:ref={nameEl}
        data-autofocus
        onfocus={(e) => e.currentTarget.select()}
      />
      <label
        class="tagat-toggle flex cursor-pointer items-center gap-1.5 text-[0.8125rem] text-muted-foreground"
        title="Annotated tags carry a message, tagger and date"
      >
        <input type="checkbox" class="m-0 accent-primary" bind:checked={annotated} />
        <span>annotated</span>
      </label>
      {#if annotated}
        <Input
          class="rounded-lg"
          placeholder="Message (makes the tag annotated)"
          aria-label="Tag message"
          bind:value={message}
        />
      {/if}
      <div class="mt-1 flex justify-end gap-2">
        <Button
          class="rounded-full"
          variant="outline"
          type="button"
          onclick={() => onCancel?.()}
        >
          Cancel
        </Button>
        <Button
          class="rounded-full px-5"
          type="submit"
          disabled={!name.trim() || (annotated && !message.trim())}
        >
          Create tag
        </Button>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>
