<script lang="ts">
  import { untrack } from "svelte";
  import type { Issue, IssueMetadata } from "$lib/issues/model";
  import { issuesStore } from "$lib/stores/issues.svelte";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Label } from "$lib/components/ui/label/index.js";

  let {
    repoId,
    open,
    issue = null,
    onClose,
    onSaved,
  }: {
    repoId: string;
    open: boolean;
    issue?: Issue | null;
    onClose: () => void;
    onSaved: (issue: Issue) => void;
  } = $props();

  let title = $state("");
  let body = $state("");
  let labels = $state<string[]>([]);
  let assignees = $state<string[]>([]);
  let milestone = $state<number | null>(null);
  let metadata = $state<IssueMetadata | null>(null);
  let loadingMetadata = $state(false);
  let metadataError = $state<string | null>(null);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let resetKey = $state("");
  let loadSequence = 0;
  const titleId = $derived(`issue-editor-title-${repoId.replace(/[^a-zA-Z0-9_-]/g, "-")}`);
  const errorId = $derived(`${titleId}-error`);

  const editing = $derived(issue !== null);
  const dialogTitle = $derived(editing ? `Edit issue #${issue!.number}` : "New issue");

  $effect(() => {
    if (!open) {
      resetKey = "";
      return;
    }
    const key = `${repoId}:${issue?.id ?? "new"}`;
    if (resetKey === key) return;
    resetKey = key;
    title = issue?.title ?? "";
    body = issue?.body ?? "";
    labels = [...(issue?.labels.map((label) => label.name) ?? [])];
    assignees = [...(issue?.assignees.map((assignee) => assignee.login) ?? [])];
    milestone = issue?.milestone?.number ?? null;
    metadata = null;
    metadataError = null;
    error = null;
    saving = false;
    const sequence = ++loadSequence;
    loadingMetadata = true;
    untrack(() => {
      void issuesStore.loadMetadata(repoId).then(() => {
        if (sequence !== loadSequence) return;
        const entry = issuesStore.metadata(repoId);
        if (entry.state === "ready") {
          metadata = entry.data;
          metadataError = null;
        } else if (entry.state === "error") {
          metadataError = entry.message;
        }
        loadingMetadata = false;
      });
    });
  });

  function toggleLabel(name: string, checked: boolean): void {
    labels = checked ? [...new Set([...labels, name])] : labels.filter((entry) => entry !== name);
  }

  function toggleAssignee(login: string, checked: boolean): void {
    assignees = checked ? [...new Set([...assignees, login])] : assignees.filter((entry) => entry !== login);
  }

  async function save(): Promise<void> {
    if (saving || title.trim() === "") return;
    saving = true;
    error = null;
    const input = {
      title: title.trim(),
      body,
      labels,
      assignees,
      milestone,
    };
    try {
      const saved = issue === null
        ? await issuesStore.create(repoId, input)
        : await issuesStore.update(repoId, issue.number, input);
      toast(issue === null ? `Issue #${saved.number} created` : `Issue #${saved.number} updated`, { kind: "success" });
      onSaved(saved);
      onClose();
    } catch (err) {
      error = describeError(err);
    } finally {
      saving = false;
    }
  }
</script>

<DialogShell
  {open}
  labelledBy={titleId}
  describedBy={error === null ? undefined : errorId}
  width="40rem"
  onCancel={() => {
    if (!saving) onClose();
  }}
  onsubmit={() => void save()}
