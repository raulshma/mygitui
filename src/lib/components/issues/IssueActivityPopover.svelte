<script lang="ts">
  import { issueActivity } from "$lib/issues/activity.svelte";
  import { issueSubscriptions } from "$lib/issues/subscriptions.svelte";
  import { describeError } from "$lib/errors";
  import { floating } from "$lib/components/overlay/floating";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { badgeCount } from "$lib/components/tabBadge";
  import { normalizeIssueRoot } from "$lib/issues/prefs";

  let { repoId, root }: { repoId: string; root: string } = $props();

  const iconActivity = $derived(appIcon("history"));
  const iconRefresh = $derived(appIcon("refresh"));
  let open = $state(false);
  let anchor = $state<HTMLButtonElement | null>(null);
  let panel = $state<HTMLDivElement | null>(null);
  let saving = $state(false);
  let retrying = $state(false);
  let error = $state<string | null>(null);
  let restoreFocus = $state(false);
  const sync = $derived(issueSubscriptions.state(root));
  let activityRoot = $state("");
  const issueUnreadByRoot = $derived(issueActivity.unreadCounts());
  // The activity store hydrates an outbox on first read. Keep that cache write
  // in an effect and only derive from roots already loaded by the effect.
  $effect(() => {
    const nextRoot = normalizeIssueRoot(root);
    issueActivity.entries(root);
    activityRoot = nextRoot;
  });
  const unread = $derived(issueActivity.entries(activityRoot).filter((event) => event.readAt === null));
  const unreadCount = $derived(issueUnreadByRoot[activityRoot] ?? 0);
  const enabled = $derived(sync.newIssues || sync.newComments);
  const phaseLabel = $derived.by(() => {
    switch (sync.phase) {
      case "baseline-pending": return "Establishing a baseline…";
      case "ready": return "Up to date";
      case "auth-error": return "Authentication required";
      case "offline": return "Offline — will retry when connected";
      case "storage-error": return "Activity storage unavailable";
      case "paused":
        if (sync.lastError === "rate-limited") return "Paused by GitHub rate limit";
        if (sync.lastError === "auth-error") return "Authentication required";
        if (sync.lastError === "offline") return "Offline";
        return sync.lastError === "request-failed" ? "Sync paused after a request failure" : "Sync paused";
    }
  });

  $effect(() => {
    if (!open || anchor === null || panel === null) return;
    const action = floating(panel, { anchor, alignEnd: true, gap: 5 });
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Node && (panel?.contains(target) || anchor?.contains(target))) return;
      open = false;
    };
    const onKeydown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      event.stopPropagation();
      open = false;
      anchor?.focus();
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeydown, true);
    requestAnimationFrame(() => panel?.focus());
    return () => {
      action.destroy();
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeydown, true);
    };
  });

  $effect(() => {
    if (open) {
      restoreFocus = true;
      error = null;
    } else if (restoreFocus) {
      restoreFocus = false;
      anchor?.focus();
    }
  });

  function toggle(): void {
    open = !open;
    if (open) issueSubscriptions.syncLanes();
  }

  async function setSubscription(field: "newIssues" | "newComments", checked: boolean): Promise<void> {
    if (saving) return;
    saving = true;
    error = null;
    try {
      await issueSubscriptions.setSubscription(repoId, { [field]: checked });
    } catch (err) {
      error = describeError(err);
    } finally {
      saving = false;
    }
  }

  async function retry(): Promise<void> {
    if (retrying || !enabled) return;
    retrying = true;
    error = null;
    try {
      await issueSubscriptions.refresh(repoId);
    } catch (err) {
      error = describeError(err);
    } finally {
      retrying = false;
    }
  }

  function markAllRead(): void {
    issueActivity.markAllRead(root);
  }

  function timeLabel(value: string): string {
    const date = new Date(value);
    return Number.isFinite(date.getTime()) ? date.toLocaleString() : value;
  }
</script>

<Button
  bind:ref={anchor}
  class="relative h-7 w-7 flex-none rounded-sm"
  variant="ghost"
  size="icon-sm"
  type="button"
  aria-label="Issue activity"
  title="Issue activity"
  aria-expanded={open}
  aria-haspopup="dialog"
  onclick={toggle}
