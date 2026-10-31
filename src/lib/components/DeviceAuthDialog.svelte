<script lang="ts">
  /**
   * Device auth dialog (remote devices) — bound to the remote-devices
   * store's pending `device-auth-request`.
   *
   * The device connect flow mirrors the repo op flow of AuthDialog: the
   * backend pauses for SSH credentials, emits `device-auth-request`, and
   * RemoteDeviceStore surfaces it as `authPending`. This dialog renders the
   * backend's prompt with one password field (labelled "Passphrase" for
   * `ssh-passphrase` requests) and a "remember" checkbox. Submit answers
   * via `device_auth_respond` (with `store` = remember); Cancel/Escape
   * answers `null` (backend aborts the connect). Extra requests queue in
   * the store; App.svelte mounts one instance.
   */
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  const request = $derived(remoteDevices.authPending);
  const isPassphrase = $derived(request?.kind === "ssh-passphrase");
  const open = $derived(request !== null);

  let password = $state("");
  let store = $state(false);
  let passField = $state<HTMLElement | null>(null);

  // Reset the form and focus the password field whenever a request appears.
  $effect(() => {
    if (request) {
      password = "";
      store = false;
      // Focus after the dialog enters the DOM.
      requestAnimationFrame(() => passField?.focus());
    }
  });

  async function submit(): Promise<void> {
    await remoteDevices.answerAuth(password || null, store);
  }

  async function cancel(): Promise<void> {
    await remoteDevices.dismissAuth();
  }

  // Escape (the only close path besides the buttons) answers null.
  function onOpenChange(next: boolean): void {
    if (!next) void cancel();
  }
</script>

<Dialog.Root {open} {onOpenChange}>
  <Dialog.Content
    class="sdlg-dialog dialog w-full gap-4 px-6 py-5 sm:max-w-[26rem]"
    interactOutsideBehavior="ignore"
    showCloseButton={false}
    aria-describedby="device-auth-prompt"
  >
    <Dialog.Header>
      <Dialog.Title id="device-auth-title" class="title text-lg font-medium">
        Device authentication
      </Dialog.Title>
      <Dialog.Description
        id="device-auth-prompt"
        class="prompt wrap-anywhere"
      >
        {request?.prompt}
      </Dialog.Description>
    </Dialog.Header>

    <form
      class="flex flex-col gap-3"
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <Label class="field flex flex-col items-start gap-1">
        <span class="field-label text-muted-foreground text-xs">
          {isPassphrase ? "Key passphrase" : "Password"}
        </span>
        <Input
          class="input"
          type="password"
          autocomplete="current-password"
          bind:value={password}
          bind:ref={passField}
        />
      </Label>

      <Label class="check text-muted-foreground flex cursor-pointer items-center gap-2">
        <Checkbox bind:checked={store} />
        <span>Remember (store in keyring)</span>
      </Label>

      <div class="actions mt-1 flex justify-end gap-2">
        <Button class="primary" size="sm" type="submit">
          {isPassphrase ? "Unlock" : "Sign in"}
        </Button>
        <Button
          class="secondary"
          variant="outline"
          size="sm"
          type="button"
          onclick={() => void cancel()}
        >
          Cancel
        </Button>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>
