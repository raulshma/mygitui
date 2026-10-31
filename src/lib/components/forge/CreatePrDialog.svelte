<!--
  Create-PR dialog (M6 forge) — AI-prefillable PR creation.

  On open it resets, loads the base-branch options (`branches()`,
  preferring remote-tracked branches) and up to 10 recent commits (first
  log page) as the AI hint. "Generate title/body" emits the typed
  `ai-generate-pr` UI event (`$lib/palette/events`; payload
  `{ repoId, commits: [{ sha, summary }] }`) and the dialog listens for
  `ai-pr-result` (`{ subject, body }`) to fill the fields — user typing
  clears the pending wait (docs/contracts.md "M6 FE contracts";
  reconciled with H1's commit-message contract by the orchestrator).

  Create runs `prCreate` (120 s backend timeout): success toasts, calls
  `onCreated` (the owner refreshes context + PR list; the dialog stays
  open showing the URL + "Open on GitHub" until Done), `AlreadyExists`
  offers the existing PR's URL instead. Cancel / Escape / Done →
  `onClose()`; the owner flips `open`.
-->
<script lang="ts">
  import {
    branches,
    ForgeIpcError,
    prCreate,
    streamLog,
    type PrCreated,
  } from "$lib/ipc/client";
  import type { BranchInfo } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";
  import { openExternal } from "$lib/stores/forge.svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { prNumberFromUrl } from "./forgeModel";

  let {
    repoId,
    open,
    onClose,
    onCreated,
  }: {
    repoId: string;
    /** Rendered only when true — mounting is opening. */
    open: boolean;
    onClose: () => void;
    /** Called after a successful create (owner refreshes; dialog stays open). */
    onCreated: (created: PrCreated) => void;
  } = $props();

  let branchList = $state<BranchInfo[]>([]);
  let base = $state("");
  let title = $state("");
  let body = $state("");
  let draft = $state(false);
  let loading = $state(false);
  let creating = $state(false);
  let error = $state<string | null>(null);
  /** Set on success (form switches to the created view). */
  let created = $state<PrCreated | null>(null);
  /** Set when gh reports an existing PR for the branch. */
  let existingUrl = $state<string | null>(null);
  /** Recent commits handed to the AI layer as context. */
  let commits = $state<{ sha: string; summary: string }[]>([]);
  let aiPending = $state(false);
  /** Inline failure from the last AI generation (cleared on retry/success). */
  let aiError = $state<string | null>(null);

  let seq = 0;

  $effect(() => {
    if (!open) return;
    const current = ++seq;
    branchList = [];
    base = "";
    title = "";
    body = "";
    draft = false;
    creating = false;
    error = null;
    created = null;
    existingUrl = null;
    commits = [];
    aiPending = false;
    aiError = null;
    loading = true;

    branches(repoId)
      .then((list) => {
        if (current !== seq) return;
        branchList = list;
        base = defaultBase(list);
        loading = false;
      })
      .catch((err: unknown) => {
        if (current !== seq) return;
        error = err instanceof Error ? err.message : String(err);
        loading = false;
      });

    // First log page → up to 10 recent commit summaries as the AI hint.
    const buffer: { sha: string; summary: string }[] = [];
    streamLog(
      repoId,
      { regex: false, refs: [], follow: false },
      (page) => {
        for (const commit of page.commits) {
          if (buffer.length >= 10) break;
          buffer.push({ sha: commit.sha, summary: commit.summary });
        }
      },
    )
      .then(() => {
        if (current === seq) commits = buffer;
      })
      .catch(() => {
        // AI hint is optional; creation works without it.
      });
  });

  // AI result listener (only while open). Failure events clear the wait and
  // show the reason inline — without this the button sticks on "Waiting…".
  $effect(() => {
    if (!open) return;
    const unregisterResult = onUiEvent("ai-pr-result", (detail) => {
      if (detail.subject.trim()) {
        title = detail.subject.trim();
      }
      body = detail.body;
      aiPending = false;
      aiError = null;
    });
    const unregisterError = onUiEvent("ai-pr-error", (detail) => {
      aiPending = false;
      aiError =
        detail.message.length > 0 ? detail.message : "AI generation failed";
    });
    return () => {
      unregisterResult();
      unregisterError();
    };
  });

  /** Base options: remote-tracked branches first (fallback: all). */
  const baseOptions = $derived.by(() => {
    const tracked = branchList.filter((b) => b.upstream);
    return (tracked.length > 0 ? tracked : branchList).map((b) => b.name);
  });

  /** Existing-PR number for the already-exists view (URL-only survivor). */
  const existingNumber = $derived(prNumberFromUrl(existingUrl));

  /** "main"/"master" when present, else the first option. */
  function defaultBase(list: BranchInfo[]): string {
    const tracked = list.filter((b) => b.upstream);
    const names = (tracked.length > 0 ? tracked : list).map((b) => b.name);
    return names.includes("main") ? "main" : names.includes("master") ? "master" : (names[0] ?? "");
  }

  function requestAi(): void {
    aiPending = true;
    aiError = null;
    emitUiEvent("ai-generate-pr", { repoId, commits });
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !creating) {
      event.preventDefault();
      onClose();
    }
  }

  const canCreate = $derived(!!base && !!title.trim() && !creating && !loading && !created);

  async function create(): Promise<void> {
    if (!canCreate) return;
    creating = true;
    error = null;
    try {
      const pr = await prCreate(repoId, base, title.trim(), body, draft);
      toast(`PR #${pr.number} created`, { kind: "success" });
      created = pr;
      onCreated(pr);
    } catch (err) {
      if (err instanceof ForgeIpcError) {
        if (err.kind === "AlreadyExists") {
          existingUrl = err.url;
        } else if (err.kind === "NoGh") {
          error = "gh CLI not found — install it from https://cli.github.com/.";
        } else if (err.kind === "NotAuthed") {
          error = "gh is not authenticated — run `gh auth login` in a terminal.";
        } else {
          error = err.message;
        }
      } else {
        error = err instanceof Error ? err.message : String(err);
      }
    } finally {
      creating = false;
    }
  }

  async function openCreated(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }
