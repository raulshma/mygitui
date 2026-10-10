# IPC Contract Registry

Source of truth for Tauri command names, payloads, channels, and events.
Lane agents extend additively; breaking changes go through the orchestrator.
Types: Rust `src-tauri/src/engine/types.rs` ↔ TS `src/lib/ipc/types.ts` (keep mirrored).

## Commands (M1)

| Command | Args | Returns | Notes |
|---|---|---|---|
| `repo_open` | `path: string` | `RepoInfo` | opens repo, starts watcher, new tab |
| `repo_close` | `repo_id: string` | `void` | stops watcher, drops caches |
| `repo_status` | `repo_id: string` | `RepoStatus` | |
| `repo_diff` | `repo_id, old: DiffSide, new: DiffSide, paths?: string[]` | `FileDiff[]` | small/medium diffs |
| `repo_diff_stream` | `repo_id, old, new, paths?, channel: Channel<FileDiff[]>` | `void` | pages of ~50 files |
| `repo_log_stream` | `repo_id, filter: LogFilter, channel: Channel<LogPage>` | `void` | pages of 500; see LogPage |
| `repo_blame` | `repo_id, path: string, from?: string` | `BlameLine[]` | |
| `repo_refs` | `repo_id` | `[string, string][]` | ref name → sha |
| `repo_file_history` | `repo_id, path: string, channel: Channel<LogPage>` | `void` | follow renames |

`LogPage = { commits: CommitInfo[]; rows: GraphRow[]; next_cursor: string | null; generation: number }`

Streaming rules: every stream carries a `generation` number; when repo state
changes the backend bumps the generation and clients discard stale pages.
Cancel = drop the channel (backend detects closed channel) or next request
with same kind+repo replaces the prior stream.

## Commands (M0, already live)

`secrets_get(key) -> Option<string>`, `secrets_set(key, value)`, `secrets_delete(key)`

## Events (backend → frontend)

