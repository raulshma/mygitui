# Native Contract Reference

mygitui is one process: an **iced 0.14** UI on top of a service layer and a
git2-rs engine. There is no IPC, no webview, and no generated type mirror —
the UI calls the `service::*` functions in-process and receives
[`BackendEvent`](../../app/src/service/events.rs)s over an in-process
broadcast bus.

```
iced UI (app/src/ui/)  ⇄  service layer (app/src/service/*)  ⇄  engine (app/src/engine/*)
        ▲                            │                             │
        └──── BackendEvent bus ──────┘                    git2-rs (primary) │ git CLI (fallback)
                                                          remote SSH engine (app/src/remote/)
```

Rust types in `app/src/engine/types.rs` are the single source of truth; the
same structs are shared by both sides of every call. `serde` derives exist
only where data actually crosses a file boundary (config, bisect/fsck state
files) or is serialized into events for logging — never for a wire format.

Layer rules:

- **UI** (`ui/`) holds no git state of its own: it renders payloads from
  service calls and events, and dispatches every mutation through a service
  fn.
- **Service** (`service/*`) is a thin per-domain function surface. Every
  body takes `&AppServices` and returns `Result<T, String>`; engine calls
  run on the per-repo serial op queue (see "UI ⇄ service contract").
- **Engine** (`engine/*`) is synchronous git2-rs work plus a sanitized CLI
  layer for what libgit2 cannot do (hooks, signing, gc/repack, `git tag -s`,
  fsck, sparse-checkout, LFS). The remote twin (`remote/engine.rs`) runs
  `git` on an SSH device and parses porcelain output into the same DTO
  types.

## Service surface

Every function below is a public `service::*` (or root-backend-module) fn
whose signature/doc comment in code is authoritative. All async fns take
`&AppServices` as the first argument; `repo_id: RepoId` selects the open
repo tab and errors with `"repo not open: …"` when absent. Local and remote
(SSH device) repos share one surface: when the handle carries a remote
backend the same fn enqueues the remote twin on the same serial queue.

### Repo lifecycle — `service/repo.rs`

| Fn | Semantics |
|---|---|
| `repo_open(path)` | Discovers the repo at/above `path` (canonicalized first so recents survive cwd changes), opens it once, starts its FS watcher; returns `RepoInfo`. Reuses the existing handle for an already-open root. |
| `repo_close(repo_id)` | Stops the watcher, cancels the repo's log/file-history streams, shuts the op queue down (drain semantics), drops caches. |
| `repo_status(repo_id)` | Full status snapshot (entries by change kind, branch, ahead/behind). |
| `repo_refs(repo_id)` | Local + remote branches and tags as `(ref name, target sha)` pairs. |
| `repo_blame(repo_id, path, from?)` | Per-line blame; `from` re-blames at a parent commit. |
| `repo_clone(url, destination, depth?)` | Clones into a missing-or-empty directory (optionally shallow), publishing `BackendEvent::CloneProgress`; resolves with `CloneStats`. |

### Status / diff — `service/diff.rs`

| Fn | Semantics |
|---|---|
| `repo_diff(repo_id, old: DiffSide, new: DiffSide, paths?)` | One-shot diff returning `Vec<FileDiff>`; `paths` prefix-restricts the diff. Large diffs are rendered virtually by the UI from this same payload — there is no paged variant. |

### History / search — `service/log.rs`, `service/branches.rs`

| Fn | Semantics |
|---|---|
| `repo_log_stream(repo_id, filter, request_id)` | Streams the commit log in 500-commit pages as `BackendEvent::LogPage`s (commits + pre-laid-out `GraphRow`s + `next_cursor` + `generation`). A new call for the same repo replaces (cancels) the previous stream; streams abort when the repo generation moves and the UI re-requests. |
| `repo_file_history(repo_id, path, request_id)` | Same paging for one file's history, following renames. |
| `reflog(repo_id, kind?, name?)` | Reflog entries for HEAD or a branch/remote-ref. |
| `describe(repo_id, commitish?)` | `git describe --tags` for a commit-ish. |

### Staging / commit / safety net — `service/misc.rs`

