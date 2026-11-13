<script lang="ts">
  /**
   * AiMarkdown (M13) — renders AI text safely.
   *
   * While `streaming`, the text renders as plain pre-wrapped characters
   * with a blinking cursor: markdown-it re-parsing on every delta would
   * burn CPU on the one thing the user is watching. When the text
   * settles, it renders through the app's sanitized pipeline
   * (`markdownModel.renderMarkdown` — markdown-it + DOMPurify, lazily
   * imported so the AI components stay out of the markdown chunk's
   * dependency graph until first use). External links open through the
   * opener plugin — the webview itself never navigates.
   *
   * This component contains NO AI logic — the assistant panel, the
   * explain panel, the review modal and the new M13 features all share
   * it so AI output looks and behaves the same everywhere.
   */
  import { openUrl } from "@tauri-apps/plugin-opener";

  let {
    text,
    streaming = false,
    /** Extra classes on the rendered root (typography hooks). */
    class: klass = "",
  }: {
    text: string;
    streaming?: boolean;
    class?: string;
  } = $props();

  let html = $state<string | null>(null);

  // Markdown render once settled; plain text while streaming (see doc).
  $effect(() => {
    if (streaming) {
      html = null;
      return;
    }
    const source = text;
    let cancelled = false;
    void import("$lib/components/files/markdownModel").then((m) => {
      if (!cancelled) html = m.renderMarkdown(source).html;
    });
    return () => {
      cancelled = true;
    };
  });

  /** External links go to the OS browser; the webview never navigates. */
  function onClick(event: MouseEvent): void {
    const anchor = (event.target as HTMLElement | null)?.closest("a") ?? null;
    if (anchor === null) return;
    const href = anchor.getAttribute("href") ?? "";
    if (/^https?:/i.test(href)) {
      event.preventDefault();
      void openUrl(href).catch(() => {});
    }
  }
</script>

{#if html !== null}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="ai-md {klass} [&_a]:text-primary [&_a]:underline [&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-2 [&_blockquote]:text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-px [&_h1]:mb-1 [&_h1]:mt-2 [&_h1]:text-base [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:mt-2 [&_h2]:font-semibold [&_h3]:mb-1 [&_h3]:mt-1 [&_h3]:font-medium [&_hr]:border-border [&_li]:my-px [&_ol]:list-decimal [&_ol]:pl-5 [&_p]:my-1 [&_pre]:my-1 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted [&_pre]:p-2 [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_strong]:font-semibold [&_table]:w-full [&_td]:border [&_td]:border-border [&_td]:px-1 [&_td]:py-px [&_th]:border [&_th]:border-border [&_th]:px-1 [&_ul]:list-disc [&_ul]:pl-5"
    onclick={onClick}
  >
    {@html html}
  </div>
{:else}
  <div class={klass}>
    <p class="m-0 whitespace-pre-wrap leading-[1.45] [overflow-wrap:anywhere]">{text}</p>
    {#if streaming}
      <span class="ml-px inline-block animate-pulse text-primary" aria-hidden="true">▍</span>
    {/if}
  </div>
{/if}
