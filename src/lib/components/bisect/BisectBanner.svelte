<!--
  BisectBanner (M10) — the bisect stepper, mounted once per workspace in
  RepoView (like the conflict banner).

  Inactive: a compact "Bisect…" button that opens the start form (bad prefills
  HEAD; good is the last-known-good ref). Active: shows the detached probe,
  remaining count, and Good/Bad/Skip marks + Reset. When `remaining === 0`
  the bad tip IS the first bad commit — shown with a "Show in history"
  action (the typed event bus reaches HistoryView in this same window).
-->
<script lang="ts">
  import { bisectLog, bisectMark, bisectReset, bisectStart, bisectState } from "$lib/ipc/client";
  import type { BisectLogEntry, BisectState } from "$lib/ipc/types";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { formatDateTime } from "$lib/stores/history-logic";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";

  let { repoId }: { repoId: string } = $props();

  let bisect = $state<BisectState | null>(null);
  let starting = $state(false);
  let startOpen = $state(false);
  let badRef = $state("");
  let goodRef = $state("");
  let marking = $state(false);

  // M12: mark-history dialog.
  let logOpen = $state(false);
  let logLoading = $state(false);
  let logEntries = $state<BisectLogEntry[] | null>(null);

  const status = $derived(tabStore.tabs.find((t) => t.id === repoId)?.status ?? null);

  $effect(() => {
    void repoId;
    bisect = null;
    void reload();
  });

  // Palette command + external start requests.
  $effect(() => {
    const unlisteners = [
      onUiEvent("bisect-open-start", () => {
        badRef = status?.head ?? "";
        goodRef = "";
        startOpen = true;
      }),
    ];
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  });

  async function reload(): Promise<void> {
    try {
      bisect = await bisectState(repoId);
    } catch {
      bisect = null;
    }
  }

  /** M12: opens the mark-history dialog (fetches fresh, handles empty). */
  async function onLog(): Promise<void> {
    logOpen = true;
    logLoading = true;
    logEntries = null;
    try {
      logEntries = await bisectLog(repoId);
    } catch (err) {
      toast(
        `Bisect log failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
      logOpen = false;
    } finally {
      logLoading = false;
    }
  }

  async function onStart(): Promise<void> {
    if (starting) return;
    starting = true;
    try {
      bisect = await bisectStart(repoId, badRef.trim() || undefined, goodRef.trim() || undefined);
      startOpen = false;
      await tabStore.refreshStatus(repoId);
      toast("Bisect started — HEAD detached on the first probe", { kind: "success" });
    } catch (err) {
      toast(
        `Bisect start failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      starting = false;
    }
  }

  async function onMark(mark: "good" | "bad" | "skip"): Promise<void> {
    if (marking) return;
    marking = true;
    try {
      bisect = await bisectMark(repoId, mark);
      await tabStore.refreshStatus(repoId);
      if (bisect.remaining === 0) {
        toast(`First bad commit: ${bisect.bad.slice(0, 8)}`, { kind: "success" });
      }
    } catch (err) {
      toast(
        `Bisect mark failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      marking = false;
    }
  }

  async function onReset(): Promise<void> {
    if (marking) return;
    marking = true;
    try {
      await bisectReset(repoId);
      await tabStore.refreshStatus(repoId);
      await reload();
      toast("Bisect ended — back on the original branch", { kind: "success" });
    } catch (err) {
      toast(
        `Bisect reset failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      marking = false;
    }
  }
</script>

{#if bisect?.active}
  <div class="banner" class:done={bisect.remaining === 0} role="status">
    {#if bisect.remaining === 0}
      <span class="title">Bisect complete</span>
      <span class="detail">
        First bad commit: <code>{bisect.bad.slice(0, 8)}</code>
      </span>
      <button class="btn" type="button" onclick={() => emitUiEvent("history-select-commit", { sha: bisect?.bad ?? "" })}>
        Show in history
      </button>
    {:else}
      <span class="title">Bisect</span>
      <span class="detail">
        testing <code>{(bisect.current ?? "").slice(0, 8)}</code>
        · {bisect.remaining} candidate{bisect.remaining === 1 ? "" : "s"} left
      </span>
      <button class="btn good" type="button" disabled={marking} onclick={() => void onMark("good")}>Good</button>
      <button class="btn bad" type="button" disabled={marking} onclick={() => void onMark("bad")}>Bad</button>
      <button class="btn" type="button" disabled={marking} onclick={() => void onMark("skip")}>Skip</button>
    {/if}
    <button class="btn subtle" type="button" disabled={marking} onclick={() => void onLog()}>
      Log…
    </button>
    <button class="btn subtle" type="button" disabled={marking} onclick={() => void onReset()}>
      Reset
    </button>
  </div>
{:else}
  <button class="launcher" type="button" onclick={() => {
    badRef = status?.head ?? "";
    goodRef = "";
    startOpen = true;
  }}>
    Bisect…
  </button>
{/if}

{#if startOpen}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="scrim"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        startOpen = false;
      }
    }}
  >
    <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
    <form
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="bisect-title"
      onsubmit={(e) => {
        e.preventDefault();
        void onStart();
      }}
    >
      <h2 id="bisect-title" class="title">Start bisect</h2>
      <p class="text">
        Binary-search history for the first bad commit. HEAD detaches onto
        probes until you mark and reset.
      </p>
      <label class="field">
        <span>Bad (search starts here)</span>
        <input bind:value={badRef} placeholder="HEAD or ref" aria-label="Bad commit-ish" />
      </label>
      <label class="field">
        <span>Good (last known-good, optional)</span>
        <input bind:value={goodRef} placeholder="ref or sha" aria-label="Good commit-ish" />
      </label>
      <div class="actions">
        <button class="secondary" type="button" onclick={() => (startOpen = false)}>
          Cancel
        </button>
        <button class="primary" type="submit" disabled={starting || !badRef.trim()}>
          {starting ? "Starting…" : "Start"}
        </button>
      </div>
    </form>
  </div>
{/if}

{#if logOpen}
  <!-- M12: bisect mark history (oldest first; empty state handled). -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="scrim"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        logOpen = false;
      }
    }}
  >
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="bisect-log-title">
      <h2 id="bisect-log-title" class="title">Bisect log</h2>
      {#if logLoading}
        <p class="text">Loading…</p>
      {:else if logEntries === null || logEntries.length === 0}
        <p class="text">No marks recorded yet — mark a probe Good, Bad or Skip first.</p>
      {:else}
        <ol class="log-list" aria-label="Marks, oldest first">
          {#each logEntries as entry, i (i)}
            <li class="log-row">
              <span class={`mark mark-${entry.mark}`}>{entry.mark}</span>
              <code class="log-sha" title={entry.sha}>{entry.sha.slice(0, 8)}</code>
              <span class="log-time">{formatDateTime(entry.at)}</span>
            </li>
          {/each}
        </ol>
      {/if}
      <div class="actions">
        <button class="secondary" type="button" onclick={() => (logOpen = false)}>
          Close
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    padding: 0.375rem 0.75rem;
    background: var(--m3-tertiary-container, var(--m3-surface-container));
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
    border-bottom: 1px solid var(--m3-outline-variant, transparent);
    font-size: 0.8125rem;
  }

  .banner.done {
    background: var(--m3-primary-container, var(--m3-surface-container));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
  }

  .title {
    font-weight: 600;
    flex: none;
  }

  .detail {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  code {
    font-family: ui-monospace, Consolas, monospace;
  }

  .btn {
    flex: none;
    border: 1px solid var(--m3-outline-variant, transparent);
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface, none);
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.7rem;
    cursor: pointer;
  }

  .btn.good {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    border: none;
  }

  .btn.bad {
    background: var(--m3-error);
    color: var(--m3-on-error);
    border: none;
  }

  .btn.subtle {
    margin-left: auto;
  }

  .btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .launcher {
    align-self: flex-start;
    margin: 0.25rem 0.5rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.7rem;
    cursor: pointer;
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(24rem, calc(100vw - 2rem));
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

  .text {
    margin: 0 0 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin-bottom: 0.6rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .field input {
    font: inherit;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.4rem 0.6rem;
  }

  .field input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.75rem;
  }

  /* M12: mark-history dialog rows. */
  .log-list {
    margin: 0 0 0.25rem;
    padding: 0;
    list-style: none;
    max-height: 16rem;
    overflow-y: auto;
    font-size: 0.75rem;
  }

  .log-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.15rem 0;
  }

  .mark {
    flex: none;
    width: 3rem;
    padding: 0 0.35rem;
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.625rem;
    text-align: center;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .mark-good {
    background: var(--m3-primary-container, transparent);
    color: var(--m3-on-primary-container, inherit);
  }

  .mark-bad {
    background: var(--m3-error-container, transparent);
    color: var(--m3-on-error-container, inherit);
  }

  .mark-skip {
    background: var(--m3-surface-container-highest, transparent);
    color: var(--m3-on-surface-variant, inherit);
  }

  .log-sha {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-primary);
  }

  .log-time {
    margin-left: auto;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
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

  .primary:disabled {
    opacity: 0.55;
    cursor: not-allowed;
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
</style>
