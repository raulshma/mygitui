# mygitui

A fast, configurable Git GUI for daily use on Windows, Linux, and macOS. One
native process: an **iced 0.14** UI on top of a **git2-rs** engine, SSH remote
support (russh), forge integration, and opt-in AI assist via **opencode**
(default when connected) or **OpenRouter**.

The shell is the native **iced 0.14** UI: the former Svelte webview is gone,
and the backend runs in-process behind the `service::*` functions and the
typed `BackendEvent` bus (see `docs/contracts.md`). The migration is
complete — embedded terminal, diff popouts, command palette + keybinds +
context menus, OS-accent theming, installers (Windows MSI, macOS app
bundles, Linux AppImage via [cargo-dist](https://opensource.axo.dev/cargo-dist/)),
and an in-app updater (checksum-verified download-and-install from GitHub
releases) are all in the native UI.

## Quickstart

Prerequisites: [Rust stable](https://rustup.rs) (with `rustfmt` + `clippy`)
and `git`. On Linux install the windowing/SSL headers:
`libxkbcommon-dev libwayland-dev libssl-dev pkg-config`.

```bash
cd app
cargo run --release
```

## Commands

| Command                    | What it does                                  |
| -------------------------- | --------------------------------------------- |
| `cargo run`                | Build and run the app (in `app/`)             |
| `cargo fmt --check`        | Rust format check (run in `app/`)             |
| `cargo clippy --all-targets -- -D warnings` | Rust lint, warnings fatal (in `app/`) |
| `cargo test`               | Rust tests (in `app/`)                        |
| `cargo bench --bench perf` | Criterion benchmarks (in `app/`)              |

CI (`.github/workflows/`) runs the Rust fmt/clippy/test suite across
Ubuntu/Windows/macOS on every push to `main` and every PR. The `fast-checks`
job runs fmt + clippy first so lint mistakes get one clear, fast failure; the
3-OS matrix covers tests (and per-OS clippy, since lints can be
cfg-dependent). Linux tests run inside a private D-Bus session with an
unlocked gnome-keyring so the Secret Service keyring path is exercised, not
skipped.

### Releases

Pushing a `v*` tag runs the [cargo-dist](https://opensource.axo.dev/cargo-dist/)
release workflow: release builds across four targets (Linux/macOS x2/Windows),
tar.xz/zip archives, shell + PowerShell installers, a Windows MSI, SHA-256
sidecars, plus two hand-added jobs producing the Linux AppImage and macOS
`.app` bundles the updater installs from — all published to a GitHub release.
The running app checks that release feed on startup (toggleable) and its
palette offers `Install available update`, which downloads the platform
artifact, verifies the checksum, and applies it (AppImage self-replace /
silent MSI / bundle swap, then restart). Configure dist in
`dist-workspace.toml` (run `dist` from the repo root); `dist plan` previews
a release without building.

## Pre-push hook & test conventions

Enable the `pre-push` git hook once per clone:

```bash
git config core.hooksPath hooks
```

It runs the same checks as CI locally — fmt, clippy, cargo test — so failures
surface in seconds instead of as a red run on `main`. Bypass in an emergency
with `git push --no-verify`.

Two conventions keep the 3-OS test matrix green:

- **Tests touching a platform facility** (OS keyring, keychain, git
  credential helpers) must degrade gracefully when it's absent — probe and
  skip, never unwrap. See `backend_available()` in
  `app/src/keyring_store.rs` for the pattern.
- **Property tests**: strategies sharing a line pool can draw equal inputs
  (no diff to stage, nothing to restore) — guard with `prop_assume!`. When a
  property test fails anywhere, proptest appends the shrunk seed to
  `app/proptest-regressions/`; that file is committed and every future
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
- **Platform**: custom shell actions **with keyboard shortcuts**, GitHub PRs
  + CI checks (`gh` CLI), split panes, command palette + keybinds + context
  menus, shadcn-derived light/dark theming, syntax-highlighted diffs,
  OS-accent theming, embedded terminal, diff popout windows, single-instance
  forwarding + deep links, session restore + window-geometry persistence
  across runs, installers + self-update (MSI / .app / AppImage,
  SHA-256-verified in-app update flow), a11y throughout
- **AI assist (opt-in per repo)**: commit message, PR title/desc,
  explain-hunk, staged-diff review, stash message, branch-name
  suggestion; connection health chip
- **Diff workflow**: line-granular stage/unstage/discard (gutter
  selection), blame with age heat and re-blame-at-parent
- **Quality gates**: proptest roundtrips (hunk apply, checkpoint
  restore), criterion benches with nightly baseline comparison, watcher
  soak with RSS gate, 3-OS CI matrix

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
proptests + soak + baseline-compare gates) →
**M13–M14 native UI** (iced 0.14 cutover: in-process service bus, native
shell, status/commit/diff/history panels, remote + safety surfaces).

M13–M14 are landed on `main` — the iced cutover is done, and the Svelte-era
feature surface (palette, keybinds, context menus, quick switcher, embedded
terminal, forge, bisect, reflog, health, worktrees, submodules, tags, blame,
file history, compare, custom actions, output log, file editor, contribution
stats, syntax highlighting, OS-accent theming, diff popouts, single-instance
forwarding, deep links, AI assist) is fully restored in the native UI.
M8's packaging + updater goal is also landed: tag pushes build installers
for all three OSes (MSI / .app / AppImage) and the app can update itself
from GitHub releases.


## License

MIT
