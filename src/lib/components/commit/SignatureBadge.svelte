<script lang="ts">
  /**
   * SignatureBadge (M12, lane C) — a compact commit-signature verification
   * chip. On mount it resolves `commitSignature(repoId, sha)` and renders:
   *
   *   unsigned            → neutral "Unsigned" (hidden by default — set
   *                         `hideUnsigned={false}` to show it)
   *   gpg  + valid        → green  "Verified"
   *   ssh  + valid        → green  "Verified (SSH)"
   *   signed + invalid    → red    "Bad signature"
   *   signed + unverifiable → amber "Unverifiable"
   *
   * The hover tooltip carries the backend's trimmed verification output
   * (`detail`). IPC failures render NOTHING — the badge is decorative and
   * must never break a commit row. While loading it also renders nothing
   * (no layout-ja "…" placeholder).
   *
   * Mount anywhere: `<SignatureBadge {repoId} {sha} />` (HistoryView's
   * commit detail mounts it; also fit for CommitBar and lists).
   */
  import { commitSignature } from "$lib/ipc/client";
  import type { CommitSignature } from "$lib/ipc/types";
  import { Badge } from "$lib/components/ui/badge/index.js";

  let {
    repoId,
    sha,
    hideUnsigned = true,
  }: {
    repoId: string;
    sha: string;
    /** Render nothing for unsigned commits (default: hide). */
    hideUnsigned?: boolean;
  } = $props();

  let sig = $state<CommitSignature | null>(null);
  let failed = $state(false);
  let reqToken = 0;

  $effect(() => {
    void repoId;
    void sha;
    const token = ++reqToken;
    sig = null;
    failed = false;
    commitSignature(repoId, sha)
      .then((result) => {
        if (token !== reqToken) return;
        sig = result;
      })
      .catch(() => {
        if (token !== reqToken) return;
        failed = true; // decorative: render nothing
      });
  });

  type BadgeView = {
    tone: "verified" | "bad" | "unverifiable" | "neutral";
    label: string;
    title: string;
  } | null;

  /** Tone → restyling utilities (status colors keep the `--app-*` tokens). */
  const TONE_CLASS: Record<Exclude<BadgeView, null>["tone"], string> = {
    verified:
      "border-transparent bg-[var(--app-added-bg)] font-semibold text-[var(--app-added)]",
    bad: "border-transparent bg-[color-mix(in_srgb,var(--destructive)_15%,transparent)] font-semibold text-destructive",
    unverifiable: "border-transparent bg-[var(--app-modified-bg)] text-[var(--app-modified)]",
    neutral: "border-border text-muted-foreground",
  };

  const view = $derived.by((): BadgeView => {
    return badgeView(sig, failed, hideUnsigned);
  });

  /**
   * Pure mapping (exercised through the component tests):
   * null = render nothing.
   */
  function badgeView(
    s: CommitSignature | null,
    errored: boolean,
    hide: boolean,
  ): BadgeView {
    if (errored || !s) return null;
    if (!s.signed) {
      if (hide) return null;
      return { tone: "neutral", label: "Unsigned", title: "This commit is not signed" };
    }
    if (s.valid === true) {
      return {
        tone: "verified",
        label: s.kind === "ssh" ? "Verified (SSH)" : "Verified",
        title: s.detail || "Signature verified",
      };
    }
    if (s.valid === false) {
      return {
        tone: "bad",
        label: "Bad signature",
        title: s.detail || "Signature verification failed",
      };
    }
    return {
      tone: "unverifiable",
      label: "Unverifiable",
      title: s.detail || "Signature could not be verified here",
    };
  }
</script>

{#if view}
  <Badge
    variant="outline"
    class={"sig-badge " + view.tone + " " + TONE_CLASS[view.tone] +
      " h-auto flex-none items-center gap-1 rounded-full px-[0.45rem] text-[0.625rem] leading-[1.5] tracking-[0.02em] whitespace-nowrap select-none"}
    title={view.title}
    data-sha={sha}
  >{view.label}</Badge>
{/if}
