# State Machine Registry

Every lifecycle in the app — where it lives, how it is encoded, and its
status. Companion to `docs/contracts.md` (which owns the wire shapes; this
file owns the behavior machines that produce and consume them).

Statuses: **explicit** = a typed union / persisted state file, invalid
states unrepresentable — keep it that way. **implicit-ok** = a simple
2-state machine whose nullable/boolean encoding is clear and tested — no
work needed. **fixed** = was implicit, caused a real defect, now explicit
or repaired (2026-11 audit). **follow-up** = still implicit; change is
warranted but not urgent — see the backlog at the bottom.

## Encoding archetypes (the vocabulary)

These are the six ways a state machine hides in this codebase. When adding
or reviewing code, match against them:

1. **Nullable-pile mode fields** — "one of N modes" encoded as N parallel
   nullable fields resolved by an if-chain. Every transition must
   hand-reset the siblings; one missed reset is a bug. Rule: **one mode =
   one union, one `$state`** (see the CenterPane pattern in RepoView).
2. **String/regex sentinels** — a protocol carried as an untyped string,
   classified by pattern-matching on the other side. Rule: **type the wire
  ** (ts-rs enum) and keep exactly one sentinel comparison site.
3. **Parallel loading/error/data triples** — `X` + `XLoading` + `XErrors`
   maps with reachable invalid combos (`loading && error`). Rule: derive,
   don't store; or carry one `{ state: "idle" | "loading" | "ready" |
   "error" }` per key.
4. **Hand-rolled transition guards** — stale tokens, edge detectors,
   epoch mirrors, armed/fired toast-once flags. Correct but untyped; every
   async callback must remember the guard. Acceptable when isolated;
   consolidate when they multiply.
5. **Lifecycle ownership gaps** — nobody kills/cleans up: a session, a
   map entry, a dialog outliving its reason. Rule: **the thing that ends
   the lifecycle unregisters itself** (exit waiter, reader task, done
   event), and every "insert before async" needs a failure-path removal.
6. **Missing terminal states** — a two-state machine (`null` / data) with
   no error state, so failure renders as an eternal spinner. Rule: **if a
   fetch can fail permanently, the UI needs a third state.**

---

## App shell & tabs

| Machine | States | Source of truth | Status |
|---|---|---|---|
| App shell view | `launching → home/recents ⇄ workspace` (+ offline sub-state per tab) | `App.svelte` derives off `tabStore` (`launching = restoring && activeId === null`) | implicit-ok (tested: `tabs.test.ts`) |
| Tab lifecycle | `closed → opening → open ⇄ active` (+ `offline` for remote) | `src/lib/stores/tabs.svelte.ts` (`Tab.id`/`openError`; remote tab id = its `ssh://` URI) | fixed: `openRemoteTab` re-finds the tab after the await — a double-clicked Retry used to `splice(-1, 1, …)` an unrelated tab away |
| Session restore | `restoring → restored` | `TabStore.restoring` + `restoreSession()` | implicit-ok (tested; popouts never call it — see follow-ups) |
| Terminal teardown | per-repo pty: `absent → starting → live → exited/failed`; teardown = kill + forget | `src/lib/terminal/terminalStore.svelte.ts` (`TerminalStatus` union — explicit); teardown trigger | fixed: `TabStore.closeTab` disposes the repo's terminals (the single close choke point); RepoView unmount stays as backstop. Previously only last-tab unmount disposed — background tab closes leaked shells (M5 contract violation) |
| Status fetch | `unloaded (null) → loaded`, **now also `error`** | `Tab.status` + `Tab.statusError` (`tabs.svelte.ts`); rendered by `StatusPanel` | fixed: a failed first `repo_status` used to spin forever; now an error state with Retry. A failed refresh on a loaded tab still keeps the previous status |

