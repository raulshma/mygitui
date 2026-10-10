<script lang="ts">
  /**
   * Commit bar (M2) — sits at the bottom of the repo workspace.
   *
   * Message textarea (Ctrl/Cmd+Enter commits), Amend switch, --no-verify
   * switch ("skip hooks"), signing badge (`signing_info`), per-hook badges
   * (`hooks_list`, present + executable → "pre-commit will run"), an author
   * override disclosure, and the Commit button (spinner while a commit op
   * runs per the OpStore; disabled with no message unless amending).
   * The side column groups the controls into rows: status chips, then the
   * option switches (Amend / --no-verify left, Author… right), then the
   * actions row (Fixup left; AI generate + Commit pinned bottom-right). The
   * author override fields expand as a full-width row under the message row.
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
  import type { GitSignature, HookInfo, LogFilter, SigningInfo } from "$lib/ipc/types";
  import { busy } from "$lib/stores/ops.svelte";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { Switch } from "$lib/components/m3";
  import CommitMessageButton from "$lib/components/ai/CommitMessageButton.svelte";
  import { fixupMessage, hasStagedChanges, headCommitInPage } from "$lib/commit/commitBarModel";

  let {
    repoId,
    onCommitted = undefined,
  }: {
    repoId: string;
    /** Called after a successful commit (RepoView refreshes the status). */
    onCommitted?: () => void;
  } = $props();

  let message = $state("");
  let amend = $state(false);
  let noVerify = $state(false);
  let showAuthor = $state(false);
  let authorName = $state("");
  let authorEmail = $state("");
  let error = $state<string | null>(null);
  let committing = $state(false);

  let signing = $state<SigningInfo | null>(null);
  let hooks = $state<HookInfo[]>([]);
  let metaToken = 0;

  /** Hooks that will actually run on commit (present + executable). */
  const runningHooks = $derived(
    hooks.filter((h) => h.present && h.executable && !h.kind.startsWith("pre-push")),
  );

  const commitBusy = $derived(busy(repoId, "commit"));
  const canCommit = $derived((amend || message.trim().length > 0) && !commitBusy && !committing);

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

  // -- M12: "Fixup into HEAD" quick action -------------------------------------

  let fixing = $state(false);

  /** The repo's latest status snapshot (staged detection + HEAD sha). */
  const status = $derived(
    tabStore.tabs.find((tab) => tab.id === repoId)?.status ?? null,
  );
  const staged = $derived(hasStagedChanges(status?.entries ?? []));
  const canFixup = $derived(
    Boolean(status?.head) && staged && !commitBusy && !committing && !fixing,
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
    const headSha = status?.head ?? null;
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
  class="commit-bar"
  aria-label="Create commit"
  onsubmit={(e) => {
    e.preventDefault();
    void doCommit();
  }}
>
  <div class="row-main">
    <label class="sr-only" for="commit-message">Commit message</label>
    <textarea
      id="commit-message"
      class="message"
      placeholder="Commit message… (Ctrl+Enter to commit)"
      rows="2"
      bind:value={message}
      onkeydown={onMessageKeydown}
      disabled={commitBusy || committing}
    ></textarea>

    <div class="side">
      <div class="chips" aria-hidden="false">
        {#if signing?.active}
          <span
            class="chip chip-sign"
            title={signing.key_id ? `Signing key ${signing.key_id}` : "Commits will be signed"}
          >
            signed: {signing.format}
          </span>
        {/if}
        {#each runningHooks as hook (hook.kind)}
          <span class="chip chip-hook" title="This hook is present and executable">
            {hook.kind} will run
          </span>
        {/each}
      </div>

      <div class="options">
        <div class="opt-switches">
          <Switch bind:checked={amend} title="Amend the previous commit">
            Amend
          </Switch>
          <Switch
            bind:checked={noVerify}
            title="Skip the pre-commit and commit-msg hooks (--no-verify)"
          >
            --no-verify
          </Switch>
        </div>
        <button
          class="disclosure"
          type="button"
          aria-expanded={showAuthor}
          title="Override the commit author"
          onclick={() => (showAuthor = !showAuthor)}
        >
          Author…
        </button>
      </div>

      <div class="actions">
        <button
          class="disclosure fixup"
          type="button"
          title="Commit the staged changes as fixup! into HEAD (message not needed)"
          disabled={!canFixup}
          onclick={() => void doFixup()}
        >
          {fixing ? "Fixing…" : "Fixup into HEAD"}
        </button>

        <div class="primary">
          <CommitMessageButton
            {repoId}
            disabled={commitBusy || committing}
            onResult={(d) => {
              message = d.body ? `${d.subject}

${d.body}` : d.subject;
            }}
          />

          <button class="go" type="submit" disabled={!canCommit}>
            {#if commitBusy || committing}
              <span class="spinner" aria-hidden="true"></span>
              <span>Committing…</span>
            {:else}
              <span>Commit</span>
            {/if}
          </button>
        </div>
      </div>
    </div>
  </div>

  {#if showAuthor}
    <div class="author">
      <label>
        <span class="al">Name</span>
        <input
          type="text"
          class="ai-input"
          placeholder="Ada Lovelace"
          bind:value={authorName}
          aria-label="Author name override"
        />
      </label>
      <label>
        <span class="al">Email</span>
        <input
          type="email"
          class="ai-input"
          placeholder="ada@example.com"
          bind:value={authorEmail}
          aria-label="Author email override"
        />
      </label>
    </div>
  {/if}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}
</form>

<style>
  .commit-bar {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.375rem 0.5rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.8125rem;
  }

  .row-main {
    display: flex;
    gap: 0.5rem;
    min-height: 0;
  }

  .message {
    flex: 1;
    min-width: 0;
    resize: none;
    font: inherit;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.3rem 0.5rem;
  }

  .message:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .side {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    width: 18.5rem;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    min-height: 1rem;
  }

  .chip {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.05rem 0.5rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    white-space: nowrap;
  }

  .chip-sign {
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
    background: var(--m3-secondary-container, transparent);
    border-color: transparent;
  }

  .chip-hook {
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
    background: var(--m3-tertiary-container, transparent);
    border-color: transparent;
  }

  .options {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .opt-switches {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    min-width: 0;
  }

  .options .disclosure {
    margin-left: auto;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    margin-top: auto;
  }

  .primary {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
  }

  .disclosure {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .disclosure[aria-expanded="true"] {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .fixup {
    color: var(--m3-tertiary, var(--m3-on-surface-variant, var(--m3-on-surface)));
  }

  .fixup:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .disclosure:focus-visible,
  .go:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .go {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    min-width: 6.5rem;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.35rem 1.25rem;
    cursor: pointer;
  }

  .go:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .spinner {
    width: 0.75rem;
    height: 0.75rem;
    border: 2px solid color-mix(in srgb, var(--m3-on-primary) 40%, transparent);
    border-top-color: var(--m3-on-primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .author {
    display: flex;
    gap: 0.5rem;
    padding-top: 0.125rem;
  }

  .author label {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
  }

  .al {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .ai-input {
    flex: 1;
    min-width: 0;
    font: inherit;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.375rem;
  }

  .ai-input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .error {
    margin: 0;
    color: var(--m3-error, inherit);
    font-size: 0.72rem;
    overflow-wrap: anywhere;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