| Event | Payload | Meaning |
|---|---|---|
| `cli-args` | `string[]` | second instance launched with args (argv[0] = the executable's own path, stripped backend-side) |
| `repo-changed` | `{ repo_id, paths: string[], head_moved: bool, full: bool }` | watcher fired; `full=true` → resync everything |

## TS mirrors

`src/lib/ipc/types.ts` exports the same shapes (camelCase fields, serde uses
snake_case + `rename_all` where noted — mirror the Rust serde attributes).
`src/lib/ipc/client.ts` provides typed wrappers; UI never calls `invoke` directly.

## Commands (M2 — mutations)

All run through the per-repo serial op queue; progress streams on the
`op-progress` event. Arg keys snake_case (commands use `rename_all`).

| Command | Args | Returns |
|---|---|---|
| `stage` | `repo_id, request: StageRequest` | `void` |
| `stage_all` | `repo_id, unstage: bool` | `void` |
| `commit` | `repo_id, options: CommitOptions` | `String` (new sha) |
| `signing_info` | `repo_id` | `SigningInfo` |
| `hooks_list` | `repo_id` | `HookInfo[]` |
| `branches` | `repo_id` | `BranchInfo[]` |
| `branch_create` | `repo_id, name, from?, checkout: bool` | `void` |
| `branch_switch` | `repo_id, name, force: bool` | `void` |
| `branch_is_merged` | `repo_id, name, into` | `bool` |
| `branch_delete` | `repo_id, name, force: bool` | `void` |
| `branch_rename` | `repo_id, old, new` | `void` |
| `tag_create` | `repo_id, name, target?, message?` | `void` |
| `tag_delete` | `repo_id, name` | `void` |
| `remotes` | `repo_id` | `RemoteInfo[]` |
| `remote_add` / `remote_remove` | `repo_id, name [, url]` | `void` |
| `remote_set_url` | `repo_id, name, url, push: bool` | `void` |
| `fetch` | `repo_id, options: FetchOptions` | `NetStats` |
| `pull` | `repo_id, options: PullOptions` | `NetStats` |
| `push` | `repo_id, options: PushOptions` | `NetStats` |
| `auth_respond` | `op_id: string, username?, password?, store: bool` | `void` |

## Events (M2)

| Event | Payload | Meaning |
|---|---|---|
| `op-progress` | `{ repo_id, op_id, kind, message, pct: number \| null, done: bool, error: string \| null }` | op queue progress; kinds: `stage`, `commit`, `branch`, `fetch`, `pull`, `push`, `clone` |
| `auth-request` | `{ op_id, repo_id, url, kind: "https-user" \| "https-pass" \| "ssh-passphrase", prompt }` | engine needs credentials; FE shows dialog, answers via `auth_respond` |

## Commands (M3 — power + safety)

Same op-queue + op-progress machinery as M2. All `#[tauri::command(rename_all = "snake_case")]`.

| Command | Args | Returns |
|---|---|---|
| `merge_branch` | `repo_id, ref_name, no_ff: bool` | `MergeResult` |
| `merge_abort` | `repo_id` | `void` |
| `conflicts` | `repo_id` | `ConflictFile[]` |
| `conflict_resolve` | `repo_id, path, resolution, custom_content?: number[] (bytes)` | `void` |
| `cherry_pick` | `repo_id, shas: string[]` | `MergeResult` |
| `revert` | `repo_id, shas: string[]` | `MergeResult` |
| `sequencer_abort` | `repo_id` | `void` (aborts in-progress cherry-pick/revert state) |
| `reset` | `repo_id, kind, to` | `void` (checkpoint auto-created before hard) |
| `rebase_start` | `repo_id, plan: RebaseStep[], onto?: string` | `RebaseState` |
| `rebase_state` | `repo_id` | `RebaseState` |
| `rebase_continue` / `rebase_abort` | `repo_id` | `RebaseState` / `void` |
| `stash_list` | `repo_id` | `StashInfo[]` |
| `stash_push` | `repo_id, message?, keep_index, include_untracked` | `void` |
| `stash_apply` / `stash_drop` | `repo_id, index` (+`pop: bool` for apply) | `void` |
| `stash_branch` | `repo_id, name, index` | `void` |
| `worktrees` | `repo_id` | `WorktreeInfo[]` |
| `worktree_add` | `repo_id, path, branch?, new_branch?` | `void` |
| `worktree_remove` | `repo_id, name, force` | `void` |
| `reflog` | `repo_id, name?` | `ReflogEntry[]` |
| `checkpoint_create` | `repo_id, reason` | `CheckpointInfo` |
| `checkpoints` | `repo_id` | `CheckpointInfo[]` |
| `checkpoint_restore` | `repo_id, id` | `void` (new checkpoint of current state first) |
| `checkpoint_gc` | `repo_id, older_than_days` | `u32` (count removed) |
| `ops_preview` | `repo_id, kind: "reset_hard" \| "clean" \| "checkout_force" \| "branch_delete", params` | `PreviewInfo` |

`PreviewInfo = { kind: string; summary: string; files: { path: string; change: string }[] }`
(dangerous ops: FE calls `ops_preview` → dialog → `guard_checkpoint` → confirm → op).

Checkpoints live at `refs/mygitui/checkpoints/<ts>-<reason>`; worktree state is a stash-style
2-parent commit; GC default 30 days; restore = reset soft to checkpoint + apply worktree blob.

## Commands (M4 — housekeeping)

| Command | Args | Returns |
|---|---|---|
| `submodules` | `repo_id` | `SubmoduleInfo[]` |
| `submodule_update` | `repo_id, path, init: bool, recursive: bool` | `void` (op queue, kind `submodule`) |
| `submodule_sync` | `repo_id, path?` (empty = all) | `void` |
| `gitignore_add` | `repo_id, pattern` | `void` (appends to .gitignore, creates if missing) |
| `gitignore_templates` | — | `{name, description, patterns: string}[]` (curated builtin list) |
| `repo_clean` | `repo_id, paths: string[], checkpoint_reason: string` | `u32` (count removed; checkpoint auto-created first) |
| `action_run` | `repo_id, name, command` | `run_id: string` (streams `action-output` events) |
| `action_cancel` | `run_id` | `void` |

## Events (M4)

| Event | Payload |
|---|---|
| `action-output` | `{ run_id, repo_id, name, line, done: bool, exit_code: number \| null }` |

Custom actions config lives FE-side (localStorage `mygitui.actions`), scope global or per-repo.
Layout presets FE-side (localStorage `mygitui.layouts` + per-repo overlay `mygitui.layouts.<root>`).

Gitignore quick-add: `gitignore_templates` ships a curated builtin list (Node,
Rust, Python, C++, Go, Java, macOS, Windows, VS Code, JetBrains, Svelte, Vite,
Terraform, Unreal, Unity); `patterns` is one newline-joined `.gitignore`
snippet, applied line-by-line via `gitignore_add` (idempotent exact-line
append at the repo root; single-line patterns only).

## Commands (M5 — embedded terminal)

Backend: `src-tauri/src/pty.rs` (portable-pty; ConPTY on Windows). One
process-global `PtyRegistry` (tauri-managed state) owns every session; ids
are `pty-<counter hex>`.

ConPTY handshake: conhost opens every session with a cursor-position probe
(`ESC[6n`) and withholds child output until it is answered. The backend
answers the probe and strips it from `pty-output` — the FE never sees it
and must NOT reply to it (a stray cursor report would pollute shell input).

| Command | Args | Returns | Notes |
|---|---|---|---|
| `pty_create` | `repo_id, rows?: u16, cols?: u16` | `session_id: string` | spawns an interactive shell in a fresh pty with cwd = repo root (rows/cols default 24/80; open repo required — no app-cwd sessions) |
| `pty_write` | `session_id, data: string` | `void` | frontend keystrokes/paste to the shell |
| `pty_resize` | `session_id, rows: u16, cols: u16` | `void` | panel size changed; informs the kernel winsize |
| `pty_kill` | `session_id` | `void` (error if unknown/gone) | kills the shell, unregisters; `pty-exit` still fires |

Shell discovery: Windows prefers `pwsh.exe` → `powershell.exe` → `cmd.exe`
(PATH probe); unix uses `$SHELL` → `bash` → `/bin/sh`, spawned with
`TERM=xterm-256color` on top of the inherited env.

## Events (M5)

| Event | Payload | Meaning |
|---|---|---|
| `pty-output` | `{ session_id, data: string }` | terminal bytes, lossy UTF-8, coalesced into ≤1 event per 16 ms window (256 KiB/event cap, surplus split across consecutive events — never dropped) |
| `pty-exit` | `{ session_id, code: number }` | final event of a session; `code` is the real exit code (`1` on signal/kill); the session is unregistered before it fires |

Lifecycle: at most 8 concurrent sessions (`pty_create` rejects past the
cap); a session that exits on its own unregisters itself. Sessions are NOT
killed backend-side on repo close — the FE must `pty_kill` each terminal
panel's session when its tab closes; as a backstop the registry's `Drop`
kills every remaining shell on app exit.

## Commands (M6 — forge / gh-assisted GitHub flow)

All GitHub interaction shells out to the user's `gh` CLI (JSON mode) with the
repo workdir as cwd — mygitui stores no GitHub tokens; `gh` brings its own
auth. Processes run argv-array-only (no shell), streams drained concurrently,
hard timeout with kill on exceed: `--version` 3 s, `auth status` 10 s,
`pr list` 60 s, `pr checks` 30 s, `pr create` 120 s. Commands run on
`spawn_blocking` (no op-queue membership — long ops with their own timeouts,
no event stream). Logic: `src-tauri/src/forge.rs`; tests: `forge_tests.rs`.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `forge_status` | — | `ForgeStatus` | `gh --version` + `gh auth status` |
| `forge_context` | `repo_id` | `ForgeContext` | owner/repo from `origin` URL parse + current branch |
| `pr_create` | `repo_id, base, title, body, draft: bool` | `PrCreated` | `gh pr create --base --title --body-file <tmp> [--draft]`; rejection is structured `ForgeError` |
| `pr_list` | `repo_id` | `PrInfo[]` | `gh pr list --json number,title,headRefName,baseRefName,state,isDraft,url,createdAt --limit 50` |
| `pr_checks` | `repo_id, number` | `CheckInfo[]` | `gh pr checks <n> --json name,state,bucket`; table fallback (✓/✗/- rows) for older gh |

Types (TS mirrors in `src/lib/ipc/client.ts`):

```
ForgeStatus  = { available: bool, version: string, authed: bool }
ForgeContext = { owner, repo, branch, remote_url }
PrCreated    = { url, number }
PrInfo       = { number, title, head_ref_name, base_ref_name, state: "OPEN"|"MERGED"|"CLOSED",
                 is_draft: bool, url, created_at: string | null }
CheckInfo    = { name, state: "pass"|"fail"|"pending"|"skipping" }
ForgeError   = { kind: "NoGh"|"NotAuthed"|"AlreadyExists"|"Other", message, url: string | null }
```

`pr_create` rejects with `ForgeError` (serialized object, not a string):
`NoGh` = gh missing from PATH; `NotAuthed` = auth markers in gh output
(run `gh auth login`); `AlreadyExists` = a PR exists for the branch —
`url` carries the existing PR's URL when one is parseable. Remote URL
parsing accepts `https://github.com/o/r(.git)`, `git@github.com:o/r.git`,
`ssh|git://…github.com/o/r(.git)` (host matched case-insensitively, `www.`
stripped); non-GitHub remotes fail `forge_context` with an explanatory
error and the FE panel explains the gh flow is unavailable.

### M6 FE contracts (orchestrator notes)

- **Mounting**: `ForgePanel.svelte` takes `{ repoId }` and needs a
  `PanelId` registry entry to be placeable — add `"forge"` to the `PanelId`
  union + `PANEL_META` (`src/lib/layout/layoutModel.ts`, read-only for this
  lane) + a `RepoView.svelte` snippet + `TREE_PANELS` membership.
- **AI event contract** (CreatePrDialog prefill, independent of H1's
  commit-message contract — reconcile names if H1 shipped a different pair):
  - dialog **emits** `ai-generate-pr` on `window`:
    `detail = { repoId: string, commits: [{ sha, summary }] }` (up to 10
    recent commits, may be empty);
  - dialog **listens** for `ai-pr-result` on `window`:
    `detail = { subject: string, body: string }` fills the title/body
    fields (user typing clears the pending state);
  - RepoView **emits** `ai-pr-error` on `window` when the generation
    fails (added post-M12): `detail = { message: string }` — the
    dialog clears its "Waiting for AI…" pending state and shows the
    message inline (retry / manual fill).
- **Open in browser** goes through `@tauri-apps/plugin-opener`'s JS API
  (`openUrl`), guarded outside Tauri — no Rust command needed.

## Commands (M7 — contribution statistics)

Pure backend reads (repo mutex + `spawn_blocking`, no op queue, no generation
bump). Logic: `src-tauri/src/engine/stats.rs` (inherent `*_impl` methods on
`Libgit2Engine`, the M2-lane convention); tests: `engine/stats_tests.rs`.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `commit_activity` | `repo_id, max_days: u32, author?: string` | `DayCount[]` | commits per local day, day ascending; zero days omitted |
| `contributor_stats` | `repo_id, max_days: u32` | `Contributor[]` | per-author rollups, count descending |

```
DayCount    = { day: string,        // "YYYY-MM-DD", committer local date
                count: number }
Contributor = { name, email, count: number,
                first_day: string,      // "YYYY-MM-DD"
                last_day: string }
```

Walk semantics (both commands): every commit reachable from
`refs/heads/*`, `refs/remotes/*`, `refs/tags/*` or HEAD (revwalk-deduped —
shared history counts once). Internal `refs/mygitui/*` namespaces
(checkpoint snapshots) are deliberately NOT pushed and never count as
activity. Window is committer time in `[now - max_days, now]`; buckets use
the committer timestamp plus its recorded UTC offset ("local date").
`author` on `commit_activity` is a case-insensitive substring matched
against the author name OR email (blank/None = no filter). Contributor
identity is the exact `(name, email)` pair. The walk caps at 100k commits
per call. TS mirrors live in `src/lib/ipc/client.ts` (`DayCount`,
`Contributor`, `commitActivity`, `contributorStats`).

### M7 FE contracts (lane I1)

- **Stats panel**: `PanelId "stats"` (registry + `PANEL_META` + default
  left tab group, placed before `terminal`); `StatsPanel.svelte` takes
  `{ repoId, root }` and hosts:
  - the heatmap grid (GitHub-style: weeks × 7 weekday rows, 53 columns,
    last column ends today; a real `<table>` with per-cell
    `aria-label="N commits on DATE"`, intensity via `--m3-primary`
    `color-mix` buckets over quartile thresholds of the nonzero counts),
  - summary cards (commits, active days, avg per active day, current +
    longest streaks — an idle today defers the current streak to
    yesterday, GitHub semantics),
  - an author filter (datalist from the contributor rollups → re-fetch
    `commit_activity` with the author substring),
  - the contributor list (name, count bar, active range),
  - the **"Colors…" popover** — the editor for branch color rules. It was
    placed in StatsPanel by design (simplest single home for M7's
    settings-ish UI).
- **Branch color rules**: per-repo localStorage
  `mygitui.branchcolors.<root>` = `[{ pattern, color }]` (array order =
  match priority). Pattern grammar: `feature/*` prefix, `*-hotfix` suffix,
  `main` exact, `*` catch-all; case-sensitive. `branchColorForRefs(refs,
  rules)` matches SHORT ref decoration names ("main", "origin/main");
  `HEAD -> …` decorations never match; first matching rule wins. Store +
  pure matcher: `src/lib/stats/branchColors.svelte.ts`.
- **Commit bookmarks**: per-repo localStorage
  `mygitui.bookmarks.<root>` = `[{ sha, label }]`. Store:
  `src/lib/components/graph/bookmarks.svelte.ts`
  (`add`/`remove`/`list`/`toggle`/`shas`; injectable storage). M7 ships
  the store + graph display only — creation UI is intentionally out of
  scope (display-only decorations).
- **GraphCanvas additive props** (beyond the B2 contract):
  `bookmarks?: ReadonlySet<string>` (shas → small dashed ring marker on
  the node, same geometry as the HEAD ring but dashed) and
  `branchColors?: (refs: string[]) => string | null` (resolved per drawn
  row against the row commit's `refs`; a hit recolors that row's node +
  outgoing edges instead of the lane palette). `render.ts` threads both
  through `DrawParams` as the additive optional fields
  `bookmarks` / `colorOverrides` (`Map<rowIndex, color>`).
- **Wiring**: `RepoView` hydrates both per-root stores in an `$effect`
  (never inside `$derived`, like the layout overlays) and passes
  `bookmarks`/`branchColors` into `HistoryView` (additive optional props)
  → `GraphCanvas`. Popout history windows omit the props (safe:
  decorations are optional).

---

## M9 — Complete git core (engine + UI)

### Engine additions (all mirrored in `src/lib/ipc/types.ts`)

- **`MergeOptions`** `{ no_ff, squash, no_commit, favor }` — `favor` is
  `none | ours | theirs` (git `-X` file favor). `merge_branch(repo_id,
  ref_name, opts)` replaces the old `no_ff: bool` arg. New
  `MergeOutcome` values: `squashed` (result staged + `SQUASH_MSG`
  written, NO merge state — the next commit is a normal 1-parent
  commit, also when the squash merge conflicted) and `no_commit`
  (clean merge, MERGE_HEAD stays — the finishing commit goes through
  the normal commit path).
- **Commit during a merge**: the finishing commit now takes MERGE_HEAD
  as its second parent and consumes the merge state
  (MERGE_HEAD/MERGE_MSG/MERGE_MODE via `cleanup_state`).
  `RepoStatus::merging` drops. Amend during a merge is rejected.
- **Pull `--rebase` (diverged)**: replays the local-only commits onto
  the fetched tip through the custom rebase engine. A conflict pauses
  in rebase state — `RepoStatus::rebasing` (the custom rebase state
  file is now visible to status + conflict-source detection) and the
  FE rebase monitor/conflict editor drive it; abort restores the
  pre-pull state.
- **`PushOptions`** gains `force_with_lease` (refuses without a
  tracking ref — the tracking ref IS the lease value), `refs: string[]`
  (explicit refs, bare names allowed; multi-ref push), `tags: true`
  (`--tags`), `delete: true` (delete the remote refs named in `refs`;
  `NetStats.updated_refs` reports `"(deleted)"` as the new value).
- **`tag_list`** → `TagInfo[]` `{ name, sha, target, annotated, tagger?,
  message? }` (`sha` = the ref's direct target: the tag object for
  annotated, the commit for lightweight). **`tag_create_signed`**
  creates signed annotated tags via the git CLI (`git tag -s`,
  sanitized env, message via `-F` file).
- **`remote_branches`** → `RemoteBranchInfo[]` `{ remote, name, sha,
  tracked_by? }` (HEAD refs skipped). **`branch_checkout_remote`**
  creates + checks out a local branch tracking `<remote>/<name>`
  with upstream configured; returns the local name; errors when it
  already exists.
- **`discard`** `{ targets: StageTarget[] }` — `git checkout --` on
  steroids: whole file = index+workdir back to HEAD (or deleted when
  not in HEAD); hunk/line targets = workdir-only line surgery (the
  index is untouched; libgit2 `apply(WorkDir)` cannot be used because
  its preimage is the index). A checkpoint is created BEFORE any side
  effect (same ordering rule as `repo_clean`).
- **Regex log filter**: `LogFilter.regex` is live (Rust `regex` crate,
  compiled once per query; invalid patterns → `Invalid`).
- **Diff Index→Commit** side pair is implemented (index materialized
  to a throwaway tree; old side = Index).
- **`mergetool_info`** → `MergetoolInfo` `{ tool?, gui_tool? }` (git
  config); **`mergetool_run(repo_id, path, tool?)`** launches
  `git mergetool --no-prompt` for one path — deliberately NOT on the
  op queue (interactive, minutes-long; watcher still fires). The
  built-in 3-way editor stays the default.
- **`gitignore_apply_template(repo_id, name)`** — appends a builtin
  template's patterns (idempotent per line) under a `# <name>` header
  in one write.

### Frontend

- **Typed event bus**: `src/lib/palette/events.ts` (`emitUiEvent` /
  `onUiEvent`) — replaces the dead CustomEvent wiring hints. Panels
  subscribe in `$effect`s: RepoView (`focus-panel`, `open-conflicts`,
  `conflicts-recheck`), HistoryView (`history-*`,
  `history-select-commit`, `commit-action`), BranchPanel
  (`branches-focus-*`, `branches-cleanup`), DiffViewer
  (`diff-toggle-mode`).
- **Global context menu**: `src/lib/components/menu/contextMenu.svelte.ts`
  (`show`/`hide`/`move`, keyboard-accessible) + `ContextMenu.svelte`
  (mounted once in App.svelte). Menus: history commits (cherry-pick,
  revert, rebase-from-here, bookmark, copy), changed files (file
  history, blame), status rows (stage/unstage/discard/ignore/file
  history/open diff), branches (switch/merge/push/upstream/rename/
  reset/delete/copy), reflog entries (copy/show/reset/branch).
- **MergeDialog** (`src/lib/components/merge/`) — ff policy, squash,
  --no-commit, -X favor; wired from BranchPanel rows + context menu.
- **TagsSection** (in BranchPanel) — list/create (lightweight,
  annotated, signed)/delete/push-tags.
- **SubmodulePanel is registered** (PanelId `submodules`).
- **FileHistoryView** — popout `?panel=filehistory&repo=<id>&path=`
  (rename-following walk via `HistoryStore({ follow: true })`);
  opened from changed-file menus/buttons.
- **CommitDetail** (`src/lib/components/panels/CommitDetail.svelte`) —
  the commit detail pane shared by HistoryView and its own popout
  `?panel=commitdetail&repo=<id>&sha=<full sha>` ("Pop out" in the
  detail header; `PopoutCommitDetail` self-wires the `CommitInfo`
  fetch via a one-root log walk, parent clicks retarget the window;
  the bookmark star stays hidden in popouts — bookmarks persist per
  worktree root, which only the main window knows).
- **Hunk actions in DiffViewer** — optional `repoId` +
  `hunkStaging` ("stage"|"unstage") + `hunkDiscard` props; RepoView's
  working-copy pane enables them only when the shown file has no
  staged changes (head→worktree hunks must coincide with the
  index→workdir hunks the backend targets).
- **Branch cleanup wizard** — "Clean merged…" scans
  `branch_is_merged` into HEAD, checkbox list, batch delete.
- **Reflog recovery** — reset-to (ResetDialog) / branch-from-here
  (PromptDialog) / show-in-history; BlameView sha-click selects the
  commit in history (stale M2 toast removed).
- **Bookmark UI** — context-menu toggle on history commits
  (`mygitui.bookmarks.<root>`); the M7 "display-only" note is
  obsolete.
- **Regex filter checkbox is live** in the history filter bar
  (`HistoryFilterFields.regex`).
- **ConfirmDialog / PromptDialog**
  (`src/lib/components/safety/`) replace every `window.confirm` /
  `window.prompt` (stash drop, remote/worktree removal, layout
  save-as, aborts, GC, merged-delete).
- **GitignoreGallery** — template gallery dialog from the StatusPanel
  toolbar ("Templates…").
- Palette: `branches.clean-merged` command added; PopoutPanel
  extended with `filehistory`.

---

## M10 — Power git (bisect, describe, autosquash, exec, pickaxe, remote branches)

### Engine (`<gitdir>/mygitui/bisect.json` persistence; probe = first-parent
walk midpoint, known-bad tip excluded)