## Center editor (RepoView)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Center pane mode | `empty \| diff(entry) \| commit(commit) \| compare(result) \| workflowRun(runId) \| actionRun(runId) \| files` | `CenterPane` union, one `$state` in `RepoView.svelte` (`src/lib/components/panels/RepoView.svelte`); per-mode projections are `$derived` | fixed: was six parallel nullables (`diffEntry`, `selectedCommit`, `compareResult`, `workflowRunId`, `actionRunId`, `filesMode`) + EditorPane's priority if-chain; the repo-switch reset missed three of them, so repo A's file tabs ran IPC against repo B |
| Diff stream | `idle → streaming → loaded` (token-guarded) | `diffFiles`/`diffLoading`/`diffHunkable` + `diffToken` in RepoView | implicit-ok (stale-token pattern; per-repo reset owned by the mode union) |
| File workspace tabs | `openFiles[]` + `activeFile`, VS Code-style | RepoView (repo-scoped; reset on repo switch) | fixed: the same missed-reset bug kept repo A's open files mounted in repo B |
| Auto-open first change | fires once per repo when `center.kind === "empty"` | RepoView `autoOpenedFor` | fixed: the old guard checked five nullables but not `filesMode`, yanking an open workspace aside |

## Dialogs & overlays

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Per-dialog open/args | `closed → open(args) → closed` | one boolean/nullable per dialog + arg fields (`App.svelte` overlays, `FilesPanel` prompt/confirm, `HistoryView` sha slots, `branchOps` slots, safety dialogs) | follow-up (see backlog: `DialogState<T>` primitive) |
| Modal stacking / Escape | unarbitrated — palette, context menu, conflict editor, dialogs can stack | `z-*` classes; per-component Escape handling | follow-up |
| Confirm/prompt flows | same as above; danger classification is explicit | `src/lib/components/safety/safetyModel.ts` (`DangerLevel` — explicit, tested) | explicit (model) / follow-up (wiring) |

## Operations (op queue → FE)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Op lifecycle | `queued → running → done(completed\|cancelled\|failed)`; queued-cancel skips execution | backend `src-tauri/src/ops.rs` (`OpOutcome::for_error` — one sentinel comparison site, `CANCELLED_ERR`); FE `src/lib/stores/ops.svelte.ts` (`isCancelledOp` prefers the typed `outcome`) | fixed: FE classified cancellation by matching the error string against `/cancel/i`; the wire now carries `OpProgress.outcome` (typed enum), regex kept as fallback only |
| Op cancel UX | `running → cancelling(flag) → done` | `ActiveOp.cancelling` flag, un-flagged by the done event or a failed `op_cancel` | implicit-ok (tested) |
| Hook/CLI log session | `fresh(op_id) → done → dismissed/replaced` | `OpStore.logs` (newest-op-wins per repo) | implicit-ok |
| Auth request → dialog | `pending(null) → showing(request) → answer/dismiss → next queued` | `AuthStore` (`ops.svelte.ts`); correlation key `op_id` | fixed: a dialog whose op died without an answer stayed up forever — `OpStore.onOpFinished` (injectable hook) now retires it; plus a 120 s `pruneStale` mirroring the backend prompt timeout |
| Clone progress | `idle → cloning(url-keyed progress) → done/failed`; dialog may hide mid-run | `src/lib/components/CloneDialog.svelte` (`cloning`, `progress`, `activeUrl`) | follow-up (no run id; hidden clones unresurfaced) |

## Terminals & sessions (backend)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Local pty session | `spawn → output pipeline → exit waiter → unregister → pty-exit` | `src-tauri/src/pty.rs` (cap via `RegistryInner::reserve_slot` RAII reservation) | fixed: cap check and insert were separated by openpty/spawn — two concurrent `pty_create` could overshoot `MAX_SESSIONS`; now one atomic reserve/commit with abort-on-failure |
| Remote shell session | same lifecycle, reader task owns end-of-life | `src-tauri/src/remote/term.rs` (`RegistryInner::finish_session`; registry testable via the `ShellChannel` seam) | fixed: device-side channel close emitted `pty-exit` but never unregistered — 8 shell exits wedged `rshell_create` until app restart; the reader now unregisters before the exit event (mirrors the local exit waiter), and a failed reader spawn tears the session back down |
| Cursor-probe handshake | conhost probe → answer → stream | `pty.rs` `strip_cursor_probes` (pure, unit-tested) | explicit |

