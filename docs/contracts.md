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
| `cli-args` | `string[]` | second instance launched with args |
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
    fields (user typing clears the pending state).
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
