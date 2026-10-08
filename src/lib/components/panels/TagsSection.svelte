<!--
  TagsSection (M9 F3) — collapsible tag manager inside BranchPanel.

  Lists tags (annotated chips + tagger/date + message tooltip), creates
  lightweight or annotated tags (signed via the git CLI), deletes with a
  confirm, and pushes all tags to a remote. Every mutation reloads and
  notifies the owner.
-->
<script lang="ts">
  import {
    pushRepo,
    remotes,
    tagCreate,
    tagCreateSigned,
    tagDelete,
    tagList,
  } from "$lib/ipc/client";
  import type { RemoteInfo, TagInfo } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import { formatRelativeTime } from "$lib/stores/history-logic";

  let {
    repoId,
    targets = [],
    onMutated = undefined,
  }: {
    repoId: string;
    /** Create-target suggestions (branch names + HEAD). */
    targets?: string[];
    onMutated?: () => void;
  } = $props();

  let open = $state(false);
  let list = $state<TagInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Create form
  let newName = $state("");
  let newTarget = $state("");
  let newMessage = $state("");
  let newSigned = $state(false);

  let deleting = $state<TagInfo | null>(null);
  let pushing = $state(false);

  $effect(() => {
    void repoId;
    newName = "";
    newTarget = "";
    newMessage = "";
    newSigned = false;
    if (open) void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      list = await tagList(repoId);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function after(): void {
    void reload();
    onMutated?.();
  }

  async function onCreate(): Promise<void> {
    const name = newName.trim();
    if (!name) return;
    try {
      if (newSigned) {
        await tagCreateSigned(
          repoId,
          name,
          newMessage.trim() || `tag ${name}`,
          newTarget.trim() || undefined,
        );
      } else {
        await tagCreate(
          repoId,
          name,
          newTarget.trim() || undefined,
          newMessage.trim() || undefined,
        );
      }
      toast(`Created tag ${name}`, { kind: "success" });
      newName = "";
      newMessage = "";
      after();
    } catch (err) {
      toast(
        `Tag create failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  async function onDelete(): Promise<void> {
    const tag = deleting;
    if (!tag) return;
    deleting = null;
    try {
      await tagDelete(repoId, tag.name);
      toast(`Deleted tag ${tag.name}`, { kind: "success" });
      after();
    } catch (err) {
      toast(
        `Tag delete failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  async function onPushTags(): Promise<void> {
    if (pushing) return;
    pushing = true;
    try {
      const list = await remotes(repoId);
      const remote: RemoteInfo | undefined =
        list.find((r) => r.name === "origin") ?? list[0];
      if (!remote) {
        toast("No remote configured — cannot push tags", { kind: "error" });
        return;
      }
      const stats = await pushRepo(repoId, {
        remote: remote.name,
        branch: "",
        force: false,
        force_with_lease: false,
        set_upstream: false,
        refs: [],
        tags: true,
        delete: false,
      });
      toast(
        `Pushed ${stats.updated_refs.length} tag${stats.updated_refs.length === 1 ? "" : "s"} to ${remote.name}`,
        { kind: "success" },
      );
      after();
    } catch (err) {
      toast(
        `Push tags failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      pushing = false;
    }
  }
</script>

<section class="tags" aria-label="Tags">
  <div class="head">
    <button
      class="disclose"
      type="button"
      aria-expanded={open}
      onclick={() => (open = !open)}
    >
      <span class="chev" aria-hidden="true">{open ? "▾" : "▸"}</span>
      Tags {list.length > 0 ? `(${list.length})` : ""}
    </button>
    {#if open}
      <button class="tb" type="button" onclick={() => void onPushTags()} disabled={pushing}>
        {pushing ? "Pushing…" : "Push tags"}
      </button>
    {/if}
  </div>

  {#if open}
    <form
      class="create"
      aria-label="Create tag"
      onsubmit={(e) => {
        e.preventDefault();
        void onCreate();
      }}
    >
      <input
        class="name"
        type="text"
        placeholder="New tag name"
        aria-label="New tag name"
        bind:value={newName}
      />
      <label class="from">
        <span class="fl">at</span>
        <select bind:value={newTarget} aria-label="Tag target">
          <option value="">HEAD</option>
          {#each targets as target (target)}
            <option value={target}>{target}</option>
          {/each}
        </select>
      </label>
      <input
        class="msg"
        type="text"
        placeholder="Message (makes it annotated)"
        aria-label="Tag message"
        bind:value={newMessage}
      />
      <label class="toggle" title="Create a signed tag (git tag -s; requires signing config)">
        <input type="checkbox" bind:checked={newSigned} />
        <span>sign</span>
      </label>
      <button class="go" type="submit" disabled={!newName.trim()}>Create</button>
    </form>

    {#if error}
      <p class="state error" role="alert">{error}</p>
    {:else if !loading && list.length === 0}
      <p class="state">No tags.</p>
    {:else}
      <ul class="list" role="list" aria-label="Tags">
        {#each list as tag (tag.name)}
          <li class="row">
            <span class="tname" title={tag.message ?? tag.name}>{tag.name}</span>
            {#if tag.annotated}<span class="chip">annotated</span>{/if}
            {#if tag.tagger}
              <span class="meta" title={tag.message ?? ""}>
                {tag.tagger.name} · {formatRelativeTime(tag.tagger.time)}
              </span>
            {/if}
            <span class="sha">{tag.target.slice(0, 7)}</span>
            <span class="actions">
              <button
                class="tb danger"
                type="button"
                aria-label={`Delete tag ${tag.name}`}
                onclick={() => (deleting = tag)}
              >
                Delete
              </button>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<ConfirmDialog
  bind:open={
    () => deleting !== null,
    (v) => {
      if (!v) deleting = null;
    }
  }
  title={deleting ? `Delete tag ${deleting.name}?` : ""}
  message="The tag ref is removed from this repository. Commits are untouched; a pushed tag must be deleted on the remote separately."
  confirmLabel="Delete tag"
  danger
  onConfirm={() => void onDelete()}
/>

<style>
  .tags {
    border-top: 1px solid var(--m3-outline-variant, transparent);
    margin-top: 0.25rem;
    flex: none;
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.25rem 0.5rem;
  }

  .disclose {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    border: none;
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.75rem;
    font-weight: 600;
    cursor: pointer;
    padding: 0.15rem 0.25rem;
  }

  .chev {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .tb {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb.danger {
    color: var(--m3-error, inherit);
  }

  .create {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0 0.5rem 0.375rem;
    flex-wrap: wrap;
  }

  .create input,
  select {
    font: inherit;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  .name {
    width: 8rem;
  }

  .msg {
    flex: 1;
    min-width: 8rem;
  }

  .from {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  select {
    max-width: 8rem;
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .toggle input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .go {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .go:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .state {
    padding: 0.25rem 0.75rem;
    margin: 0;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .error {
    color: var(--m3-error, inherit);
  }

  .list {
    margin: 0;
    padding: 0 0 0.5rem;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.15rem 0.5rem;
    font-size: 0.72rem;
  }

  .row:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tname {
    font-family: ui-monospace, Consolas, monospace;
    font-weight: 600;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip {
    flex: none;
    padding: 0 0.35rem;
    border: 1px solid var(--m3-outline-variant, currentColor);
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.625rem;
    color: var(--m3-on-surface-variant, inherit);
  }

  .meta {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .sha {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .actions {
    flex: none;
  }
</style>
