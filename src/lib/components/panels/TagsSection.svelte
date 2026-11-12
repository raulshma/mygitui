<!--
  TagsSection (M9 F3) — collapsible tag manager inside BranchPanel.

  Lists tags (annotated chips + tagger/date + message tooltip), creates
  lightweight or annotated tags (signed via the git CLI), deletes with a
  confirm, and pushes all tags to a remote. Every mutation reloads and
  notifies the owner.
-->
<script lang="ts">
  import {
    archiveSpec,
    pickSaveFile,
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
  import { formatRelativeTime } from "$lib/git/format";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import EmptyState from "../layout/EmptyState.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

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

  const iconPushTags = $derived(appIcon("cloud-upload")!);
  const iconExport = $derived(appIcon("archive")!);
  const iconDelete = $derived(appIcon("delete")!);

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
    // Stale-response guard: the section is keep-alive across repo switches —
    // a slow tag list from repo A must never land after the switch.
    const id = repoId;
    loading = true;
    error = null;
    try {
      const fresh = await tagList(id);
      if (id !== repoId) return;
      list = fresh;
    } catch (err) {
      if (id !== repoId) return;
      error = err instanceof Error ? err.message : String(err);
    } finally {
      if (id === repoId) loading = false;
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

  /** M12: zip/tar export of the tag's tree via the save picker (the tag
   *  name is the `git archive` spec; mirrors RepoPanel's archive form). */
  async function onExportArchive(tag: TagInfo): Promise<void> {
    const destination = await pickSaveFile("Export archive", `${tag.name}.zip`, [
      { name: "Zip archive", extensions: ["zip"] },
      { name: "Tar archive", extensions: ["tar", "gz"] },
    ]);
    if (!destination) return;
    const format = destination.endsWith(".zip")
      ? "zip"
      : destination.endsWith(".tar")
        ? "tar"
        : "tar.gz";
    try {
      await archiveSpec(repoId, tag.name, format, destination);
      toast(`Archive written to ${destination}`, { kind: "success" });
    } catch (err) {
      toast(
        `Archive failed: ${err instanceof Error ? err.message : String(err)}`,
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

<section class="tags mt-1 shrink-0 border-t border-border" aria-label="Tags">
  <div class="head flex items-center justify-between gap-2 px-2 py-1">
    <Button
      class="h-auto gap-ui-3 px-1 py-0.5 text-xs font-semibold dark:hover:bg-muted/50"
      variant="ghost"
      type="button"
      aria-expanded={open}
      onclick={() => (open = !open)}
    >
      <span class="chev text-muted-foreground" aria-hidden="true">{open ? "▾" : "▸"}</span>
      Tags {list.length > 0 ? `(${list.length})` : ""}
    </Button>
    {#if open}
      <Button
        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Push tags"
        title="Push tags to the remote"
        disabled={pushing}
        onclick={() => void onPushTags()}
      >
        <Icon icon={iconPushTags} size={16} />
      </Button>
    {/if}
  </div>

  {#if open}
    <form
      class="create flex flex-wrap items-center gap-1.5 px-2 pb-1.5"
      aria-label="Create tag"
      onsubmit={(e) => {
        e.preventDefault();
        void onCreate();
      }}
    >
      <Input
        class="h-auto w-32 rounded-md px-2 py-0.5 text-ui-meta"
        type="text"
        placeholder="New tag name"
        aria-label="New tag name"
        bind:value={newName}
      />
      <label class="from flex items-center gap-1 text-ui-micro text-muted-foreground">
        <span class="fl">at</span>
        <select
          class="max-w-32 rounded-md border border-input bg-background px-2 py-0.5 font-[inherit] text-ui-meta text-foreground"
          bind:value={newTarget}
          aria-label="Tag target"
        >
          <option value="">HEAD</option>
          {#each targets as target (target)}
            <option value={target}>{target}</option>
          {/each}
        </select>
      </label>
      <Input
        class="h-auto min-w-32 flex-1 rounded-md px-2 py-0.5 text-ui-meta"
        type="text"
        placeholder="Message (makes it annotated)"
        aria-label="Tag message"
        bind:value={newMessage}
      />
      <Hint label="Create a signed tag (git tag -s; requires signing config)">
        {#snippet child({ props })}
          <label
            {...props}
            class="toggle flex cursor-pointer items-center gap-1 text-ui-meta text-muted-foreground"
          >
            <input type="checkbox" class="m-0 accent-primary" bind:checked={newSigned} />
            <span>sign</span>
          </label>
        {/snippet}
      </Hint>
      <Button
        class="h-auto rounded-full px-3 py-0.5 text-ui-meta"
        type="submit"
        disabled={!newName.trim()}
      >
        Create
      </Button>
    </form>

    {#if error}
      <p class="state m-0 px-3 py-1 text-xs text-destructive" role="alert"
        >{error}</p
      >
    {:else if loading && list.length === 0}
      <p class="state m-0 px-3 py-1 text-xs text-muted-foreground" role="status">
        <span class="inline-flex items-center gap-ui-3">
          <Spinner class="size-3" role="presentation" aria-hidden="true" />
          Loading tags…
        </span>
      </p>
    {:else if !loading && list.length === 0}
      <EmptyState title="No tags." />
    {:else}
      <ul class="list m-0 list-none p-0 pb-2" role="list" aria-label="Tags">
        {#each list as tag (tag.name)}
          <li class="row flex items-center gap-1.5 px-2 py-0.5 text-ui-meta hover:bg-muted/60">
            <Hint label={tag.message ?? tag.name}>
              {#snippet child({ props })}
                <span
                  {...props}
                  class="tname overflow-hidden text-ellipsis whitespace-nowrap font-mono font-semibold text-foreground"
                  >{tag.name}</span
                >
              {/snippet}
            </Hint>
            {#if tag.annotated}
              <Badge variant="outline" class="chip h-auto shrink-0 rounded-full px-1.5 py-0 text-ui-tiny text-muted-foreground"
                >annotated</Badge
              >
            {/if}
            {#if tag.tagger}
              {@const tagger = tag.tagger}
              <Hint label={tag.message ?? ""}>
                {#snippet child({ props })}
                  <span
                    {...props}
                    class="meta min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-muted-foreground">
                    {tagger.name} · {formatRelativeTime(tagger.time)}
                  </span>
                {/snippet}
              </Hint>
            {/if}
            <span class="sha shrink-0 font-mono text-muted-foreground"
              >{tag.target.slice(0, 7)}</span
            >
            <span class="actions shrink-0">
              <Hint label="Export this tag's tree as a zip/tar archive">
                {#snippet child({ props })}
                  <Button
                    {...props}
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Export archive of tag ${tag.name}`}
                    onclick={() => void onExportArchive(tag)}
                  >
                    <Icon icon={iconExport} size={16} />
                  </Button>
                {/snippet}
              </Hint>
              <Hint label="Delete this tag">
                {#snippet child({ props })}
                  <Button
                    {...props}
                    class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Delete tag ${tag.name}`}
                    onclick={() => (deleting = tag)}
                  >
                    <Icon icon={iconDelete} size={16} />
                  </Button>
                {/snippet}
              </Hint>
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