| Fn | Semantics |
|---|---|
| `stage(repo_id, req)` | Stage paths / whole files / hunks / line ranges (`StageTarget`); hunk+line granularity is built by `diffcore` as minimal `git apply --cached` patches. |
| `stage_all(repo_id)` | Stage everything (tracked + untracked). |
| `discard(repo_id, targets)` | Discard files or hunks. A checkpoint is snapshotted BEFORE any side effect — a failed discard leaves the repo untouched. |
| `commit(repo_id, opts)` | Commit via the engine; when hooks or signing are active (`EngineError::Unsupported`) it falls back to the git CLI (`cli::commit_via_cli`) so hooks and GPG/SSH signing actually run. Returns the new sha. |
| `signing_info(repo_id)` | Whether GPG/SSH commit signing is configured (`user.signingkey` etc.). |
| `hooks_list(repo_id)` | Active repo hooks for the commit-bar badges. |
| `checkpoint_create(repo_id, reason)` | Snapshot index+workdir to a hidden ref `refs/mygitui/checkpoints/<unix_millis>-<reason>`. |
| `checkpoints(repo_id)` | List checkpoints, newest first. |
| `checkpoint_restore(repo_id, id)` | Restore index+workdir from a checkpoint (HEAD/refs/reflog untouched). A pre-restore checkpoint is snapshotted first. |
| `checkpoint_gc(repo_id)` | Delete checkpoint refs older than the cutoff, always keeping the newest (safety floor). |
| `ops_preview(repo_id, kind, params)` | Pure what-if preview of a dangerous op (`reset_hard` / `clean` / `checkout_force` / `branch_delete`) — no mutation, no checkpoint. |
| `guard_checkpoint(repo_id, reason)` | Snapshot an undo point right before a dangerous op (queue mutation, serialized with the op it guards). UI flow: `ops_preview` → dialog → `guard_checkpoint` → op. |
| `os_accent_color()` | OS accent color for the dynamic-color pipeline (Windows DWM registry read; `None` elsewhere). |
| `op_cancel(repo_id, op_id)` | Best-effort cancel of a queued/running op (atomics only, never touches the repo mutex). Queued-not-started ops are skipped; running ops observe the flag cooperatively (netops fetch/push). |
| `action_run(repo_id, spec)` | Spawn a user-defined shell action with cwd = repo root, streaming `BackendEvent::ActionOutput`; returns the run id (cancel token). Remote repos run the action on the device over SSH exec through the same sink. |
| `action_cancel(run_id)` | Kill a running action; `false` when unknown/finished (never an error). |
| `cli_args_initial()` | argv captured at first launch (`mygitui <path>`), consumed once. Second-launch args arrive as `BackendEvent::CliArgs`. |

### Branches / tags — `service/branches.rs`

| Fn | Semantics |
|---|---|
| `branches(repo_id)` | Local branches with tracking/ahead/behind info. |
| `branch_create(repo_id, name, start?, checkout?)` | Create (optionally from a start point, optionally switch). |
| `branch_switch(repo_id, name)` | Switch; the UI resolves dirty-workdir choices via the dialog first. |
| `branch_is_merged(repo_id, name, into)` | Merged-guard preview: is `name` fully merged into `into`? |
| `branch_delete(repo_id, name)` | Delete after the merged-guard; the branch is preserved under `refs/mygitui/trash/<id>` first (deleted-branch trash). |
| `branch_rename(repo_id, name, new_name)` | Rename a branch. |
| `tag_create(repo_id, name, target?, message?)` | Lightweight or annotated tag. |
| `tag_create_signed(repo_id, name, target?, message?)` | Signed annotated tag via `git tag -s` (libgit2 cannot sign); message travels via `-F` temp file. |
| `tag_delete(repo_id, name)` / `tag_list(repo_id)` | Delete / list tags. |
| `remote_branches(repo_id)` | Remote tracking branches (`refs/remotes/*`). |
| `branch_checkout_remote(repo_id, remote, name, new_local?)` | Check out a remote branch into a local branch. |
| `branch_trash_list(repo_id)` | Branches preserved under `refs/mygitui/trash/*`, newest first. |
| `branch_trash_restore(repo_id, id, new_name?)` | Restore a trashed branch (optionally renamed); returns the branch name. |
| `commit_signature(repo_id, sha)` | Verification status of one commit's signature (queued read). |
| `autosquash_plan(repo_id, base)` | Autosquash rebase plan for `base..HEAD` (feeds `rebase_start`). |