- **`bisect_start(repo_id, bad?, good?)`** → `BisectState` — HEAD detaches
  onto the first probe (bad defaults to HEAD; guards: same good/bad,
  unknown refs, double start). **`bisect_state`** → `BisectState`
  (`active: false` singleton when none). **`bisect_mark(mark)`**
  (`good|bad|skip`) → next `BisectState` — moves the good/bad bound or
  adds to `skipped`, checks out the next midpoint; when
  `remaining === 0` the `bad` tip IS the first bad commit (the bisect
  stays active until `bisect_reset` for inspection).
  **`bisect_reset`** restores the original branch/HEAD and removes the
  state file. `BisectState` = `{ active, bad, good?, current?, remaining,
  skipped[], orig_head, orig_branch? }`.
- **`describe(repo_id, spec)`** — `git describe --tags` (libgit2
  `Object::describe`); falls back to the 7-char sha when no tag is
  reachable.
- **`autosquash_plan(repo_id, base)`** → `RebaseStep[]` — `fixup!` /
  `squash!` commits in `base..HEAD` moved right after their (subject-
  matched) targets, as `fixup`/`squash` steps; everything else `pick`.
  Feeds `rebase_start` unchanged.
- **Rebase `exec` action** — `RebaseStep { action: "exec", new_message:
  <command> }` runs in the workdir (`sh -c` / `cmd /c`, sanitized env).
  A failing exec pauses the rebase: `RebaseState.paused_for_exec` +
  `exec_error` (new additive fields); `rebase_continue` RE-RUNS the
  failed step (git semantics); validation demands a non-empty command;
  plans of only exec steps require `onto`.