>
  <h2 class="sdlg-title" id={titleId}>{dialogTitle}</h2>
  <p class="sdlg-message">
    {editing ? "Update the issue details below." : "Create an issue in this repository."}
  </p>
  <div class="grid gap-3">
    <Label class="grid gap-1">
      <span>Title</span>
      <input
        class="h-8 rounded-sm border border-border bg-background px-2 text-ui-base text-foreground outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring"
        value={title}
        placeholder="Issue title"
        required
        maxlength={256}
        data-autofocus
        aria-invalid={error !== null && title.trim() === ""}
        oninput={(event) => (title = event.currentTarget.value)}
      />
    </Label>
    <Label class="grid gap-1">
      <span>Description</span>
      <textarea
        class="min-h-32 resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring"
        value={body}
        rows={7}
        placeholder="Describe the issue (Markdown supported)"
        oninput={(event) => (body = event.currentTarget.value)}
      ></textarea>
    </Label>
    <div class="grid gap-3 sm:grid-cols-2">
      <fieldset class="min-w-0 rounded-sm border border-border p-2">
        <legend class="px-1 text-ui-meta font-medium">Labels</legend>
        <div class="max-h-36 overflow-y-auto">
          {#if metadata !== null && metadata.labels.length > 0}
            {#each metadata.labels as label (label.id)}
              <label class="flex cursor-pointer items-center gap-2 py-0.5 text-ui-meta">
                <Checkbox
                  checked={labels.includes(label.name)}
                  onCheckedChange={(checked) => toggleLabel(label.name, checked)}
                  aria-label={label.name}
                />
                <span class="min-w-0 truncate">{label.name}</span>
              </label>
            {/each}
          {:else if !loadingMetadata && metadataError === null}
            <p class="m-0 text-ui-meta text-muted-foreground">No labels available.</p>
          {/if}
        </div>
      </fieldset>
      <fieldset class="min-w-0 rounded-sm border border-border p-2">
        <legend class="px-1 text-ui-meta font-medium">Assignees</legend>
        <div class="max-h-36 overflow-y-auto">
          {#if metadata !== null && metadata.assignees.length > 0}
            {#each metadata.assignees as assignee (assignee.id)}
              <label class="flex cursor-pointer items-center gap-2 py-0.5 text-ui-meta">
                <Checkbox
                  checked={assignees.includes(assignee.login)}
                  onCheckedChange={(checked) => toggleAssignee(assignee.login, checked)}
                  aria-label={assignee.login}
                />
                <span class="min-w-0 truncate">{assignee.login}</span>
              </label>
            {/each}
          {:else if !loadingMetadata && metadataError === null}
            <p class="m-0 text-ui-meta text-muted-foreground">No assignees available.</p>
          {/if}
        </div>
      </fieldset>
    </div>
    <Label class="grid max-w-xs gap-1">
      <span>Milestone</span>
      <select
        class="h-8 rounded-sm border border-border bg-background px-2 text-ui-base text-foreground"
        value={milestone === null ? "" : String(milestone)}
        onchange={(event) => (milestone = event.currentTarget.value === "" ? null : Number(event.currentTarget.value))}
      >
        <option value="">No milestone</option>
        {#each metadata?.milestones ?? [] as entry (entry.number)}
          <option value={entry.number}>{entry.title}{entry.state === "closed" ? " (closed)" : ""}</option>
        {/each}
      </select>
    </Label>
    {#if loadingMetadata}
      <p class="m-0 text-ui-meta text-muted-foreground" role="status">Loading issue metadata…</p>
    {:else if metadataError !== null}
      <div class="flex flex-wrap items-center gap-2 text-ui-meta text-destructive" role="alert">
        <span class="min-w-0 flex-1">Metadata could not be loaded: {metadataError}</span>
        <Button
          class="h-6 rounded-sm text-ui-micro"
          variant="outline"
          type="button"
          onclick={() => {
            const sequence = ++loadSequence;
            loadingMetadata = true;
            metadataError = null;
            untrack(() => {
              void issuesStore.loadMetadata(repoId).then(() => {
                if (sequence !== loadSequence) return;
                const entry = issuesStore.metadata(repoId);
                if (entry.state === "ready") metadata = entry.data;
                else if (entry.state === "error") metadataError = entry.message;
                loadingMetadata = false;
              });
            });
          }}
        >Retry metadata</Button>
      </div>
    {/if}
    {#if error !== null}
      <p class="m-0 break-words text-ui-meta text-destructive" id={errorId} role="alert">{error}</p>
    {/if}
  </div>
  {#snippet actions()}
    <button class="sdlg-secondary" type="button" disabled={saving} onclick={onClose}>Cancel</button>
    <button class="sdlg-primary" type="submit" disabled={saving || title.trim() === ""}>
      {saving ? "Saving…" : editing ? "Save changes" : "Create issue"}
    </button>
  {/snippet}
</DialogShell>
