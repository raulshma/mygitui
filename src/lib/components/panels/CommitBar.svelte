<script lang="ts">
  /**
   * Commit bar (M2) — sits at the bottom of the source-control sidebar (the
   * workspace's left column), so the commit message and button stay visible
   * while the changes list scrolls above.
   *
   * Message textarea (Ctrl/Cmd+Enter commits; Ctrl/Cmd+Shift+Enter commits
   * and pushes; starts one row tall and grows with the draft up to a cap,
   * then scrolls), compact icon toggles for Amend / skip-hooks / author
   * override, signing + hook status icons, a Fixup action, the Commit
   * button and a secondary Commit & Push button (spinner while the commit /
   * push op runs per the OpStore; disabled with no message unless amending).
   * Controls sit in one dense row under the message (wrapping only when the
   * sidebar gets too narrow): options left, Fixup + AI + Commit right. The
   * author override fields expand as a full-width row under that.
   * Success clears the form, toasts the short sha and asks the owner to
   * refresh (`onCommitted` → RepoView refreshStatus). Failures render
   * inline next to the button; a push failure after a landed commit toasts
   * "Committed — push failed: …". The AI button is a disabled ghost (M6).
   *
   * M12 (lane C): "Fixup into HEAD" commits the staged changes straight
   * into `fixup! <HEAD summary>` (no message needed; respects the
   * --no-verify toggle and the author override). Disabled until something
   * is staged and HEAD exists (read from the tab store's status); the HEAD
   * summary is fetched lazily through a one-page `stream_log`.
   */
  import { commit, hooksList, signingInfo, streamLog } from "$lib/ipc/client";
  import type { GitSignature, HookInfo, LogFilter, RepoStatus, SigningInfo } from "$lib/ipc/types";
  import { describeError } from "$lib/errors";
  import { pushCurrentBranch, syncSkipMessage } from "$lib/git/sync";
  import { busy } from "$lib/stores/ops.svelte";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import AiFeatureButton from "$lib/components/ai/AiFeatureButton.svelte";
  import { fixupMessage, hasStagedChanges, headCommitInPage } from "$lib/commit/commitBarModel";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    repoId,
    status = undefined,
    onCommitted = undefined,
  }: {
    repoId: string;
    /** Explicit status snapshot (e.g. for submodules); falls back to tabStore. */
    status?: RepoStatus | null;
    /** Called after a successful commit (RepoView refreshes the status). */
    onCommitted?: () => void;
  } = $props();

  const iconAmend = $derived(appIcon("edit")!);
  const iconSkipHooks = $derived(appIcon("block")!);
  const iconAuthor = $derived(appIcon("person")!);
  const iconFixup = $derived(appIcon("merge-type")!);
  const iconProgress = $derived(appIcon("progress-activity")!);
  const iconPush = $derived(appIcon("arrow-upward")!);
  const iconSigning = $derived(appIcon("verified")!);
  const iconHook = $derived(appIcon("bolt")!);
  const iconOptions = $derived(appIcon("tune")!);

  let message = $state("");
  let messageEl = $state<HTMLTextAreaElement | null>(null);
  let amend = $state(false);
  let noVerify = $state(false);
  let showAuthor = $state(false);
  let authorName = $state("");
  let authorEmail = $state("");
  let error = $state<string | null>(null);
  let committing = $state(false);
  /** The push half of Commit & Push is in flight (instant feedback). */
  let pushing = $state(false);

  /** Grows the message box from one row up to a readable cap. */
  const MAX_MESSAGE_HEIGHT = 176;

  function resizeMessage(): void {
    const el = messageEl;
    if (!el) return;
    el.style.height = "auto";
    const next = Math.min(el.scrollHeight, MAX_MESSAGE_HEIGHT);
    el.style.height = `${next}px`;
    el.style.overflowY = el.scrollHeight > MAX_MESSAGE_HEIGHT ? "auto" : "hidden";
  }

  $effect(() => {
    // Re-measure whenever the draft changes (including programmatic AI fills).
    void message;
    resizeMessage();
  });

  let signing = $state<SigningInfo | null>(null);
  let hooks = $state<HookInfo[]>([]);
  let metaToken = 0;

  /** Hooks that will actually run on commit (present + executable). */
  const runningHooks = $derived(
    hooks.filter((h) => h.present && h.executable && !h.kind.startsWith("pre-push")),
  );

  const commitBusy = $derived(busy(repoId, "commit"));
  /** True while a push op runs for this repo (any entry point). */
  const pushBusy = $derived(busy(repoId, "push"));

  /** The repo's latest status snapshot (staged detection + HEAD sha). */
  const activeStatus = $derived(
    status !== undefined
      ? status
      : (tabStore.tabs.find((tab) => tab.id === repoId)?.status ?? null),
  );
  const staged = $derived(hasStagedChanges(activeStatus?.entries ?? []));

  /**
   * Normal commits need something in the index; amending may reuse HEAD's
   * message. While the first status is still loading (`status === null`) we
   * keep the button available rather than guessing.
   */
  const needsStaged = $derived(!amend && activeStatus !== null && !staged);
  const hasMessage = $derived(message.trim().length > 0);
  const canCommit = $derived(
    (amend || hasMessage) && !needsStaged && !commitBusy && !committing,
  );
  /** Commit & Push shares Commit's preconditions plus the push phase. */
  const canCommitPush = $derived(canCommit && !pushBusy && !pushing);

  /** Explains what the primary button will do (title/cursor affordance). */
  const commitHint = $derived(
    !hasMessage && !amend
      ? "Write a commit message to enable Commit"
      : needsStaged
        ? "Stage a change before committing"
        : amend
          ? "Amend the previous commit (Ctrl+Enter)"
          : "Commit staged changes (Ctrl+Enter)",
  );

  /**
   * Amend hint (nothing staged + exactly one commit ahead of the upstream):
   * the likely intent is patching the just-made local commit, so the plain
   * "stage a file" line swaps to an amend affordance while that state holds.
   * Hidden automatically once amend is toggled on or something is staged
   * (both flip `needsStaged` off).
   */
  const showAmendHint = $derived(
    needsStaged && activeStatus !== null && activeStatus.ahead === 1,
  );

  /** Only warn when there are changes to stage (a clean tree is self-evident). */
  const showStagedHint = $derived(
    !showAmendHint &&
      needsStaged &&
      activeStatus !== null &&
      activeStatus.entries.length > 0,
  );

  /** The inline hint (amend suggestion or stage reminder) the buttons describe. */
  const stagedHintId = $derived(
    showAmendHint
      ? "commit-amend-hint"
      : showStagedHint
        ? "commit-staged-hint"
        : undefined,
  );

  $effect(() => {
    // Load signing + hooks state (reloads when the repo switches).
    void repoId;
    const token = ++metaToken;
    signing = null;
    hooks = [];
    signingInfo(repoId)
      .then((info) => {
        if (token === metaToken) signing = info;
      })
      .catch(() => {
        // Signing info is decorative; ignore failures.
      });
    hooksList(repoId)
      .then((list) => {
        if (token === metaToken) hooks = list;
      })
      .catch(() => {
        // Hook badges are decorative; ignore failures.
      });
  });

  /** Builds the CommitOptions author override, or null when not filled. */
  function authorOverride(): GitSignature | null {
    const name = authorName.trim();
    const email = authorEmail.trim();
    if (!name || !email) return null;
    const now = new Date();
    return {
      name,
      email,
      time: Math.floor(now.getTime() / 1000),
      offset_minutes: -now.getTimezoneOffset(),
    };
  }

  /**
   * Commits the staged changes (or amend). With `pushAfter` the existing
   * commit flow is chained into a push of the current branch via the shared
   * header/palette helper (primary remote, plain push — no force, no
   * force-with-lease overrides). A landed commit followed by a failed push
   * says so explicitly; a commit failure renders inline like Commit does.
   */
  async function doCommit(pushAfter = false): Promise<void> {
    if (!canCommit) return;
    committing = true;
    error = null;
    let sha: string | null = null;
    try {
      sha = await commit(repoId, {
        message: message.trimEnd(),
        amend,
        no_verify: noVerify,
        allow_empty: false,
        author: authorOverride(),
      });
    } catch (err) {
      error = describeError(err);
      committing = false;
      return;
    }
    committing = false;
    if (sha === null) return; // unreachable: success always yields a sha

    // Commit landed: clear the form, refresh, then optionally push.
    message = "";
    authorName = "";
    authorEmail = "";
    showAuthor = false;
    amend = false;
    noVerify = false;
    onCommitted?.();
    if (!pushAfter) {
      toast(`Committed ${sha.slice(0, 7)}`, { kind: "success" });
      return;
    }
    await pushAfterCommit();
  }

  /**
   * The "push" half of Commit & Push (never runs standalone). Pushes the
   * current branch with the same defaults as the header quick actions.
   */
  async function pushAfterCommit(): Promise<void> {
    pushing = true;
    try {
      const outcome = await pushCurrentBranch(
        repoId,
        activeStatus?.branch ?? null,
      );
      if (outcome.kind === "skipped") {
        // Push never touched the network (no remote / detached HEAD) —
        // still worth saying the commit itself landed.
        toast(
          `Committed — push failed: ${syncSkipMessage(outcome.reason, "push")}`,
          { kind: "error" },
        );
        return;
      }
      toast("Committed & pushed", { kind: "success" });
    } catch (err) {
      toast(`Committed — push failed: ${describeError(err)}`, {
        kind: "error",
      });
    } finally {
      pushing = false;
    }
  }

  function onMessageKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      void doCommit(event.shiftKey);
    }
  }

  // -- More commit options (UI review P2 #7) -----------------------------------
  //
  // Amend / skip-hooks / author were three unlabeled icon toggles with no
  // visible state. They now live in one menu with checkmarks, and anything
  // active shows as a text chip so the state survives a glance.

  const anyOption = $derived(amend || noVerify || showAuthor);

  const optionSummary = $derived.by(() => {
    const parts: string[] = [];
    if (amend) parts.push("Amend");
    if (noVerify) parts.push("Hooks off");
    if (showAuthor) parts.push("Author");
    return parts.join(" · ");
  });

  /** Options menu: checkmark prefix shows the current toggle state. */
  function openCommitOptions(event: MouseEvent): void {
    showMenuAtElement(event.currentTarget as HTMLElement, [
      {
        id: "amend",
        label: `${amend ? "✓ " : ""}Amend previous commit`,
        icon: iconAmend,
        run: () => (amend = !amend),
      },
      {
        id: "no-verify",
        label: `${noVerify ? "✓ " : ""}Skip hooks (--no-verify)`,
        icon: iconSkipHooks,
        run: () => (noVerify = !noVerify),
      },
      {
        id: "author",
        label: `${showAuthor ? "✓ " : ""}Author override…`,
        icon: iconAuthor,
        run: () => (showAuthor = !showAuthor),
      },
    ]);
  }

  // -- M12: "Fixup into HEAD" quick action -------------------------------------

  let fixing = $state(false);

  const canFixup = $derived(
    Boolean(activeStatus?.head) && staged && !commitBusy && !committing && !fixing,
  );

  const HEAD_LOG_FILTER: LogFilter = {
    text: null,
    regex: false,
    author: null,
    path: null,
    refs: [],
    follow: false,
  };

  /** HEAD's summary via a one-page log stream (shallow: stops at page 1). */
  async function fetchHeadSummary(): Promise<string | null> {
    const headSha = activeStatus?.head ?? null;
    if (!headSha) return null;
    return new Promise<string | null>((resolve) => {
      void streamLog(repoId, HEAD_LOG_FILTER, (page) => {
        resolve(headCommitInPage(page, headSha)?.summary ?? null);
      }).catch(() => resolve(null));
    });
  }

  async function doFixup(): Promise<void> {
    if (!canFixup) return;
    fixing = true;
    error = null;
    try {
      const head = await fetchHeadSummary();
      if (head === null) {
        toast("Fixup failed: could not determine HEAD", { kind: "error" });
        return;
      }
      const sha = await commit(repoId, {
        message: fixupMessage(head),
        amend: false,
        no_verify: noVerify,
        allow_empty: false,
        author: authorOverride(),
      });
      toast(`Fixup committed into ${sha.slice(0, 7)}`, { kind: "success" });
      onCommitted?.();
    } catch (err) {
      toast(`Fixup failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      fixing = false;
    }
  }
</script>

<form
  class="commit-bar flex-none flex flex-col gap-1 px-2 py-1.5 border-t border-border bg-card text-ui-base"
  aria-label="Create commit"
  onsubmit={(e) => {
    e.preventDefault();
    void doCommit();
  }}
>
  <div class="row-main flex flex-col gap-1 min-h-0">
    <label class="sr-only" for="commit-message">Commit message</label>
    <Hint label="Commit message (Ctrl+Enter commits, Ctrl+Shift+Enter commits and pushes)">
      {#snippet child({ props })}
        <Textarea
          class="message flex-none w-full min-w-0 box-border field-sizing-fixed min-h-7 max-h-44 resize-none overflow-y-hidden rounded-sm px-2 py-0.5 text-xs leading-[1.35] font-mono bg-background"
          placeholder="Commit message… Ctrl+↵"
          aria-keyshortcuts="Control+Enter Meta+Enter Control+Shift+Enter Meta+Shift+Enter"
          rows={1}
          bind:ref={messageEl}
          bind:value={message}
          {...props}
          id="commit-message"
          onkeydown={onMessageKeydown}
          disabled={commitBusy || committing}
        />
      {/snippet}
    </Hint>

    <div class="controls flex items-center flex-wrap gap-x-[0.375rem] gap-y-1 w-full min-w-0">
      <div class="opt-switches flex items-center gap-0.5 min-w-0">
        {#if anyOption}
          <Hint label="Active commit options">
            {#snippet child({ props })}
              <span class="opt-summary shrink-0 max-w-48 overflow-hidden text-ellipsis whitespace-nowrap px-2 py-px rounded-full bg-muted text-muted-foreground text-ui-tiny leading-normal" role="status" {...props}>
                {optionSummary}
              </span>
            {/snippet}
          </Hint>
        {/if}
        <Hint label="Amend, skip hooks, or override the author">
          {#snippet child({ props })}
            <Button
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground"
              variant="ghost"
              size="icon-sm"
              type="button"
              {...props}
              aria-label="More commit options"
              aria-pressed={anyOption}
              aria-haspopup="menu"
              onclick={openCommitOptions}
            >
              <Icon icon={iconOptions} size={16} />
            </Button>
          {/snippet}
        </Hint>

        {#if signing?.active || runningHooks.length > 0}
          <span class="meta-icons inline-flex items-center gap-0.5 ml-0.5 pl-1 border-l border-border">
            {#if signing?.active}
              {@const active = signing}
              <Hint
                label={active.key_id
                  ? `Commits will be signed with ${active.format} (key ${active.key_id})`
                  : `Commits will be signed (${active.format})`}
              >
                {#snippet child({ props })}
                  <span
                    class="meta-icon sign inline-flex items-center justify-center text-foreground/80 opacity-90"
                    role="img"
                    {...props}
                    aria-label={`Signing active: ${active.format}`}
                  >
                    <Icon icon={iconSigning} size={14} />
                  </span>
                {/snippet}
              </Hint>
            {/if}
            {#each runningHooks as hook (hook.kind)}
              <Hint label={`${hook.kind} will run on commit`}>
                {#snippet child({ props })}
                  <span
                    class="meta-icon hook inline-flex items-center justify-center text-primary opacity-90"
                    role="img"
                    {...props}
                    aria-label={`${hook.kind} hook will run`}
                  >
                    <Icon icon={iconHook} size={14} />
                  </span>
                {/snippet}
              </Hint>
            {/each}
          </span>
        {/if}
      </div>

      <div class="actions ml-auto flex items-center flex-wrap gap-1">
        <Hint label="Commit the staged changes as fixup! into HEAD (message not needed)">
          {#snippet child({ props })}
            <Button
              class={`text-[var(--app-added)] hover:bg-accent hover:text-accent-foreground ${fixing ? "[&_svg]:animate-spin" : ""}`}
              variant="ghost"
              size="icon-sm"
              type="button"
              {...props}
              aria-label="Fixup into HEAD"
              disabled={!canFixup}
              onclick={() => void doFixup()}
            >
              <Icon icon={fixing ? iconProgress : iconFixup} size={16} />
            </Button>
          {/snippet}
        </Hint>

        <div class="primary inline-flex items-center gap-ui-375">
          <AiFeatureButton
            {repoId}
            icon
            gateVisibility={false}
            rootClass="ai-commit"
            label="Generate commit message"
            title="Generate a commit message from the staged diff"
            ariaLabel="Generate commit message with AI"
            confirmText={"Send the staged diff to {backend} to draft a commit message?"}
            errorPrefix="AI commit message failed"
            successToast="Commit message generated — review before committing"
            disabled={commitBusy || committing}
            run={(opts) => ai.generateCommitMessage(repoId, undefined, opts)}
            onResult={(outcome) => {
              const m = outcome.message ?? { subject: outcome.result.text, body: "" };
              message = m.body ? `${m.subject}

${m.body}` : m.subject;
            }}
          />

          <Hint label="Commit, then push the branch to the primary remote (Ctrl+Shift+Enter)">
            {#snippet child({ props })}
              <Button
                class="h-7 gap-ui-35 rounded-full px-3 text-xs"
                variant="secondary"
                type="button"
                {...props}
                aria-keyshortcuts="Control+Shift+Enter Meta+Shift+Enter"
                aria-describedby={stagedHintId}
                disabled={!canCommitPush}
                onclick={() => void doCommit(true)}
              >
                {#if pushing || pushBusy}
                  <Spinner class="size-3" role="presentation" aria-hidden="true" />
                  <span>Pushing…</span>
                {:else}
                  <Icon icon={iconPush} size={14} />
                  <span>Commit &amp; Push</span>
                {/if}
              </Button>
            {/snippet}
          </Hint>

          <Hint label={commitHint}>
            {#snippet child({ props })}
              <Button
                class="h-7 min-w-[5.25rem] gap-ui-35 rounded-full px-4 text-xs"
                {...props}
                type="submit"
                aria-describedby={stagedHintId}
                disabled={!canCommit}
              >
                {#if commitBusy || committing}
                  <Spinner class="size-3" role="presentation" aria-hidden="true" />
                  <span>Committing…</span>
                {:else}
                  <span>Commit</span>
                {/if}
              </Button>
            {/snippet}
          </Hint>
        </div>
      </div>
    </div>
  </div>

  {#if showAmendHint}
    <p id="commit-amend-hint" class="amend-hint m-0 flex items-center gap-1 text-muted-foreground text-ui-micro leading-[1.3]" role="status">
      <span class="shrink-0">Nothing staged — amend the last commit instead?</span>
      <Hint label="Fold the next change into the previous commit (amend)">
        {#snippet child({ props })}
          <Button
            class="h-auto min-w-0 rounded-full px-1.5 py-px text-ui-micro leading-[1.4] text-primary hover:bg-accent hover:text-accent-foreground"
            variant="ghost"
            type="button"
            {...props}
            onclick={() => (amend = true)}
          >
            Amend last commit
          </Button>
        {/snippet}
      </Hint>
    </p>
  {:else if showStagedHint}
    <p id="commit-staged-hint" class="staged-hint m-0 text-muted-foreground text-ui-micro leading-[1.3]" role="status">
      No staged changes yet — stage a file to commit.
    </p>
  {/if}

  {#if showAuthor}
    <div class="author flex flex-col gap-1 pt-0.5">
      <label class="flex-1 flex items-center gap-ui-375 min-w-0">
        <span class="al shrink-0 text-muted-foreground text-ui-micro">Name</span>
        <Input
          type="text"
          class="h-auto flex-1 min-w-0 rounded-sm px-1.5 py-0.5 text-ui-meta"
          placeholder="Ada Lovelace"
          bind:value={authorName}
          aria-label="Author name override"
        />
      </label>
      <label class="flex-1 flex items-center gap-ui-375 min-w-0">
        <span class="al shrink-0 text-muted-foreground text-ui-micro">Email</span>
        <Input
          type="email"
          class="h-auto flex-1 min-w-0 rounded-sm px-1.5 py-0.5 text-ui-meta"
          placeholder="ada@example.com"
          bind:value={authorEmail}
          aria-label="Author email override"
        />
      </label>
    </div>
  {/if}

  {#if error}
    <p class="error m-0 text-destructive text-ui-meta [overflow-wrap:anywhere]" role="alert">{error}</p>
  {/if}
</form>
