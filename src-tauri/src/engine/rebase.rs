//! Interactive rebase engine (lane D2): a custom sequencer built on
//! cherry-pick primitives.
//!
//! git2 cannot edit an upstream todo list, so the plan lives in a JSON state
//! file at `<gitdir>/mygitui/rebase.json` — the file's presence means a
//! rebase is active. Each step replays its commit's diff with a 3-way tree
//! merge (`Repository::merge_trees`, the primitive `git cherry-pick` itself
//! uses). `Repository::apply` was rejected deliberately: it fails hard on
//! conflict without materializing resolution state, while `merge_trees`
//! produces a conflicted index — exactly what D1's `conflicts` /
//! `conflict_resolve` consume.
//!
//! Steps run greedily: `rebase_start` applies steps until the plan is
//! exhausted, an `edit` pause, or a conflict. `rebase_continue` commits the
//! resolved index (after a conflict) or just unpauses (after `edit`), then
//! keeps running the loop. `rebase_abort` force-restores `orig_head`.
//!
//! State file schema (superset of the public `RebaseState`):
//! `{ plan, current, paused_for_edit, mid_merge, rewritten, onto, orig_head, orig_branch }`
//! - `mid_merge`: the current step's merge already sits in the index (and
//!   workdir, with markers when conflicted) — `rebase_continue` commits that
//!   index with the step's semantics instead of re-applying the step.
//! - `orig_branch`: full ref name (`refs/heads/...`) HEAD was attached to at
//!   start; `None` when detached. With `onto` the replay runs detached, so
//!   completion moves this ref to the final rewritten head and reattaches;
//!   abort restores it.
//! - `rewritten`: `(old sha, new sha)` pairs in plan order. `squash`/`fixup`
//!   fold into the previous commit, so they update the previous entry's new
//!   sha and record no entry of their own; `drop` records nothing.
//!
//! WIRING NOTE: `Libgit2Engine` may carry only one `impl GitEngineM3`
//! (Rust coherence) and lane D1 owns its placeholder today, so the four
//! rebase entry points land here as inherent `*_impl` methods — the same
//! pattern as M2 (`mutations.rs` + the forwarding impl in `libgit2.rs`).
//! Integration forwards `GitEngineM3::rebase_*` to these.

use serde::{Deserialize, Serialize};

use git2::build::CheckoutBuilder;
use git2::{Commit, MergeOptions, Oid, Repository, StatusOptions};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{RebaseState, RebaseStep};

/// Actions accepted in `RebaseStep::action`. `exec` runs `new_message` as
/// a shell command in the repo workdir (`git rebase -i`'s exec lines).
const ACTIONS: [&str; 7] = ["pick", "squash", "fixup", "drop", "edit", "reword", "exec"];

/// Persisted sequencer state (schema in the module docs).
#[derive(Debug, Serialize, Deserialize)]
struct RebaseFileState {
    plan: Vec<RebaseStep>,
    current: usize,
    paused_for_edit: bool,
    /// Current step's merge result sits in the index awaiting resolution /
    /// commit instead of a fresh apply.
    mid_merge: bool,
    /// The current step is a failed `exec`: `rebase_continue` re-runs it.
    #[serde(default)]
    paused_for_exec: bool,
    /// Diagnostics of the failed exec step (surface to the FE).
    #[serde(default)]
    exec_error: Option<String>,
    /// Committed rewrites so far: (original sha, rewritten sha).
    rewritten: Vec<(String, String)>,
    /// Resolved base commit sha when `onto` was given (informational).
    onto: Option<String>,
    /// HEAD sha when the rebase started — the abort target.
    orig_head: String,
    /// Full branch ref HEAD pointed at when the rebase started.
    orig_branch: Option<String>,
}

// ---------------------------------------------------------------------------
// state file
// ---------------------------------------------------------------------------

fn state_path(repo: &Repository) -> std::path::PathBuf {
    repo.path().join("mygitui").join("rebase.json")
}

fn load_state(repo: &Repository) -> EngineResult<Option<RebaseFileState>> {
    let path = state_path(repo);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| EngineError::Invalid(format!("corrupt mygitui/rebase.json: {e}")))
}