>
  {#if iconActivity}<Icon icon={iconActivity} size={15} />{/if}
  {#if unreadCount > 0}
    <span
      class="absolute -right-0.5 -top-0.5 min-w-3.5 rounded-full bg-primary px-1 text-center text-[9px] leading-[1.4] font-semibold text-primary-foreground tabular-nums"
      aria-label={`${unreadCount} unread issue events`}
    >{badgeCount(unreadCount)}</span>
  {/if}
</Button>

{#if open}
  <div
    bind:this={panel}
    class="fixed z-[80] flex max-h-[min(32rem,calc(100vh-1rem))] w-[min(24rem,calc(100vw-1rem))] flex-col rounded-md border border-border bg-popover text-popover-foreground shadow-xl outline-none"
    role="dialog"
    aria-label="Issue activity"
    aria-describedby="issue-activity-status"
    tabindex={-1}
  >
    <header class="flex flex-none items-center gap-2 border-b border-border px-3 py-2">
      <h2 class="m-0 min-w-0 flex-1 text-ui-base font-semibold">Issue activity</h2>
      <Button
        class="h-6 rounded-sm px-2 text-ui-micro"
        variant="outline"
        type="button"
        aria-label="Mark all read"
        disabled={unreadCount === 0}
        onclick={markAllRead}
      >Mark all read</Button>
    </header>

    <section class="flex flex-none flex-col gap-2 border-b border-border px-3 py-2" aria-label="Issue subscriptions">
      <p class="m-0 text-ui-micro font-semibold uppercase tracking-wide text-muted-foreground">Notify me about</p>
      <label class="flex cursor-pointer items-center gap-2 text-ui-meta">
        <Checkbox
          checked={sync.newIssues}
          disabled={saving}
          onCheckedChange={(checked) => void setSubscription("newIssues", checked)}
        />
        New issues
      </label>
      <label class="flex cursor-pointer items-center gap-2 text-ui-meta">
        <Checkbox
          checked={sync.newComments}
          disabled={saving}
          onCheckedChange={(checked) => void setSubscription("newComments", checked)}
        />
        New comments
      </label>
      <div class="flex flex-wrap items-center gap-1 text-ui-micro text-muted-foreground" id="issue-activity-status" role="status" aria-live="polite">
        <span class="inline-flex size-1.5 rounded-full bg-current" class:text-destructive={sync.phase !== "ready"} aria-hidden="true"></span>
        <span>{enabled ? phaseLabel : "Notifications are off"}</span>
        {#if sync.lastSuccessAt !== null}
          <span>· last success {timeLabel(sync.lastSuccessAt)}</span>
        {/if}
        {#if saving}<span>· saving…</span>{/if}
      </div>
      {#if error !== null}
        <p class="m-0 break-words text-ui-meta text-destructive" role="alert">{error}</p>
      {/if}
      <div>
        <Button
          class="h-6 gap-1 rounded-sm px-2 text-ui-micro"
          variant="outline"
          type="button"
          disabled={!enabled || retrying}
          onclick={() => void retry()}
        >
          {#if iconRefresh}<Icon icon={iconRefresh} size={11} />{/if}
          {retrying ? "Retrying…" : "Retry sync"}
        </Button>
      </div>
    </section>

    <section class="min-h-0 flex-1 overflow-y-auto" aria-label="Unread issue activity">
      <h3 class="m-0 px-3 py-2 text-ui-micro font-semibold uppercase tracking-wide text-muted-foreground">
        Unread activity{#if unreadCount > 0} ({unreadCount}){/if}
      </h3>
      {#if unread.length === 0}
        <p class="m-0 px-3 pb-4 text-ui-meta text-muted-foreground">No unread issue activity.</p>
      {:else}
        <ul class="m-0 grid list-none gap-1 p-0 px-2 pb-2">
          {#each unread as event (event.id)}
            <li>
              <button
                class="flex w-full cursor-pointer flex-col items-start gap-0.5 rounded-sm border border-transparent bg-transparent px-2 py-1.5 text-left text-ui-meta text-foreground hover:border-border hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
                type="button"
                onclick={() => {
                  open = false;
                  issueActivity.open(root, event.id, repoId);
                }}
              >
                <span class="min-w-0 self-stretch truncate font-medium">{event.title}</span>
                <span class="text-ui-micro text-muted-foreground">
                  #{event.issueNumber} · {event.kind === "issue" ? "New issue" : "New comment"} by {event.actor || "ghost"}
                </span>
                <span class="text-ui-micro text-muted-foreground">{timeLabel(event.createdAt)}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
{/if}
