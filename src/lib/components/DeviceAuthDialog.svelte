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

  const request = $derived(remoteDevices.authPending);
  const isPassphrase = $derived(request?.kind === "ssh-passphrase");

  let password = $state("");
  let store = $state(false);
  let passField: HTMLInputElement | undefined = $state();

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

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      void cancel();
    }
  }
</script>

{#if request}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="scrim" role="presentation" onkeydown={onKeydown}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="device-auth-title"
      aria-describedby="device-auth-prompt"
    >
      <h2 id="device-auth-title" class="title">Device authentication</h2>
      <p id="device-auth-prompt" class="prompt">{request.prompt}</p>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <label class="field">
          <span class="field-label">
            {isPassphrase ? "Key passphrase" : "Password"}
          </span>
          <input
            class="input"
            type="password"
            autocomplete="current-password"
            bind:value={password}
            bind:this={passField}
          />
        </label>

        <label class="check">
          <input type="checkbox" bind:checked={store} />
          <span>Remember (store in keyring)</span>
        </label>

        <div class="actions">
          <button class="primary" type="submit">
            {isPassphrase ? "Unlock" : "Sign in"}
          </button>
          <button class="secondary" type="button" onclick={() => void cancel()}>
            Cancel
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
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(26rem, calc(100vw - 2rem));
    padding: 1.25rem 1.5rem;
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  .title {
    margin: 0 0 0.5rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .prompt {
    margin: 0 0 0.875rem;
    color: var(--m3-on-surface);
    overflow-wrap: anywhere;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field-label {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .input {
    font: inherit;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.45rem 0.6rem;
  }

  .input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .check input {
    accent-color: var(--m3-primary);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.25rem;
  }

  .primary {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-weight: 500;
    padding: 0.45rem 1.25rem;
    cursor: pointer;
  }

  .secondary {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .primary:focus-visible,
  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }
</style>
