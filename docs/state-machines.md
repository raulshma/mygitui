# State Machine Registry

Every lifecycle in the app — where it lives, how it is encoded, and its
status. Companion to `docs/contracts.md` (which owns the wire shapes; this
file owns the behavior machines that produce and consume them).

Statuses: **explicit** = a typed union / persisted state file, invalid
states unrepresentable — keep it that way. **implicit-ok** = a simple
2-state machine whose nullable/boolean encoding is clear and tested — no
work needed. **fixed** = was implicit, caused (or invited) a real defect,
now explicit or repaired. **follow-up** = still implicit; change is
warranted but not urgent — see the backlog at the bottom.

## Shared primitives (the canonical encodings)

The two machines that kept being re-invented ad hoc now have one tested
home each; new code reaches for these instead of re-encoding:

- **`AsyncData<T>` / `AsyncMap<T>`** — `src/lib/state/async.svelte.ts`.
  One idle/loading/ready/error value per cache key; ready data rides
  along as `previous` across refreshes, and only the newest run's result
  lands (the stale-token guard is built in, so no async callback has to
  remember one). `silent` refreshes keep the visible entry; `set` patches
  a ready entry; `reset`/`resetPrefix` drop subtrees. Canonical for
  archetypes 3 + 4.
- **`DialogState<A>`** — `src/lib/state/dialog.ts`. `{ open: false } |
  ({ open: true } & A)` — a dialog's args travel with its open flag, so
  no open/close site can hand-reset siblings and "open with last run's
  args" is unrepresentable. Canonical for archetype 1 (as applied to
  dialogs).
- **`OverlayStack`** — `src/lib/state/overlays.svelte.ts`. Layered owner
  of "which overlay is on top" (panel < palette < context menu < dialog);
  Escape affordances of stackable overlays defer to it.
- **`loadVersioned`** — `src/lib/state/persist.ts`. The `{ v, data }`
  storage envelope + migration registry; reads accept legacy unversioned
  payloads as v1 and drop versions newer than the registered upgraders.

## Encoding archetypes (the vocabulary)

These are the ways a state machine hides in this codebase. When adding
or reviewing code, match against them — the shared primitives above are
the canonical encodings for the first three:

1. **Nullable-pile mode fields** — "one of N modes" encoded as N parallel
   nullable fields resolved by an if-chain. Every transition must
   hand-reset the siblings; one missed reset is a bug. Rule: **one mode =
   one union, one `$state`** (`DialogState<A>` for dialogs; the
   CenterPane pattern in RepoView for view modes).
2. **String/regex sentinels** — a protocol carried as an untyped string,
   classified by pattern-matching on the other side. Rule: **type the
   wire** (ts-rs enum) and keep exactly one sentinel comparison site.
3. **Parallel loading/error/data triples** — `X` + `XLoading` + `XErrors`
   maps with reachable invalid combos (`loading && error`). Rule: **one
   `AsyncData<T>` per key**; derive what you can, don't co-store.
4. **Hand-rolled transition guards** — stale tokens, edge detectors,
   epoch mirrors, armed/fired toast-once flags. Correct but untyped; every
   async callback must remember the guard. Acceptable when isolated
   (`AsyncMap` when they multiply); consolidate when they spread.
5. **Lifecycle ownership gaps** — nobody kills/cleans up: a session, a
   map entry, a dialog outliving its reason. Rule: **the thing that ends
   the lifecycle unregisters itself** (exit waiter, reader task, done
   event, op cancel, repo close, device disconnect), and every "insert
   before async" needs a failure-path removal.
6. **Missing terminal states** — a two-state machine (`null` / data) with
   no error state, so failure renders as an eternal spinner. Rule: **if a
   fetch can fail permanently, the UI needs a third state** (`AsyncData`
   error carries the stale data forward).

---

## App shell & tabs