### Network — `service/network.rs`

| Fn | Semantics |
|---|---|
| `remotes(repo_id)` | Configured remotes with URLs. |
| `remote_add` / `remote_remove` / `remote_set_url` | Remote config mutations. |
| `fetch(repo_id, opts)` | Fetch with prune/depth options; streams throttled progress and forwards engine callbacks to `OpCtx::emit_progress`. |
| `pull(repo_id, opts)` | Pull with ff / merge / rebase policy. |
| `push(repo_id, opts)` | Push with force, force-with-lease (real expected-sha checking), tags, multi-ref, remote ref deletion; the hook path routes through `cli::push_via_cli` so `pre-push` really runs and streams output. |
| `auth_respond(op_id, answer)` | UI answer to an `BackendEvent::AuthRequest`; routes `(username, password, store)` to the blocked credentials callback. |

### Stash / worktrees — `service/stash.rs`

| Fn | Semantics |
|---|---|
| `stash_list(repo_id)` | Stashes, newest first. |
| `stash_push(repo_id, message?, keep_index?)` | Stash current changes. |
| `stash_apply(repo_id, index)` / `stash_drop(repo_id, index)` | Apply (without dropping) / drop one stash. |
| `stash_branch(repo_id, index, name)` | Apply a stash onto a new branch. |
| `worktrees(repo_id)` | Registered worktrees with main-entry marking. |
| `worktree_add(repo_id, path, branch?)` / `worktree_remove(repo_id, path)` | Add / remove a worktree. |
| `worktree_prune(repo_id)` (in `service/merge.rs`) | `git worktree prune`; returns the number pruned. |

### Merge / rebase / bisect — `service/merge.rs`

| Fn | Semantics |
|---|---|
| `merge_branch(repo_id, opts)` | Merge with ff policy / squash / `--no-commit` / `-X ours|theirs`. The UI runs it only after a confirm dialog (shows the branch being merged into the current one, offers exactly these options) and disables the confirm button while uncommitted staged+unstaged changes exist ("Stash your changes first"). |
| `merge_abort(repo_id)` / `sequencer_abort(repo_id)` | Abort an in-progress merge / cherry-pick-revert sequence. |
| `conflicts(repo_id)` | Current conflict list with marker previews. |
| `conflict_resolve(repo_id, path, resolution)` | Resolve one conflict (ours/theirs/both/manual content). |
| `cherry_pick(repo_id, sha)` / `revert(repo_id, sha)` | Cherry-pick / revert (sequences surface through `rebase_state`). |
| `reset(repo_id, kind, to?)` | Reset soft/mixed/hard to a commit-ish. |
| `rebase_start(repo_id, base, steps?)` | Start an interactive rebase with an explicit step list (reorder, squash/fixup/drop/edit/reword, `exec`). |
| `rebase_plan(repo_id, base)` | Plain plan: every commit in `base..HEAD` oldest first; empty means nothing to rebase. |
| `rebase_state(repo_id)` | Progress + remaining steps + conflicts of the running rebase. |
| `rebase_continue(repo_id)` / `rebase_abort(repo_id)` | Continue (resolving conflicts first) / abort. |
| `bisect_start(repo_id, good, bad?)` | Begin a bisect (`<gitdir>/mygitui/bisect.json` is the state file). |
| `bisect_state(repo_id)` | Current run: commits left, marks, probe. |
| `bisect_mark(repo_id, mark)` | Good/Bad/Skip on the current probe. |
| `bisect_reset(repo_id)` | End the bisect, restore the original HEAD. |
| `bisect_log(repo_id)` | The live run's mark history, else the last archived run. |

### Maintenance / health — `service/maintenance.rs`

