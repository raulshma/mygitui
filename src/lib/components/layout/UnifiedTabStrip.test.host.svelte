<!--
  Test-only host for UnifiedTabStrip: the `tab` snippet (spreading
  triggerProps onto a real button) cannot be built from plain JS tests.
  Used exclusively by UnifiedTabStrip.svelte.test.ts.
-->
<script lang="ts">
  import UnifiedTabStrip from "./UnifiedTabStrip.svelte";

  interface TestTab {
    id: string;
    label: string;
  }

  let {
    tabs,
    activeId = null,
    label = "Test tabs",
    onActivate = undefined,
    activateOnArrow = false,
    overflowLabel = "More tabs",
    edgeFades = true,
  }: {
    tabs: TestTab[];
    activeId?: string | null;
    label?: string;
    onActivate?: (item: TestTab) => void;
    activateOnArrow?: boolean;
    overflowLabel?: string;
    edgeFades?: boolean;
  } = $props();

  // The strip's keyboard/overflow activation flows through onActivate; real
  // consumers update their active state from it — mirror that here so the
  // roving tabindex + arrow-activation walk the way they do in the app.
  let currentActive = $state<string | null>(null);
  $effect(() => {
    currentActive = activeId;
  });

  function activate(item: TestTab): void {
    currentActive = item.id;
    onActivate?.(item);
  }
</script>

<UnifiedTabStrip
  {tabs}
  activeId={currentActive}
  {label}
  {overflowLabel}
  {edgeFades}
  stripClass="test-strip"
  {activateOnArrow}
  onActivate={activate}
>
  {#snippet tab(item, cell)}
    <button
      class="test-tab"
      type="button"
      {...cell.triggerProps}
      onclick={() => activate(item)}
    >
      {item.label ?? item.id}
    </button>
  {/snippet}
</UnifiedTabStrip>
