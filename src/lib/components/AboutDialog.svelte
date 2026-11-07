<!--
  AboutDialog — the "About mygitui" modal (palette command `app.about`).

  Mounted on demand by `./aboutDialog` (host seam; see that module), so this
  component never appears in a statically-rendered tree: open = unmounts on
  close. Rendering reuses the shadcn ui/dialog primitives — scrim, focus
  trap, focus restore and Escape/outside-click dismissal are exactly the
  AppearanceDialog / safety-dialog chrome.

  The version comes from the Tauri runtime (`getVersion()`, which reads the
  Cargo/tauri.conf metadata) when running in the app; in plain-browser/e2e
  runs — the same `isTauri()` guard the IPC/dragdrop seams use — it falls
  back to the constant below. Keep that in sync with package.json.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { isTauri } from "$lib/entry/dragdrop";
  import { closeAboutDialog } from "./aboutDialog";

  /** Fallback when no Tauri runtime answers (browser/e2e); = package.json. */
  const FALLBACK_VERSION = "0.1.11";

  let version = $state(FALLBACK_VERSION);

  onMount(() => {
    if (!isTauri()) return;
    void getVersion()
      .then((v) => {
        if (typeof v === "string" && v !== "") version = v;
      })
      .catch(() => {
        // Runtime refused/could not answer — keep the fallback constant.
      });
  });

  function close(): void {
    closeAboutDialog();
  }
</script>

<Dialog.Root
  open
  onOpenChange={(next) => {
    if (!next) close();
  }}
>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>mygitui</Dialog.Title>
      <Dialog.Description>Fast, configurable Git GUI</Dialog.Description>
    </Dialog.Header>

    <dl class="m-0 flex flex-col gap-1 text-sm text-muted-foreground">
      <div class="flex items-center justify-between gap-4">
        <dt>Version</dt>
        <dd class="m-0 font-mono tabular-nums text-foreground">{version}</dd>
      </div>
      <div class="flex items-center justify-between gap-4">
        <dt>License</dt>
        <dd class="m-0 text-foreground">MIT License</dd>
      </div>
    </dl>

    <div class="mt-4 flex justify-end">
      <Button type="button" variant="outline" onclick={close}>Close</Button>
    </div>
  </Dialog.Content>
</Dialog.Root>
