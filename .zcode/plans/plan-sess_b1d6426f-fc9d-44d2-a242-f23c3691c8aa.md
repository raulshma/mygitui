# mygitui — Git GUI (Tauri v2 + Svelte 5 + git2-rs) — Build Plan (rev 4)

Personal daily-driver git client, 3 OS daily. Perf + robustness as CI-enforced gates. AI via opencode server (default when connected) + OpenRouter. Every milestone ends with a usable app.

## Implementation orchestration (per user: subagents, max 4 concurrent)
- All build work dispatched to parallel subagents, hard cap **4 at a time**.
- Four standing lanes: **L1 Rust git core** (engine/repo/ops/diffcore/graph/search), **L2 Rust platform** (watcher/checkpoints/pty/forge/auth/submodules), **L3 Frontend core** (stores/ipc/diff view/canvas graph/search UI), **L4 Frontend UX** (panels/layout/palette/AI/theme/popouts).
- Per milestone: (1) orchestrator fixes interface contracts first — `GitEngine` trait signatures, typed IPC command/event schema, component props — committed as a contracts patch; (2) ≤4 subagents build lane tasks against contracts in parallel; (3) orchestrator runs integration, tests, benches, polishes seams. No agent edits shared files outside its lane; cross-lane needs go through contract change → orchestrator.
- Each subagent task ships with its own tests (nextest / vitest) as definition-of-done.

## Locked decisions
1. **Scope/distribution**: personal, Win/Linux/mac daily; CI 3-OS matrix + Tauri updater + GitHub Releases (mac ad-hoc + updater keys; Win NSIS; Linux AppImage+deb).
2. **Shell**: single window, multi-repo tabs, recent-repos home. Active tab holds hot watcher/caches.
3. **Git engine**: git2-rs core behind `GitEngine` trait; CLI fallback + per-op capability table for libgit2 weak spots. Mutations via serial per-repo op queue, streamed progress, cancellation (CLI killable; git2 limits documented).
4. **Frontend**: Vite + Svelte 5 runes SPA (no SvelteKit); m3-svelte wrapped in local components; M3 dynamic color (wallpaper-derived) + system light/dark; thin router; English-only with string table from day one.
5. **Git features (v1)**: status; stage file/hunk/line; commit/amend; branch ops + merged-branch cleanup wizard + ahead/behind badges; tags; fetch/pull/push/prune + remotes; merge + built-in 3-way conflict editor + external mergetool launch + per-file strategy (ours/theirs/both); interactive rebase (reorder/squash/fixup/drop/edit/reword); cherry-pick (multi/range); revert; guarded resets; reflog + recovery; stash suite (message/keep-index/apply/pop/branch); worktrees (add/list/remove/open-as-tab); blame (+re-blame previous commit, age heat); submodules (status/init/update/sync/open-as-tab); git clean UI (preview + patterns, checkpoint-gated); .gitignore quick-add + template gallery.
6. **Search (all v1)**: message text/regex, author, date-range, path filters (Rust-side, <16ms incremental); file history with rename-following; two-ref/commit compare view; pickaxe -S/-G content search with async progress + cancel.
7. **Diff viewer full-spec**: side-by-side + unified; imara-diff word diffs (Rust); virtualized rows (10k+ hunks @ 60fps); async Shiki, plain-text-first (worker + cache); image diff (side-by-side + opacity slider); binary detect; line-level staging.
8. **History graph**: full-viewport canvas, custom scroll + hit-testing, DPR-aware, M3-palette colors; lane layout Rust-side during revwalk, streamed in pages; branch color rules; commit bookmarks.
9. **Undo/safety (v1)**: hidden-ref checkpoints (`refs/mygitui/checkpoints/*` + working-tree snapshot via stash-create semantics) before every destructive op; Undo panel w/ one-click restore; destructive-op preview dialogs; deleted-branch trash; 30-day auto-GC of checkpoints.
10. **Signing + hooks (v1)**: detect `commit.gpgsign`/`gpg.format`/`signingkey` → signed commits/tags route through CLI path (gpg/ssh-agent identical to terminal); signature badges on commits; `--no-verify` toggle + hook-visibility in commit dialog.
11. **Layout**: named user presets + resizable splitters + panel show/hide/reorder; global + per-repo persistence; popout windows (diff/graph); PTY + custom-action output as registry panels.
12. **Terminal (v1)**: embedded PTY panel — portable-pty (Rust) + xterm.js, cwd=repo root, resize/scrollback; doubles as interactive auth surface (ssh passphrase, credential prompts).
13. **Onboarding (v1)**: in-app clone (URL + progress + auth); quick-switcher (cmd+K over pinned+recent registry); CLI launcher `mygitui <path>` via single-instance handoff + URL scheme; drag-drop folder open.
14. **Workflow (v1)**: custom actions (user shell commands, per-repo/global, shortcuts, output panel, pre-filled branch/sha args); per-repo auto-fetch + upstream-moved toasts; toast/notification system.
15. **AI (v1)**: dual backend behind `AiProvider` adapter —
    - **OpenCodeProvider (default when connected)**: attach to opencode server; autodiscover 127.0.0.1:4096 via `GET /global/health`; manual URL + basic auth (`OPENCODE_SERVER_PASSWORD`) for remote; `@opencode-ai/sdk` (`createOpencodeClient`); features as opencode sessions (one `mygitui` session per repo, `POST /session/:id/message`, SSE `/event`, `noReply` context injection); model picker from `/config/providers`; per-feature model override.
    - **OpenRouterDirect (secondary)**: Vercel AI SDK + `@openrouter/ai-sdk-provider`, streaming, key in keyring.
    - Features: commit msg gen (conventional), PR title/desc, explain-hunk, staged-diff review, stash message, branch name. Opt-in per repo — code leaves machine only on explicit action.
    - Supervisor (t3code lessons): single retry owner, jittered exp backoff (5min cap), probe-before-reconnect, transport health ≠ data freshness, never auto-replay mutations. Health chip UI.