## Devices (remote projects)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Device connection | `disconnected → connecting → connected` (+ watchdog decay to disconnected) | Rust: `DeviceStatus` enum + `watch` channel (`remote/transport.rs`); wire: typed enum end to end (`DeviceStatusEvent.status`, `DeviceInfo.status`, `device_status`/`device_connect` returns) | fixed: the enum was flattened to `String` at every boundary with a `#[ts(type=…)]` annotation holding the union together |
| Dial race | concurrent `session_for` → first insert wins, loser disconnects | `DeviceRegistry::session_for` | fixed: the second caller used to overwrite the first — an authenticated SSH session leaked and two `device-status` streams ran for one device |
| Host-key TOFU | `Unknown → Trusted → Changed → (re-confirm) → Trusted` | `remote/hostkeys.rs` (`HostKeyStatus`, atomic file, tested) | explicit |
| Device prompts | `pending map → emitted event → answered/120s-timeout → resolved` | `remote/prompts.rs` (oneshot + deadline); FE queues in `remoteDevices.svelte.ts` (`pruneStale`, tested) | implicit-ok (backend cleanup on app exit covered by 120 s deadline; not tied to op-cancel — see follow-ups) |
| Remote repo poller | `sleep(5s) → fingerprint compare → generation bump + full resync` | `remote/polling.rs` | implicit-ok (first-tick `head_moved=true` is benign; pause is global by design) |
| FE device status mirror | optimistic `connecting` → authoritative `device-status` events | `remoteDevices.svelte.ts` | follow-up (resolve-path can overwrite an authoritative event; two prompt queues can stack) |

## Actions (custom command runs)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Local action run | `register → readers → waiter → set_done → done event`; cancel kills | `src-tauri/src/actions.rs` (`ActionRegistry`) | fixed: reader/waiter thread-spawn failure left a `done=false` entry forever — 3 failures rejected every later action for the repo (`running_count_for`); `abort()` now tears the run down on any post-register failure |
| Remote action run | `insert flag → exec_stream(≤60min) → done event → remove` | `src-tauri/src/remote/actions.rs` | fixed: insert-before-spawn leaked the entry on spawn failure (`cancel` reported true forever); entry removed on the failure path now. Run status classifies via `exit_code: Some(-1)` — cancelled vs failed is not distinguished here (noted in backlog) |

## Repo state watching

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Watcher debounce | `idle → collecting(≤200ms) → emit(batch, generation++)` | `src-tauri/src/watcher.rs` | implicit-ok (generation protocol documented in contracts.md) |
| Watcher liveness | `running → (3 restarts w/ backoff) → dead` | `RepoWatcher.is_dead()` (flag flipped when the debounce thread exits); `RepoManager::open` replaces a dead watcher | fixed: a dead watcher stayed in its slot forever — reopening the repo did not revive it, despite the log line claiming it would |
| Git in-progress detection | `clean ⇄ merging / rebasing / sequencer` (re-derived from git plumbing each status) | `engine/libgit2.rs` + `engine/merge.rs` (+ `remote/parsers.rs` for devices) | follow-up (precedence rule duplicated in two files; active bisect invisible in `RepoStatus`) |

## Rebase / bisect / merge state

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Custom rebase | `running ⇄ paused_for_edit / paused_for_conflict / paused_for_exec → done/aborted`; persisted before every step | `src-tauri/src/engine/rebase.rs` (`RebaseFileState` at `<gitdir>/mygitui/rebase.json`); FE monitor `src/lib/components/rebase/rebaseStore.svelte.ts` | explicit (backend) / fixed (FE): the monitor's `paused` getter now requires `active` — the finished-rebase summary state used to leave Continue enabled against a dead rebase |
| Bisect | `inactive → probing(remaining>0) → complete(remaining===0) → reset` | `engine/bisect.rs` (`BisectFileState`, archive on reset); FE `BisectBanner` | explicit (backend) / follow-up (completion encoded as `remaining === 0`; not surfaced in `RepoStatus`) |
| Merge / sequencer | MERGE_HEAD etc. consumed by the finishing commit | `engine/merge.rs` | implicit-ok (tested via merge_tests) |

## Forge (GitHub via gh CLI) & AI

