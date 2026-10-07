#!/usr/bin/env bash
#
# make-fixtures.sh — generate local git fixture repositories under ./fixtures/
# for mygitui development and testing.
#
# Fixtures created (each run wipes and recreates its directory — idempotent):
#   fixtures/basic          small repo: a few files/commits, branch, tag,
#                           one untracked file
#   fixtures/conflicted     repo left mid-merge with a real content conflict
#   fixtures/submodules     parent repo + child repo wired as a submodule
#   fixtures/commits-100k   bulk-history repo (fast-import). Quick mode
#                           (default): 500 commits. Full mode (--full): 100000
#   fixtures/worktree       repo + a linked worktree on a feature branch
#
# Usage:
#   bash scripts/make-fixtures.sh            # quick mode (~500-commit stub)
#   bash scripts/make-fixtures.sh --full     # full 100k-commit history
#   MYGITUI_FIXTURE_COMMITS=50 bash scripts/make-fixtures.sh
#
# Requires: git (>= 2.28 for `git init -b`). Safe: only ever writes inside
# <repo-root>/fixtures/, and refuses to run if not invoked from the mygitui
# project layout. Fixtures are generated output — do not commit them.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURES="$ROOT/fixtures"

# Safety: never rm -rf outside a recognizable mygitui checkout.
if [[ ! -f "$ROOT/package.json" || ! -d "$ROOT/src-tauri" ]]; then
  echo "ERROR: $ROOT does not look like the mygitui project root (package.json / src-tauri missing)." >&2
  echo "Refusing to touch anything." >&2
  exit 1
fi

QUICK_COMMITS="${MYGITUI_FIXTURE_COMMITS:-500}"
FULL_COMMITS=100000
MODE="quick"

