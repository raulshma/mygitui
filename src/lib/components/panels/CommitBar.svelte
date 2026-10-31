<script lang="ts">
  /**
   * Commit bar (M2) — sits at the bottom of the source-control sidebar (the
   * workspace's left column), so the commit message and button stay visible
   * while the changes list scrolls above.
   *
   * Message textarea (Ctrl/Cmd+Enter commits; starts one row tall and grows
   * with the draft up to a cap, then scrolls), compact icon toggles for
   * Amend / skip-hooks / author override, signing + hook status icons, a
   * Fixup action, and the Commit button (spinner while a commit op runs per
   * the OpStore; disabled with no message unless amending).
   * Controls sit in one dense row under the message (wrapping only when the
   * sidebar gets too narrow): options left, Fixup + AI + Commit right. The
   * author override fields expand as a full-width row under that.
   * Success clears the form, toasts the short sha and asks the owner to
   * refresh (`onCommitted` → RepoView refreshStatus). Failures render
   * inline next to the button. The AI button is a disabled ghost (M6).
   *
   * M12 (lane C): "Fixup into HEAD" commits the staged changes straight
   * into `fixup! <HEAD summary>` (no message needed; respects the
   * --no-verify toggle and the author override). Disabled until something
   * is staged and HEAD exists (read from the tab store's status); the HEAD
   * summary is fetched lazily through a one-page `stream_log`.
   */
  import { commit, hooksList, signingInfo, streamLog } from "$lib/ipc/client";
  import type { GitSignature, HookInfo, LogFilter, RepoStatus, SigningInfo } from "$lib/ipc/types";
  import { busy } from "$lib/stores/ops.svelte";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import AiFeatureButton from "$lib/components/ai/AiFeatureButton.svelte";
  import { fixupMessage, hasStagedChanges, headCommitInPage } from "$lib/commit/commitBarModel";
  import iconAmend from "@ktibow/iconset-material-symbols/edit";
  import iconSkipHooks from "@ktibow/iconset-material-symbols/block";
  import iconAuthor from "@ktibow/iconset-material-symbols/person";
  import iconFixup from "@ktibow/iconset-material-symbols/merge-type";
  import iconProgress from "@ktibow/iconset-material-symbols/progress-activity";
  import iconSigning from "@ktibow/iconset-material-symbols/verified";
  import iconHook from "@ktibow/iconset-material-symbols/bolt";
  import iconOptions from "@ktibow/iconset-material-symbols/tune";

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

  let message = $state("");
  let messageEl = $state<HTMLTextAreaElement | null>(null);
  let amend = $state(false);
  let noVerify = $state(false);
  let showAuthor = $state(false);
  let authorName = $state("");
  let authorEmail = $state("");
  let error = $state<string | null>(null);
  let committing = $state(false);

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

  /** Only warn when there are changes to stage (a clean tree is self-evident). */
  const showStagedHint = $derived(
    needsStaged && activeStatus !== null && activeStatus.entries.length > 0,
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

  async function doCommit(): Promise<void> {
    if (!canCommit) return;
    committing = true;
    error = null;
    try {
      const sha = await commit(repoId, {
        message: message.trimEnd(),
        amend,
        no_verify: noVerify,
        allow_empty: false,
        author: authorOverride(),
      });
      message = "";
      authorName = "";
      authorEmail = "";
      showAuthor = false;
      amend = false;
      noVerify = false;
      toast(`Committed ${sha.slice(0, 7)}`, { kind: "success" });
      onCommitted?.();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      committing = false;
    }
  }

  function onMessageKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      void doCommit();
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
      toast(
        `Fixup failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      fixing = false;
    }
  }
</script>

<form
  class="commit-bar flex-none flex flex-col gap-[0.3rem] px-[0.45rem] py-1.5 border-t border-border bg-card text-[0.8125rem]"
  aria-label="Create commit"
  onsubmit={(e) => {
    e.preventDefault();
    void doCommit();
  }}
>
  <div class="row-main flex flex-col gap-[0.3rem] min-h-0">
    <label class="sr-only" for="commit-message">Commit message</label>
    <Textarea
      id="commit-message"
      class="message flex-none w-full min-w-0 box-border field-sizing-fixed min-h-[1.7rem] max-h-44 resize-none overflow-y-hidden rounded-sm px-2 py-[0.28rem] text-xs leading-[1.35] font-mono bg-background"
      placeholder="Commit message… Ctrl+↵"
      title="Commit message (Ctrl+Enter commits)"
      aria-keyshortcuts="Control+Enter Meta+Enter"
      rows={1}
      bind:ref={messageEl}
      bind:value={message}
      onkeydown={onMessageKeydown}
      disabled={commitBusy || committing}
    />

    <div class="controls flex items-center flex-wrap gap-x-[0.375rem] gap-y-1 w-full min-w-0">
      <div class="opt-switches flex items-center gap-0.5 min-w-0">
        {#if anyOption}
          <span class="opt-summary shrink-0 max-w-48 overflow-hidden text-ellipsis whitespace-nowrap px-[0.45rem] py-px rounded-full bg-muted text-muted-foreground text-[0.65rem] leading-normal" role="status" title="Active commit options">
            {optionSummary}
          </span>
        {/if}
        <Button
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="More commit options"
          aria-pressed={anyOption}
          aria-haspopup="menu"
          title="Amend, skip hooks, or override the author"
          onclick={openCommitOptions}
        >
          <Icon icon={iconOptions} size={16} />
        </Button>

        {#if signing?.active || runningHooks.length > 0}
          <span class="meta-icons inline-flex items-center gap-0.5 ml-0.5 pl-1 border-l border-border">
            {#if signing?.active}
              <span
                class="meta-icon sign inline-flex items-center justify-center text-foreground/80 opacity-90"
                role="img"
                title={signing.key_id
                  ? `Commits will be signed with ${signing.format} (key ${signing.key_id})`
                  : `Commits will be signed (${signing.format})`}
                aria-label={`Signing active: ${signing.format}`}
              >
                <Icon icon={iconSigning} size={14} />
              </span>
            {/if}
            {#each runningHooks as hook (hook.kind)}
              <span
                class="meta-icon hook inline-flex items-center justify-center text-primary opacity-90"
                role="img"
                title="{hook.kind} will run on commit"
                aria-label={`${hook.kind} hook will run`}
              >
                <Icon icon={iconHook} size={14} />
              </span>
            {/each}
          </span>
        {/if}
      </div>

      <div class="actions ml-auto flex items-center flex-wrap gap-1">
        <Button
          class={`text-[var(--app-added)] hover:bg-accent hover:text-accent-foreground ${fixing ? "[&_svg]:animate-spin" : ""}`}
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Fixup into HEAD"
          title="Commit the staged changes as fixup! into HEAD (message not needed)"
          disabled={!canFixup}
          onclick={() => void doFixup()}
        >
          <Icon icon={fixing ? iconProgress : iconFixup} size={16} />
        </Button>

        <div class="primary inline-flex items-center gap-[0.375rem]">
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

          <Button
            class="h-7 min-w-[5.25rem] gap-[0.35rem] rounded-full px-4 text-xs"
            type="submit"
            title={commitHint}
            aria-describedby={showStagedHint ? "commit-staged-hint" : undefined}
            disabled={!canCommit}
          >
            {#if commitBusy || committing}
              <Spinner class="size-3" role="presentation" aria-hidden="true" />
              <span>Committing…</span>
            {:else}
              <span>Commit</span>
            {/if}
          </Button>
        </div>
      </div>
    </div>
  </div>

  {#if showStagedHint}
    <p id="commit-staged-hint" class="staged-hint m-0 text-muted-foreground text-[0.6875rem] leading-[1.3]" role="status">
      No staged changes yet — stage a file to commit.
    </p>
  {/if}

  {#if showAuthor}
    <div class="author flex flex-col gap-[0.3rem] pt-0.5">
      <label class="flex-1 flex items-center gap-[0.375rem] min-w-0">
        <span class="al shrink-0 text-muted-foreground text-[0.6875rem]">Name</span>
        <Input
          type="text"
          class="h-auto flex-1 min-w-0 rounded-sm px-1.5 py-[0.15rem] text-[0.72rem]"
          placeholder="Ada Lovelace"
          bind:value={authorName}
          aria-label="Author name override"
        />
      </label>
      <label class="flex-1 flex items-center gap-[0.375rem] min-w-0">
        <span class="al shrink-0 text-muted-foreground text-[0.6875rem]">Email</span>
        <Input
          type="email"
          class="h-auto flex-1 min-w-0 rounded-sm px-1.5 py-[0.15rem] text-[0.72rem]"
          placeholder="ada@example.com"
          bind:value={authorEmail}
          aria-label="Author email override"
        />
      </label>
    </div>
  {/if}

  {#if error}
    <p class="error m-0 text-destructive text-[0.72rem] [overflow-wrap:anywhere]" role="alert">{error}</p>
  {/if}
</form>
