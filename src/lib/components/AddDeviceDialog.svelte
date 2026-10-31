<script lang="ts">
  /**
   * Add/edit-device dialog (remote devices).
   *
   * App.svelte owns the `open` flag (bindable) and passes `edit` to switch
   * into edit mode (the device's profile pre-fills every field; secrets are
   * never rendered back — leaving a secret blank keeps the stored one, so
   * `has_passphrase` only clears when the profile had none).
   *
   * "Test connection" probes the form's credentials via `device_test`
   * WITHOUT registering (inline error/success); Save registers the device
   * through the remote-devices store (`device_add` / `device_update`).
   */
  import { deviceTest } from "$lib/ipc/client";
  import type { AuthKind, DeviceInput, DeviceProfile } from "$lib/ipc/types";
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import { toast } from "$lib/toast";
  import { trapTab } from "$lib/components/safety/trapTab";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  let {
    open = $bindable(false),
    edit = null,
  }: {
    open?: boolean;
    /** When set, the dialog edits this device instead of adding one. */
    edit?: DeviceProfile | null;
  } = $props();

  let name = $state("");
  let host = $state("");
  let port = $state(22);
  let username = $state("");
  let kind = $state<AuthKind>("password");
  let keyPath = $state("");
  let passphrase = $state("");
  let password = $state("");
  let pollSecs = $state(5);

  let busy = $state(false);
  let testing = $state(false);
  let error = $state("");
  let testResult = $state("");
  let testError = $state("");
  let nameInput = $state<HTMLElement | null>(null);
  let dialogEl = $state<HTMLElement | null>(null);
  /** Element focused before the dialog opened (focus returns there on close). */
  let restoreTo: HTMLElement | null = null;

  const canSave = $derived(
    name.trim() !== "" &&
      host.trim() !== "" &&
      username.trim() !== "" &&
      (kind === "password" || keyPath.trim() !== "") &&
      !busy,
  );

  // Fresh form whenever the dialog opens (pre-filled in edit mode).
  $effect(() => {
    if (open) {
      restoreTo =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      name = edit?.name ?? "";
      host = edit?.host ?? "";
      port = edit?.port ?? 22;
      username = edit?.username ?? "";
      kind = edit?.auth.kind ?? "password";
      keyPath = edit?.auth.key_path ?? "";
      passphrase = "";
      password = "";
      pollSecs = edit?.poll_secs ?? 5;
      error = "";
      testResult = "";
      testError = "";
      requestAnimationFrame(() => nameInput?.focus());
    } else if (restoreTo !== null) {
      restoreTo.focus();
      restoreTo = null;
    }
  });

  /** Escape closes; Tab stays inside the dialog (shared focus trap). */
  function onDialogKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    trapTab(dialogEl, event);
  }

  function close(): void {
    open = false;
  }

  /** The form as a probe/update payload (id carried for edit mode). */
  function buildInput(): DeviceInput {
    return {
      id: edit?.id ?? null,
      name: name.trim(),
      host: host.trim(),
      port,
      username: username.trim(),
      kind,
      key_path: kind === "key_file" && keyPath.trim() !== "" ? keyPath.trim() : null,
      has_passphrase:
        passphrase !== "" || (edit !== null && edit.auth.has_passphrase),
      password: password !== "" ? password : null,
      passphrase: passphrase !== "" ? passphrase : null,
      poll_secs: pollSecs,
    };
  }

  async function runTest(): Promise<void> {
    if (host.trim() === "" || username.trim() === "") return;
    testing = true;
    testResult = "";
    testError = "";
    try {
      testResult = await deviceTest(buildInput());
    } catch (err) {
      testError = err instanceof Error ? err.message : String(err);
    } finally {
      testing = false;
    }
  }

  async function save(): Promise<void> {
    if (!canSave) return;
    busy = true;
    error = "";
    try {
      const input = buildInput();
      if (edit !== null) {
        await remoteDevices.update(input);
        toast(`Updated device ${name.trim()}`, { kind: "success" });
      } else {
        await remoteDevices.add({ ...input, id: null });
        toast(`Added device ${name.trim()}`, { kind: "success" });
      }
      open = false;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog.Root bind:open={open}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Content
    class="sdlg-dialog dialog max-h-[84vh] w-full gap-4 overflow-y-auto px-6 py-6 sm:max-w-[30rem]"
    aria-describedby={undefined}
    showCloseButton={false}
    bind:ref={dialogEl}
    onkeydown={onDialogKeydown}
  >
    <div class="title-row mb-4 flex items-center gap-2">
      <h2
        id="device-title"
        class="title m-0 flex-1 text-[1.375rem] font-normal text-foreground"
      >
        {edit !== null ? "Edit device" : "Add device"}
      </h2>
      <Button
        class="close rounded-full text-xl leading-none"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label="Close device dialog"
        title="Close"
        onclick={close}
      >
        ×
      </Button>
    </div>

    <form
      class="flex flex-col gap-3.5"
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div class="grid grid-cols-2 gap-3">
        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Name
          </span>
          <Input
            class="input"
            type="text"
            placeholder="build server"
            autocomplete="off"
            spellcheck="false"
            bind:value={name}
            bind:ref={nameInput}
            disabled={busy}
          />
        </Label>

        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Username
          </span>
          <Input
            class="input"
            type="text"
            placeholder="deploy"
            autocomplete="off"
            spellcheck="false"
            bind:value={username}
            disabled={busy}
          />
        </Label>

        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Host
          </span>
          <Input
            class="input"
            type="text"
            placeholder="server.example.com"
            autocomplete="off"
            spellcheck="false"
            bind:value={host}
            disabled={busy}
          />
        </Label>

        <Label class="field narrow flex w-36 max-w-full flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Port
          </span>
          <Input
            class="input"
            type="number"
            min="1"
            max="65535"
            bind:value={port}
            disabled={busy}
          />
        </Label>
      </div>

      <div class="field flex flex-col items-start gap-1">
        <span class="field-label text-muted-foreground text-xs font-medium">
          Authentication
        </span>
        <span class="radios flex gap-5">
          <Label class="check flex cursor-pointer items-center gap-1.5 text-sm text-foreground">
            <input
              type="radio"
              value="password"
              bind:group={kind}
              disabled={busy}
              class="accent-primary"
            />
            <span>Password</span>
          </Label>
          <Label class="check flex cursor-pointer items-center gap-1.5 text-sm text-foreground">
            <input
              type="radio"
              value="key_file"
              bind:group={kind}
              disabled={busy}
              class="accent-primary"
            />
            <span>Key file</span>
          </Label>
        </span>
      </div>

      {#if kind === "key_file"}
        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Private key path
          </span>
          <Input
            class="input"
            type="text"
            placeholder="~/.ssh/id_ed25519"
            autocomplete="off"
            spellcheck="false"
            bind:value={keyPath}
            disabled={busy}
          />
        </Label>

        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Key passphrase (optional)
          </span>
          <Input
            class="input"
            type="password"
            autocomplete="new-password"
            bind:value={passphrase}
            placeholder={edit?.auth.has_passphrase
              ? "Stored — leave blank to keep"
              : ""}
            disabled={busy}
          />
        </Label>
      {:else}
        <Label class="field flex flex-col items-start gap-1">
          <span class="field-label text-muted-foreground text-xs font-medium">
            Password (optional)
          </span>
          <Input
            class="input"
            type="password"
            autocomplete="new-password"
            bind:value={password}
            disabled={busy}
          />
        </Label>
      {/if}

      <Label class="field narrow flex w-36 max-w-full flex-col items-start gap-1">
        <span class="field-label text-muted-foreground text-xs font-medium">
          Poll interval (seconds)
        </span>
        <Input
          class="input"
          type="number"
          min="1"
          bind:value={pollSecs}
          disabled={busy}
        />
      </Label>

      <div class="test-row flex flex-wrap items-center gap-2.5">
        <Button
          class="secondary"
          variant="outline"
          size="xs"
          type="button"
          disabled={testing || busy || host.trim() === "" || username.trim() === ""}
          onclick={() => void runTest()}
        >
          {testing ? "Testing…" : "Test connection"}
        </Button>
        {#if testResult}
          <span class="test-ok text-primary wrap-anywhere text-xs" role="status">
            {testResult}
          </span>
        {:else if testError}
          <span class="test-fail text-destructive wrap-anywhere text-xs" role="alert">
            {testError}
          </span>
        {/if}
      </div>

      {#if error}
        <p class="error text-destructive m-0 text-[0.8125rem]" role="alert">
          {error}
        </p>
      {/if}

      <div class="actions flex justify-end gap-2">
        <Button class="secondary" variant="outline" size="sm" type="button" onclick={close}>
          Cancel
        </Button>
        <Button class="primary" size="sm" type="submit" disabled={!canSave}>
          {busy ? "Saving…" : edit !== null ? "Save changes" : "Add device"}
        </Button>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>
