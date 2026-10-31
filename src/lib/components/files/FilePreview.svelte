<!--
  FilePreview — non-text files in the file workspace: images render inline
  (bytes over `repo_read_file` → blob URL, revoked on destroy), binary and
  oversized files show an info card with a system-app handoff
  (`plugin-opener.openPath`) instead of garbage text.
-->
<script lang="ts">
  import type { FileMeta } from "$lib/ipc/types";
  import { readFile } from "$lib/ipc/client";
  import { openPath } from "@tauri-apps/plugin-opener";
  import { isTauri } from "$lib/entry/dragdrop";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { toast } from "$lib/toast";

  let {
    repoId,
    root,
    path,
    meta,
  }: {
    repoId: string;
    /** Worktree root (absolute, for the system-app handoff). */
    root: string;
    path: string;
    meta: FileMeta;
  } = $props();

  const iconOpen = $derived(appIcon("open-in-new")!);

  let phase = $state<"loading" | "ready" | "error">("loading");
  let errorText = $state("");
  let url = $state<string | null>(null);
  let naturalSize = $state<{ w: number; h: number } | null>(null);

  const MIME: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    bmp: "image/bmp",
    svg: "image/svg+xml",
    ico: "image/x-icon",
    avif: "image/avif",
  };

  const ext = $derived.by(() => {
    const name = path.slice(path.lastIndexOf("/") + 1);
    const dot = name.lastIndexOf(".");
    return dot === -1 ? "" : name.slice(dot + 1).toLowerCase();
  });
  const isImage = $derived(meta.kind === "image" && MIME[ext] !== undefined);

  $effect(() => {
    if (!isImage) {
      phase = "ready";
      return;
    }
    let alive = true;
    let objectUrl: string | null = null;
    phase = "loading";
    void (async () => {
      try {
        const bytes = await readFile(repoId, path);
        if (!alive) return;
        objectUrl = URL.createObjectURL(
          new Blob([new Uint8Array(bytes)], { type: MIME[ext] }),
        );
        url = objectUrl;
        phase = "ready";
      } catch (err) {
        if (!alive) return;
        errorText = String(err);
        phase = "error";
      }
    })();
    return () => {
      alive = false;
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl);
    };
  });

  function formatSize(size: number): string {
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }

  function openExternally(): void {
    if (!isTauri()) return;
    openPath(`${root}/${path}`).catch((err) =>
      toast(String(err), { kind: "error" }),
    );
  }
</script>

<div class="preview flex h-full min-h-0 flex-col items-center justify-center gap-[0.4rem] overflow-auto bg-card p-6" role="figure" aria-label="Preview of {path}">
  {#if isImage}
    {#if phase === "loading"}
      <p class="state inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground" role="status"><Spinner class="size-[14px]" role="presentation" aria-hidden="true" /> Loading image…</p>
    {:else if phase === "error"}
      <p class="state state-error inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-destructive" role="alert">{errorText}</p>
    {:else if url !== null}
      <!-- svelte-ignore a11y_img_redundant_alt -->
      <img
        class="image max-h-full max-w-[min(100%,64rem)] border border-border rounded-sm bg-[repeating-conic-gradient(var(--muted)_0%_25%,var(--card)_0%_50%)] bg-[position:50%] bg-[size:16px_16px] object-contain"
        src={url}
        alt={path}
        onload={(event) => {
          const el = event.currentTarget as HTMLImageElement;
          naturalSize = { w: el.naturalWidth, h: el.naturalHeight };
        }}
      />
      <p class="caption m-0 text-[0.6875rem] tabular-nums text-muted-foreground">
        {formatSize(meta.size)}{naturalSize ? ` · ${naturalSize.w}×${naturalSize.h}` : ""}
      </p>
    {/if}
  {:else}
    <div class="card flex flex-col items-center gap-[0.45rem] rounded-md border border-border bg-card px-[1.8rem] py-[1.4rem] text-center">
      <p class="ctitle m-0 text-sm font-medium text-foreground">{meta.kind === "binary" ? "Binary file" : "No inline preview"}</p>
      <p class="ctext m-0 font-mono text-xs text-muted-foreground">
        {path} · {formatSize(meta.size)}{meta.readonly ? " · read-only" : ""}
      </p>
      <Button
        class="cbtn mt-[0.3rem] h-auto gap-[0.35rem] rounded-full px-[0.85rem] py-[0.3rem] text-[0.78rem] font-normal"
        variant="outline"
        type="button"
        onclick={openExternally}
      >
        <Icon icon={iconOpen} size={14} />
        Open with system app
      </Button>
    </div>
  {/if}
</div>
