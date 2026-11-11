<!--
  CustomAppsDialog — thin dialog wrapper around CustomAppsSection for the
  header "Open with" menu's "Manage custom apps…" item. All behavior lives
  in the section (embeddable without dialog chrome for a future Settings
  entry); this only provides the chrome, mirroring AppearanceDialog.
-->
<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import CustomAppsSection from "./CustomAppsSection.svelte";

  let { open = $bindable(false) }: { open?: boolean } = $props();
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Custom "Open with" apps</Dialog.Title>
      <Dialog.Description>
        User-defined actions shown alongside the detected apps in the header's
        "Open with" menu. Stored on this machine; nothing is synced.
      </Dialog.Description>
    </Dialog.Header>
    <CustomAppsSection />
  </Dialog.Content>
</Dialog.Root>
