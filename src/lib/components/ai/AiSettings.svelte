<script lang="ts">
  /**
   * Backwards-compatible standalone AI settings dialog.
   *
   * Renders `AiSettingsSection` inside a Dialog modal when invoked directly.
   * The primary home for AI settings is now the AI section in SettingsDialog.
   */
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import AiSettingsSection from "./AiSettingsSection.svelte";

  const iconClose = $derived(appIcon("close")!);

  let {
    open = $bindable(false),
    onclose = undefined,
  }: {
    open: boolean;
    /** Called after the dialog is closed. */
    onclose?: () => void;
  } = $props();

  function close(): void {
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
      class="dialog flex w-[44rem] max-w-none flex-col overflow-y-auto text-popover-foreground max-h-[calc(100dvh_-_2rem)] wm-stacked:max-h-[calc(100dvh_-_1rem)] sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="ai-settings-title"
      aria-describedby="ai-settings-description"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <header class="dialog-header mb-4 flex flex-none items-start justify-between gap-4">
        <div>
          <Dialog.Title id="ai-settings-title" class="title m-0 text-base font-semibold text-foreground">
            AI settings
          </Dialog.Title>
          <p id="ai-settings-description" class="subtitle mb-0 mt-1 text-ui-base text-muted-foreground">
            Configure providers, capabilities, models, and budgets.
          </p>
        </div>
        <Button
          variant="ghost"
          size="icon"
          class="icon-button flex-none rounded-full text-muted-foreground [&_svg:not([class*='size-'])]:size-4"
          type="button"
          aria-label="Close AI settings"
          onclick={close}
        >
          <Icon icon={iconClose} size={16} />
        </Button>
      </header>

      <div class="dialog-body flex min-h-0 flex-1 flex-col overflow-y-auto pr-1">
        <AiSettingsSection />
      </div>

      <footer class="mt-4 flex flex-none justify-end gap-2 border-t border-border pt-3">
        <Button variant="outline" class="rounded-full text-primary-ink" type="button" onclick={close}>
          Done
        </Button>
      </footer>
    </Dialog.Content>
  </Dialog.Root>
{/if}