usage() {
  sed -n '3,21p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --full) MODE="full"; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

# Per-repo defaults so fixture creation is deterministic and never blocked by
# host gitconfig (identity, GPG signing, CRLF rewriting).
cfg() {
  git -C "$1" config user.name  "Fixture Bot"
  git -C "$1" config user.email "fixtures@mygitui.local"
  git -C "$1" config commit.gpgsign false
  git -C "$1" config tag.gpgsign false
  git -C "$1" config core.autocrlf false
}

fresh_dir() { # fresh_dir <path-under-fixtures> — wipe + (re)create
  local dir="$FIXTURES/$1"
  rm -rf "$dir"
  mkdir -p "$dir"
  echo "$dir"
}

count_commits() {
  git -C "$1" rev-list --count HEAD
}

# ---------------------------------------------------------------- basic ----
make_basic() {
  local dir
  dir="$(fresh_dir basic)"
  git init -q -b main "$dir"
  cfg "$dir"

  echo "# basic fixture" > "$dir/README.md"
  mkdir -p "$dir/src"
  printf 'def main():\n    print("hello")\n' > "$dir/src/main.py"
  git -C "$dir" add README.md
  git -C "$dir" commit -q -m "init: readme"

  git -C "$dir" add src/main.py
  git -C "$dir" commit -q -m "feat: main entrypoint"

  printf 'def util():\n    return 42\n' > "$dir/src/util.py"
  git -C "$dir" add src/util.py
  git -C "$dir" commit -q -m "feat: util helper"

  git -C "$dir" tag v1.0.0

  # A side branch not merged into main.
  git -C "$dir" checkout -q -b feature/login
  printf 'login stub\n' > "$dir/src/login.py"
  git -C "$dir" add src/login.py
  git -C "$dir" commit -q -m "feat(login): stub"
  git -C "$dir" checkout -q main

  # Untracked file for status-panel testing.
  printf 'scratch notes\n' > "$dir/notes.txt"

  echo "basic:          $(count_commits "$dir") commits, branch feature/login, tag v1.0.0, 1 untracked file"
}

# ----------------------------------------------------------- conflicted ----
make_conflicted() {
  local dir
  dir="$(fresh_dir conflicted)"
  git init -q -b main "$dir"
  cfg "$dir"

  printf 'line1\nline2\nline3\n' > "$dir/app.txt"
  printf 'clean file\n' > "$dir/clean.txt"
  git -C "$dir" add .
  git -C "$dir" commit -q -m "base: app + clean"

  git -C "$dir" checkout -q -b feature/change
  printf 'line1\nline2 (feature)\nline3\n' > "$dir/app.txt"
  git -C "$dir" add app.txt
  git -C "$dir" commit -q -m "feature: edit line2"

  git -C "$dir" checkout -q main
  printf 'line1\nline2 (main)\nline3\n' > "$dir/app.txt"
  git -C "$dir" add app.txt
  git -C "$dir" commit -q -m "main: edit line2"

  # Start the merge and let it conflict; repo is left mid-merge on purpose.
  git -C "$dir" merge feature/change >/dev/null 2>&1 || true

  if git -C "$dir" diff --name-only --diff-filter=U | grep -q .; then
    echo "conflicted:     mid-merge on main, app.txt in conflict (MERGE_HEAD present)"
  else
    echo "conflicted:     ERROR — expected a conflict but none was produced" >&2
    exit 1
  fi
}

# ----------------------------------------------------------- submodules ----
make_submodules() {
  local base child parent
  base="$(fresh_dir submodules)"
  child="$base/child"
  parent="$base/parent"

  git init -q -b main "$child"
  cfg "$child"
  printf 'child lib v1\n' > "$child/lib.txt"
  git -C "$child" add lib.txt
  git -C "$child" commit -q -m "child: initial"

  git init -q -b main "$parent"
  cfg "$parent"
  printf '# parent\n' > "$parent/README.md"
  git -C "$parent" add README.md
  git -C "$parent" commit -q -m "parent: initial"

  # protocol.file.allow: git >= 2.38.1 blocks file:// in submodules by default.
  git -C "$parent" -c protocol.file.allow=always submodule add -q ../child child
  git -C "$parent" commit -q -m "parent: add child submodule"

  # Advance the child so the parent is one commit behind — useful for
  # submodule-update testing.
  printf 'child lib v2\n' > "$child/lib.txt"
  git -C "$child" add lib.txt
  git -C "$child" commit -q -m "child: v2"

  echo "submodules:     parent @ $(git -C "$parent" rev-parse --short HEAD) (child pinned to v1; child HEAD is v2)"
}

# --------------------------------------------------------- commits-100k ----
make_commits_scale() {
  local dir n
  dir="$(fresh_dir commits-100k)"

  if [[ "$MODE" == "full" ]]; then
    n=$FULL_COMMITS
  else
    n=$QUICK_COMMITS
  fi

  git init -q -b main "$dir"
  cfg "$dir"

  # Bulk history via one fast-import process: N commits touching a rotating
  # file plus a per-commit counter file. Far faster than N `git commit`s and
  # the same mechanism that will drive the 100k-commit perf-gate fixture
  # (search <16ms on 100k commits; graph at 250k).
  #
  # Full mode (--full) generates $FULL_COMMITS commits: expect a larger pack
  # and a run of a few minutes on spinning setups; quick mode is the default
  # stub (~$QUICK_COMMITS commits, seconds).
  {
    echo "reset refs/heads/main"
    local ts msg content f
    for ((i = 1; i <= n; i++)); do
      ts=$((1600000000 + i))
      msg="bulk commit #$i"
      content="counter: $i"
      case $((i % 3)) in
        0) f=src/a.txt ;;
        1) f=src/b.txt ;;
        *) f=src/c.txt ;;
      esac
      echo "commit refs/heads/main"
      echo "mark :$i"
      echo "author Fixture Bot <fixtures@mygitui.local> $ts +0000"
      echo "committer Fixture Bot <fixtures@mygitui.local> $ts +0000"
      # data length must count the trailing newline we write with the payload
      printf 'data %d\n' "$(( ${#msg} + 1 ))"
      printf '%s\n' "$msg"
      if ((i > 1)); then
        echo "from :$((i - 1))"
      fi
      printf 'M 100644 inline %s\n' "$f"
      printf 'data %d\n' "$(( ${#content} + 1 ))"
      printf '%s\n' "$content"
    done
  } | git -C "$dir" fast-import --quiet

  git -C "$dir" reset -q --hard   # materialize working tree from imported HEAD
  git -C "$dir" checkout -q main 2>/dev/null || true

  echo "commits-100k:   $(count_commits "$dir") commits ($MODE mode)"
}

# -------------------------------------------------------------- worktree ----
make_worktree() {
  local base repo wt
  base="$(fresh_dir worktree)"
  repo="$base/repo"
  wt="$base/feature-wt"

  git init -q -b main "$repo"
  cfg "$repo"
  printf 'main file\n' > "$repo/main.txt"
  git -C "$repo" add main.txt
  git -C "$repo" commit -q -m "main: first"

  git -C "$repo" branch feature
  git -C "$repo" worktree add -q "$wt" feature
  printf 'work on feature\n' > "$wt/feature.txt"
  git -C "$wt" add feature.txt
  git -C "$wt" commit -q -m "feature: from linked worktree"

  echo "worktree:       repo on main + linked worktree at feature-wt (feature @ $(git -C "$wt" rev-parse --short HEAD))"
}

echo "mygitui fixtures -> $FIXTURES"
mkdir -p "$FIXTURES"

make_basic
make_conflicted
make_submodules
make_commits_scale
make_worktree

echo "Done. Fixtures live in $FIXTURES (generated output — do not commit)."