| Machine | States | Source of truth | Status |
|---|---|---|---|
| App shell view | `launching → home/recents ⇄ workspace` (+ offline sub-state per tab) | `App.svelte` derives off `tabStore` (`launching = restoring && activeId === null`) | implicit-ok (tested: `tabs.test.ts`) |
| Tab lifecycle | `closed → opening → open ⇄ active` (+ `offline` for remote) | `src/lib/stores/tabs.svelte.ts` (`Tab.id`/`openError`; remote tab id = its `ssh://` URI) | fixed: `openRemoteTab` re-finds the tab after the await — a double-clicked Retry used to `splice(-1, 1, …)` an unrelated tab away |
| Session restore | `restoring → restored` | `TabStore.restoring` + `restoreSession()`; popouts call `skipRestore()` at startup so `launching` can never wedge | fixed: popouts never ran `restoreSession`, leaving `restoring` true forever — any future popout-side reader wedged (2026-11 wave #13) |
| Terminal teardown | per-repo pty: `absent → starting → live → exited/failed`; teardown = kill + forget | `src/lib/terminal/terminalStore.svelte.ts` (`TerminalStatus` union — explicit); teardown trigger | fixed: `TabStore.closeTab` disposes the repo's terminals (the single close choke point); RepoView unmount stays as backstop. Previously only last-tab unmount disposed — background tab closes leaked shells (M5 contract violation) |
| Status fetch | `unloaded (null) → loaded`, **now also `error`** | `Tab.status` + `Tab.statusError` (`tabs.svelte.ts`); rendered by `StatusPanel` | fixed: a failed first `repo_status` used to spin forever; now an error state with Retry. A failed refresh on a loaded tab still keeps the previous status |

## Center editor (RepoView)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Center pane mode | `empty \| diff(entry) \| commit(commit) \| compare(result) \| workflowRun(runId) \| actionRun(runId) \| files` | `CenterPane` union, one `$state` in `RepoView.svelte`; per-mode projections are `$derived` | fixed: was six parallel nullables; the repo-switch reset missed three of them, so repo A's file tabs ran IPC against repo B |
| Diff stream | `idle → streaming → loaded` (token-guarded) | `diffFiles`/`diffLoading`/`diffHunkable` + `diffToken` in RepoView | implicit-ok (stale-token pattern; per-repo reset owned by the mode union) |
| File workspace tabs | `openFiles[]` + `activeFile`, VS Code-style | RepoView (repo-scoped; reset on repo switch) | fixed: the same missed-reset bug kept repo A's open files mounted in repo B |
| Auto-open first change | fires once per repo when `center.kind === "empty"` | RepoView `autoOpenedFor` | fixed: the old guard checked five nullables but not `filesMode`, yanking an open workspace aside |
| History multi-select | `∅ ⇄ multiShas (sha-ordered)` + `anchorSha` for shift-click ranges | `HistoryView.svelte` (`multiShas`, sha-owned like the single select; `selectedShas` feeds cherry-pick/revert) | fixed: was index-based, held alive only by an epoch-mirror effect — a missed mirror let Cherry-pick act on rows the user never picked; the mirror is gone and the selection now survives stream restarts by design (2026-11 wave #11) |

## Dialogs & overlays

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Per-dialog open/args | `closed → open(args) → closed` | `DialogState<A>` (`src/lib/state/dialog.ts`), one `$state` per dialog: `branchOps` controller slots, App add/edit-device, `FilesPanel` prompt/confirm, `HistoryView` planner + sha slots | fixed: was "boolean + N arg fields, reset ad hoc" across ~10 files — an "Add" dialog could pre-fill with a stale device, and a callback lived in `$state` (2026-11 wave #1) |
| Modal stacking / Escape | layered stack (panel < palette < context menu < dialog) | `OverlayStack` (`src/lib/state/overlays.svelte.ts`); palette/context-menu/conflict-editor register; palette Escape defers to a higher overlay | fixed: was unarbitrated (2026-11 wave #10). Dialog-family Escape stays self-closing — bits-ui paths are focus-scoped and close only themselves |
| Conflict editor busy | `null → resolve(path) \| abort → null` | `ConflictEditor.svelte` (`busyState` tagged union; per-file resolve disables only that file's buttons) | fixed: was `busyPath` doubling as the magic `"::abort"` sentinel (a real path could collide); the armed-abort 4 s timer also leaked past unmount — both repaired (2026-11 wave #8) |
| Confirm/prompt flows | same primitives; danger classification is explicit | `src/lib/components/safety/safetyModel.ts` (`DangerLevel` — explicit, tested) | explicit (model) / fixed (wiring) |

## Operations (op queue → FE)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Op lifecycle | `queued → running → done(completed\|cancelled\|failed)`; queued-cancel skips execution | backend `src-tauri/src/ops.rs` (`OpOutcome::for_error` — one sentinel comparison site, `CANCELLED_ERR`); FE `src/lib/stores/ops.svelte.ts` (`isCancelledOp` prefers the typed `outcome`) | fixed: FE classified cancellation by matching the error string against `/cancel/i`; the wire now carries `OpProgress.outcome` (typed enum), regex kept as fallback only |
| Op cancel UX | `running → cancelling(flag) → done` | `ActiveOp.cancelling` flag, un-flagged by the done event or a failed `op_cancel` | implicit-ok (tested) |
| Hook/CLI log session | `fresh(op_id) → done → dismissed/replaced` | `OpStore.logs` (newest-op-wins per repo) | implicit-ok |
| Auth request → dialog | `pending(null) → showing(request) → answer/dismiss → next queued` | `AuthStore` (`ops.svelte.ts`); correlation key `op_id`. Backend: `op_cancel` wakes the broker waiter, repo close resolves the repo's pending requests (`auth.rs cancel_for_repo`) | fixed: a dialog whose op died without an answer stayed up forever — `OpStore.onOpFinished` retires it; and the handshake thread used to sit out its full 60 s after its op was cancelled / its repo closed (2026-11 wave #4) |
| Clone progress | `idle → cloning(run-id-keyed progress) → done/failed`; hidden clones resurface on reopen | `src/lib/components/CloneDialog.svelte` (mints a run id per clone; `repo_clone` echoes it in every event; stall hint after 45 s of quiet — no auto-cancel) | fixed: matching was URL-keyed (URLs repeat across windows; run ids do not), and the registry overstated the hidden-clone risk — the single session-mounted instance resurfaces fine; corrected 2026-11 wave #5 |

## Terminals & sessions (backend)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Local pty session | `spawn → output pipeline → exit waiter → unregister → pty-exit` | `src-tauri/src/pty.rs` (cap via `RegistryInner::reserve_slot` RAII reservation) | fixed: cap check and insert were separated by openpty/spawn — two concurrent `pty_create` could overshoot `MAX_SESSIONS`; now one atomic reserve/commit with abort-on-failure |
| Remote shell session | same lifecycle, reader task owns end-of-life | `src-tauri/src/remote/term.rs` (`RegistryInner::finish_session`) | fixed: device-side channel close emitted `pty-exit` but never unregistered — 8 shell exits wedged `rshell_create` until app restart |
| Cursor-probe handshake | conhost probe → answer → stream | `pty.rs` `strip_cursor_probes` (pure, unit-tested) | explicit |

## Devices (remote projects)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Device connection | `disconnected → connecting → connected` (+ watchdog decay to disconnected) | Rust: `DeviceStatus` enum + `watch` channel (`remote/transport.rs`); wire: typed enum end to end | fixed: the enum was flattened to `String` at every boundary with a `#[ts(type=…)]` annotation holding the union together |
| Dial race | concurrent `session_for` → first insert wins, loser disconnects | `DeviceRegistry::session_for` | fixed: the second caller used to overwrite the first — an authenticated SSH session leaked and two `device-status` streams ran for one device |
| Host-key TOFU | `Unknown → Trusted → Changed → (re-confirm) → Trusted` | `remote/hostkeys.rs` (`HostKeyStatus`, atomic file, tested) | explicit |
| Device prompts | `pending map → emitted event → answered/120s-timeout → resolved`; one FE slot, one shared queue; device disconnect/remove resolves the device's prompts as dismissed | backend `remote/prompts.rs` (`cancel_for_device`); FE `remoteDevices.svelte.ts` (`pendingPrompt` union; `hostkeyPending`/`authPending` are derived views) | fixed: two FE queues could show a host-key and an auth prompt stacked as two modals, and a disconnect left handshake threads sitting out the 120 s (2026-11 wave #4/#9) |
| Remote repo poller | `sleep(5s) → fingerprint compare → generation bump + full resync` | `remote/polling.rs` | implicit-ok (first-tick `head_moved=true` is benign; pause is global by design) |
| FE device status mirror | optimistic write → authoritative `device-status` events always win | `remoteDevices.svelte.ts` (`connect()` completes its optimistic transition only if the status still shows it) | fixed: the connect resolve used to overwrite an authoritative event that arrived mid-await (2026-11 wave #9) |

## Actions (custom command runs)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Local action run | `register → readers → waiter → set_done → done event`; cancel kills | `src-tauri/src/actions.rs` (`ActionRegistry`) | fixed: reader/waiter thread-spawn failure left a `done=false` entry forever; `abort()` now tears the run down on any post-register failure |
| Remote action run | `insert flag → exec_stream(≤60min) → done event → remove` | `src-tauri/src/remote/actions.rs` | fixed: insert-before-spawn leaked the entry on spawn failure. Run outcome is now typed on the wire (`ActionOutput.outcome`: `completed \| cancelled \| failed`; cancel wins over the transport result) — a cancelled run no longer renders as "failed" (2026-11 wave #6) |

## Repo state watching

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Watcher debounce | `idle → collecting(≤200ms) → emit(batch, generation++)` | `src-tauri/src/watcher.rs` | implicit-ok (generation protocol documented in contracts.md) |
| Watcher liveness | `running → (3 restarts w/ backoff) → dead` | `RepoWatcher.is_dead()`; `RepoManager::open` replaces a dead watcher | fixed: a dead watcher stayed in its slot forever — reopening the repo did not revive it |
| Git in-progress detection | `clean ⇄ merge / rebase / cherry-pick / revert` (+ independent `bisecting`) | `src-tauri/src/engine/in_progress.rs` — one precedence function (`rebase > cherry-pick > revert > merge`) shared by the local engine, the merge conflict-source label, and the remote state probe; `RepoStatus.bisecting` surfaced (StatusPanel badge) | fixed: the precedence rule was duplicated by hand in `libgit2.rs` and `merge.rs`, and the remote probe had drifted (checked MERGE_HEAD first, mislabeling a remote rebase's conflicts as "merge"); an active bisect was invisible entirely (2026-11 wave #3) |

## Rebase / bisect / merge state

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Custom rebase | `running ⇄ paused_for_edit / paused_for_conflict / paused_for_exec → done/aborted`; persisted before every step | `src-tauri/src/engine/rebase.rs` (`RebaseFileState` at `<gitdir>/mygitui/rebase.json`); FE monitor `src/lib/components/rebase/rebaseStore.svelte.ts` | explicit (backend) / fixed (FE): the monitor's `paused` getter now requires `active` |
| Bisect | `inactive → probing(remaining>0) → complete(remaining===0) → reset`; surfaced in `RepoStatus.bisecting` so the UI never shows "clean" mid-search | `engine/bisect.rs` (`BisectFileState`, archive on reset); FE `BisectBanner` + StatusPanel badge | explicit (backend) / fixed (surfacing) |
| Merge / sequencer | MERGE_HEAD etc. consumed by the finishing commit | `engine/merge.rs` | implicit-ok (tested via merge_tests) |

## Forge (GitHub via gh CLI) & AI

| Machine | States | Source of truth | Status |
|---|---|---|---|
| gh invocation | `spawn → drain → exit \| timeout-kill` (stateless per command) | `src-tauri/src/forge.rs` (`ProcOutcome`, per-class timeouts, tested) | implicit-ok (no run registry — cancellation impossible by design) |
| PR/action tab data | per-key `AsyncData<T>`: `idle → loading(previous) → ready \| error` | `src/lib/stores/forge.svelte.ts` (one `AsyncMap` per target group; `busy`/`runBusy`/`dispatchBusy` action flags stay booleans — implicit-ok) | fixed: was 14 parallel data/loading/error maps + 8 seq-counter maps; `loading && error` was reachable and PR/check fetches had NO error state (silent stale/empty lists). PRs/checks now surface errors, a failed refresh keeps the previous list visible, and a new load clears the error the moment it starts (2026-11 wave #2) |
| Run polling | `loading → live-polling(3.5s, visibility-gated) → finished(one final fetch)` | `WorkflowRunView.svelte` (polls only a seen run; the finished-edge resets per view) | fixed: the `wasActive` edge detector misfired a final fetch on mount of a finished run and across view switches (2026-11 wave #14) |
| AI connection | `unknown → checking → ok/down/unauthenticated` (coalesced probes + jittered backoff) | `src/lib/ai/connection.ts` | explicit (tested) |
| AI commit/PR generation | `idle → generating(busy-guard) → result \| error`; dialog prefill round-trips the typed UI bus **with a 45 s deadline** | `src/lib/ai/ai.svelte.ts`, `CreatePrDialog.svelte` | fixed: with no bridge listening the button stuck on "Waiting…" forever — the deadline turns the wedge into a recoverable error (2026-11 wave) |
| opencode serve | `empty → starting(≤30s) ⇄ running → stopped` (single slot) | `src-tauri/src/opencode.rs` registry | implicit-ok |

## Persistence (localStorage)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Per-key parse/store | `unparsed → enveloped {v, data} → migrated → validated (malformed dropped) → debounced last-wins write` | `src/lib/state/persist.ts` (`loadVersioned` + migration registry); `railPins` adopted as the canonical example (legacy lists load as v1 and re-save enveloped) | fixed: no schema versioning existed and the one migration (autofetch's legacy-key heuristic) was a guess — the next shape change migrates instead (2026-11 wave #7) |

---

## Follow-up backlog

All 14 items from the 2026-11 audit are resolved — each table above names
the wave that fixed it (#1 DialogState, #2 forge triples, #3 in-progress
unification + bisect surfacing, #4 prompt↔lifecycle coupling, #5 clone
run id, #6 actions outcome, #7 storage envelope, #8 conflict-editor busy
union, #9 device mirror + prompt slot, #10 overlay stack, #11 sha-owned
multi-select, #12 remap ownership, #13 popout restore, #14 polling
derivation). The backlog section is kept as a pointer: new machines enter
via the archetypes above, and implicit-ok machines get revisited when
they stop being isolated.

Still `implicit-ok` on purpose (correct, tested, isolated — do not
migrate for uniformity): autofetch's tombstone tokens, `auth.rs`'s
attempts counter, the forge/action busy booleans, App's
`deviceDetailId` view-mode nullable, the palette/switcher open flags,
WorktreePanel/BranchPanel single-fetch loading triples.
