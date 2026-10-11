# mygitui

A fast, configurable Git GUI for daily use on Windows, Linux, and macOS. Native
performance via **Tauri v2** (Rust backend) with a **Svelte 5** SPA frontend,
git operations on a **git2-rs** core, Material 3 dynamic theming, and opt-in AI
assist via **opencode** (default when connected) or **OpenRouter**.

## Quickstart

Prerequisites: [Node 22+](https://nodejs.org), [Rust stable](https://rustup.rs)
(with `rustfmt` + `clippy`), and `git`.

```bash
npm install
npm run tauri dev
```

## Commands

| Command                    | What it does                                  |
| -------------------------- | --------------------------------------------- |
| `npm run dev`              | Frontend only (Vite dev server)               |
| `npm run tauri dev`        | Full app: Rust backend + frontend, hot reload |
| `npm run test`             | Frontend unit tests (vitest)                  |
| `npm run check`            | Type check (svelte-check)                     |
| `npm run build`            | Production frontend build                     |
| `cargo fmt --check`        | Rust format check (run in `src-tauri/`)       |
| `cargo clippy --all-targets -- -D warnings` | Rust lint, warnings fatal (in `src-tauri/`) |
| `cargo test`               | Rust tests (in `src-tauri/`)                  |

CI (`.github/workflows/`) runs the frontend suite on Ubuntu and the Rust
fmt/clippy/test suite across Ubuntu/Windows/macOS on every push to `main` and
every PR. The `fast-checks` job runs fmt + clippy first so lint mistakes get
one clear, fast failure; the 3-OS matrix covers tests (and per-OS clippy,
since lints can be cfg-dependent). Linux tests run inside a private
D-Bus session with an unlocked gnome-keyring so the Secret Service keyring
path is exercised, not skipped.

## Pre-push hook & test conventions

`npm install` wires a `pre-push` git hook (`git config core.hooksPath hooks`)
that runs the same checks as CI locally — fmt, clippy, cargo test,
svelte-check, vitest — so failures surface in seconds instead of as a red
run on `main`. Bypass in an emergency with `git push --no-verify`.

Two conventions keep the 3-OS test matrix green:

- **Tests touching a platform facility** (OS keyring, keychain, git
  credential helpers) must degrade gracefully when it's absent — probe and
  skip, never unwrap. See `backend_available()` in
  `src-tauri/src/keyring_store.rs` for the pattern.
- **Property tests**: strategies sharing a line pool can draw equal inputs
  (no diff to stage, nothing to restore) — guard with `prop_assume!`. When a
  property test fails anywhere, proptest appends the shrunk seed to
  `src-tauri/proptest-regressions/`; that file is committed and every future
  run replays those seeds, so a found bug stays caught.

## Test fixtures

`bash scripts/make-fixtures.sh` generates throwaway fixture repos under
`fixtures/` (basic, conflicted, submodules, a ~500-commit bulk-history stub
with `--full` for 100k, and a worktree sample) for manual testing against the
app. The script is idempotent and only writes inside `fixtures/`, which is
generated output — do not commit it.

## Features

- **Working copy**: status tree, whole-file + hunk + line-granular staging,
  discard (checkpoint-backed), `.gitignore` quick-add and template gallery
- **Commits**: amend, author override, signing-aware (GPG/SSH via the git
  CLI), hook badges, AI message generation (opt-in)
- **Branches**: create/switch (dirty-choice dialog)/rename/delete with
  merged-guard previews, reset-to, merge dialog (ff policy, squash,
  `--no-commit`, `-X ours|theirs`), merged-branch cleanup wizard, remote
  branch checkout and deletion
- **Tags**: lightweight + annotated + signed (`git tag -s`), push tags
- **History**: virtualized canvas graph (250k commits), regex + pickaxe
  (`-S`/`-G`) filters, two-ref compare, blame with jump-to-commit, file
  history popout (rename-following), describe line, bookmarks, archive
  export
- **Rewriting**: interactive rebase (reorder, squash/fixup/drop/edit/
  reword, `exec` lines), autosquash `fixup!`/`squash!`, cherry-pick and
  revert sequences with a 3-way conflict editor + external mergetool
  launch
- **Safety net**: checkpoint undo system (pre-restore snapshots, GC),
  deleted-branch trash with one-click restore, dangerous-op previews,
  reflog recovery (reset/branch/show)
- **Network**: fetch (prune/depth), pull (ff/merge/**rebase**), push
  (force, **force-with-lease** with real expected-sha checking, tags,
  multi-ref, remote ref deletion), ssh-agent/keyring auth, blobless
  (`--filter=blob:none`) clones
- **Bisect**: full stepper (start form, Good/Bad/Skip, first-bad result)
  with mark-history log
- **Housekeeping**: worktrees (main entry + prune), submodules, stashes,
  repo health panel (sizes/objects/**fsck dangling census**) with `gc` /
  `prune` / `commit-graph` / `pack-refs`, sparse checkout (cone mode),
  LFS status + pull/push
- **Verification**: commit signature badges (GPG/SSH via
  `git verify-commit`), signed fixture repos
- **Platform**: embedded terminal, custom shell actions **with keyboard
  shortcuts**, GitHub PRs + CI checks (`gh` CLI), configurable layouts
  with popouts and split panes, command palette + keybinds + context
  menus, M3 theming (accent-seed dynamic color + OS accent), syntax
  highlighting in diffs (Shiki), a11y throughout
- **AI assist (opt-in per repo)**: commit message, PR title/desc,
  explain-hunk, staged-diff review, stash message, branch-name
  suggestion; connection health chip
- **Diff workflow**: line-granular stage/unstage/discard (gutter
  selection), blame with age heat and re-blame-at-parent
- **Quality gates**: proptest roundtrips (hunk apply, checkpoint
  restore), Playwright smoke flows, criterion benches with nightly
  baseline comparison, watcher soak with RSS gate, 3-OS CI matrix

## Roadmap

M0 scaffold → M1 read core → M2 mutations → M3 power + safety →
M4 housekeeping → M5 PTY panel → M6 AI + forge → M7 luxury + a11y →
M8 hardening (perf gates, soak, packaging + updater) →
**M9 complete git core** (merge UI, tags, submodules panel, file history,
hunk staging, discard, context menus, pull --rebase, force-with-lease,
push upgrades, mergetool, regex filter) →
**M10 power git** (bisect, describe, autosquash, rebase `exec`, remote
branch management, pickaxe) →
**M11 repo health** (maintenance ops, archive, sparse checkout, blobless
clone, LFS, syntax highlighting, i18n groundwork) →
**M12 gap closure** (branch trash + restore, real force-with-lease,
fsck census, op cancellation, gitignore-aware watcher, auto checkpoint
GC, signature badges, pickaxe `-G`, bisect log, line staging, blame
heat + re-blame, all six AI features, deep links, first-launch argv,
proptests + soak + baseline-compare gates, Playwright smoke). See
`docs/contracts.md` for the per-milestone IPC contracts.


## License

MIT
