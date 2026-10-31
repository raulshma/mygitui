<script lang="ts">
  /**
   * Host-key dialog (remote devices) — bound to the remote-devices store's
   * pending `hostkey-request`.
   *
   * When a device connect reaches the SSH handshake the backend emits
   * `hostkey-request` and pauses; RemoteDeviceStore surfaces it as
   * `hostkeyPending` and this dialog renders the decision:
   *   - `status "unknown"` (first contact): "First connect" copy, host +
   *     algorithm + SHA256 fingerprint, Trust / Reject.
   *   - `status "changed"`: HARD WARNING styling (error color) with the
   *     pinned vs presented fingerprints side by side — the user must
   *     explicitly accept to proceed.
   * Accept answers via `device_hostkey_respond(request_id, true)`; Reject /
   * Escape answers `false` (the backend aborts the connect). Extra requests
   * queue in the store; App.svelte mounts one instance.
   */
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  const request = $derived(remoteDevices.hostkeyPending);
  const changed = $derived(request?.status === "changed");
  const open = $derived(request !== null);

  // Focus lands on Reject (the safe answer) whenever a request appears.
  let rejectButton = $state<HTMLElement | null>(null);
  $effect(() => {
    if (request) {
      requestAnimationFrame(() => rejectButton?.focus());
    }
  });

  // Escape (the only close path besides the buttons) rejects the key.
  function onOpenChange(next: boolean): void {
    if (!next) void remoteDevices.dismissHostKey();
  }
</script>

<Dialog.Root {open} {onOpenChange}>
  <Dialog.Content
    class="sdlg-dialog dialog w-full gap-4 px-6 py-5 sm:max-w-[28rem] {changed
      ? "danger border-destructive bg-destructive/10"
      : ""}"
    role="alertdialog"
    interactOutsideBehavior="ignore"
    showCloseButton={false}
    aria-describedby="hostkey-summary"
  >
    <Dialog.Header>
      <Dialog.Title
        id="hostkey-title"
        class="title text-lg font-medium {changed ? "text-destructive" : ""}"
      >
        {changed ? "Host key changed!" : "Unknown host"}
      </Dialog.Title>
      <Dialog.Description id="hostkey-summary" class="summary leading-relaxed">
        {#if changed}
          The host key presented by this server does not match the key pinned
          on first connect. This can mean a server reinstall — or an
          interception attack.
        {:else}
          First connect to this server. Verify its key fingerprint before
          trusting it.
        {/if}
      </Dialog.Description>
    </Dialog.Header>

    <dl class="facts mb-4 flex flex-col gap-1.5">
      <div class="fact flex flex-col gap-0.5">
        <dt class="text-muted-foreground text-xs">Host</dt>
        <dd class="mono m-0 font-mono text-xs wrap-anywhere">
          {request?.host}:{request?.port}
        </dd>
      </div>
      <div class="fact flex flex-col gap-0.5">
        <dt class="text-muted-foreground text-xs">Key algorithm</dt>
        <dd class="mono m-0 font-mono text-xs wrap-anywhere">{request?.algo}</dd>
      </div>
      {#if changed}
        <div class="fact warn flex flex-col gap-0.5">
          <dt class="text-destructive text-xs font-medium">Pinned fingerprint</dt>
          <dd class="mono m-0 font-mono text-xs wrap-anywhere">
            {request?.pinned_fingerprint ?? "—"}
          </dd>
        </div>
        <div class="fact warn flex flex-col gap-0.5">
          <dt class="text-destructive text-xs font-medium">New fingerprint</dt>
          <dd class="mono m-0 font-mono text-xs wrap-anywhere">
            {request?.fingerprint}
          </dd>
        </div>
      {:else}
        <div class="fact flex flex-col gap-0.5">
          <dt class="text-muted-foreground text-xs">Fingerprint</dt>
          <dd class="mono m-0 font-mono text-xs wrap-anywhere">
            {request?.fingerprint}
          </dd>
        </div>
      {/if}
    </dl>

    <div class="actions flex justify-end gap-2">
      <Button
        class="secondary"
        variant="outline"
        type="button"
        bind:ref={rejectButton}
        onclick={() => void remoteDevices.dismissHostKey()}
      >
        Reject
      </Button>
      <Button
        class={changed ? "danger" : "primary"}
        variant={changed ? "destructive" : "default"}
        type="button"
        onclick={() => void remoteDevices.answerHostKey(true)}
      >
        {changed ? "I verified — trust new key" : "Trust and connect"}
      </Button>
    </div>
  </Dialog.Content>
</Dialog.Root>
