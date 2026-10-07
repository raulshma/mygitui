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
