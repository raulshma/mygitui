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
  import { formatDateTime } from "$lib/git/format";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

  let { repoId, showLauncher = true }: { repoId: string; showLauncher?: boolean } = $props();

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
  <div
    class={"banner flex flex-wrap items-center gap-2 border-b border-border px-3 py-1.5 text-[0.8125rem] " +
      (bisect.remaining === 0
        ? "done bg-primary/15 text-primary"
        : "bg-secondary text-secondary-foreground")}
    role="status"
  >
    {#if bisect.remaining === 0}
      <span class="title flex-none font-semibold">Bisect complete</span>
      <span class="detail min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
        First bad commit: <code class="font-mono">{bisect.bad.slice(0, 8)}</code>
      </span>
      <Button variant="outline" size="xs" class="flex-none rounded-full px-2.5 text-[0.72rem]" type="button" onclick={() => emitUiEvent("history-select-commit", { sha: bisect?.bad ?? "" })}>
        Show in history
      </Button>
    {:else}
      <span class="title flex-none font-semibold">Bisect</span>
      <span class="detail min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
        testing <code class="font-mono">{(bisect.current ?? "").slice(0, 8)}</code>
        · {bisect.remaining} candidate{bisect.remaining === 1 ? "" : "s"} left
      </span>
      <Button variant="default" size="xs" class="flex-none rounded-full px-2.5 text-[0.72rem]" type="button" disabled={marking} onclick={() => void onMark("good")}>Good</Button>
      <Button variant="destructive" size="xs" class="flex-none rounded-full px-2.5 text-[0.72rem]" type="button" disabled={marking} onclick={() => void onMark("bad")}>Bad</Button>
      <Button variant="outline" size="xs" class="flex-none rounded-full px-2.5 text-[0.72rem]" type="button" disabled={marking} onclick={() => void onMark("skip")}>Skip</Button>
    {/if}
    <Button variant="outline" size="xs" class="btn subtle ml-auto flex-none rounded-full px-2.5 text-[0.72rem]" type="button" disabled={marking} onclick={() => void onLog()}>
      Log…
    </Button>
    <Button variant="outline" size="xs" class="btn subtle ml-auto flex-none rounded-full px-2.5 text-[0.72rem]" type="button" disabled={marking} onclick={() => void onReset()}>
      Reset
    </Button>
  </div>
{:else if showLauncher}
  <Button
    variant="outline"
    size="xs"
    class="launcher mx-2 my-1 self-start rounded-full px-2.5 text-[0.72rem] text-muted-foreground"
    type="button"
    onclick={() => {
      badRef = status?.head ?? "";
      goodRef = "";
      startOpen = true;
    }}
  >
    Bisect…
  </Button>
{/if}

{#if startOpen}
  <Dialog.Root open={true}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class="dialog w-[min(24rem,calc(100vw-2rem))] max-w-none gap-0 rounded-2xl bg-card px-6 py-5 text-sm text-popover-foreground shadow-lg sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="bisect-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          startOpen = false;
        }
      }}
    >
      <form
        onsubmit={(e) => {
          e.preventDefault();
          void onStart();
        }}
      >
        <Dialog.Title id="bisect-title" class="mb-2 text-lg font-medium text-foreground">Start bisect</Dialog.Title>
        <p class="text mb-3 text-muted-foreground">
          Binary-search history for the first bad commit. HEAD detaches onto
          probes until you mark and reset.
        </p>
        <label class="field mb-2.5 flex flex-col gap-1 text-xs text-muted-foreground">
          <span>Bad (search starts here)</span>
          <Input
            class="h-auto rounded-lg px-1.5 py-1 text-[0.8125rem] focus-visible:-outline-offset-1"
            bind:value={badRef}
            placeholder="HEAD or ref"
            aria-label="Bad commit-ish"
          />
        </label>
        <label class="field mb-2.5 flex flex-col gap-1 text-xs text-muted-foreground">
          <span>Good (last known-good, optional)</span>
          <Input
            class="h-auto rounded-lg px-1.5 py-1 text-[0.8125rem] focus-visible:-outline-offset-1"
            bind:value={goodRef}
            placeholder="ref or sha"
            aria-label="Good commit-ish"
          />
        </label>
        <div class="actions mt-3 flex justify-end gap-2">
          <Button variant="outline" class="text-muted-foreground" type="button" onclick={() => (startOpen = false)}>
            Cancel
          </Button>
          <Button variant="default" type="submit" disabled={starting || !badRef.trim()}>
            {starting ? "Starting…" : "Start"}
          </Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}

{#if logOpen}
  <!-- M12: bisect mark history (oldest first; empty state handled). -->
  <Dialog.Root open={true}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class="dialog w-[min(24rem,calc(100vw-2rem))] max-w-none gap-0 rounded-2xl bg-card px-6 py-5 text-sm text-popover-foreground shadow-lg sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="bisect-log-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          logOpen = false;
        }
      }}
    >
      <Dialog.Title id="bisect-log-title" class="mb-2 text-lg font-medium text-foreground">Bisect log</Dialog.Title>
      {#if logLoading}
        <p class="text mb-3 text-muted-foreground">Loading…</p>
      {:else if logEntries === null || logEntries.length === 0}
        <p class="text mb-3 text-muted-foreground">No marks recorded yet — mark a probe Good, Bad or Skip first.</p>
      {:else}
        <ol class="log-list m-0 mb-1 max-h-64 list-none overflow-y-auto p-0 text-xs" aria-label="Marks, oldest first">
          {#each logEntries as entry, i (i)}
            <li class="log-row flex items-center gap-2 py-0.5">
              <span
                class={"mark w-12 flex-none rounded-full px-1.5 text-center text-[0.625rem] font-semibold uppercase tracking-wide " +
                  `mark-${entry.mark} ` +
                  (entry.mark === "good"
                    ? "bg-primary/15 text-primary"
                    : entry.mark === "bad"
                      ? "bg-destructive/10 text-destructive"
                      : "bg-muted text-muted-foreground")}
              >{entry.mark}</span>
              <code class="log-sha font-mono text-primary" title={entry.sha}>{entry.sha.slice(0, 8)}</code>
              <span class="log-time ml-auto text-[0.6875rem] text-muted-foreground">{formatDateTime(entry.at)}</span>
            </li>
          {/each}
        </ol>
      {/if}
      <div class="actions mt-3 flex justify-end gap-2">
        <Button variant="outline" class="text-muted-foreground" type="button" onclick={() => (logOpen = false)}>
          Close
        </Button>
      </div>
    </Dialog.Content>
  </Dialog.Root>
{/if}