fn save_state(repo: &Repository, st: &RebaseFileState) -> EngineResult<()> {
    std::fs::create_dir_all(repo.path().join("mygitui"))?;
    let json = serde_json::to_string_pretty(st)
        .map_err(|e| EngineError::Invalid(format!("cannot serialize rebase state: {e}")))?;
    std::fs::write(state_path(repo), json)?;
    Ok(())
}

fn remove_state(repo: &Repository) {
    let _ = std::fs::remove_file(state_path(repo));
}

fn active_state(st: &RebaseFileState) -> RebaseState {
    RebaseState {
        active: true,
        plan: st.plan.clone(),
        current: st.current,
        paused_for_edit: st.paused_for_edit,
        paused_for_exec: st.paused_for_exec,
        exec_error: st.exec_error.clone(),
        rewritten: st.rewritten.clone(),
    }
}

// ---------------------------------------------------------------------------
// entry points (inherent impls; see the WIRING NOTE in the module docs)
// ---------------------------------------------------------------------------

// Rust only allows a single `impl GitEngineM3 for Libgit2Engine` (currently
// owned by merge.rs), so until the integration pass wires the forwarding
// there, the methods below are only called from `rebase_tests` and count as
// dead code in the non-test build (same arrangement as stash.rs).
#[allow(dead_code)]
impl Libgit2Engine {
    pub(crate) fn rebase_start_impl(
        &self,
        repo: &Repository,
        plan: &[RebaseStep],
        onto: Option<&str>,
    ) -> EngineResult<RebaseState> {
        if load_state(repo)?.is_some() {
            return Err(EngineError::Invalid(
                "a rebase is already in progress; continue or abort it first".into(),
            ));
        }
        ensure_clean_worktree(repo)?;
        let orig_head_commit =
            head_commit(repo).map_err(|e| EngineError::Invalid(format!("cannot rebase: {e}")))?;
        let orig_head = orig_head_commit.id().to_string();

        // Plan validation (RELAXED chain rules: any shas in any order, but
        // each must exist and actions must be usable where they stand).
        if plan.is_empty() {
            return Err(EngineError::Invalid("rebase plan is empty".into()));
        }
        for (i, step) in plan.iter().enumerate() {
            if !ACTIONS.contains(&step.action.as_str()) {
                return Err(EngineError::Invalid(format!(
                    "step {i}: unknown action `{}` (expected one of {})",
                    step.action,
                    ACTIONS.join("|")
                )));
            }
            if matches!(step.action.as_str(), "squash" | "fixup") && i == 0 {
                return Err(EngineError::Invalid(format!(
                    "step {i}: `{}` folds into a previous commit, which does not exist",
                    step.action
                )));
            }
            if step.action == "reword"
                && step
                    .new_message
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
            {
                return Err(EngineError::Invalid(format!(
                    "step {i}: `reword` requires a non-empty new_message"
                )));
            }
            if step.action == "exec"
                && step
                    .new_message
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
            {
                return Err(EngineError::Invalid(format!(
                    "step {i}: `exec` requires the command in new_message"
                )));
            }
            if step.action != "exec" {
                resolve_commitish(repo, &step.sha)?;
            }
        }

        let onto_sha = match onto {
            Some(spec) => Some(resolve_commitish(repo, spec)?.id().to_string()),
            None => None,
        };
        // Replay base: `onto` when given, else the parent of the first planned
        // commit (the no-op base for a plan covering the whole chain). The
        // branch is rebuilt from there, dropping whatever the plan omits.
        let base_commit = match &onto_sha {
            Some(sha) => repo.find_commit(Oid::from_str(sha)?)?,
            None => {
                let first = plan
                    .iter()
                    .find(|step| step.action != "exec")
                    .ok_or_else(|| {
                        EngineError::Invalid(
                            "plan contains only `exec` steps: provide `onto`".into(),
                        )
                    })?;
                let first = resolve_commitish(repo, &first.sha)?;
                match first.parent(0) {
                    Ok(parent) => parent,
                    Err(_) => {
                        return Err(EngineError::Invalid(
                            "plan starts at a root commit: provide `onto`".into(),
                        ))
                    }
                }
            }
        };
        // Full branch ref name before any detach; None = detached HEAD.
        let orig_branch = repo
            .find_reference("HEAD")
            .ok()
            .and_then(|head| head.symbolic_target().ok().flatten().map(str::to_owned));

        let mut st = RebaseFileState {
            plan: plan.to_vec(),
            current: 0,
            paused_for_edit: false,
            mid_merge: false,
            paused_for_exec: false,
            exec_error: None,
            rewritten: Vec::new(),
            onto: onto_sha.clone(),
            orig_head,
            orig_branch,
        };
        save_state(repo, &st)?;

        // Replay detached from the base; the branch ref moves to the rewritten
        // head on completion (finish_rebase) and abort restores it.
        if base_commit.id().to_string() != st.orig_head {
            repo.set_head_detached(base_commit.id())?;
            hard_checkout(repo, &base_commit)?;
        }

        run_rebase(repo, &mut st)
    }