- **Pickaxe**: `LogFilter.pickaxe` (optional) — `-S` semantics: a commit
  matches when its first-parent patch adds or removes the string
  (per-hunk patch line scan, rename detection on). Mirrored:
  `HistoryFilterFields.pickaxe` + the History filter-bar
  "Changes containing… (-S)" input.

### Frontend

- **BisectBanner** (`src/lib/components/bisect/`) — mounted in RepoView
  under the conflict banner. Inactive: "Bisect…" launcher → start form
  (bad prefills HEAD). Active: probe sha, remaining count, Good/Bad/Skip
  marks, Reset; on completion the bad tip gets "Show in history" (typed
  bus). Palette: `bisect.start` → `bisect-open-start` event.
- **Describe** line in the history commit-detail meta (lazy per
  selection, decorative failures swallowed).
- **RebasePlanner "Autosquash" button** (visible when a base is set) —
  replaces the plan rows with `autosquash_plan` output, summaries
  preserved by sha lookup.
- **RemotePanel "Branches" toggle per remote** — lists
  `remote_branches()` with per-branch Checkout (tracking local branch;
  hidden when already tracked) and Delete (push `:refs/heads/<name>`).

---

## M11 (cont.) — maintenance/sparse/LFS/archive, syntax highlighting, i18n

