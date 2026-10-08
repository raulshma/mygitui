# mygitui — "Complete Git Solution" Plan (M9–M11)

Recon result: engine is strong (M0–M8 done), but the app is not yet a complete git client. Three gap classes drive this plan:
1. **Orphaned UI** — backend exists, zero UI callers: `merge_branch`, `tag_create`/`tag_delete`, `submodules` (SubmodulePanel.svelte is complete but unregistered), `repo_file_history`, hunk/line `StageTarget`, `gitignore_templates`, ~10 palette commands dispatch CustomEvents nobody listens to.
2. **Missing git ops** — pull-rebase (diverged → `Unsupported`), force-with-lease, push tags / delete remote ref, squash-merge, merge strategy options, bisect, describe, archive, gc/fsck/maintenance, autosquash, remote-branch checkout, regex log filter, diff Index→Commit, sparse-checkout, partial clone, LFS.
3. **Known bugs/polish** — MERGE_HEAD not consumed after merge-finishing commit (WIP only fixed the sequencer half), stale "lands in M2/M4" toasts, `window.confirm/prompt` remnants, no context menus, no syntax highlighting, branch cleanup wizard (plan §5) never built, bookmarks display-only.

User decisions: full scope M9–M11 · targeted caching perf · context-menus+palette UI paradigm · external mergetool via launch.

---

## Step 0 — Land WIP baseline
Uncommitted work (sequencer_abort + SplitPane, ~+492/−111, with tests) is sound and post-M8-consistent. Run `cargo test` + `vitest` + `svelte-check`, then commit as its own commit before touching anything.

---

## M9 — Complete Git Core (close orphans + known gaps)

### M9 Backend (Rust, `src-tauri/src/engine/`)
1. **MERGE_HEAD consumption** — extend `clear_sequencer_files()` in `mutations.rs` commit(): when `status.merging` and the new commit has 2 parents, also remove MERGE_HEAD/MERGE_MSG/MERGE_MODE. Tests in `merge_tests.rs`.
2. **Pull --rebase** — `netops.rs` pull(rebase=true, diverged): build an automatic pick plan (HEAD..upstream reversed), replay via the existing `rebase.rs` merge_trees core onto FETCH_HEAD, fast-forward the branch ref. Conflicts surface as normal rebase state so ConflictEditor + rebaseMonitor keep working.
3. **Push upgrades** (`netops.rs`) — force-with-lease (expected sha from remote-tracking ref, `PushUpdate` reject on mismatch), push tags (`refs/tags/*` or explicit), delete remote branch (`:refs/heads/x` refspec), multi-ref push (`Vec<String>`).
4. **Regex log filter** — add `regex` crate; implement in `libgit2.rs` log path (compile once per query, size-capped). FE flips the disabled Regex checkbox.
5. **Diff Index→Commit** — add side combination in `libgit2.rs::diff()`.
6. **Merge options** — `merge_branch` takes options: `no_ff`, `squash` (merge_trees → stage only + SQUASH_MSG), `no_commit`, `favor: ours|theirs|none` (libgit2 merge_file favors).
7. **Tags** — `tag_list` with metadata (annotated: tagger/message/target; lightweight), tag push, signed tags via CLI fallback (`git tag -s`, same sanitized-spawn pattern as `cli.rs`).
8. **Remote-branch checkout** — checkout/create local from remote-tracking with upstream auto-set.
9. **Discard op** — `discard(paths, hunks?)`: whole-file = force-checkout from HEAD; hunk-range = reverse-apply patch. Always behind `guard_checkpoint` (never side effects without checkpoint — same ordering rule as repo_clean).
10. **Mergetool** — config introspection (`merge.tool`, `mergetool.<t>.path/cmd`) + launch via `git mergetool --tool=<t> -- <file>` (inherits trustExitCode/markers semantics); exit 0 → offer mark-resolved. Built-in editor stays default; per-repo opt-in.

### M9 Frontend (Svelte, `src/lib/`)
1. **ContextMenu component** (keyboard-accessible, roving focus, Escape/arrow nav) + wiring:
   - commit rows (HistoryView): cherry-pick, revert, branch-from-here, tag-at-here, bookmark, copy sha, reset-to, rebase-from-here
   - status files: stage/unstage, discard (preview+checkpoint), ignore (exact/ext/dir), file history, blame, open file
   - branch rows (BranchPanel): switch, merge-into-current, rename, delete, push, set-upstream
   All actions also registered as palette commands (replacing the dead CustomEvent dispatches with a small typed event-bus store — `src/lib/palette/` keeps command registry single-source).
