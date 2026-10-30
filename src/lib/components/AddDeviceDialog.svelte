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
  import {
    remoteDevices,
  } from "$lib/stores/remoteDevices.svelte";
  import { toast } from "$lib/toast";
  import { trapTab } from "$lib/components/safety/trapTab";

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
  let nameInput = $state<HTMLInputElement | undefined>();
  let dialogEl = $state<HTMLDivElement | undefined>();
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

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="scrim"
    role="presentation"
    onclick={(event) => {
      if (event.target === event.currentTarget) close();
    }}
  >
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="device-title"
      tabindex="-1"
      bind:this={dialogEl}
      onkeydown={onDialogKeydown}
    >
      <div class="title-row">
        <h2 id="device-title" class="title">
          {edit !== null ? "Edit device" : "Add device"}
        </h2>
        <button
          class="close"
          type="button"
          aria-label="Close device dialog"
          title="Close"
          onclick={close}
        >
          ×
        </button>
      </div>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <div class="grid">
          <label class="field">
            <span class="field-label">Name</span>
            <input
              class="input"
              type="text"
              placeholder="build server"
              autocomplete="off"
              spellcheck="false"
              bind:value={name}
              bind:this={nameInput}
              disabled={busy}
            />
          </label>

          <label class="field">
            <span class="field-label">Username</span>
            <input
              class="input"
              type="text"
              placeholder="deploy"
              autocomplete="off"
              spellcheck="false"
              bind:value={username}
              disabled={busy}
            />
          </label>

          <label class="field">
            <span class="field-label">Host</span>
            <input
              class="input"
              type="text"
              placeholder="server.example.com"
              autocomplete="off"
              spellcheck="false"
              bind:value={host}
              disabled={busy}
            />
          </label>

          <label class="field narrow">
            <span class="field-label">Port</span>
            <input
              class="input"
              type="number"
              min="1"
              max="65535"
              bind:value={port}
              disabled={busy}
            />
          </label>
        </div>

        <div class="field">
          <span class="field-label">Authentication</span>
          <span class="radios">
            <label class="check">
              <input type="radio" value="password" bind:group={kind} disabled={busy} />
              <span>Password</span>
            </label>
            <label class="check">
              <input type="radio" value="key_file" bind:group={kind} disabled={busy} />
              <span>Key file</span>
            </label>
          </span>
        </div>

        {#if kind === "key_file"}
          <label class="field">
            <span class="field-label">Private key path</span>
            <input
              class="input"
              type="text"
              placeholder="~/.ssh/id_ed25519"
              autocomplete="off"
              spellcheck="false"
              bind:value={keyPath}
              disabled={busy}
            />
          </label>

          <label class="field">
            <span class="field-label">Key passphrase (optional)</span>
            <input
              class="input"
              type="password"
              autocomplete="new-password"
              bind:value={passphrase}
              placeholder={edit?.auth.has_passphrase
                ? "Stored — leave blank to keep"
                : ""}
              disabled={busy}
            />
          </label>
        {:else}
          <label class="field">
            <span class="field-label">Password (optional)</span>
            <input
              class="input"
              type="password"
              autocomplete="new-password"
              bind:value={password}
              disabled={busy}
            />
          </label>
        {/if}

        <label class="field narrow">
          <span class="field-label">Poll interval (seconds)</span>
          <input
            class="input"
            type="number"
            min="1"
            bind:value={pollSecs}
            disabled={busy}
          />
        </label>

        <div class="test-row">
          <button
            class="secondary"
            type="button"
            disabled={testing || busy || host.trim() === "" || username.trim() === ""}
            onclick={() => void runTest()}
          >
            {testing ? "Testing…" : "Test connection"}
          </button>
          {#if testResult}
            <span class="test-ok" role="status">{testResult}</span>
          {:else if testError}
            <span class="test-fail" role="alert">{testError}</span>
          {/if}
        </div>

        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}

        <div class="actions">
          <button class="secondary" type="button" onclick={close}>
            Cancel
          </button>
          <button class="primary" type="submit" disabled={!canSave}>
            {busy ? "Saving…" : edit !== null ? "Save changes" : "Add device"}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: color-mix(in srgb, var(--m3-scrim, #000) 32%, transparent);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 8vh;
  }

  .dialog {
    width: min(30rem, calc(100vw - 2rem));
    max-height: 84vh;
    overflow-y: auto;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-extra-large, 28px);
    box-shadow: var(--m3-elevation-3, 0 4px 8px rgba(0, 0, 0, 0.3));
    padding: 1.5rem;
  }

  .title {
    margin: 0;
    font-size: 1.375rem;
    font-weight: 400;
    color: var(--m3-on-surface);
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0 0.25rem 1rem;
  }

  .title-row .title {
    flex: 1;
  }

  .close {
    flex: none;
    width: 1.75rem;
    height: 1.75rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--m3-on-surface-variant);
    font: inherit;
    font-size: 1.25rem;
    line-height: 1;
    cursor: pointer;
  }

  .close:hover {
    background: var(--m3-surface-container-highest);
    color: var(--m3-on-surface);
  }

  .close:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
  }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.75rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field.narrow {
    max-width: 9rem;
  }

  .field-label {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .input {
    flex: 1;
    min-width: 0;
    border: none;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-highest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.8125rem;
    padding: 0.55rem 0.75rem;
  }

  .input:focus-visible {
    outline: 2px solid var(--m3-primary);
  }

  .input:disabled {
    opacity: 0.6;
  }

  .radios {
    display: flex;
    gap: 1.25rem;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
    cursor: pointer;
  }

  .check input {
    accent-color: var(--m3-primary);
  }

  .test-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.6rem;
  }

  .test-ok {
    color: var(--m3-primary);
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .test-fail {
    color: var(--m3-error);
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .error {
    margin: 0;
    color: var(--m3-error);
    font-size: 0.8125rem;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .primary,
  .secondary {
    font: inherit;
    font-size: 0.875rem;
    padding: 0.5rem 1.25rem;
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
  }

  .primary {
    border: none;
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .primary:disabled {
    background: color-mix(in srgb, var(--m3-on-surface) 12%, transparent);
    color: color-mix(in srgb, var(--m3-on-surface) 38%, transparent);
    cursor: not-allowed;
  }

  .secondary {
    border: 1px solid var(--m3-outline, var(--m3-primary));
    background: none;
    color: var(--m3-primary);
  }

  .secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .primary:focus-visible,
  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }
</style>