### Backend (`src-tauri/src/maintenance.rs`; sanitized CLI layer)

- **`repo_health(repo_id)`** → `RepoHealth` `{ git_size_bytes,
  worktree_size_bytes, loose_objects, packed_objects?, pack_files,
  has_commit_graph, commit_graph_bytes, packed_refs, last_gc? }` — pure fs
  walk (bounded depth 3). `last_gc` is the mtime of `<gitdir>/gc.log`
  (git writes it only on gc warnings) — null for cleanly maintained
  repos, not a true "last gc ran" timestamp.
- **`maintenance_run(repo_id, op)`** — op queue; `gc` (`git gc --auto`),
  `prune`, `commit_graph` (`git commit-graph write --reachable
  --split=replace` — revwalk acceleration; the Health panel surfaces a
  "none — write one" hint when absent), `pack_refs` (`--all --prune`),
  `count_objects` (parses `count-objects -v`).
- **`archive(repo_id, spec, format, destination)`** — `git archive`;
  formats `zip|tar|tar.gz`; metacharacter guard on spec/destination.
- **`sparse_info` / `sparse_apply(repo_id, patterns, add)`** — cone-mode
  `git sparse-checkout`; empty pattern list = `disable` (full checkout).
- **`lfs_status`** → `LfsStatus` `{ installed, version?, tracked_patterns }`
  (`git lfs version` probe + `filter=lfs` patterns from tracked
  `.gitattributes`); **`lfs_run(subcommand)`** — `pull|push|fetch|install`
  only (allowlist).