</script>

{#if open}
  <Dialog.Root {open}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/45" role="presentation" />
    <Dialog.Content
      class="dialog flex max-h-[calc(100vh-4rem)] w-[min(34rem,calc(100vw-3rem))] max-w-none gap-2.5 overflow-y-auto rounded-lg border border-border bg-card px-5 py-4 text-popover-foreground shadow-xl sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="create-pr-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <Dialog.Title id="create-pr-title" class="m-0 text-base font-semibold">
        Create pull request
      </Dialog.Title>

      {#if created}
        {@const done = created}
        <p class="created-line m-0 font-semibold" aria-live="polite">
          PR #{done.number} created successfully.
        </p>
        <p class="created-url m-0 truncate font-mono text-[0.72rem] text-muted-foreground" title={done.url}>
          {done.url}
        </p>
        <div class="actions mt-1 flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            class="confirm flex-none rounded-full px-4 text-[0.7813rem]"
            type="button"
            onclick={() => void openCreated(done.url)}
          >
            Open PR
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="cancel flex-none rounded-full px-4 text-[0.7813rem] text-muted-foreground"
            type="button"
            onclick={onClose}
          >
            Done
          </Button>
        </div>
      {:else if existingUrl !== null}
        {@const url = existingUrl}
        <p class="created-line m-0 font-semibold" role="alert">
          A pull request already exists for this branch
          {existingNumber !== null ? `(#{existingNumber})` : ""}.
        </p>
        <p class="created-url m-0 truncate font-mono text-[0.72rem] text-muted-foreground" title={url}>
          {url}
        </p>
        <div class="actions mt-1 flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            class="confirm flex-none rounded-full px-4 text-[0.7813rem]"
            type="button"
            onclick={() => void openCreated(url)}
          >
            Open PR
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="cancel flex-none rounded-full px-4 text-[0.7813rem] text-muted-foreground"
            type="button"
            onclick={onClose}
          >
            Close
          </Button>
        </div>
      {:else}
        <label class="field flex flex-col gap-1">
          <span class="label text-[0.72rem] text-muted-foreground">Base branch</span>
          <select
            class="rounded-sm border border-border bg-background px-[0.45rem] py-[0.3rem] text-[0.8125rem] text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
            bind:value={base}
            disabled={loading || baseOptions.length === 0}
          >
            {#each baseOptions as name (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>
        </label>

        <div class="ai-row flex flex-wrap items-center gap-2">
          <Button
            variant="secondary"
            size="xs"
            class="tb self-start rounded-full px-3 text-[0.72rem]"
            type="button"
            onclick={requestAi}
            disabled={loading || aiPending}
          >
            {aiPending ? "Waiting for AI…" : "Generate title/body"}
          </Button>
          {#if aiPending}
            <span class="ai-hint text-[0.6875rem] text-muted-foreground [overflow-wrap:anywhere]">
              Waiting for the AI layer — type to fill manually.
            </span>
          {:else if aiError}
            <span
              class="ai-hint ai-fail text-[0.6875rem] text-destructive [overflow-wrap:anywhere]"
              role="alert"
            >{aiError}</span>
          {/if}
        </div>

        <label class="field flex flex-col gap-1">
          <span class="label text-[0.72rem] text-muted-foreground">Title</span>
          <Input
            class="h-auto rounded-sm px-[0.45rem] py-[0.3rem] text-[0.8125rem] focus-visible:-outline-offset-1"
            type="text"
            bind:value={title}
            placeholder="PR title"
            oninput={() => (aiPending = false)}
          />
        </label>

        <label class="field flex flex-col gap-1">
          <span class="label text-[0.72rem] text-muted-foreground">Description</span>
          <Textarea
            class="h-auto min-h-0 resize-y rounded-sm px-[0.45rem] py-[0.3rem] text-[0.8125rem] focus-visible:-outline-offset-1 md:text-[0.8125rem]"
            rows={7}
            bind:value={body}
            placeholder="What does this PR change?"
            oninput={() => (aiPending = false)}
          ></Textarea>
        </label>

        <label
          class="draft flex cursor-pointer items-center gap-1.5 text-xs text-muted-foreground"
          title="Open the PR as a draft (no reviewers requested)"
        >
          <Checkbox bind:checked={draft} />
          <span>Draft</span>
        </label>

        {#if error}
          <p class="error m-0 text-xs text-destructive" role="alert">{error}</p>
        {/if}

        <div class="actions mt-1 flex flex-wrap items-center gap-2">
          <span class="hint min-w-[10rem] flex-1 text-[0.6875rem] text-muted-foreground">
            Runs `gh pr create` with your gh credentials.
          </span>
          <Button
            size="sm"
            class="confirm flex-none rounded-full px-4 text-[0.7813rem]"
            type="button"
            disabled={!canCreate}
            onclick={() => void create()}
          >
            {creating ? "Creating…" : "Create PR"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="cancel flex-none rounded-full px-4 text-[0.7813rem] text-muted-foreground"
            type="button"
            disabled={creating}
            onclick={onClose}
          >
            Cancel
          </Button>
        </div>
      {/if}
    </Dialog.Content>
  </Dialog.Root>
{/if}
