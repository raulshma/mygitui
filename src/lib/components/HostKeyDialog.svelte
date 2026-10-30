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

  const request = $derived(remoteDevices.hostkeyPending);
  const changed = $derived(request?.status === "changed");

  // Focus lands on Reject (the safe answer) whenever a request appears.
  let rejectButton = $state<HTMLButtonElement | undefined>();
  $effect(() => {
    if (request) {
      requestAnimationFrame(() => rejectButton?.focus());
    }
  });

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      void remoteDevices.dismissHostKey();
    }
  }
</script>

{#if request}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="scrim" role="presentation" onkeydown={onKeydown}>
    <div
      class="dialog"
      class:danger={changed}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="hostkey-title"
      aria-describedby="hostkey-summary"
    >
      <h2 id="hostkey-title" class="title">
        {changed ? "Host key changed!" : "Unknown host"}
      </h2>
      <p id="hostkey-summary" class="summary">
        {#if changed}
          The host key presented by this server does not match the key pinned
          on first connect. This can mean a server reinstall — or an
          interception attack.
        {:else}
          First connect to this server. Verify its key fingerprint before
          trusting it.
        {/if}
      </p>

      <dl class="facts">
        <div class="fact">
          <dt>Host</dt>
          <dd class="mono">{request.host}:{request.port}</dd>
        </div>
        <div class="fact">
          <dt>Key algorithm</dt>
          <dd class="mono">{request.algo}</dd>
        </div>
        {#if changed}
          <div class="fact warn">
            <dt>Pinned fingerprint</dt>
            <dd class="mono">{request.pinned_fingerprint ?? "—"}</dd>
          </div>
          <div class="fact warn">
            <dt>New fingerprint</dt>
            <dd class="mono">{request.fingerprint}</dd>
          </div>
        {:else}
          <div class="fact">
            <dt>Fingerprint</dt>
            <dd class="mono">{request.fingerprint}</dd>
          </div>
        {/if}
      </dl>

      <div class="actions">
        <button
          class="secondary"
          type="button"
          bind:this={rejectButton}
          onclick={() => void remoteDevices.dismissHostKey()}
        >
          Reject
        </button>
        <button
          class={changed ? "danger" : "primary"}
          type="button"
          onclick={() => void remoteDevices.answerHostKey(true)}
        >
          {changed ? "I verified — trust new key" : "Trust and connect"}
        </button>
      </div>
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
    width: min(28rem, calc(100vw - 2rem));
    padding: 1.25rem 1.5rem;
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  /* Hard-warning variant for a CHANGED key (M3 error surface). */
  .dialog.danger {
    border: 1px solid var(--m3-error);
    background: var(--m3-error-container, var(--m3-surface-container-high));
  }

  .title {
    margin: 0 0 0.5rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .dialog.danger .title {
    color: var(--m3-error);
  }

  .summary {
    margin: 0 0 0.875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    line-height: 1.45;
  }

  .dialog.danger .summary {
    color: var(--m3-on-surface);
  }

  .facts {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin: 0 0 1rem;
  }

  .fact {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .fact dt {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  .fact dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .fact.warn dt {
    color: var(--m3-error);
    font-weight: 500;
  }

  .mono {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.78rem;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
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

  .danger {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, white);
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
  .danger:focus-visible,
  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }
</style>
