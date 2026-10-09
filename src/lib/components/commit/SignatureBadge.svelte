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
  <span
    class="sig-badge {view.tone}"
    title={view.title}
    data-sha={sha}
  >{view.label}</span>
{/if}

<style>
  .sig-badge {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.45rem;
    font-size: 0.625rem;
    line-height: 1.5;
    letter-spacing: 0.02em;
    white-space: nowrap;
    user-select: none;
  }

  .sig-badge.verified {
    border-color: transparent;
    background: color-mix(in srgb, #2e7d32 18%, transparent);
    color: #2e7d32;
    font-weight: 600;
  }

  .sig-badge.bad {
    border-color: transparent;
    background: color-mix(in srgb, var(--m3-error, #ba1a1a) 15%, transparent);
    color: var(--m3-error, #ba1a1a);
    font-weight: 600;
  }

  .sig-badge.unverifiable {
    border-color: transparent;
    background: color-mix(in srgb, #b26a00 16%, transparent);
    color: #b26a00;
  }

  .sig-badge.neutral {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }
</style>
