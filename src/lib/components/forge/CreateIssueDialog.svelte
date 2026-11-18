<!--
  Create-issue dialog (Phase 6 batch A) — title + body issue creation.

  Follows {@link CreatePrDialog}'s shell patterns (same ui/dialog chrome,
  same reset-on-open effect, same ForgeIpcError mapping) minus the branch
  picker, draft flag and AI bridge: an issue needs only a title and an
  optional description. Create runs `issueCreate` (120 s backend timeout):
  success toasts, calls `onCreated` (the owner refreshes the issue list;
  the dialog stays open showing the URL + "Open on GitHub" until Done).
  Cancel / Escape / Done → `onClose()`; the owner flips `open`.
-->
<script lang="ts">
  import { ForgeIpcError, issueCreate, type IssueCreated } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import { openExternal } from "$lib/stores/forge.svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { describeError } from "$lib/errors";

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
    onCreated: (created: IssueCreated) => void;
  } = $props();

  let title = $state("");
  let body = $state("");
  let creating = $state(false);
  let error = $state<string | null>(null);
  /** Set on success (form switches to the created view). */
  let created = $state<IssueCreated | null>(null);

  let seq = 0;

  $effect(() => {
    if (!open) return;
    const current = ++seq;
    title = "";
    body = "";
    creating = false;
    error = null;
    created = null;
  });

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !creating) {
      event.preventDefault();
      onClose();
    }
  }

  const canCreate = $derived(!!title.trim() && !creating && !created);

  async function create(): Promise<void> {
    if (!canCreate) return;
    creating = true;
    error = null;
    try {
      const issue = await issueCreate(repoId, title.trim(), body);
      toast(`Issue #${issue.number} created`, { kind: "success" });
      created = issue;
      onCreated(issue);
    } catch (err) {
      if (err instanceof ForgeIpcError) {
        if (err.kind === "NoGh") {
          error = "gh CLI not found — install it from https://cli.github.com/.";
        } else if (err.kind === "NotAuthed") {
          error = "gh is not authenticated — run `gh auth login` in a terminal.";
        } else {
          error = err.message;
        }
      } else {
        error = describeError(err);
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
      class="dialog flex max-h-[calc(100vh-4rem)] w-[min(34rem,calc(100vw-3rem))] max-w-none overflow-y-auto bg-card text-popover-foreground shadow-xl sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="create-issue-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <Dialog.Title id="create-issue-title" class="m-0 text-base font-semibold">
        Create issue
      </Dialog.Title>

      {#if created}
        {@const done = created}
        <p class="created-line m-0 font-semibold" aria-live="polite">
          Issue #{done.number} created successfully.
        </p>
        <Hint label={done.url}>
          {#snippet child({ props })}
            <p class="created-url m-0 truncate font-mono text-ui-meta text-muted-foreground" {...props}>
              {done.url}
            </p>
          {/snippet}
        </Hint>
        <div class="actions mt-1 flex flex-wrap items-center gap-2">
          <Button
            class="confirm flex-none rounded-full px-4"
            type="button"
            onclick={() => void openCreated(done.url)}
          >
            Open issue
          </Button>
          <Button
            variant="outline"
            class="cancel flex-none rounded-full px-4 text-muted-foreground"
            type="button"
            onclick={onClose}
          >
            Done
          </Button>
        </div>
      {:else}
        <Label class="field flex flex-col gap-1">
          <span class="label text-ui-meta text-muted-foreground">Title</span>
          <Input
            class="focus-visible:-outline-offset-1"
            type="text"
            bind:value={title}
            placeholder="Issue title"
          />
        </Label>

        <Label class="field flex flex-col gap-1">
          <span class="label text-ui-meta text-muted-foreground">Description</span>
          <Textarea
            class="min-h-0 resize-y focus-visible:-outline-offset-1"
            rows={7}
            bind:value={body}
            placeholder="What happened, what did you expect? (optional)"
          ></Textarea>
        </Label>

        {#if error}
          <p class="error m-0 text-xs text-destructive" role="alert">{error}</p>
        {/if}

        <div class="actions mt-1 flex flex-wrap items-center gap-2">
          <span class="hint min-w-[10rem] flex-1 text-ui-micro text-muted-foreground">
            Runs `gh issue create` with your gh credentials.
          </span>
          <Button
            class="confirm flex-none rounded-full px-4"
            type="button"
            disabled={!canCreate}
            onclick={() => void create()}
          >
            {creating ? "Creating…" : "Create issue"}
          </Button>
          <Button
            variant="outline"
            class="cancel flex-none rounded-full px-4 text-muted-foreground"
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
