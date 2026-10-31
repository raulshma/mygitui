<!--
  DialogShell — shared chrome for the safety dialogs: scrim, panel box, and
  the action row. Body and actions are snippets; the shell marks the element
  carrying `data-autofocus` for initial focus. Classes are prefixed
  `sdlg-` because they style snippet content rendered in the caller.

  P1 migration: now a thin wrapper over the shadcn ui/dialog primitives —
  the scrim, focus trap, focus restore, and Escape dismissal come from
  bits-ui. The legacy `sdlg-*` snippet styling hooks live on as Tailwind
  arbitrary-variant utilities on the content box until every caller
  (FileWorkspace, ForgePanel, panels/BranchPanel) renders ui/dialog itself.
-->
<script lang="ts">
  import type { Snippet } from "svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";

  let {
    open = $bindable(false),
    labelledBy,
    describedBy = undefined,
    role = "dialog",
    width = "26rem",
    onCancel = undefined,
    onsubmit = undefined,
    children,
    actions,
  }: {
    open?: boolean;
    /** id of the element labelling the dialog (aria-labelledby). */
    labelledBy: string;
    /** id of the element describing the dialog (aria-describedby). */
    describedBy?: string | undefined;
    role?: "dialog" | "alertdialog";
    width?: string;
    onCancel?: () => void;
    /** When set, the panel is a `<form>`; Enter submits (preventDefault'd). */
    onsubmit?: (() => void) | undefined;
    children: Snippet;
    actions: Snippet;
  } = $props();

  let contentEl = $state<HTMLElement | null>(null);

  // bits-ui focuses the first focusable element on open; callers' snippets
  // mark the intended target with `data-autofocus` — re-apply after that.
  $effect(() => {
    if (open) {
      requestAnimationFrame(() => {
        contentEl?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
      });
    }
  });

  function cancel(): void {
    open = false;
    onCancel?.();
  }
</script>

<Dialog.Root bind:open>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
  <Dialog.Content
    bind:ref={contentEl}
    class="sdlg-dialog max-w-none gap-0 rounded-lg bg-card px-6 py-5 text-sm text-card-foreground shadow-xl sm:max-w-none [&_.sdlg-danger]:cursor-pointer [&_.sdlg-danger]:rounded-full [&_.sdlg-danger]:border-0 [&_.sdlg-danger]:bg-destructive [&_.sdlg-danger]:px-4 [&_.sdlg-danger]:py-[0.45rem] [&_.sdlg-danger]:text-white [&_.sdlg-message]:m-0 [&_.sdlg-message]:mb-4 [&_.sdlg-message]:text-muted-foreground [&_.sdlg-message]:[overflow-wrap:anywhere] [&_.sdlg-primary]:cursor-pointer [&_.sdlg-primary]:rounded-full [&_.sdlg-primary]:border-0 [&_.sdlg-primary]:bg-primary [&_.sdlg-primary]:px-5 [&_.sdlg-primary]:py-[0.45rem] [&_.sdlg-primary]:font-medium [&_.sdlg-primary]:text-primary-foreground [&_.sdlg-secondary]:cursor-pointer [&_.sdlg-secondary]:rounded-full [&_.sdlg-secondary]:border [&_.sdlg-secondary]:border-border [&_.sdlg-secondary]:bg-transparent [&_.sdlg-secondary]:px-4 [&_.sdlg-secondary]:py-[0.45rem] [&_.sdlg-secondary]:text-muted-foreground [&_.sdlg-title]:m-0 [&_.sdlg-title]:mb-2 [&_.sdlg-title]:text-lg [&_.sdlg-title]:font-medium [&_button:disabled]:cursor-not-allowed [&_button:disabled]:bg-[var(--app-disabled-bg)] [&_button:disabled]:text-[var(--app-disabled-fg)] [&_button:focus-visible]:outline-2 [&_button:focus-visible]:-outline-offset-1 [&_button:focus-visible]:outline-ring"
    style="width:{width}"
    role={role}
    aria-modal="true"
    aria-labelledby={labelledBy}
    aria-describedby={describedBy}
    showCloseButton={false}
    escapeKeydownBehavior="ignore"
    interactOutsideBehavior="ignore"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      }
    }}
  >
    {#if onsubmit}
      <form
        onsubmit={(e) => {
          e.preventDefault();
          onsubmit();
        }}
      >
        {@render children()}
        <div class="sdlg-actions mt-4 flex flex-nowrap justify-end gap-2">{@render actions()}</div>
      </form>
    {:else}
      {@render children()}
      <div class="sdlg-actions mt-4 flex flex-wrap justify-end gap-2">{@render actions()}</div>
    {/if}
  </Dialog.Content>
</Dialog.Root>
