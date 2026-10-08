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