    pub(crate) fn rebase_state_impl(&self, repo: &Repository) -> EngineResult<RebaseState> {
        Ok(match load_state(repo)? {
            Some(st) => active_state(&st),
            None => RebaseState {
                active: false,
                plan: Vec::new(),
                current: 0,
                paused_for_edit: false,
                paused_for_exec: false,
                exec_error: None,
                rewritten: Vec::new(),
            },
        })
    }

    pub(crate) fn rebase_continue_impl(&self, repo: &Repository) -> EngineResult<RebaseState> {
        let mut st = load_state(repo)?.ok_or_else(|| {
            EngineError::Invalid("no rebase in progress (mygitui/rebase.json missing)".into())
        })?;
        if st.paused_for_edit {
            // `edit` pause done: unpause, advance past the edit step, keep
            // going. Any user amend/commit made during the pause stays as-is.
            st.paused_for_edit = false;
            st.current += 1;
        }
        // A paused `exec` intentionally does NOT advance: continue re-runs it
        // (the user is expected to fix whatever failed first).
        run_rebase(repo, &mut st)
    }

    pub(crate) fn rebase_abort_impl(&self, repo: &Repository) -> EngineResult<()> {
        let st = load_state(repo)?.ok_or_else(|| {
            EngineError::Invalid("no rebase in progress (mygitui/rebase.json missing)".into())
        })?;
        let orig = repo.find_commit(Oid::from_str(&st.orig_head)?)?;
        if let Some(branch) = st.orig_branch.as_deref() {
            repo.reference(branch, orig.id(), true, "rebase: abort")?;
            repo.set_head(branch)?;
        } else {
            repo.set_head_detached(orig.id())?;
        }
        hard_checkout(repo, &orig)?;
        remove_state(repo);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// sequencer loop
// ---------------------------------------------------------------------------

fn run_rebase(repo: &Repository, st: &mut RebaseFileState) -> EngineResult<RebaseState> {
    loop {
        if st.current >= st.plan.len() {
            return finish_rebase(repo, st);
        }
        // Persist the position before mutating anything: a crash mid-apply
        // leaves the state pointing at the step that was being applied.
        save_state(repo, st)?;
        let step = st.plan[st.current].clone();

        // Resume after a conflict pause: the index holds the step's merged
        // content (user-resolved or still conflicted).
        let mut resume = false;
        if st.mid_merge {
            if repo.index()?.has_conflicts() {
                // Still blocked; the FE resolves via conflicts/conflict_resolve.
                return Ok(active_state(st));
            }
            st.mid_merge = false;
            resume = true;
        }
        // Resume after an exec pause: the step re-runs (git semantics — the
        // user fixed whatever failed, then continued).
        if st.paused_for_exec {
            st.paused_for_exec = false;
            save_state(repo, st)?;
        }

        if step.action == "drop" {
            st.current += 1;
            continue;
        }

        if step.action == "exec" {
            let command = step.new_message.as_deref().unwrap_or_default();
            match run_exec(repo, command) {
                Ok(()) => {
                    st.current += 1;
                    continue;
                }
                Err(err) => {
                    st.paused_for_exec = true;
                    st.exec_error = Some(err);
                    save_state(repo, st)?;
                    return Ok(active_state(st));
                }
            }
        }

        let commit = resolve_commitish(repo, &step.sha)?;
        if !resume && apply_step(repo, &commit)? {
            // Fresh apply hit a conflict: park here until the FE resolves.
            st.mid_merge = true;
            save_state(repo, st)?;
            return Ok(active_state(st));
        }

        commit_step(repo, &commit, &step, st)?;

        if step.action == "edit" {
            // Pause after the step's pick landed; the user amends the worktree
            // with other ops, then calls rebase_continue.
            st.paused_for_edit = true;
            save_state(repo, st)?;
            return Ok(active_state(st));
        }
        st.current += 1;
    }
}

/// Runs an `exec` step's command in the repo workdir. `sh -c` on Unix,
/// `cmd /c` on Windows (git's exec lines are shell commands by contract).
/// Returns the captured diagnostics on failure.
fn run_exec(repo: &Repository, command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("exec command is empty".into());
    }
    let workdir = repo
        .workdir()
        .ok_or_else(|| "bare repository has no workdir".to_owned())?;
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/c").arg(command);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    cmd.current_dir(workdir);
    cmd.env_clear();
    for (key, value) in std::env::vars() {
        if key.starts_with("GIT_")
            || [
                "PATH",
                "HOME",
                "SYSTEMROOT",
                "COMSPEC",
                "PATHEXT",
                "TMP",
                "TEMP",
                "APPDATA",
                "LOCALAPPDATA",
                "USERPROFILE",
                "LANG",
                "LC_ALL",
            ]
            .contains(&key.as_str())
        {
            cmd.env(key, value);
        }
    }
    let output = cmd
        .output()
        .map_err(|err| format!("failed to spawn exec command: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let mut detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&stderr);
    }
    Err(format!(
        "exec `{command}` failed (exit {}): {detail}",
        output.status.code().unwrap_or(-1)
    ))
}

fn finish_rebase(repo: &Repository, st: &mut RebaseFileState) -> EngineResult<RebaseState> {
    let final_head = head_commit(repo)?;
    if let Some(branch) = st.orig_branch.as_deref() {
        // Detached replay (onto given): land the branch on the rewritten head
        // and reattach. Attached replay already sits here; this is a no-op.
        repo.reference(branch, final_head.id(), true, "rebase: finish")?;
        repo.set_head(branch)?;
    }
    remove_state(repo);
    Ok(RebaseState {
        active: false,
        plan: st.plan.clone(),
        current: st.current,
        paused_for_edit: false,
        paused_for_exec: false,
        exec_error: None,
        rewritten: st.rewritten.clone(),
    })
}

/// Replay `commit`'s diff (parent tree → commit tree) onto the current HEAD
/// with a 3-way merge. Returns whether the merge is conflicted. Either way
/// the workdir and the repo's persistent index already hold the merge result
/// (`merge_trees` yields an in-memory index only — it cannot be written — so
/// it is mirrored into the on-disk index, conflict stages included).
fn apply_step(repo: &Repository, commit: &Commit<'_>) -> EngineResult<bool> {
    let head = head_commit(repo)?;
    let our_tree = head.tree()?;
    let their_tree = commit.tree()?;
    let base_tree = match commit.parent(0) {
        Ok(parent) => parent.tree()?,
        Err(_) => empty_tree(repo)?,
    };
    let opts = MergeOptions::new();
    let mut merged = repo.merge_trees(&base_tree, &our_tree, &their_tree, Some(&opts))?;
    let conflicted = merged.has_conflicts();

    let mut checkout = CheckoutBuilder::new();
    if conflicted {
        // Write the conflicted file with standard markers; the stage entries
        // land in the persistent index below for D1's conflicts()/resolve.
        checkout.allow_conflicts(true).update_index(false);
    } else {
        // Full materialization: between steps the worktree always matches
        // HEAD, so force is a deterministic sync to the merged tree. Files
        // the merge adds need recreate_missing under checkout_index.
        checkout.force().recreate_missing(true);
    }
    repo.checkout_index(Some(&mut merged), Some(&mut checkout))?;

    // Mirror the merged index into the persistent one so `rebase_continue`
    // can commit the exact merge result (plus user resolutions).
    let mut disk = repo.index()?;
    disk.clear()?;
    for entry in merged.iter() {
        disk.add(&entry)?;
    }
    disk.write()?;
    Ok(conflicted)
}

/// Commit the merged/resolved index with the step's semantics and record the
/// rewrite. HEAD (and its branch, when attached) moves to the new commit.
fn commit_step(
    repo: &Repository,
    orig: &Commit<'_>,
    step: &RebaseStep,
    st: &mut RebaseFileState,
) -> EngineResult<()> {
    let committer = repo.signature().map_err(|e| {
        EngineError::Invalid(format!(
            "committer identity not configured (set user.name / user.email): {e}"
        ))
    })?;
    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let head = head_commit(repo)?;

    match step.action.as_str() {
        "squash" | "fixup" => {
            let message = if step.action == "squash" {
                format!(
                    "{}\n\n{}",
                    head.message().unwrap_or_default().trim_end(),
                    orig.message().unwrap_or_default().trim()
                )
            } else {
                head.message().unwrap_or_default().to_owned()
            };
            // Amend HEAD: same parents and author, new tree/message.
            let parents: Vec<Commit<'_>> = head.parents().collect();
            let parent_refs: Vec<&Commit<'_>> = parents.iter().collect();
            let oid = repo.commit(
                None,
                &head.author(),
                &committer,
                &message,
                &tree,
                &parent_refs,
            )?;
            let ref_name = repo
                .find_reference("HEAD")?
                .symbolic_target()
                .ok()
                .flatten()
                .unwrap_or("HEAD")
                .to_owned();
            repo.reference(&ref_name, oid, true, "rebase: fold commit (squash/fixup)")?;
            // The folded commit replaces the previous step's rewrite target.
            let entry = st.rewritten.last_mut().ok_or_else(|| {
                EngineError::Invalid(format!(
                    "`{}` requires a previously rewritten commit",
                    step.action
                ))
            })?;
            entry.1 = oid.to_string();
        }
        "pick" | "edit" | "reword" => {
            let message = if step.action == "reword" {
                step.new_message.clone().unwrap_or_default()
            } else {
                orig.message().unwrap_or_default().to_owned()
            };
            let parents = [&head];
            let oid = repo.commit(
                Some("HEAD"),
                &orig.author(),
                &committer,
                &message,
                &tree,
                &parents,
            )?;
            st.rewritten.push((step.sha.clone(), oid.to_string()));
        }
        other => {
            return Err(EngineError::Invalid(format!(
                "unknown rebase action `{other}`"
            )))
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Rebase guard: no index/worktree changes; untracked files are fine.
fn ensure_clean_worktree(repo: &Repository) -> EngineResult<()> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    let dirty = statuses
        .iter()
        .any(|entry| entry.status() != git2::Status::WT_NEW);
    if dirty {
        return Err(EngineError::Invalid(
            "rebase requires a clean working tree; stash first".into(),
        ));
    }
    Ok(())
}

/// Resolve a commit-ish (sha, short sha, ref) to a commit.
fn resolve_commitish<'r>(repo: &'r Repository, spec: &str) -> EngineResult<Commit<'r>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?;
    obj.peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("`{spec}` is not a commit: {e}")))
}