| Fn | Semantics |
|---|---|
| `repo_health(repo_id)` | Fast FS-only snapshot (repo + gitdir sizes, object counts, cached fsck dangling census). |
| `maintenance_run(repo_id, op)` | One of `gc` / `prune` / `commit_graph` / `pack_refs` / `count_objects` on the op queue; resolves with the command output. |
| `archive(repo_id, spec, destination)` | `git archive --format <zip\|tar\|tar.gz> <ref> -o <destination>`; spec/destination are metacharacter-guarded. |
| `repo_clean(repo_id, paths, dirs?)` | Checkpoint first, then delete each validated path; returns the number removed. |
| `sparse_info(repo_id)` / `sparse_apply(repo_id, patterns, add?)` | Sparse-checkout state / apply (cone mode; empty = full checkout). |
| `lfs_status(repo_id)` / `lfs_run(repo_id, subcommand)` | `git lfs` availability + tracked patterns / run `pull`\|`push`\|`install`\|`fetch`. |
| `clone_blobless(url, destination, depth?)` | Blobless (partial) clone via `git clone --filter=blob:none`. |

### Files / editor — `service/files.rs`, `workspace.rs`, `editors.rs`

Every `path` argument is workdir-relative with forward slashes and
validated: absolute forms, `..` components, Windows prefixes, and anything
escaping the root reject; writers additionally refuse paths inside `.git`,
and mutating workspace ops run on the op queue (serialized,
generation-bumping) so the watcher → `RepoChanged` → refresh pipeline
updates the UI for free.

