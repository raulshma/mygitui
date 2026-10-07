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

  const request = $derived(authStore.pending);
  const isPassphrase = $derived(request?.kind === "ssh-passphrase");

  let username = $state("");
  let password = $state("");
  let store = $state(false);
  let userField: HTMLInputElement | undefined = $state();
  let passField: HTMLInputElement | undefined = $state();

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
      aria-labelledby="auth-title"
      aria-describedby="auth-prompt"
    >
      <h2 id="auth-title" class="title">Authentication required</h2>
      <p id="auth-prompt" class="prompt">{request.prompt}</p>
      <p class="meta" title={request.url}>{request.url}</p>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        {#if !isPassphrase}
          <label class="field">
            <span class="field-label">Username</span>
            <input
              class="input"
              type="text"
              autocomplete="username"
              spellcheck="false"
              bind:value={username}
              bind:this={userField}
            />
          </label>
        {/if}

        <label class="field">
          <span class="field-label">{isPassphrase ? "Passphrase" : "Password"}</span>
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
          <span>Store in keyring</span>
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
    margin: 0 0 0.25rem;
    color: var(--m3-on-surface);
    overflow-wrap: anywhere;
  }

  .meta {
    margin: 0 0 0.875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
