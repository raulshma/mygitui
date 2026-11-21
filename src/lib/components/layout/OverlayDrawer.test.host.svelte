<!--
  Test-only host for OverlayDrawer: an invoking toggle + a two-button
  drawer body, with the bindable `open` wired to local state exactly like
  real callers do. Used exclusively by OverlayDrawer.svelte.test.ts.
-->
<script lang="ts">
  import OverlayDrawer from "./OverlayDrawer.svelte";

  let {
    open = false,
    onOpenChange = undefined,
    side = "right",
    label = "Panel",
  }: {
    open?: boolean;
    onOpenChange?: (next: boolean) => void;
    side?: "left" | "right";
    label?: string;
  } = $props();

  // svelte-ignore state_referenced_locally -- initial value only; the effect below syncs the prop
  let internal = $state(open);
  $effect(() => {
    internal = open;
  });

  export function drawerEl(): HTMLElement | null {
    return document.querySelector<HTMLElement>("[data-slot='overlay-drawer']");
  }
</script>

<button class="invoker" type="button" onclick={() => (internal = true)}>
  Open drawer
</button>

<OverlayDrawer bind:open={internal} {onOpenChange} {side} {label}>
  <button class="d-first" type="button">First</button>
  <button class="d-second" type="button">Second</button>
</OverlayDrawer>
