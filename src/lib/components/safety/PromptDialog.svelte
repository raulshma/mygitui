<!--
  PromptDialog (M9 F9) — accessible replacement for `window.prompt`.

  Props: `open` (bindable), `title`, `message`, `placeholder`, `initial`,
  `confirmLabel`, `cancelLabel`, `onSubmit(value)` (trimmed value;
  cancel = no call), `onCancel`. Input autofocuses with the initial value
  selected; Enter submits, Escape cancels. Chrome comes from the shadcn
  ui/dialog primitives.
-->
<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

  let {
    open = $bindable(false),
    title,
    message = "",
    placeholder = "",
    initial = "",
    confirmLabel = "OK",
    cancelLabel = "Cancel",
    onSubmit,
    onCancel = undefined,
  }: {
    open?: boolean;
    title: string;
    message?: string;
    placeholder?: string;
    initial?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    onSubmit: (value: string) => void;
    onCancel?: () => void;
  } = $props();

  let value = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);

  $effect(() => {
    if (open) {
      value = initial;
      // Autofocus with the initial value selected (the old data-autofocus
      // contract; select() also fires from the focus handler below).
      requestAnimationFrame(() => {
        inputEl?.focus();
        inputEl?.select();
      });
    }
  });

  function close(cancelOnly = false): void {
    open = false;
    if (cancelOnly) onCancel?.();
  }

  function submit(): void {
    const trimmed = value.trim();
    if (!trimmed) return;
    close();
    onSubmit(trimmed);
  }
</script>

<Dialog.Root bind:open>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
  <Dialog.Content
    class="dialog w-[24rem] max-w-none gap-0 rounded-lg bg-card px-6 py-5 text-sm text-popover-foreground shadow-xl sm:max-w-none"
    role="dialog"
    aria-modal="true"
    aria-labelledby="prompt-title"
    showCloseButton={false}
    escapeKeydownBehavior="ignore"
    interactOutsideBehavior="ignore"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close(true);
      }
    }}
  >
    <form
      onsubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <Dialog.Title id="prompt-title" class="m-0 mb-2 text-lg font-medium">{title}</Dialog.Title>
      {#if message}<p class="m-0 mb-4 text-muted-foreground [overflow-wrap:anywhere]">{message}</p>{/if}
      <Input
        bind:ref={inputEl}
        bind:value
        class="h-auto rounded-sm px-[0.6rem] py-[0.45rem] text-sm focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-ring"
        type="text"
        {placeholder}
        aria-label={title}
        onfocus={(e) => e.currentTarget.select()}
      />
      <div class="mt-4 flex flex-nowrap justify-end gap-2">
        <Button
          variant="outline"
          class="text-muted-foreground"
          type="button"
          onclick={() => close(true)}
        >
          {cancelLabel}
        </Button>
        <Button type="submit" disabled={!value.trim()}>
          {confirmLabel}
        </Button>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>
