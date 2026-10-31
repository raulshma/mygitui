<script lang="ts">
  /**
   * Credential dialog (M2) — bound to the AuthStore's pending request.
   *
   * When the backend pauses a network op for credentials it emits
   * `auth-request`; AuthStore surfaces it as `pending` and this dialog
   * renders the backend's prompt with username + password fields (the
   * username field hides for `ssh-passphrase` requests, the password label
   * becomes "Passphrase"). Submit answers via `authRespond` (backend
   * continues); Cancel/Escape answers empty (backend aborts the op).
   * RepoView mounts one instance; extra requests queue in the store.
   */
  import { authStore } from "$lib/stores/ops.svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  const request = $derived(authStore.pending);
  const isPassphrase = $derived(request?.kind === "ssh-passphrase");
  const open = $derived(request !== null);

  let username = $state("");
  let password = $state("");
  let store = $state(false);
  let userField = $state<HTMLElement | null>(null);
  let passField = $state<HTMLElement | null>(null);

  // Reset the form and focus the first field whenever a request appears.
  $effect(() => {
    if (request) {
      username = "";
      password = "";
      store = false;
      // Focus after the dialog enters the DOM.
      requestAnimationFrame(() =>
        (isPassphrase ? passField : userField)?.focus(),
      );
    }
  });

  async function submit(): Promise<void> {
    if (!request) return;
    await authStore.answer({
      username: isPassphrase ? undefined : username,
      password: password || undefined,
      store,
    });
  }

  async function cancel(): Promise<void> {
    await authStore.dismiss();
  }

  // Escape (the only close path besides the buttons) answers empty.
  function onOpenChange(next: boolean): void {
    if (!next) void cancel();
  }
</script>

<Dialog.Root {open} {onOpenChange}>
  <Dialog.Content
    class="sdlg-dialog dialog w-full gap-4 px-6 py-5 sm:max-w-[26rem]"
    interactOutsideBehavior="ignore"
    showCloseButton={false}
    aria-describedby="auth-prompt"
  >
    <Dialog.Header>
      <Dialog.Title id="auth-title" class="title text-lg font-medium">
        Authentication required
      </Dialog.Title>
      <Dialog.Description id="auth-prompt" class="prompt wrap-anywhere">
        {request?.prompt}
      </Dialog.Description>
    </Dialog.Header>
    <p
      class="meta text-muted-foreground truncate font-mono text-[0.72rem]"
      title={request?.url}
    >
      {request?.url}
    </p>

    <form
      class="flex flex-col gap-3"
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      {#if !isPassphrase}
        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs">Username</span>
          <Input
            class="input"
            type="text"
            autocomplete="username"
            spellcheck="false"
            bind:value={username}
            bind:ref={userField}
          />
        </Label>
      {/if}

      <Label class="field flex flex-col items-start gap-1">
        <span class="field-label text-muted-foreground text-xs">
          {isPassphrase ? "Passphrase" : "Password"}
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
        <span>Store in keyring</span>
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
