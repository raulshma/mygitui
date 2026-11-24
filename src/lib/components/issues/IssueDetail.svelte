<script lang="ts">
  import { untrack } from "svelte";
  import type { Issue, IssueComment, IssueState } from "$lib/issues/model";
  import { issuesStore } from "$lib/stores/issues.svelte";
  import { openExternal } from "$lib/stores/forge.svelte";
  import { toast } from "$lib/toast";
  import { copyText } from "$lib/clipboard";
  import { describeError } from "$lib/errors";
  import AiMarkdown from "$lib/components/ai/AiMarkdown.svelte";
  import AuthorAvatar from "$lib/components/panels/AuthorAvatar.svelte";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    repoId,
    issueNumber,
    commentId = undefined,
    requestNonce = undefined,
    onEdit,
    onChanged,
  }: {
    repoId: string;
    issueNumber: number;
    /** Public center contract is numeric; persisted runtime events carry node IDs. */
    commentId?: number | string;
    requestNonce?: number;
    onEdit: (issue: Issue) => void;
    onChanged: () => void;
  } = $props();

  const iconEdit = $derived(appIcon("edit"));
  const iconOpen = $derived(appIcon("open-in-new"));
  const iconRefresh = $derived(appIcon("refresh"));
  const iconComment = $derived(appIcon("add"));

  let commentBody = $state("");
  let commentError = $state<string | null>(null);
  let composerRepo = $state(untrack(() => repoId));
  let composerIssue = $state(untrack(() => issueNumber));
  let stateAction = $state<IssueState | null>(null);
  let stateComment = $state("");
  let stateError = $state<string | null>(null);
  let highlightedCommentId = $state<string | null>(null);
  let deepLinkMissing = $state(false);
  let deepLinkRequest = $state<{ key: string; id: string; found: boolean } | null>(null);

  const identityEntry = $derived(issuesStore.identity(repoId));
  const detailEntry = $derived(issuesStore.detail(repoId, issueNumber));
  const commentsEntry = $derived(issuesStore.comments(repoId, issueNumber));
  const issue = $derived(
    detailEntry.state === "ready"
      ? detailEntry.data
      : detailEntry.state === "loading" || detailEntry.state === "error"
        ? detailEntry.previous
        : null,
  );
  const loadingDetail = $derived(detailEntry.state === "loading" && issue === null);
  const refreshingDetail = $derived(detailEntry.state === "loading" && issue !== null);
  const detailError = $derived(
    detailEntry.state === "error"
      ? detailEntry.message
      : identityEntry.state === "error"
        ? identityEntry.message
        : null,
  );
  const thread = $derived(
    commentsEntry.state === "ready"
      ? commentsEntry.data
      : commentsEntry.state === "loading" || commentsEntry.state === "error"
        ? commentsEntry.previous
        : null,
  );
  const comments = $derived.by(() => {
    if (thread === null) return [];
    const byId = new Map<string, IssueComment>();
    for (const page of thread.pages) {
      for (const comment of page.items) byId.set(comment.id, comment);
    }
    return [...byId.values()].sort((a, b) => a.created_at.localeCompare(b.created_at));
  });
  const earliestPage = $derived.by(() => {
    if (thread === null) return null;
    return [...thread.pages].sort((a, b) => (a.page ?? 0) - (b.page ?? 0))[0] ?? null;
  });
  const commentsLoading = $derived(commentsEntry.state === "loading" && thread === null);
  const commentsError = $derived(commentsEntry.state === "error" ? commentsEntry.message : null);
  const busy = $derived(issuesStore.busy(repoId, issueNumber));
  const stateTitle = $derived(
    stateAction === "closed" ? `Close issue #${issueNumber}` : `Reopen issue #${issueNumber}`,
  );
  const stateButtonLabel = $derived(stateAction === "closed" ? "Close issue" : "Reopen issue");

  $effect(() => {
    const targetRepo = repoId;
    const number = issueNumber;
    untrack(() => {
      void issuesStore.loadDetail(targetRepo, number);
      void issuesStore.loadLatestComments(targetRepo, number);
    });
  });

  $effect(() => {
    if (composerRepo !== repoId || composerIssue !== issueNumber) {
      composerRepo = repoId;
      composerIssue = issueNumber;
      commentBody = "";
      commentError = null;
      stateAction = null;
      stateComment = "";
      stateError = null;
    }
  });

  // requestNonce is part of the identity: a repeated open of the same comment
  // must scroll/highlight again even though its stable node id did not change.
  $effect(() => {
    const wanted = commentId === undefined || commentId === null ? null : String(commentId);
    if (wanted === null) {
      deepLinkRequest = null;
      highlightedCommentId = null;
      deepLinkMissing = false;
      return;
    }
    const key = `${repoId}:${issueNumber}:${requestNonce ?? 0}:${wanted}`;
    deepLinkRequest = { key, id: wanted, found: false };
    highlightedCommentId = null;
    deepLinkMissing = false;
  });

  // A linked comment may be on an earlier page. Walk backwards until it is
  // present (the store bounds this to five pages) or the thread is exhausted.
  $effect(() => {
    const request = deepLinkRequest;
    if (request === null || request.found || commentsEntry.state !== "ready") return;
    if (comments.some((comment) => comment.id === request.id)) {
      deepLinkRequest = { ...request, found: true };
      highlightedCommentId = request.id;
      requestAnimationFrame(() => {
        document.getElementById(commentDomId(repoId, issueNumber, request.id))?.scrollIntoView({
          block: "center",
          behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
        });
      });
      return;
    }
    if (earliestPage?.has_earlier) {
      untrack(() => void issuesStore.loadEarlierComments(repoId, issueNumber));
      return;
    }
    deepLinkRequest = { ...request, found: true };
    deepLinkMissing = true;
  });

  function commentDomId(targetRepo: string, number: number, id: string): string {
    return `issue-comment-${encodeURIComponent(targetRepo)}-${number}-${encodeURIComponent(id)}`;
  }

  function openUrl(url: string): void {
    if (url === "") return;
    void openExternal(url).then((ok) => {
      if (!ok) toast(`Could not open ${url}`, { kind: "error" });
    });
  }

  async function postComment(): Promise<void> {
    const body = commentBody.trim();
    if (body === "" || busy !== null) return;
    commentError = null;
    try {
      await issuesStore.comment(repoId, issueNumber, commentBody);
      commentBody = "";
      toast("Comment posted", { kind: "success" });
      onChanged();
    } catch (err) {
      commentError = describeError(err);
    }
  }

  function openStateDialog(state: IssueState): void {
    stateAction = state;
    stateComment = "";
    stateError = null;
  }

  async function confirmState(): Promise<void> {
    if (stateAction === null || busy !== null) return;
    stateError = null;
    try {
      await issuesStore.setState(repoId, issueNumber, stateAction, stateComment.trim() || null);
      toast(stateAction === "closed" ? `Issue #${issueNumber} closed` : `Issue #${issueNumber} reopened`, { kind: "success" });
      stateAction = null;
      stateComment = "";
      onChanged();
    } catch (err) {
      stateError = describeError(err);
    }
  }

  export async function refresh(): Promise<void> {
    await Promise.all([
      issuesStore.loadDetail(repoId, issueNumber, { force: true }),
      issuesStore.loadLatestComments(repoId, issueNumber),
    ]);
  }

  function dateLabel(value: string): string {
    const date = new Date(value);
    return Number.isFinite(date.getTime())
      ? date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })
      : value;
  }