fn head_commit(repo: &Repository) -> EngineResult<Commit<'_>> {
    Ok(repo.head()?.peel_to_commit()?)
}

/// The shared empty tree, used as merge base when replaying a root commit.
fn empty_tree(repo: &Repository) -> EngineResult<git2::Tree<'_>> {
    let oid = repo.treebuilder(None)?.write()?;
    Ok(repo.find_tree(oid)?)
}

/// Force the workdir+index to `commit`'s tree without touching any refs
/// (reset --hard minus the ref move).
fn hard_checkout(repo: &Repository, commit: &Commit<'_>) -> EngineResult<()> {
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_tree(commit.as_object(), Some(&mut checkout))?;
    // Normalize the index from the tree: clears any conflict stages checkout
    // may have left behind.
    let tree = commit.tree()?;
    let mut index = repo.index()?;
    index.read_tree(&tree)?;
    index.write()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// M10: autosquash planning + describe
// ---------------------------------------------------------------------------

impl Libgit2Engine {
    /// `git rebase --autosquash` planning: the `fixup!`/`squash!` commits in
    /// `base..HEAD` (oldest first) are reordered to sit right after their
    /// target commit (matched by subject prefix), with `fixup!` → `fixup`
    /// and `squash!` → `squash` actions; every other commit becomes `pick`.
    /// The plan feeds `rebase_start` unchanged.
    pub(crate) fn autosquash_plan_impl(
        &self,
        repo: &Repository,
        base: &str,
    ) -> EngineResult<Vec<RebaseStep>> {
        let base_commit = resolve_commitish(repo, base)?;
        let mut walk = repo.revwalk()?;
        walk.push(repo.head()?.peel_to_commit()?.id())?;
        walk.hide(base_commit.id())?;
        walk.set_sorting(git2::Sort::REVERSE | git2::Sort::TOPOLOGICAL)?;

        struct Entry {
            sha: String,
            subject: String,
            action: String,
            new_message: Option<String>,
        }
        let mut entries: Vec<Entry> = Vec::new();
        for oid in walk.filter_map(Result::ok) {
            let commit = repo.find_commit(oid)?;
            let message = commit.message().unwrap_or_default();
            let subject = message.lines().next().unwrap_or_default().trim();
            let (action, new_message) = if let Some(target) = subject.strip_prefix("fixup! ") {
                ("fixup".to_owned(), Some(target.to_owned()))
            } else if let Some(target) = subject.strip_prefix("squash! ") {
                ("squash".to_owned(), Some(target.to_owned()))
            } else {
                ("pick".to_owned(), None)
            };
            entries.push(Entry {
                sha: oid.to_string(),
                subject: subject.to_owned(),
                action,
                new_message,
            });
        }

        // Reorder: move each fixup/squash right after the first commit whose
        // subject matches its target (fallback: keep position — the run will
        // surface it as a fold onto whatever precedes it, like git's
        // unmatched-autosquash behavior of leaving it in place).
        let mut plan: Vec<Entry> = Vec::with_capacity(entries.len());
        while let Some(entry) = entries.first() {
            if entry.action == "pick" {
                plan.push(entries.remove(0));
                continue;
            }
            // Find the target among the already-placed picks.
            let target = entry.new_message.clone().unwrap_or_default();
            let position = plan
                .iter()
                .position(|placed| placed.action == "pick" && placed.subject == target)
                .map(|index| {
                    // After the target's own fold chain (fixups already moved).
                    let mut last = index;
                    while plan.get(last + 1).is_some_and(|next| next.action != "pick") {
                        last += 1;
                    }
                    last + 1
                });
            let entry = entries.remove(0);
            match position {
                Some(at) => plan.insert(at, entry),
                None => plan.push(entry),
            }
        }

        Ok(plan
            .into_iter()
            .map(|entry| RebaseStep {
                sha: entry.sha,
                action: entry.action,
                new_message: entry.new_message,
            })
            .collect())
    }

    /// `git describe --tags` for a commit-ish (annotated tags preferred by
    /// libgit2's default options); errors fall back to the short sha.
    pub(crate) fn describe_impl(&self, repo: &Repository, spec: &str) -> EngineResult<String> {
        let commit = resolve_commitish(repo, spec)?;
        let mut opts = git2::DescribeOptions::new();
        opts.describe_tags();
        let described = commit.as_object().describe(&opts);
        match described {
            Ok(describe) => Ok(describe.format(None)?),
            Err(_) => Ok(commit.id().to_string()[..7].to_owned()),
        }
    }
}