2. **Merge UI** — BranchPanel toolbar + branch context menu → MergeDialog (ff/no-ff/squash/no-commit, dirty-guard, conflict path lands in existing banner/ConflictEditor flow).
3. **Tags UI** — Tags section in BranchPanel: list with annotated chips, create dialog (lightweight/annotated/message/target datalist), delete confirm; "Push tags" in RemotePanel.
4. **Register SubmodulePanel** — add `PanelId`, `PANEL_META`, RepoView snippet registry (trivial; panelModel tests).
5. **File history view** — from status-file/commit-file menus: opens a popout-capable view reusing `streamFileHistory` + GraphCanvas row model; "Blame" cross-links.
6. **Hunk/line staging + discard in DiffViewer** — per-hunk stage/discard buttons on hunk headers (backend `StageTarget` exists); line-range selection → stage selected lines.
7. **Branch cleanup wizard** — scan merged branches, batch delete behind one guard checkpoint; palette command.
8. **Reflog/Blame action fixes** — ReflogPanel entries get reset-to/branch-from/copy actions; BlameView sha-click selects the commit in history (kill stale toasts).
9. **Bookmark UI** — star toggle on selected commit (store exists, graph rings already render).
10. **ConfirmDialog/PromptDialog** M3 components replacing all `window.confirm/prompt`.
11. **gitignore templates gallery** — wire `gitignore_templates` into the untracked-file menu.

### M9 Tests/infra
Engine tests for every new op; vitest for menu model, event-bus, dialog components; `docs/contracts.md` entries + `types.ts`/`client.ts` mirrors (existing discipline); fixtures extended (tags, squash/merge scenarios).

---

## M10 — Power Git

1. **Bisect** — backend state machine persisted at `<gitdir>/mygitui/bisect.json` (start/good/bad/skip/reset/log; candidates via revwalk good..bad, midpoint checkout; pure selection logic unit-tested). FE: stepper banner (Good/Bad/Skip, remaining-candidate count, current sha, reset) + palette commands.
2. **Describe** — git2 describe bindings; shown in commit detail; copy in context menu.
3. **Autosquash** — backend helper builds a rebase plan from `fixup!`/`squash!` subjects in range (pure fn, tested); RebasePlanner gains "Autosquash" button. CommitBar gains "Amend into previous as fixup" quick action.
4. **Rebase `exec` action** — `RebaseStep::Exec(cmd)` run via the sanitized spawn path (actions.rs pattern); non-zero exit pauses like a conflict.
5. **Remote branch management** — remotes panel lists remote branches; checkout (M9 backend), delete via push refspec, compare view shortcut.
6. **Worktree polish** — list includes main worktree; `worktree_prune` IPC.
7. **Pickaxe search (-S/-G)** — log filter extension: per-commit tree-diff token check (substring/regex), result capped + streamed; History filter bar gains pickaxe field.

---

## M11 — Repo Health, Ecosystem, Polish

1. **Repo Health panel** — `repo_health` (size, object/pack counts, dangling via fsck summary) + maintenance ops: `git gc`, `prune`, `repack`, `commit-graph write`, `packed-refs`, each via the sanitized CLI layer, op-queued with progress events. Buttons + last-run display; palette commands.
2. **Archive** — `git archive --format zip|tar <ref>` → path via dialog plugin; context menu on commits/tags.
3. **Sparse checkout** — `git sparse-checkout` list/set/add cone-mode via CLI; manager UI in repo settings.
4. **Partial clone** — clone dialog "blobless" option → CLI clone path (`--filter=blob:none`) when libgit2 RepoBuilder can't.
5. **LFS** — detect `filter=lfs` in .gitattributes; panel section: install/pull/push/status via `git lfs` CLI (detect binary, graceful degrade with guidance); no in-process LFS client.
6. **Shiki syntax highlighting** — async `shiki` import per language, token cache keyed (lang, line-range), integrated into virtualized DiffRow; plain-text fallback; 60fps gate kept (bench on 10k-hunk fixture).
7. **i18n groundwork** — extract strings to a message catalog (lightweight key map, English only), keep `Intl` date/number formatting; no translations yet.

---

## Perf — targeted caching (threaded through M9–M11)
- Maintenance writes **commit-graph + packed-refs** (libgit2 revwalk reads commit-graph — big win on 100k+ commit repos; measure before/after on the 100k fixture before rolling out further).
- **Gitignore-aware watcher** — feed real ignore sets from status into watcher filtering (replaces heuristic IGNORED_DIRS).
- New criterion benches: `tags_list`, `file_history_page`, `bisect_step`, `describe`, `pickaxe_filter`; nightly baseline flow unchanged. Gates documented in `benches/perf.rs` header.
- No architecture rewrite: keep pull-based refresh + generation invalidation.

---

## Execution & verification
- Follow existing milestone discipline: contracts patch (docs/contracts.md + types) → ≤4 parallel lanes → integration.
- Per milestone: `cargo fmt --check`, `clippy -D warnings`, `cargo test` (3-OS matrix in CI), `vitest` + `svelte-check` + `vite build`, criterion vs nightly baseline, manual smoke on regenerated fixtures.
- Order: Step 0 → M9 (backend lanes 1-10 ∥ FE context-menu/dialog skeleton) → M10 → M11 → README feature-table update.

## Risks
- Pull-rebase reusing the rebase engine: conflict-state fidelity — covered by dedicated engine tests before FE wiring.
- Mergetool/LFS/gc spawn cross-platform quirks — all through the existing sanitized-CLI pattern with timeouts; graceful degrade.
- Shiki bundle/parse cost — async import + token cache + plain fallback; benched.
- Commit-graph effectiveness under libgit2 — measured first on the 100k fixture; cut if the win is marginal.