- **`clone_blobless(url, destination, depth?)`** — `git clone
  --filter=blob:none` CLI path (libgit2 cannot filter-promote).
- **Pickaxe budget**: `-S` page requests scan at most
  `LOG_CURSOR_SCAN_LIMIT` (100k) commits per page (~2.6 ms/commit measured
  on the bench fixture — `git log -S` is inherently expensive; the budget
  keeps huge repos responsive and the stream cancellable). On exhaustion a
  page returns the last examined commit as `next_cursor`, so the stream
  resumes in a fresh page — nothing beyond the budget is silently dropped.

### Frontend

- **RepoPanel** (PanelId `health`, label "Repo") — health facts grid +
  maintenance buttons + archive export (save-file picker) + sparse pattern
  editor + LFS section (install guidance when the binary is absent).
- **HistoryView commit context menu**: "Export archive…".
- **CloneDialog**: blobless (`--filter=blob:none`) checkbox.
- **Syntax highlighting** (`src/lib/diff/highlight.ts`, Shiki, lazy
  singleton + on-demand grammars): the virtualized render path never
  awaits — `cachedTokens` is synchronous; background parses bump a version
  counter (`onTokensLanded`) that repaints the visible window. Line cache
  keyed (scheme, lang, text), capped 20k entries, 800-char line cap,
  permanent plain-text degrade on any failure. Replaces word-level
  highlights for tokenized lines. `rowModel` gained a parallel
  `fileIndexes` array (row → owning file) — additive, no row shape change.
