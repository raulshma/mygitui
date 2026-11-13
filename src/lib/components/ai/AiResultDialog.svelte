<!--
  AiResultDialog (M13) — the shared surface for AI results that have no
  single home panel: palette-initiated runs, release notes, commit
  explanations opened from context menus when the detail view is closed.
  Renders sanitized markdown via AiMarkdown; advisory by convention.
-->
<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { toast } from "$lib/toast";
  import AiMarkdown from "./AiMarkdown.svelte";

  let {
    open = $bindable(false),
    title = "AI result",
    text = "",
    meta = null,
  }: {
    open?: boolean;
    title?: string;
    text?: string;
    meta?: string | null;
  } = $props();

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      toast("Copied", { kind: "success" });
    } catch {
      toast("Copy failed — the clipboard is unavailable", { kind: "error" });
    }
  }
</script>

<Dialog.Root {open} onOpenChange={(next) => (open = next)}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
  <Dialog.Content
    class="dialog w-[min(38rem,calc(100vw-3rem))] max-w-none bg-card text-popover-foreground shadow-xl sm:max-w-none"
    role="dialog"
    aria-modal="true"
    aria-labelledby="ai-result-title"
    showCloseButton={false}
  >
    <Dialog.Title id="ai-result-title" class="m-0 mb-2 text-[1.125rem] font-medium text-foreground">{title}</Dialog.Title>
    <div class="mb-3 max-h-[55vh] overflow-y-auto rounded-xl border border-border bg-muted/50 px-3 py-[0.625rem] focus-visible:-outline-offset-2 focus-visible:outline-2 focus-visible:outline-primary">
      <AiMarkdown {text} />
    </div>
    {#if meta !== null && meta.length > 0}
      <p class="m-0 mb-3 text-ui-micro text-muted-foreground">{meta}</p>
    {/if}
    <div class="flex flex-wrap justify-end gap-2">
      <Button variant="outline" class="text-muted-foreground" type="button" onclick={() => void copy()}>
        Copy
      </Button>
      <Button type="button" onclick={() => (open = false)}>
        Close
      </Button>
    </div>
  </Dialog.Content>
</Dialog.Root>
