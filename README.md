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
| `cargo clippy -- -D warnings` | Rust lint, warnings fatal (in `src-tauri/`) |
| `cargo test`               | Rust tests (in `src-tauri/`)                  |

CI (`.github/workflows/`) runs the frontend suite on Ubuntu and the Rust
fmt/clippy/test suite across Ubuntu/Windows/macOS on every push to `main` and
every PR.

## Test fixtures

`bash scripts/make-fixtures.sh` generates throwaway fixture repos under
`fixtures/` (basic, conflicted, submodules, a ~500-commit bulk-history stub
with `--full` for 100k, and a worktree sample) for manual testing against the
app. The script is idempotent and only writes inside `fixtures/`, which is
generated output — do not commit it.

## Roadmap

M0 scaffold → M1 read core → M2 mutations → M3 power + safety →
M4 housekeeping → M5 PTY panel → M6 AI + forge → M7 luxury + a11y →
M8 hardening (perf gates, soak, packaging + updater). See the full build plan
in `.zcode/plans/`.

## License

MIT