</script>

<div class="issue-detail flex min-h-0 min-w-0 flex-1 flex-col bg-card" aria-busy={loadingDetail || refreshingDetail || commentsLoading}>
  <header class="flex flex-none flex-wrap items-center gap-1 border-b border-border bg-popover px-2 py-1">
    <span class="min-w-0 flex-1 truncate text-ui-base font-medium text-foreground">
      {issue === null ? `Issue #${issueNumber}` : issue.title}
    </span>
    {#if issue !== null}
      <Button
        class="h-7 gap-1 rounded-sm px-2 text-ui-meta"
        variant="outline"
        type="button"
        aria-label="Edit issue"
        disabled={busy !== null}
        onclick={() => onEdit(issue)}
      >
        {#if iconEdit}<Icon icon={iconEdit} size={13} />{/if}
        Edit
      </Button>
      <Button
        class="h-7 gap-1 rounded-sm px-2 text-ui-meta"
        variant="outline"
        type="button"
        aria-label={issue.state === "open" ? "Close issue" : "Reopen issue"}
        disabled={busy !== null}
        onclick={() => openStateDialog(issue.state === "open" ? "closed" : "open")}
      >{issue.state === "open" ? "Close" : "Reopen"}</Button>
      <Button
        class="h-7 w-7 rounded-sm"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label="Open issue on GitHub"
        title="Open issue on GitHub"
        onclick={() => openUrl(issue.url)}
      >
        {#if iconOpen}<Icon icon={iconOpen} size={14} />{/if}
      </Button>
      <Button
        class="h-7 rounded-sm px-2 text-ui-micro"
        variant="ghost"
        type="button"
        aria-label="Copy issue link"
        title="Copy issue link"
        onclick={() => void copyText(issue.url, "Issue link")}
      >
        Copy
      </Button>
    {/if}
    <Button
      class="h-7 w-7 rounded-sm"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh issue"
      title="Refresh issue"
      disabled={refreshingDetail || commentsLoading}
      onclick={() => void refresh()}
    >
      {#if iconRefresh}<Icon icon={iconRefresh} size={14} />{/if}
    </Button>
  </header>

  {#if refreshingDetail}
    <div class="flex-none border-b border-border/60 bg-muted/40 px-3 py-1 text-ui-micro text-muted-foreground" role="status">Refreshing issue…</div>
  {/if}
  {#if detailError !== null && issue !== null}
    <div class="flex flex-none flex-wrap items-center gap-2 border-b border-border/60 bg-muted/40 px-3 py-1 text-ui-micro text-destructive" role="alert">
      <span class="min-w-0 flex-1">Showing saved details. {detailError}</span>
      <Button class="h-6 rounded-sm text-ui-micro" variant="outline" type="button" onclick={() => void refresh()}>Retry</Button>
    </div>
  {/if}

  <div class="min-h-0 min-w-0 flex-1 overflow-y-auto [scrollbar-width:thin]">
    {#if loadingDetail}
      <div class="grid gap-3 p-4" role="status" aria-label="Loading issue">
        <div class="h-5 w-3/4 animate-pulse rounded bg-muted"></div>
        <div class="h-3 w-1/3 animate-pulse rounded bg-muted"></div>
        <div class="mt-3 h-28 animate-pulse rounded bg-muted"></div>
      </div>
    {:else if detailError !== null && issue === null}
      <div class="flex min-h-48 flex-col items-center justify-center gap-2 p-4 text-center text-ui-meta" role="alert">
        <p class="m-0 font-medium text-foreground">Couldn't load issue</p>
        <p class="m-0 max-w-sm break-words text-muted-foreground">{detailError}</p>
        <Button class="h-7 rounded-sm" variant="outline" type="button" onclick={() => void refresh()}>Retry</Button>
      </div>
    {:else if issue !== null}
      <article class="mx-auto w-full max-w-4xl p-4" aria-labelledby={`issue-heading-${repoId}-${issue.number}`}>
        <div class="flex items-start gap-2">
          <span
            class="mt-1 inline-flex size-4 flex-none items-center justify-center rounded-full border"
            class:border-[var(--app-added)]={issue.state === "open"}
            class:border-muted-foreground={issue.state === "closed"}
            class:text-[var(--app-added)]={issue.state === "open"}
            aria-label={issue.state}
          >
            {#if issue.state === "closed"}<span class="size-1.5 rounded-full bg-current"></span>{/if}
          </span>
          <div class="min-w-0 flex-1">
            <h1 class="m-0 break-words text-lg leading-snug font-semibold text-foreground" id={`issue-heading-${repoId}-${issue.number}`}>
              {issue.title} <span class="font-normal text-muted-foreground">#{issue.number}</span>
            </h1>
            <div class="mt-1 flex flex-wrap items-center gap-2 text-ui-meta text-muted-foreground">
              <span
                class="rounded-full px-2 py-px"
                class:bg-[var(--app-added-bg)]={issue.state === "open"}
                class:text-[var(--app-added)]={issue.state === "open"}
                class:bg-accent={issue.state === "closed"}
              >{issue.state === "open" ? "Open" : "Closed"}</span>
              <span>opened {dateLabel(issue.created_at)} by {issue.author.login || "ghost"}</span>
              {#if issue.closed_at !== null}<span>closed {dateLabel(issue.closed_at)}</span>{/if}
              {#if issue.state_reason}<span>{issue.state_reason.replaceAll("_", " ")}</span>{/if}
              {#if issue.locked}<span>locked</span>{/if}
              {#if issue.issue_type}<span>{issue.issue_type.name}</span>{/if}
            </div>
          </div>
        </div>

        {#if issue.labels.length > 0 || issue.assignees.length > 0 || issue.milestone !== null}
          <dl class="mt-3 grid gap-1 border-y border-border py-2 text-ui-meta sm:grid-cols-[auto_1fr]">
            {#if issue.labels.length > 0}
              <dt class="text-muted-foreground">Labels</dt>
              <dd class="m-0 flex flex-wrap gap-1">
                {#each issue.labels as label (label.id)}
                  <span class="rounded-full border border-border px-2 text-ui-micro" title={label.description ?? label.name}>{label.name}</span>
                {/each}
              </dd>
            {/if}
            {#if issue.assignees.length > 0}
              <dt class="text-muted-foreground">Assignees</dt>
              <dd class="m-0">{issue.assignees.map((entry) => entry.login).join(", ")}</dd>
            {/if}
            {#if issue.milestone !== null}
              <dt class="text-muted-foreground">Milestone</dt>
              <dd class="m-0">{issue.milestone.title}{issue.milestone.due_on ? ` · due ${dateLabel(issue.milestone.due_on)}` : ""}</dd>
            {/if}
          </dl>
        {/if}

        <section class="issue-body mt-4 text-ui-base leading-relaxed" aria-label="Issue description">
          {#if issue.body.trim()}
            <AiMarkdown text={issue.body} />
          {:else}
            <p class="m-0 text-muted-foreground">No description provided.</p>
          {/if}
        </section>

        <section class="mt-5" aria-label="Issue comments">
          <h2 class="m-0 border-b border-border pb-1 text-ui-meta font-semibold text-foreground">
            Comments <span class="font-normal text-muted-foreground">({issue.comments_count})</span>
          </h2>
          {#if commentsLoading}
            <div class="grid gap-2 py-3" role="status" aria-label="Loading comments">
              {#each Array.from({ length: 3 }, (_, index) => index) as index (index)}
                <div class="h-16 animate-pulse rounded-sm bg-muted"></div>
              {/each}
            </div>
          {:else}
            {#if commentsError !== null}
              <div class="my-2 flex flex-wrap items-center gap-2 text-ui-meta text-destructive" role="alert">
                <span class="min-w-0 flex-1">{commentsError}</span>
                <Button class="h-6 rounded-sm text-ui-micro" variant="outline" type="button" onclick={() => void issuesStore.loadLatestComments(repoId, issueNumber)}>Retry comments</Button>
              </div>
            {/if}
            {#if earliestPage?.has_earlier}
              <div class="flex justify-center py-2">
                <Button
                  class="h-7 rounded-sm text-ui-meta"
                  variant="outline"
                  type="button"
                  disabled={commentsEntry.state === "loading"}
                  onclick={() => void issuesStore.loadEarlierComments(repoId, issueNumber)}
                >Load earlier comments</Button>
              </div>
            {/if}
            {#if deepLinkMissing}
              <p class="my-2 text-ui-meta text-muted-foreground" role="status">The linked comment is not available.</p>
            {/if}
            {#if comments.length > 0}
              <ol class="m-0 grid list-none gap-2 p-0">
                {#each comments as comment (comment.id)}
                  <li
                    id={commentDomId(repoId, issueNumber, comment.id)}
                    class="rounded-md border border-border bg-background p-3 scroll-mt-16"
                    class:ring-2={highlightedCommentId === comment.id}
                    class:ring-primary={highlightedCommentId === comment.id}
                  >
                    <div class="flex flex-wrap items-center gap-2 text-ui-meta">
                      <AuthorAvatar name={comment.author.login || "ghost"} size={20} />
                      <span class="font-semibold text-foreground">{comment.author.login || "ghost"}</span>
                      <span class="text-muted-foreground">commented {dateLabel(comment.created_at)}</span>
                      {#if comment.updated_at !== comment.created_at}
                        <span class="text-muted-foreground">edited {dateLabel(comment.updated_at)}</span>
                      {/if}
                      <span class="ml-auto inline-flex items-center gap-1">
                        <Button
                          class="h-6 w-6 rounded-sm"
                          variant="ghost"
                          size="icon-sm"
                          type="button"
                          aria-label={`Open comment by ${comment.author.login || "ghost"} on GitHub`}
                          title="Open comment on GitHub"
                          onclick={() => openUrl(comment.url)}
                        >
                          {#if iconOpen}<Icon icon={iconOpen} size={12} />{/if}
                        </Button>
                        <Button
                          class="h-6 rounded-sm px-1.5 text-ui-micro"
                          variant="ghost"
                          type="button"
                          aria-label={`Copy comment link by ${comment.author.login || "ghost"}`}
                          title="Copy comment link"
                          onclick={() => void copyText(comment.url, "Comment link")}
                        >
                          Copy
                        </Button>
                      </span>
                    </div>
                    <div class="mt-2 text-ui-base leading-relaxed">
                      <AiMarkdown text={comment.body} />
                    </div>
                  </li>
                {/each}
              </ol>
            {:else if !commentsError}
              <p class="my-3 text-ui-meta text-muted-foreground">No comments yet.</p>
            {/if}
          {/if}
        </section>
      </article>
    {/if}
  </div>

  <form
    class="flex flex-none flex-col gap-1 border-t border-border bg-popover p-2"
    aria-label="Add comment"
    onsubmit={(event) => {
      event.preventDefault();
      void postComment();
    }}
  >
    <label class="sr-only" for={`issue-comment-input-${repoId}-${issueNumber}`}>Comment body</label>
    <textarea
      id={`issue-comment-input-${repoId}-${issueNumber}`}
      class="min-h-20 resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring"
      value={commentBody}
      rows={3}
      placeholder="Write a comment (Markdown supported)"
      disabled={busy !== null}
      oninput={(event) => (commentBody = event.currentTarget.value)}
      onkeydown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
          event.preventDefault();
          void postComment();
        }
      }}
    ></textarea>
    {#if commentError !== null}
      <p class="m-0 break-words text-ui-meta text-destructive" role="alert">{commentError}</p>
    {/if}
    <div class="flex items-center justify-between gap-2">
      <span class="text-ui-micro text-muted-foreground">Mod+Enter to post</span>
      <Button class="h-7 gap-1 rounded-sm px-2.5 text-ui-meta" type="submit" disabled={busy !== null || commentBody.trim() === ""}>
        {#if iconComment}<Icon icon={iconComment} size={13} />{/if}
        {busy?.operation === "comment" ? "Posting…" : "Post comment"}
      </Button>
    </div>
  </form>

  <DialogShell
    open={stateAction !== null}
    labelledBy="issue-state-title"
    describedBy={stateError === null ? undefined : "issue-state-error"}
    width="30rem"
    onCancel={() => (stateAction = null)}
    onsubmit={() => void confirmState()}
  >
    <h2 class="sdlg-title" id="issue-state-title">{stateTitle}</h2>
    <p class="sdlg-message">Add an optional comment explaining this change.</p>
    <label class="sr-only" for="issue-state-comment">State change comment</label>
    <textarea
      id="issue-state-comment"
      class="mt-1 min-h-24 w-full resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring"
      value={stateComment}
      rows={4}
      placeholder={stateAction === "closed" ? "Why is this being closed? (optional)" : "Why is this being reopened? (optional)"}
      data-autofocus
      oninput={(event) => (stateComment = event.currentTarget.value)}
    ></textarea>
    {#if stateError !== null}
      <p class="m-0 mt-2 break-words text-ui-meta text-destructive" id="issue-state-error" role="alert">{stateError}</p>
    {/if}
    {#snippet actions()}
      <button class="sdlg-secondary" type="button" onclick={() => (stateAction = null)}>Cancel</button>
      <button
        class={stateAction === "closed" ? "sdlg-danger" : "sdlg-primary"}
        type="submit"
        disabled={busy !== null}
      >{busy !== null ? "Working…" : stateButtonLabel}</button>
    {/snippet}
</DialogShell>
</div>