16. **Forge (v1)**: gh-assisted — detect `gh` CLI; create PR in-app (base/head, AI title/body prefilled); open-PR list + CI checks per branch; open-in-browser links; clipboard fallback when gh absent; glab later.
17. **Extras (v1)**: contribution stats/heatmap panel; a11y pass (keyboard-complete, focus rings, reduced-motion, SR labels); command palette + global keybinds everywhere.
18. **Refresh**: notify watcher + 150–250ms debounce + gitignore-aware filter + incremental re-status; HEAD-change detection = external tool/agent signal; F5 manual. No polling/Watchman.
19. **Perf gates (CI-enforced)**: cold start <400ms; incremental status <50ms; full rescan 100k files <300ms; search filter <16ms on 100k commits; 60fps diff (10k hunks) + graph (250k commits); pickaxe streams progress (never blocks UI); RSS <250MB; 8h leak-free nightly soak.
20. **v1.x (deferred)**: managed opencode spawn (1.x/2.x version-probe topology); opencode permissions panel + session browser + agent activity feed; more harness adapters (Claude Code etc.); LFS; bisect UI; archive; commit templates + conventional-lint; repo health panel (gc/fsck/size); i18n translations. History-rewrite (filter-repo): out of scope.

## Architecture

```
mygitui/
  src-tauri/src/
    engine/        # GitEngine trait; Libgit2Engine; CliEngine (fallback + capability table + signing route)
    repo/          # RepoHandle: state, LRU caches, serial op queue
    ops/           # Operation → progress Channel → result; cancel
    watcher/       # notify wrapper, ignore filter, debounce, external-change detection
    diffcore/      # imara-diff: word diffs, hunk model, line-stage apply
    graph/         # revwalk → lane assignment, batched pages (pure, unit-tested)
    search/        # filtered log walks + pickaxe walker (async, cancelable, progress)
    checkpoints/   # hidden-ref snapshots, working-tree capture, restore, GC, branch trash
    pty/           # portable-pty sessions, resize, I/O channels
    forge/         # gh CLI detection + PR create/list/checks (JSON parse)
    submodules/ worktrees/ auth/ actions/ clean/
    ipc.rs         # commands + Channel streams; paginated; generation tokens cancel stale work
  src/lib/
    stores/        # per-tab RepoStore, rune-based, event-sourced
    ai/            # AiProvider adapter; opencode.ts (SDK + supervisor); openrouter.ts; routing
    components/    # diff/, graph/, panels/ (status, branches, stash, reflog, submodules, search, heatmap, terminal, output), layout/ (presets, splitters, popouts), ai/, merge-editor/, forge/
    palette/ theme/  # command palette, keybinds; M3 dynamic color
  .github/workflows/  # 3-OS: tests + bench gates + release + updater; nightly soak
```

**IPC**: request/response via commands; log/diff/blame/graph/search/pickaxe stream via channels in pages; PTY I/O over dedicated channel; stale streams cancel via generation tokens.

**Robustness**: fixture matrix (submodules, worktrees, in-progress conflicts, detached HEAD, signed repos, 100k commits, 100k files); proptest roundtrips (diff/hunk-apply/checkpoint-restore); index.lock contention handling; mock opencode server + gh stub for tests.

**Testing**: cargo nextest + criterion gates; vitest + Testing Library; Playwright smoke flows against `tauri dev` (open→stage→signed commit; conflict resolve; rebase + checkpoint restore; opencode connect + commit-gen; gh PR create dry-run).

## Milestones (each: contracts patch → ≤4 lane subagents → integration)
- **M0 Scaffold**: Tauri v2 + Vite + Svelte 5 + m3-svelte + theming tokens; CI matrix + updater; keyring; single-instance/CLI launcher; drag-drop.
- **M1 Read core**: tabs/home/quick-switcher/clone; watcher; status; diff viewer; canvas graph; blame; full search suite + compare view.
- **M2 Mutations**: staging; commit/amend + signing route + hook UX; branch/tag ops + cleanup wizard; fetch/pull/push + auth; auto-fetch + toasts; op queue.
- **M3 Power + safety**: merge + conflict editor + mergetool launch; interactive rebase; cherry-pick/revert/resets; reflog + recovery; stash; worktrees; checkpoint/undo system + previews + trash.
- **M4 Housekeeping**: layout presets + popouts; command palette + keybinds; submodules; gitignore quick-add; git clean UI; custom actions.
- **M5 PTY panel**: portable-pty + xterm.js; auth-prompt integration.
- **M6 AI + forge**: AiProvider adapter + OpenCodeProvider (default) + OpenRouterDirect; all six AI features; opt-in gates; mock opencode tests; gh-assisted PR flow.
- **M7 Luxury + a11y**: heatmap/stats; bookmarks; branch colors; a11y pass; string table.
- **M8 Hardening**: perf gates green; soak; packaging + updater release.

## Known risks
- v1 scope very large → milestones are shippable increments; cut from M7 first.
- Parallel subagents drift → contracts-first discipline + orchestrator-owned integration; lanes own disjoint file sets.
- git2 gaps → capability table routes to CLI (same path signing uses).
- m3-svelte youth → local wrappers.
- Full-canvas graph + PTY = biggest custom-code items → isolated modules, pure tested cores.
- opencode 1.x/2.x API drift → version probe at connect, pinned SDK, adapter isolates surface.
- Checkpoint restore must never corrupt: proptest roundtrips + trash before true delete.