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
  import { Spinner } from "$lib/components/m3";
  import Icon from "$lib/components/icons/Icon.svelte";
  import iconOpen from "@ktibow/iconset-material-symbols/open-in-new";
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

<div class="preview" role="figure" aria-label="Preview of {path}">
  {#if isImage}
    {#if phase === "loading"}
      <p class="state" role="status"><Spinner size={14} /> Loading image…</p>
    {:else if phase === "error"}
      <p class="state state-error" role="alert">{errorText}</p>
    {:else if url !== null}
      <!-- svelte-ignore a11y_img_redundant_alt -->
      <img
        class="image"
        src={url}
        alt={path}
        onload={(event) => {
          const el = event.currentTarget as HTMLImageElement;
          naturalSize = { w: el.naturalWidth, h: el.naturalHeight };
        }}
      />
      <p class="caption">
        {formatSize(meta.size)}{naturalSize ? ` · ${naturalSize.w}×${naturalSize.h}` : ""}
      </p>
    {/if}
  {:else}
    <div class="card">
      <p class="ctitle">{meta.kind === "binary" ? "Binary file" : "No inline preview"}</p>
      <p class="ctext">
        {path} · {formatSize(meta.size)}{meta.readonly ? " · read-only" : ""}
      </p>
      <button class="cbtn" type="button" onclick={openExternally}>
        <Icon icon={iconOpen} size={14} />
        Open with system app
      </button>
    </div>
  {/if}
</div>

<style>
  .preview {
    height: 100%;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    padding: 1.5rem;
    background: var(--m3-surface);
  }

  .image {
    max-width: min(100%, 64rem);
    max-height: 100%;
    object-fit: contain;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-small, 8px);
    background:
      repeating-conic-gradient(
        var(--m3-surface-container) 0% 25%,
        var(--m3-surface) 0% 50%
      )
      50% / 16px 16px;
  }

  .caption {
    margin: 0;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant);
    font-variant-numeric: tabular-nums;
  }

  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.45rem;
    padding: 1.4rem 1.8rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    text-align: center;
  }

  .ctitle {
    margin: 0;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .ctext {
    margin: 0;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant);
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
  }

  .cbtn {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    margin-top: 0.3rem;
    padding: 0.3rem 0.85rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.78rem;
    cursor: pointer;
  }

  .cbtn:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .cbtn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .state {
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .state-error {
    color: var(--m3-error);
  }
</style>