- **i18n groundwork** (`src/lib/i18n/`): `t(key, vars)` with `{placeholder}`
  interpolation, English catalog seeded (panels/actions/dialogs/toasts/
  states), fallback chain active→English→key, `setLocale` with unknown-
  locale guard. Applying more strings is mechanical; `Intl` keeps owning
  dates/numbers.
- **Benches** (`refs` group): `tag_list_500_commits` ≈ 1.75 ms,
  `describe_head` ≈ 3.7 ms, `log_pickaxe_substring` ≈ 1.3 s/500-commit
  full walk (budgeted as above).

## M12 — gap closure: safety, verification, infra gates

Additive contracts only; all commands take `repo_id` unless noted.

| Command | Args → Result | Notes |
| --- | --- | --- |
| `commit_signature` | `sha` → `CommitSignature` | `{signed, kind: "gpg"\|"ssh"\|null, valid: bool\|null, detail}`. Verified via `git verify-commit` (CLI route) so gpg/ssh-agent match terminal behavior; `valid: null` = signed but no key / no `gpg.ssh.allowedSignersFile`. |
| `branch_trash_list` | → `[BranchTrashEntry]` | Entries from `refs/mygitui/trash/*`, newest first: `{id, name, sha, deleted_at}`. `branch_delete` now snapshots the tip here before deleting. |
| `branch_trash_restore` | `id`, `new_name?` → `string` | Recreates the branch (optionally renamed) at the trashed sha; returns the branch name. Errors when the name exists and no `new_name`. Trash entries are cleaned by checkpoint GC (30d). |
| `worktree_prune` | → `u32` | `git worktree prune`; returns pruned count. `worktrees` now also lists the main worktree (`is_main: true`) and prune stale admin files itself. |
| `bisect_log` | → `[BisectLogEntry]` | `{mark, sha, at}` oldest first; also mirrored into `BisectState.log`. |
| `op_cancel` | `op_id` → `bool` | Best-effort cancel of a queued/running mutation. Queued → dropped; running → stops at the next cooperative checkpoint (long libgit2 ops check between steps). Returns false when already finished. |
| `cli_args_initial` | → `[string]` | argv captured at first launch (`mygitui <path>`), consumed once; second-launch args keep arriving via the `cli-args` event. |
| `opencode_detect` | → `OpencodeDetection` | `{installed, path, version, major}` — PATH × PATHEXT scan (never cached) + `opencode --version` (4s timeout, sanitized env). |
| `opencode_serve_status` | → `OpencodeServeState` | `{running, url, port, error}` of the app-managed `opencode serve` child; reaps a child that exited. Starts nothing. |
| `opencode_serve_start` | `cwd?: string` → `OpencodeServeState` | Idempotent: returns the live server or spawns on a free 127.0.0.1 port (ready = TCP connect; ≤30s wait). `cwd` (the active repo root) becomes the server's working directory for project resolution. Error copy names the fix ("not found on PATH — install opencode…"). |
| `opencode_serve_stop` | → `OpencodeServeState` | Kills (tree-kill on Windows) and reaps the managed server; no-op when none runs. |
| `open_popout` | `label`, `query`, `title` → `void` | Creates (or focuses, when `label` exists) a popout webview window: `query` is the window's own `?panel=…&repo=…` URL query joined onto the app origin. Built Rust-side instead of via core `create-webview-window` so the main window's `additionalBrowserArgs` are copied — WebView2 rejects a second webview environment on the same user data folder (0x8007139F). 960×680 (min 420×300), splash background; the capability set covers `popout-*` labels. |