| Machine | States | Source of truth | Status |
|---|---|---|---|
| gh invocation | `spawn → drain → exit \| timeout-kill` (stateless per command) | `src-tauri/src/forge.rs` (`ProcOutcome`, per-class timeouts, tested) | implicit-ok (no run registry — cancellation impossible by design) |
| PR/action tab data | per-key `data + loading + errors` triple-maps with seq guards | `src/lib/stores/forge.svelte.ts` | follow-up (error-only-clears-on-success leaves visible `loading && error` combos) |
| Run polling | `loading → live-polling(3.5s, visibility-gated) → finished(one final fetch)` | `WorkflowRunView.svelte` (`wasActive` edge detector) | follow-up (edge detector can misfire when `run` is briefly null) |
| AI connection | `unknown → checking → ok/down/unauthenticated` (coalesced probes + jittered backoff) | `src/lib/ai/connection.ts` | explicit (tested) |
| AI commit/PR generation | `idle → generating(busy-guard) → result \| error`; dialog prefill round-trips the typed UI bus | `src/lib/ai/ai.svelte.ts`, `CreatePrDialog.svelte` | follow-up (PR prefill has no deadline if no bridge answers) |
| opencode serve | `empty → starting(≤30s) ⇄ running → stopped` (single slot) | `src-tauri/src/opencode.rs` registry | implicit-ok |

## Persistence (localStorage)

| Machine | States | Source of truth | Status |
|---|---|---|---|
| Per-key parse/store | `unparsed → validated (malformed dropped) → debounced last-wins write` | every store (`mygitui.session`, `mygitui.ai`, `mygitui.layouts`, `mygitui.bookmarks`, …) | follow-up (no schema versioning; the one migration — autofetch's legacy-key drop — is a heuristic) |

---

## Follow-up backlog (ordered by value/risk)

1. **`DialogState<T>` primitive** — collapse the "boolean + N arg fields,
   reset ad hoc" pattern (`FilesPanel`, `HistoryView` sha slots,
   `branchOps` slots, App overlays) into `{ open: false } | { open: true,
   ...args }` per dialog. Kills the "forgot one reset site" class.
2. **ForgeStore triple consolidation** — one `AsyncData<T>` per key
   (idle/loading/ready/error) instead of 14 parallel maps; derive, don't
   co-store.
3. **Git in-progress detection unification** — one precedence function
   (`rebase > cherry-pick > revert > merge`) shared by `libgit2.rs`,
   `merge.rs`, and `remote/parsers.rs`; surface an active bisect in
   `RepoStatus` so the UI never shows "clean" mid-search.
4. **Prompt ↔ op-cancel coupling** — `op_cancel`/repo close should resolve
   pending `auth-request`/hostkey/device-auth prompts immediately instead
   of letting the handshake thread sit out its 120 s.
5. **CloneDialog run identity** — a run id in the clone progress payload
   (URL matching breaks on sequential same-URL clones); surface hidden
   in-flight clones; add a timeout.
6. **Actions run outcome** — distinguish cancelled from failed in the
   `action-output` done event (currently both are `exit_code: Some(-1)`).
7. **localStorage schema versioning** — a `{ v: 1, … }` envelope + one
   migration registry before the next shape change (the autofetch
   heuristic shows migrations will recur).
8. **`ConflictEditor` busy sentinel** — replace `busyPath = "::abort"`
   with a `{ kind: "resolve", path } | { kind: "abort" }` union; clear the
   armed-abort timer on unmount.
9. **FE device-status mirror** — don't let the `connect()` resolve overwrite
   an authoritative `device-status` event; arbitrate the two prompt queues.
10. **Modal stacking policy** — one owner for overlay order + Escape
    arbitration (palette, context menu, conflict editor, dialogs).
11. **`HistoryView` epoch** — the index-based multi-selection survives on
    one effect faithfully mirroring `restartEpoch`; a selection-owned
    machine (sha-based, like the single-select) would remove the mirror.
12. **`StatusPanel` pending remaps** — a failed stage leaves its selection
    remap queued and an unrelated refresh applies it; tie remaps to the
    mutation that queued them.
13. **Remote tab session restore** — popouts never clear `restoring`; any
    future popout-side reader of `launching` wedges. Give popouts their
    own restore path or a deadline.
14. **Forge run polling edge detector** — `wasActive` misfires a final
    fetch when the run is briefly `null`; derive from run presence instead.