| Fn | Semantics |
|---|---|
| `repo_list_dir(repo_id, path, include_ignored)` | One directory level (bounded walk, gitignore-aware unless `include_ignored`, dirs-first natural sort, `.git` never listed, index gitlinks surface as `submodule`, capped at 5000 entries with `truncated: true`). |
| `repo_stat_file(repo_id, path)` | Size, content class (`text`/`image`/`binary` by extension + NUL sniff of the first 8 KiB), `readonly`, `mtime_ms` — lets the UI decide how to open a file before transferring bytes. |
| `repo_read_file(repo_id, path)` | Raw bytes of one workdir file (backs the conflict editor's marker preview). Missing file resolves to an empty vec; escaping paths reject. Workdir read: no repo mutex. |
| `repo_write_file(repo_id, path, content, expected_mtime)` | The file editor's save. `expected_mtime` guard rejects a "changed on disk" write (the UI offers reload/overwrite); writes are atomic (temp + rename in the same dir). |
| `repo_create_entry(repo_id, path, kind)` | Create an empty file or a directory (with parents); errors when the path exists. |
| `repo_rename_entry(repo_id, path, new_path)` | Rename/move within the worktree; errors on missing source or existing target. |
| `repo_delete_entry(repo_id, path)` | Checkpoint first (reason `delete <path>`), then delete file or directory tree; refuses submodule checkouts. |
| `repo_file_index(repo_id)` | Flat gitignore-respecting walk for quick-open (Ctrl+P); capped at 50k entries with `truncated: true`. |
| `gitignore_add(repo_id, pattern)` | Append one pattern to the repo-root `.gitignore` (exact-line dedupe). Pure workdir write — no op queue; the watcher picks it up. |
| `gitignore_apply_template(repo_id, name)` | Apply a builtin template: every pattern appended under a `# <name>` header in ONE write. |
| `gitignore_templates_list()` / `gitignore_templates(services)` | The curated builtin template list (`{name, description, patterns}`). |
| `submodules(repo_id)` | Every submodule with checked-out vs recorded state. |
| `submodule_update(repo_id, path, init?, recursive?)` | `git submodule update` for one submodule; network fetches route credential prompts through the auth broker. |
| `submodule_sync(repo_id, path?)` | Rewrite submodule URLs from `.gitmodules` into repo config (`None` = all). |
| `detect_editors(services)` / `open_with(services, app_id)` | Probe installed editors / open the repo root in one. The id must resolve through a fresh detection, so only machine-verified paths spawn. |

### Terminal / PTY — `pty.rs`

| Fn | Semantics |
|---|---|
| `pty_create(services, repo_id, rows, cols)` | Spawn a shell in a fresh pty at the repo root; returns the session id used by write/resize/kill. Output arrives as `BackendEvent::PtyOutput` (coalesced windows), exit as `BackendEvent::PtyExit`. Capped at `MAX_SESSIONS` live sessions. |
| `pty_write(services, session_id, data)` | Send UI input (keystrokes, paste). |
| `pty_resize(services, session_id, rows, cols)` | Resize (UI panel size changed). |
| `pty_kill(services, session_id)` | Kill the shell and clean the registry entry; the `PtyExit` event still fires from the exit waiter with the real code. |

### Forge / GitHub — `service/forge.rs` (via `forge.rs`)

All gh-backed fns run `gh` through `run_program` (argv, no shell, hard
timeout) and classify failures into the structured `ForgeError`
(`NotFound` / `AuthRequired` / `Timeout` / `AlreadyExists` / `Other`).
`project_detect` / `commit_activity` / `contributor_stats` are engine reads,
not gh calls.

| Fn | Semantics |
|---|---|
| `forge_status()` | `gh` on PATH + authenticated probe (no repo). |
| `forge_context(repo_id)` | Owner/repo/branch derived from the `origin` remote + HEAD. |
| `pr_create(...)` / `pr_list(...)` | Create a PR (base/title/body/draft) / list PRs by state (limit 50). `AlreadyExists` carries the existing PR URL when parseable. |
| `pr_checks(repo_id, number)` | CI checks of one PR; states canonicalized to pass/fail/pending/skipping (JSON first, table fallback for older gh). |
| `pr_merge(...)` / `pr_close` / `pr_reopen` / `pr_ready` | Merge (`merge`/`squash`/`rebase`, optional `--delete-branch`/`--auto`) / close / reopen / draft-readiness flips. |
| `pr_checkout(repo_id, number)` | Check out the PR head branch locally. |
| `pr_comment` / `pr_review` | Comment / submit a review (`approve`, `request-changes`, or plain comment). |
| `workflow_runs(...)` / `workflow_run_detail(...)` | Recent Actions runs (branch/status/workflow filters) / one run with jobs and steps. |
| `workflow_run_logs(...)` | Logs of a run or one job (`--log` / `--log-failed`); falls back to per-job REST logs while a run is in progress — empty output while active is normal, not an error. |
| `workflow_run_cancel` / `workflow_run_rerun` / `workflow_dispatch` | Cancel a queued/running run / rerun all or failed jobs / trigger a `workflow_dispatch` on a ref. |
| `workflow_list(repo_id)` | Workflow files of the repo, including disabled ones. |
| `project_detect(root)` | Detected project types + runnable scripts under one repository (bounded root + immediate-child scan, `projects.rs`). |
| `commit_activity(repo_id, max_days, author?)` | Commits per local day, day ascending; `author` is a case-insensitive substring of name OR email. |
| `contributor_stats(repo_id, max_days)` | Per-author rollups (exact name+email identity), count descending. |

### AI / opencode — `opencode.rs`

| Fn | Semantics |
|---|---|
| `opencode_detect(services)` | Probe for a local opencode CLI (PATH scan + `--version`). |
| `opencode_serve_status(services)` | State of the managed `opencode serve` child (starts nothing). |
| `opencode_serve_start(services, cwd?)` | Start (idempotent) the managed server: opencode picks a loopback port; start resolves once it accepts connects. The registry kills the child on drop (app-exit backstop). |
| `opencode_serve_stop(services)` | Stop the managed server. |

### Secrets — `keyring_store.rs`

| Fn | Semantics |
|---|---|
| `secrets_get(services, key)` | Read a secret from the OS keyring (`Ok(None)` when unset). |
| `secrets_set(services, key, value)` | Create/overwrite a secret. |
| `secrets_delete(services, key)` | Delete a secret (idempotent). |

All keyring entries live under the service name `mygitui`. The same service
name is used directly (via the keyring crate, bypassing these generic fns)
by the auth broker for `https:<host>:user` / `https:<host>:pass` entries and
by the remote device store for device secrets — see "Persistence".

### Remote devices — `remote/commands.rs`

The remote machine is the source of truth: every read/write and git command
executes on the device; nothing is synced locally. Secrets never appear in
logged parameters: they go once into the OS keyring and are never stored in
the profile file or error strings. Host keys are trust-on-first-use
(`<data>/remote/known_hosts.json`); a changed key blocks the connection
until explicitly re-confirmed. Every user-controlled value interpolated
into a remote command passes the unit-tested POSIX quoting helper
(`remote/quoting.rs`).

| Fn | Semantics |
|---|---|
| `device_list(services)` | Registered devices + live connection status. |
| `device_add` / `device_update` / `device_remove` | Profile CRUD; secrets write once into the keyring (`device_update` with `None` secrets keeps stored ones); remove drops the session and deletes profile + secrets. |
| `device_connect` / `device_disconnect` / `device_status` | Dial + auth (surfacing host-key/auth prompts as events) / idle (ops reconnect on demand) / one-shot status probe. |
| `device_test(input)` | Test-connection probe; nothing persisted (unsaved ids clean their keyring entries up). |
| `device_hostkey_respond(request_id, accept)` | Answer a `BackendEvent::HostKeyRequest`. |
| `device_auth_respond(request_id, password?, store)` | Answer a `BackendEvent::DeviceAuthRequest`. |
| `device_forget_host_key(device_id)` / `device_host_key(device_id)` | Unpin the host key (next connect re-prompts) / read the pinned key. |
| `remote_poll_set_paused(paused)` | Pause/resume the remote change pollers. |
| `device_browse_dir(device_id, path)` | SFTP listing (entries carry absolute POSIX paths; `""`/`"~"` = home). |
| `device_scan_repos(device_id, path?)` | Find git repositories on the device as candidates. |
| `remote_repo_open(services, device_id, path)` | The remote twin of `repo_open`: opens a device repo as a first-class tab. |

### Cross-cutting backend modules (not called directly by the UI)

- `ops.rs` — per-repo serial `OpQueue`, `OpCtx` (throttled progress, log
  lines, cancellation token), `EventSink` abstraction (`BusEventSink` in
  production, collector in tests).
- `rt.rs` — process-wide tokio runtime (`spawn`, `spawn_blocking`,
  `block_on`) used by op-queue workers and sync entry points.
- `actions.rs` — action registry (run ids, per-repo concurrency budget,
  cancel), platform shell wrapping (`cmd /C` vs `sh -c`).
- `auth.rs` — global `AuthBroker`: git2 credential callback chain
  (ssh-agent → keyring/cache → git credential helpers → UI prompt
  round-trip with 60 s timeout), per-URL attempt counting so a rejected
  credential re-prompts once, then fails the op.
- `credential_helper.rs` — `git credential fill/approve/reject` plumbing.
- `cli.rs` — `commit_via_cli` / `push_via_cli` (hooks + signing run for
  real; stdout/stderr lines stream to `OpCtx::emit_log`).
- `process.rs` — sanitized `Command` builder shared by the CLI layers.
- `maintenance.rs` — sanitized `git <args>` runner + health/fsck/sparse/LFS
  implementations (CLI-only work libgit2 cannot do).
- `mergetool.rs` — `mergetool_info` (pure config read, gui-first resolution)
  and `mergetool_run` (`git mergetool -t <tool>`, interactive).
- `diffcore.rs` — hunk/line selection → minimal `git apply --cached`
  patches (`apply_selection`) and reverse-apply (`discard_selection`).
- `graph/` — pure, git2-free lane layout for the canvas commit graph:
  consumes `CommitInfo` pages in walk order, produces render-ready
  `GraphRow`s; page-boundary independent (same commits through any page
  split with a shared `LaneState` yield identical rows).
- `watcher.rs` — debounced (200 ms), gitignore-aware recursive workdir
  watcher; emits `RepoChangedEvent` batches, bumps the repo generation on
  every batch, restarts with backoff on errors (full-resync batch first).
- `editors.rs` / `projects.rs` — editor detection/spawn and project/script
  detection (pure scans).

## Event bus

`service/events.rs` defines one `BackendEvent` enum broadcast over a
process-wide tokio broadcast channel (`events::global()`, capacity 4096).
Every producer publishes through `EventBus::publish`; delivery is
best-effort — with no live subscriber the event is dropped (`publish`
returns `false`), and slow consumers lose the oldest events (they must
recover from a `Lagged` error, which the UI does by re-fetching on the next
event or the explicit refresh path).

| Variant | Payload | Producer |
|---|---|---|
| `RepoChanged` | `{ repo_id, paths, head_moved, full }` | FS watcher batch (`watcher.rs`, debounced + filtered) or remote poll tick (`remote/polling.rs`). Drives every panel refresh; `full: true` → resync everything. |
| `OpProgress` | `{ repo_id, op_id, kind, message, pct?, done, error? }` | Op queue (`ops.rs`): ops emit throttled progress (`OpCtx::emit_progress`, ≥80 ms apart), the queue emits the final `done: true` event (never throttled) success or error. Queue-dispatched reads are silent. |
| `OpLog` | `{ repo_id, op_id, line }` | `OpCtx::emit_log` — raw hook/CLI output lines (unthrottled). |
| `AuthRequest` | `{ op_id, repo_id, url, kind, prompt }` | Auth broker credential callback (`auth.rs`) when keyring, cache, and credential helpers all miss. Answer via `network::auth_respond`. |
| `HostKeyRequest` | `{ request_id, device_id, host, key, … }` | Remote transport (`remote/transport.rs`) on first contact or a changed host key. Answer via `remote::commands::device_hostkey_respond`. |
| `DeviceAuthRequest` | `{ request_id, device_id, kind, prompt }` | Remote transport secret prompt (key passphrase / device password). Answer via `device_auth_respond`. |
| `DeviceStatus` | `{ device_id, status }` | Remote transport on connection state transitions. |
| `CloneProgress` | `{ url, received, total, objects }` | `service::repo::repo_clone` (throttled ≥80 ms; `received` counts bytes, `total`/`objects` count git objects — the bar fills by objects). |
| `LogPage` | `{ request_id, page: { commits, rows, next_cursor, generation } }` | `service::log` streams (`repo_log_stream`, `repo_file_history`). `request_id` is minted by the UI per request so superseded streams can be discarded; `generation` lets clients detect stale pages. |
| `ActionOutput` | `{ run_id, repo_id, name, line, done }` | Custom action runner (`actions.rs`); empty `line` on the final event. |
| `PtyOutput` | `{ session_id, data }` | PTY sink (`pty.rs`) — lossy-UTF-8 decoded terminal bytes for one coalesced window. |
| `PtyExit` | `{ session_id, code }` | PTY exit waiter. |
| `CliArgs` | `Vec<String>` | Second-launch CLI args forwarded by the single-instance guard (`instance.rs`: per-user Unix-domain socket; second launch forwards argv and exits, primary publishes here — Unix only, Windows runs standalone). |

## UI ⇄ service contract

### Call path

`ui/app.rs` owns one `Arc<AppServices>` built at boot (`AppServices::new()`
— platform data/config dirs under the `com.raulshma.mygitui` suffix; tests
use `with_dirs`/`in_dir`). The struct is `Send + Sync` (pinned by a
dedicated test) so it can be cloned into every `Task::perform` closure.
Async service bodies run on the iced (tokio) executor; inside them the
engine work is enqueued on the repo's serial op queue, whose worker thread
locks the shared `git2::Repository` per call, and the awaited result comes
back through a oneshot receiver.

### Dispatch rules (the queue)

`service::enqueue_mutation` / `enqueue_read` (and their `enqueue_remote_*`
twins) are the only sanctioned paths for engine ops:

- **Mutations** run with the repo lock held and bump the repo generation
  afterwards (success or error — hooks may have touched state even on
  failure), so in-flight log streams abort and clients re-request.
- **Reads** run with the lock held but stay silent on the op-progress
  surface: no progress, no done event; failures surface only as the call's
  `Err(String)`.
- **Deliberately off-queue**: `repo_log_stream` / `repo_file_history`
  (their own cancellation loop and stream tokens on a dedicated thread),
  `mergetool_run` (interactive, minutes-long — blocking the queue would
  freeze the repo while the external tool writes files), the
  never-locks-the-repo commands (FS/CLI reads and spawns: maintenance
  probes, forge, clone, actions, pty, secrets, editors), and the update
  check/installer (`update_check` queries the GitHub releases API;
  `updater.rs` downloads the platform artifact, verifies its `.sha256`
  sidecar and applies it — AppImage self-replace, silent MSI, `.app`
  bundle swap — restarting the app on success).

Op kinds double as progress labels: `stage`, `commit`, `branch` (tags
report here), `fetch` (remote config ops report here), `pull`, `push`,
`clone`, `merge`, `rebase`, `bisect`, `stash`, `worktree`, `checkpoint`,
`submodule`, `maintenance`, `archive`, `sparse`, `lfs`, `clean`; queue
dispatched reads use `status`, `refs`, `blame`, `diff`, `preview`, `stats`,
`reflog`, `describe` but emit no events.

### Progress / cancellation

The footer tracks the latest `OpProgress`; while one is outstanding the UI
keeps an `ActiveOp { repo_id, op_id, kind }` and offers Cancel, which calls
`misc::op_cancel`. Cancellation is cooperative: queued-not-started ops are
skipped entirely (the job resolves with a "cancelled" error), running ops
observe the atomic flag through `OpCtx::cancelled` / `OpCtx::cancel_token`
(netops fetch/push check between chunks). A `done: true` progress event
always arrives for queued mutations/net ops — success or error — and clears
`ActiveOp` plus triggers a refresh. Reads are silent by design (see above).

### Event → UI bridging

`ui/bus.rs` wraps the bus in one iced `Subscription`
(`Subscription::run(bus::stream).map(Message::Backend)`); `Lagged` gaps are
logged and skipped. The app runs under `iced::daemon` (multi-window): the
primary window hosts the shell and diffs can pop out into secondary windows
(`App::popout`, per-window `view`/`theme`/`title`, cleanup via
`window::close_events`). The subscription batch adds the system theme-change
subscription, the keyboard listener (palette/switcher chords), the
`window::close_events` stream, and the per-tab auto-fetch timer (keyed by
repo + cadence; `0` disables). Each `BackendEvent` variant
maps to a UI behavior: `RepoChanged` → targeted panel refreshes,
`LogPage` → history model accept (matched by `request_id`), `OpProgress` →
footer/ActiveOp, prompts (`AuthRequest` / `HostKeyRequest` /
`DeviceAuthRequest`) → queued overlay dialogs, `PtyOutput`/`PtyExit` →
terminal panel, `ActionOutput` → action run view.

## Persistence

- **User config** — `<config_dir>/com.raulshma.mygitui/config.json`
  (`ui/config.rs`): `{ theme: "system"|"light"|"dark", diff_wrap: bool,
  diff_split: bool, auto_fetch_minutes: u32,
  recent_repos: [{ path, pinned, last_opened }],
  accent: bool, highlight: bool, update_check_on_start: bool,
  ai: { enabled, provider, opencode_url,
  opencode_model, openrouter_model, repo_opt_in } }` (AI secrets live in
  the OS keyring, never here). The diff prefs seed every new tab and are
  re-persisted when the diff toolbar toggles split/wrap.
  Loading never fails (missing/corrupt → defaults, unknown keys ignored);
  saves are atomic (temp + rename) and surface I/O errors to the UI.
  Recents dedupe separator/case variants.
- **Custom actions** — `<config_dir>/com.raulshma.mygitui/actions.json`
  (the app's actions panel: name, command, shortcuts; atomic writes,
  malformed file → defaults).
- **Data dir** — `<data_dir>/com.raulshma.mygitui/` (`AppServices::data_dir`):
  `remote/devices.json` (device profiles, secrets excluded — they live in
  the keyring) and `remote/known_hosts.json` (TOFU-pinned host keys). The
  suffix is inherited from the pre-iced app-data layout so existing state
  files keep being found.
- **Per-repo state** — inside each repo's gitdir under `mygitui/`:
  `bisect.json` (an active bisect's state; its presence = bisect in
  progress) and `fsck.json` (last fsck dangling census + sample shas,
  rendered by the health panel).
- **Checkpoints** — hidden refs `refs/mygitui/checkpoints/<unix_millis>-<reason>`
  capturing index + workdir; restore rewrites only index/workdir (HEAD,
  branch refs, reflog untouched); GC keeps the newest checkpoint regardless
  of age.
- **Deleted-branch trash** — deleted (merged-guard-checked) branches are
  preserved as `refs/mygitui/trash/<id>` before the branch ref goes away;
  restore re-creates the branch (optionally renamed). `refs/mygitui/*` are
  internal namespaces: excluded from branch listings, stats, and push
  refspecs.
- **OS keyring** — service name `mygitui`: generic secrets via
  `secrets_get/set/delete`, `https:<host>:user` / `https:<host>:pass`
  entries warmed/read by the auth broker, device secrets (password, key
  passphrase) written once by the device store. Tests degrade gracefully
  when no keyring backend is available (probe-and-skip).