Shape changes (additive, mirrored in `src/lib/ipc/types.ts`):
- `LogFilter.pickaxe_regex` — pickaxe `-G` (patch regex) alongside `-S`.
- `RepoHealth.fsck_dangling` / `fsck_samples` — filled from the `<gitdir>/mygitui/fsck.json` cache written by `maintenance_run(op: "fsck")` (the op parses `git fsck --no-progress --dangling` on both streams and caches count + up to 10 sample shas).
- `WorktreeInfo.is_main`; `MergetoolInfo.path` / `cmd` (mergetool.<tool> introspection).
- `CommitSignature`, `SignatureKind`, `BranchTrashEntry`, `BisectLogEntry` (new types).

Platform:
- Dynamic color: `os_accent_color` (Windows DWM registry accent → seed) + `mygitui.seed` localStorage override (palette: "Appearance: Set/Reset accent seed color…"); resolution order override → OS accent → baseline.
- opencode managed serve (M12): `opencode_detect` (PATH × PATHEXT scan, never cached — misses re-scan so fresh installs are found; `--version` probe → `{installed, path, version, major}`), `opencode_serve_status/start/stop` over the `OpenCodeServerRegistry` (one app-owned `opencode serve --hostname 127.0.0.1 --port <free>` child; ready = TCP connect succeeds; stdout `… listening on http://…` parsed when announced; `run()`'s `RunEvent::Exit` handler calls `registry.stop()` → `taskkill /T` tree-kill — Tauri does NOT drop managed state on exit, a registry `Drop` alone would not fire). Frontend resolution (`resolveOpencode`): `opencodeEnabled === false` → disabled (no probe, no spawn), else `opencodeMode` — managed (default; attach when a legacy manual `opencodeUrl` exists and no mode is set) → provider asks the registry for the URL and lazily starts the server; attach keeps the v1 autodiscovery (manual URL → `127.0.0.1:4096`). Outside Tauri managed degrades to attach.
- AI startup probe: the `AiStore` singleton runs `supervisor.checkAll()` once at construction (real app only, not tests/browser) — the health chip reflects reality immediately; managed mode spawns the server through that probe.
- opencode SSE: `subscribeOpencodeEvents` consumes `GET /event` (fetch-based, basic-auth) and feeds the supervisor's `noteTransportEvent` — transport liveness refreshes `lastCheck`; a down backend re-probes instead of trusting the stream. Retry stays with the supervisor. The base URL resolves through the provider's `health()` so managed-mode random ports work.
- Deep links: `mygitui://open?path=<abs>` — `tauri-plugin-deep-link`, schemes in `tauri.conf.json`, frontend router `src/lib/entry/deeplink.ts` (same open-repo-tab path as `cli-args`).
- First-launch argv: `CliArgs` state captured in `run()`, served by `cli_args_initial`.
- Updater: release workflow overlays `plugins.updater` (pubkey + GitHub latest.json endpoint) only when the `TAURI_UPDATER_PUBKEY` secret exists; signing env vars always wired (`TAURI_SIGNING_PRIVATE_KEY[_PASSWORD]`).
- Nightly: criterion `--baseline nightly` compare (fails on regression) + `examples/soak.rs` real RSS soak (250MB gate, monotonic-growth detector, `SOAK_MINUTES`).
- Fixtures: `signed` (ssh-keygen + `gpg.format=ssh` signed commits + allowed_signers), `detached` (detached HEAD mid-history), `files-100k` (`--files N` scale knob).

Quality gates:
- `prop_tests.rs`: proptest roundtrips — hunk-apply selection and checkpoint restore over randomized worktree states.
- Benches added: `cold/open_plus_status`, `diff/synthetic_10k_hunks`, `search/log_filter_regex_page`, `history/file_history_page`, `bisect/mark_step`.
- Playwright smoke (SPA-level, mocked `__TAURI_INTERNALS__`): app boot → open repo → stage → commit dialog.
