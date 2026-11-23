<script module lang="ts">
  /**
   * One commit scope (plan §6.1): the owning repository or a dirty
   * submodule the composer can target. `id` is stable per panel instance
   * ("main" or the submodule's repo-relative path) — it keys the per-scope
   * draft messages.
   */
  export interface CommitScope {
    id: string;
    /** Chip label (repo/module display name). */
    label: string;
    /** The scope's backend repo id (all commit/push/AI IPC targets it). */
    repoId: string;
    /** Latest status snapshot (counts + staged detection + branch). */
    status: RepoStatus | null;
    /** Full path (chip tooltip); absent for the owning repository. */
    path?: string;
  }
</script>

<script lang="ts">
  /**
   * Commit bar (M2) — the ONE composer for the STATUS panel (plan §6.1):
   * it docks to the panel's bottom edge (sticky — the changes list scrolls
   * above) and retargets to whichever scope chip is active. Scope chips
   * sit above the message box — the owning repository plus every DIRTY
   * submodule with its change count (`jellyplay (93) · jellyfin-plugin (n)`);
   * selecting a chip swaps the draft message, the staging context and the
   * commit action to that repository, so N submodules no longer mean N
   * stacked composers. A single-scope caller (the agent view) passes no
   * `scopes` and the chips row never renders.
   *
   * Message textarea (Ctrl/Cmd+Enter commits; Ctrl/Cmd+Shift+Enter commits
   * and pushes; starts one row tall and grows with the draft up to a cap,
   * then scrolls), compact icon toggles for skip-hooks / author override,
   * signing + hook status icons, a Fixup action and ONE primary `Commit`
   * button with a split caret menu (Commit & Push, Amend last, Sign off —
   * plan §6.2). The AI sparkle rides the textarea's top-right corner
   * (VS Code-style) with the tiny model-override caret beside it
   * (AiModelPicker compact — plan §6.3; no full-width dropdown anywhere in
   * the commit flow). Under the message sits one dense controls row
   * (config left, the split Commit glued right) that never wraps into two
   * rows — the composer is a container-query root (§6.7): scope chips
   * scroll horizontally and the action pair stays on one line however
   * narrow the panel gets. The author override fields expand as a
   * full-width row under that.
   * Success clears the form, toasts the short sha and asks the owner to
   * refresh (`onCommitted` → StatusPanel refreshAll). Failures render
   * inline next to the button; a push failure after a landed commit toasts
   * "Committed — push failed: …". The AI button is a disabled ghost (M6).
   *
   * M12 (lane C): "Fixup into HEAD" commits the staged changes straight
   * into `fixup! <HEAD summary>` (no message needed; respects the
   * --no-verify toggle and the author override). Disabled until something
   * is staged and HEAD exists (read from the scope's status); the HEAD
   * summary is fetched lazily through a one-page `stream_log`.
   */
  import { commit, hooksList, readFile, signingInfo, streamLog } from "$lib/ipc/client";
  import type { GitSignature, HookInfo, LogFilter, RepoStatus, SigningInfo } from "$lib/ipc/types";
  import { describeError } from "$lib/errors";
  import { pushCurrentBranch, syncSkipMessage } from "$lib/git/sync";
  import { busy } from "$lib/stores/ops.svelte";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import { toast } from "$lib/toast";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import AiFeatureButton from "$lib/components/ai/AiFeatureButton.svelte";
  import AiModelPicker from "$lib/components/ai/AiModelPicker.svelte";
  import { fixupMessage, hasStagedChanges, headCommitInPage } from "$lib/commit/commitBarModel";
  import {
    commitExtras,
    rulerLeftCss,
    templateFromBody,
  } from "$lib/commit/commitExtras.svelte";
  import { submoduleChangeCounts } from "./statusModel";
  import { untrack } from "svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    repoId,
    status = undefined,
    onCommitted = undefined,
    scopes = undefined,
  }: {
    repoId: string;
    /** Explicit status snapshot for the fallback scope; falls back to tabStore. */
    status?: RepoStatus | null;
    /** Called after a successful commit (the owner refreshes the status). */
    onCommitted?: () => void;
    /**
     * Scope chips (multi-repo STATUS composer, plan §6.1): main repo first,
     * then every dirty submodule. Omitted = single-scope composer (agent
     * view) — no chips row.
     */
    scopes?: CommitScope[];
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
  const iconRecents = $derived(appIcon("history")!);
  const iconCaret = $derived(appIcon("chevron-down")!);

  // -- Scope selection (plan §6.1) ----------------------------------------------
  //
  // The composer targets one scope at a time. The `scopes` prop carries the
  // owning repo plus every dirty submodule; without it a single fallback
  // scope is built from the props (explicit status, else the tab store).
  // Each scope keeps its own draft message (switching preserves what you
  // typed); commit-time options that bind to a specific repository
  // (amend) reset on the switch so they can never leak across repos.

  const scopeList = $derived.by<CommitScope[]>(() => {
    if (scopes !== undefined && scopes.length > 0) return scopes;
    return [
      {
        id: "main",
        label: "",
        repoId,
        status:
          status !== undefined
            ? status
            : (tabStore.tabs.find((tab) => tab.id === repoId)?.status ?? null),
      },
    ];
  });

  /**
   * Chips row visibility: every StatusPanel caller passes `scopes`, and the
   * commit target must ALWAYS be labeled — with a submodule in the tree you
   * can otherwise no longer tell whether the composer commits the main repo
   * or the module (the VS Code complaint: one input, unclear owner). So the
   * row renders even for a lone scope; only callers that pass no `scopes`
   * (the single-scope agent view) get no row.
   */
  const showScopes = $derived(scopes !== undefined && scopes.length > 0);

  let activeScopeId = $state("main");
  /** Per-scope draft messages (switching a chip preserves the drafts). */
  let drafts = $state<Record<string, string>>({});

  const activeScope = $derived(
    scopeList.find((scope) => scope.id === activeScopeId) ?? scopeList[0]!,
  );
  /** All commit/push/AI/metadata IPC targets the active scope's repo. */
  const activeRepoId = $derived(activeScope.repoId);
  /** The active scope's latest status snapshot. */
  const activeStatus = $derived(activeScope.status);

  let message = $state("");
  let messageEl = $state<HTMLTextAreaElement | null>(null);
  let amend = $state(false);
  let signoff = $state(false);
  let noVerify = $state(false);
  let showAuthor = $state(false);
  let authorName = $state("");
  let authorEmail = $state("");
  let error = $state<string | null>(null);
  let committing = $state(false);
  /** The push half of Commit & Push is in flight (instant feedback). */
  let pushing = $state(false);

  /** Retargets the composer: stash the draft, load the target's. */
  function switchScope(id: string): void {
    if (id === activeScopeId) return;
    drafts[activeScopeId] = message;
    activeScopeId = id;
    message = drafts[id] ?? "";
    // Amend folds into a specific repository's HEAD — never carry it (or
    // its inline hint state) across a scope switch.
    amend = false;
    error = null;
  }

  $effect(() => {
    // Repo switch (keep-alive panel): the scope ids belonged to the previous
    // repository — restart on the owning repo with clean drafts.
    void repoId;
    activeScopeId = "main";
    drafts = {};
    message = "";
  });

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

  const commitBusy = $derived(busy(activeRepoId, "commit"));
  /** True while a push op runs for this repo (any entry point). */
  const pushBusy = $derived(busy(activeRepoId, "push"));

  const staged = $derived(hasStagedChanges(activeStatus?.entries ?? []));

  /**
   * Normal commits need something in the index; amending may reuse HEAD's
   * message. While the scope's first status is still loading (null) we
   * keep the button available rather than guessing.
   */
  const needsStaged = $derived(!amend && activeStatus !== null && !staged);
  const hasMessage = $derived(message.trim().length > 0);
  const canCommit = $derived(
    (amend || hasMessage) && !needsStaged && !commitBusy && !committing,
  );
  /** Commit & Push shares Commit's preconditions plus the push phase. */
  const canCommitPush = $derived(canCommit && !pushBusy && !pushing);

  /**
   * The repo the composer currently targets (the active chip's label; ""
   * for the fallback scope when the caller named none). It rides the
   * placeholder and the commit hint so the target stays unambiguous even
   * when the chips row is scrolled or hidden.
   */
  const targetLabel = $derived(activeScope.label);
  const placeholder = $derived(
    targetLabel
      ? `Commit message (${targetLabel})… Ctrl+↵`
      : "Commit message… Ctrl+↵",
  );

  /** Explains what the primary button will do (title/cursor affordance). */
  const commitHint = $derived(
    !hasMessage && !amend
      ? "Write a commit message to enable Commit"
      : needsStaged
        ? targetLabel
          ? `Stage a change before committing to ${targetLabel}`
          : "Stage a change before committing"
        : amend
          ? targetLabel
            ? `Amend the previous commit on ${targetLabel} (Ctrl+Enter)`
            : "Amend the previous commit (Ctrl+Enter)"
          : targetLabel
            ? `Commit staged changes to ${targetLabel} (Ctrl+Enter)`
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

  /**
   * Whether the AI sparkle rides the textarea's corner (the component hides
   * itself when AI is switched off; the reserved padding follows so the
   * message keeps its full width when there is no button). The tiny model
   * override caret shares the corner (plan §6.3) — one generation button,
   * one override menu, no full-width picker row.
   */
  const aiMessageVisible = $derived(
    ai.isEnabled() && ai.isFeatureEnabled("commit-message"),
  );

  $effect(() => {
    // Load signing + hooks state (reloads when the active scope switches).
    void activeRepoId;
    const token = ++metaToken;
    signing = null;
    hooks = [];
    signingInfo(activeRepoId)
      .then((info) => {
        if (token === metaToken) signing = info;
      })
      .catch(() => {
        // Signing info is decorative; ignore failures.
      });
    hooksList(activeRepoId)
      .then((list) => {
        if (token === metaToken) hooks = list;
      })
      .catch(() => {
        // Hook badges are decorative; ignore failures.
      });
  });

  // -- Commit authoring niceties (template prefill, recent messages, ruler) ----

  /** The active scope's persisted-per-root identity (recents/ruler/template keys). */
  const repoRoot = $derived(
    tabStore.tabs.find((tab) => tab.id === activeRepoId)?.root ?? activeRepoId,
  );

  /** Recent accepted messages for the dropdown (reactive via the store). */
  const recentMessages = $derived(commitExtras.recentsFor(repoRoot));

  /** Wrap-ruler column for the repo (per-root setting, default 72). */
  const rulerWidth = $derived(commitExtras.widthFor(repoRoot));

  let templateToken = 0;

  $effect(() => {
    // Hydrate the per-root extras once per repo (outside derived reads),
    // then attempt the one-shot commit-template prefill. The gate reads the
    // draft untracked (a keystroke must not re-run this effect) and marks
    // the repo attempted regardless of the outcome, so a repo is only
    // probed once per session — and only prefills while it is still empty.
    const root = repoRoot;
    commitExtras.ensure(root);
    const allowed = untrack(() =>
      commitExtras.claimTemplatePrefill(root, appSettings.useCommitTemplate, message),
    );
    if (!allowed) return;
    const token = ++templateToken;
    // No config-reading command exists in the backend, so the template
    // source is the `.gitmessage` file at the workdir root (git's
    // commit.template convention). A missing file silently skips.
    readFile(activeRepoId, ".gitmessage")
      .then((bytes) => {
        if (token !== templateToken) return;
        const draft = templateFromBody(new TextDecoder().decode(bytes));
        if (draft !== "") message = draft;
      })
      .catch(() => {
        // No template in this repo — nothing to prefill.
      });
  });

  /** Recalls a recent commit message into the draft. */
  function recallMessage(text: string): void {
    message = text;
  }

  /** The recent-messages dropdown (one entry per stored message). */
  function openRecentMessages(event: MouseEvent): void {
    const entries = recentMessages.map((text) => ({
      id: `recall-${text}`,
      label: text.split("\n", 1)[0] ?? text,
      run: () => recallMessage(text),
    }));
    showMenuAtElement(event.currentTarget as HTMLElement, entries);
  }

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
   * Commits the active scope's staged changes (or amend). With `pushAfter`
   * the existing commit flow is chained into a push of the current branch
   * via the shared header/palette helper (primary remote, plain push — no
   * force, no force-with-lease overrides). A landed commit followed by a
   * failed push says so explicitly; a commit failure renders inline like
   * Commit does.
   */
  async function doCommit(pushAfter = false): Promise<void> {
    if (!canCommit) return;
    committing = true;
    error = null;
    let sha: string | null = null;
    try {
      sha = await commit(activeRepoId, {
        message: message.trimEnd(),
        amend,
        no_verify: noVerify,
        allow_empty: false,
        author: authorOverride(),
        signoff,
      });
    } catch (err) {
      error = describeError(err);
      committing = false;
      return;
    }
    committing = false;
    if (sha === null) return; // unreachable: success always yields a sha

    // Commit landed: remember the message for the recall dropdown, then
    // clear the form, refresh, and optionally push.
    commitExtras.rememberMessage(repoRoot, message);
    message = "";
    authorName = "";
    authorEmail = "";
    showAuthor = false;
    amend = false;
    noVerify = false;
    signoff = false;
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
        activeRepoId,
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

  // -- More commit options ------------------------------------------------------
  //
  // The split caret (plan §6.2) owns the commit-shape actions: Commit & Push,
  // Amend last, Sign off. The ⚙ options menu keeps the drafting toggles that
  // aren't commit actions (skip hooks, author override), and anything active
  // shows as a text chip so the state survives a glance.

  const anyOption = $derived(amend || signoff || noVerify || showAuthor);

  const optionSummary = $derived.by(() => {
    const parts: string[] = [];
    if (amend) parts.push("Amend");
    if (signoff) parts.push("Signed-off");
    if (noVerify) parts.push("Hooks off");
    if (showAuthor) parts.push("Author");
    return parts.join(" · ");
  });

  /** Split-caret menu: commit variants + the amend/sign-off toggles. */
  function openCommitActions(event: MouseEvent): void {
    showMenuAtElement(
      event.currentTarget as HTMLElement,
      [
        {
          id: "commit-push",
          label: "Commit & Push (Ctrl+Shift+↵)",
          icon: iconPush,
          disabled: !canCommitPush,
          run: () => void doCommit(true),
        },
        {
          id: "amend",
          label: `${amend ? "✓ " : ""}Amend last commit`,
          icon: iconAmend,
          run: () => (amend = !amend),
        },
        {
          id: "signoff",
          label: `${signoff ? "✓ " : ""}Sign off`,
          icon: iconSigning,
          run: () => (signoff = !signoff),
        },
      ],
      { align: "end", above: true },
    );
  }

  /** Drafting options menu: skip hooks + author override (amend lives in the caret). */
  function openCommitOptions(event: MouseEvent): void {
    showMenuAtElement(event.currentTarget as HTMLElement, [
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

  /**
   * Hint text that stays truthful in every state, including the disabled
   * ones (the button can't explain itself — disabled buttons swallow
   * pointer events, so the hint is the only voice it has).
   */
  const fixupHint = $derived.by(() => {
    if (fixing || commitBusy || committing) return "Creating the fixup commit…";
    if (!activeStatus?.head) return "Fixup needs a commit — HEAD is missing";
    if (!staged) return "Stage a change to fixup into HEAD";
    return "Commit the staged changes as fixup! into HEAD (message not needed)";
  });

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
      void streamLog(activeRepoId, HEAD_LOG_FILTER, (page) => {
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
      const sha = await commit(activeRepoId, {
        message: fixupMessage(head),
        amend: false,
        no_verify: noVerify,
        allow_empty: false,
        author: authorOverride(),
        signoff: false,
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
  class="commit-bar @container flex-none flex flex-col gap-1 px-2 py-1.5 border-t border-border bg-card text-ui-base"
  aria-label="Create commit"
  onsubmit={(e) => {
    e.preventDefault();
    void doCommit();
  }}
>
  {#if showScopes}
    <!-- Scope chips (plan §6.1): owning repo + every dirty submodule with
         its change count — always shown (even single-scope) so the commit
         target stays legible next to the message box. Horizontally
         scrollable (§6.7): overflow scrolls, chips never shrink into
         illegibility. -->
    <div
      class="scopes flex flex-none items-center gap-1 min-w-0 overflow-x-auto pb-0.5"
      role="group"
      aria-label="Commit scope"
    >
      {#each scopeList as scope (scope.id)}
        {@const counts = submoduleChangeCounts(scope.status)}
        <button
          class="scope-chip inline-flex flex-none items-center gap-1 whitespace-nowrap rounded-full border px-2 py-px text-ui-meta leading-[1.45] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring {scope.id ===
          activeScopeId
            ? "border-primary/40 bg-primary/10 text-primary-ink"
            : "border-border bg-muted/50 text-muted-foreground hover:bg-accent hover:text-foreground"}"
          type="button"
          aria-pressed={scope.id === activeScopeId}
          title={scope.path
            ? `${scope.path} — ${counts.staged} staged, ${counts.changes} unstaged`
            : `${counts.staged} staged, ${counts.changes} unstaged`}
          onclick={() => switchScope(scope.id)}
        >
          <span class="max-w-40 overflow-hidden text-ellipsis">{scope.label}</span>
          <span class="tabular-nums opacity-90">({counts.total})</span>
        </button>
      {/each}
    </div>
  {/if}

  <div class="row-main flex flex-col gap-1 min-h-0">
    {#if recentMessages.length > 0}
      <!-- Recall strip above the textarea: one dropdown of the active
           scope's last accepted messages (newest first). -->
      <div class="recall flex min-w-0 items-center gap-1">
        <Hint label="Recall a recent commit message for this repository">
          {#snippet child({ props })}
            <Button
              {...props}
              class="h-auto min-w-0 max-w-full gap-1 rounded px-1.5 py-px text-ui-micro font-normal text-muted-foreground"
              variant="ghost"
              type="button"
              aria-label="Recent commit messages"
              aria-haspopup="menu"
              onclick={openRecentMessages}
            >
              {#if iconRecents}
                <Icon icon={iconRecents} size={12} />
              {/if}
              <span class="truncate">Recent messages ({recentMessages.length})</span>
            </Button>
          {/snippet}
        </Hint>
      </div>
    {/if}
    <label class="sr-only" for="commit-message">Commit message</label>
    <div class="message-wrap relative w-full min-w-0 overflow-hidden">
      <Hint label="Commit message (Ctrl+Enter commits, Ctrl+Shift+Enter commits and pushes)">
        {#snippet child({ props })}
          <Textarea
            class={`message flex-none w-full min-w-0 box-border field-sizing-fixed min-h-7 max-h-44 resize-none overflow-y-hidden rounded-sm px-2 py-0.5 text-xs leading-[1.35] font-mono bg-background ${aiMessageVisible ? "pr-13" : ""}`}
            placeholder={placeholder}
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
      {#if aiMessageVisible}
        <!-- The AI pair rides the textarea's top-right corner: the sparkle
            generates (the draft it fills belongs to the active scope) and
            the tiny caret beside it overrides the model (plan §6.3) —
            no full-width dropdown in the controls row. The confirm/fail
            overlays are fixed-position and clamped to the viewport, so the
            textarea ancestor cannot clip them. -->
        <div class="absolute right-0.5 top-0.5 z-1 flex items-center gap-0.5">
          <AiModelPicker
            repoId={activeRepoId}
            compact
            popUp
            disabled={commitBusy || committing}
          />
          <AiFeatureButton
            repoId={activeRepoId}
            kind="commit-message"
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
            run={(opts) => ai.generateCommitMessage(activeRepoId, undefined, opts)}
            onResult={(outcome) => {
              const m = outcome.message ?? { subject: outcome.result.text, body: "" };
              message = m.body ? `${m.subject}

${m.body}` : m.subject;
            }}
          />
        </div>
      {/if}
      <!-- Wrap ruler: a static 1px guide at the per-repo commit-width column.
           The box pads px-2 and the text is monospace, so the column lands at
           `padding + width × 1ch` (see rulerLeftCss). Overlay only — no
           scroll/perf interaction with the textarea. -->
      <div
        class="ruler pointer-events-none absolute inset-y-0 border-l border-dashed border-border/80 text-xs font-mono leading-[1.35]"
        style="left: {rulerLeftCss(rulerWidth)}"
        aria-hidden="true"
      ></div>
    </div>

    <!-- One dense controls row (§6.7): config left, the split Commit glued
         right; never wraps into two rows (chips scroll instead, the split
         caret always stays attached). -->
    <div class="controls flex flex-nowrap items-center gap-x-ui-375 w-full min-w-0">
      <div class="opt-switches flex items-center gap-x-1 min-w-0 overflow-hidden">
        {#if anyOption}
          <Hint label="Active commit options">
            {#snippet child({ props })}
              <span class="opt-summary shrink-0 max-w-48 overflow-hidden text-ellipsis whitespace-nowrap px-2 py-px rounded-full bg-muted text-muted-foreground text-ui-micro leading-normal" role="status" {...props}>
                {optionSummary}
              </span>
            {/snippet}
          </Hint>
        {/if}
        <Hint label="Skip hooks or override the author">
          {#snippet child({ props })}
            <Button
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground"
              variant="ghost"
              size="icon-sm"
              type="button"
              {...props}
              aria-label="More commit options"
              aria-pressed={noVerify || showAuthor}
              aria-haspopup="menu"
              onclick={openCommitOptions}
            >
              <Icon icon={iconOptions} size={16} />
            </Button>
          {/snippet}
        </Hint>

        <Hint label={fixupHint}>
          {#snippet child({ props })}
            <!-- The hint trigger is the wrapper: a disabled button swallows
                 pointer events, so the "why is this off" text would never
                 show if the tooltip rode on the button itself. -->
            <span
              {...props}
              class="inline-flex flex-none"
            >
              <Button
                class={`text-[var(--app-added)] hover:bg-accent hover:text-accent-foreground ${fixing ? "[&_svg]:animate-spin" : ""}`}
                variant="ghost"
                size="icon-sm"
                type="button"
                aria-label="Fixup into HEAD"
                disabled={!canFixup}
                onclick={() => void doFixup()}
              >
                <Icon icon={fixing ? iconProgress : iconFixup} size={16} />
              </Button>
            </span>
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
                    class="meta-icon hook inline-flex items-center justify-center text-primary-ink opacity-90"
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

      <!-- The ONE primary action (§3.2): Commit with a split caret (plan
           §6.2). The caret menu carries Commit & Push, Amend last and Sign
           off; the primary button's accessible name stays exactly "Commit".
           The unit never wraps — on a narrow panel the chips row scrolls and
           this row stays single-line. -->
      <span class="commit-actions ml-auto inline-flex flex-none items-stretch overflow-hidden rounded-full max-w-full">
        <Hint label={commitHint}>
          {#snippet child({ props })}
            <Button
              class="h-7 min-w-[4.5rem] gap-ui-35 rounded-l-full rounded-r-none px-3 text-xs"
              {...props}
              type="submit"
              aria-describedby={stagedHintId}
              disabled={!canCommit}
            >
              {#if commitBusy || committing}
                <span>Committing…</span>
              {:else}
                <span>Commit</span>
              {/if}
            </Button>
          {/snippet}
        </Hint>

        <Hint label="More commit actions">
          {#snippet child({ props })}
            <Button
              class="h-7 w-6 flex-none rounded-l-none rounded-r-full border-0 border-l border-primary-foreground/25 px-1 hover:bg-(--primary-hover)"
              {...props}
              variant="default"
              type="button"
              aria-label="More commit actions"
              aria-haspopup="menu"
              disabled={commitBusy || committing}
              onclick={openCommitActions}
            >
              <Icon icon={iconCaret} size={14} />
            </Button>
          {/snippet}
        </Hint>
      </span>
    </div>
  </div>

  {#if showAmendHint}
    <p id="commit-amend-hint" class="amend-hint m-0 flex items-center gap-1 text-muted-foreground text-ui-micro leading-[1.3]" role="status">
      <span class="shrink-0">Nothing staged — amend the last commit instead?</span>
      <Hint label="Fold the next change into the previous commit (amend)">
        {#snippet child({ props })}
          <Button
            class="h-auto min-w-0 rounded-full px-1.5 py-px text-ui-micro leading-[1.4] text-primary-ink hover:bg-accent hover:text-accent-foreground"
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
