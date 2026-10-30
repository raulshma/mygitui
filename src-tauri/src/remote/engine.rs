//! Remote git engine: runs `git` on the device over the SSH exec channel
//! and parses porcelain output into the SAME DTO types the local libgit2
//! engine produces (`engine::types`) — that is what lets every panel work
//! unchanged against a remote repo.
//!
//! Command construction rules (SECURITY):
//! - fixed literals + values from typed IPC structs only;
//! - every FE/device-supplied string passes through
//!   [`crate::remote::quoting::shell_quote`] before interpolation;
//! - no `sh -c` wrappers, no `eval`, no secrets on command lines.
//!
//! Sync surface: like libgit2, all methods are synchronous and run on
//! blocking threads (op queue workers); each bridges into the async russh
//! world with `tauri::async_runtime::block_on`.

use std::sync::Arc;

use serde_json::Value;

use super::parsers::{
    is_image_path, parse_bisect_log, parse_blame, parse_branches, parse_count_objects,
    parse_fsck_dangling, parse_log_records, parse_numstat, parse_rebase_todo, parse_ref_pairs,
    parse_reflog, parse_remote_branches, parse_stash_list, parse_status, parse_submodule_status,
    parse_tags, parse_transfer_line, parse_updated_refs, parse_worktrees, split_patch,
    StatusHeader, TransferProgress,
};
use super::quoting::{shell_quote, shell_quote_all};
use super::transport::{DeviceSession, ExecOutput, DEFAULT_EXEC_TIMEOUT, LONG_EXEC_TIMEOUT};
use crate::engine::git_engine::{FetchProgress, PushProgress};
use crate::engine::stats::{Contributor, DayCount};
use crate::engine::types::{
    BisectLogEntry, BisectMark, BisectState, BlameLine, BranchInfo, BranchTrashEntry,
    CheckpointInfo, CommitInfo, CommitOptions, CommitSignature, ConflictFile, ConflictResolution,
    DiffSide, FetchOptions, FileDiff, HookInfo, LfsStatus, LogFilter, MergeFavor, MergeOptions,
    MergeOutcome, MergeResult, NetStats, PreviewFile, PreviewInfo, PullOptions, PushOptions,
    RebaseState, RebaseStep, ReflogEntry, RemoteBranchInfo, RemoteInfo, RepoHealth, RepoStatus,
    ResetKind, SigningInfo, SparseInfo, StageRequest, StageTarget, StashInfo, SubmoduleInfo,
    TagInfo, WorktreeInfo,
};

/// Hook kinds surfaced by the hooks panel (mirrors `engine/mutations.rs`).
const KNOWN_HOOKS: [&str; 5] = [
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "pre-push",
    "post-commit",
];

/// Checkpoint ref namespace (mirrors `engine/checkpoints.rs`).
const CHECKPOINT_PREFIX: &str = "refs/mygitui/checkpoints/";
const CHECKPOINT_MARKER: &str = "mygitui-checkpoint";

/// Deleted-branch trash namespace (mirrors `engine/trash.rs`).
const TRASH_PREFIX: &str = "refs/mygitui/trash/";

/// `git archive` responses bigger than this fail (protects host memory;
/// effectively 256 MiB — the transport's exec output cap fires first).
const ARCHIVE_CAP: usize = 512 * 1024 * 1024;

/// The engine for one open remote repo: a live device session + the repo's
/// absolute POSIX path on that device.
pub struct RemoteGitEngine {
    pub session: Arc<DeviceSession>,
    /// Absolute POSIX workdir path on the device.
    pub root: String,
}

impl RemoteGitEngine {
    pub fn new(session: Arc<DeviceSession>, root: String) -> Self {
        Self { session, root }
    }

    /// Discover the repo at/under `path` on the device: resolve the workdir
    /// toplevel, reject bare repos (same rules as local open), return
    /// (root, git_dir). Class method — usable before the engine exists.
    pub fn discover(session: &Arc<DeviceSession>, path: &str) -> Result<(String, String), String> {
        let quoted = shell_quote(path);
        let cmd = format!(
            "cd {q} 2>/dev/null || exit 64; \
             top=$(git --no-optional-locks rev-parse --show-toplevel) || exit 65; \
             bare=$(git --no-optional-locks rev-parse --is-bare-repository); \
             gdir=$(git --no-optional-locks rev-parse --absolute-git-dir); \
             printf '%s\\n%s\\n%s\\n' \"$top\" \"$bare\" \"$gdir\"",
            q = quoted
        );
        let out = tauri::async_runtime::block_on(session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?;
        if out.exit_code == 64 {
            return Err(format!("path not found on device: {path}"));
        }
        if out.exit_code != 0 {
            return Err(format!("not a git repository: {path}"));
        }
        let text = out.stdout_str();
        let mut lines = text.lines();
        let top = lines.next().unwrap_or("").trim().to_string();
        let bare = lines.next().unwrap_or("false").trim() == "true";
        let gdir = lines.next().unwrap_or("").trim().to_string();
        if bare {
            return Err(format!("bare repositories are not supported: {gdir}"));
        }
        if top.is_empty() || gdir.is_empty() {
            return Err(format!("repository discovery failed for {path}"));
        }
        Ok((top, gdir))
    }

    // ---------- exec helpers (sync bridge) ----------

    fn block<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(fut)
    }

    /// Build a `git` command line rooted at the repo.
    pub fn command(&self, args: &str) -> String {
        git_command(&self.root, args)
    }

    fn git_raw(&self, args: &str, timeout: std::time::Duration) -> Result<ExecOutput, String> {
        let cmd = self.command(args);
        self.block(self.session.exec(&cmd, timeout))
    }

    /// Run git; any non-zero exit is an `Err` carrying the stderr tail.
    fn git_checked(&self, args: &str, what: &str) -> Result<ExecOutput, String> {
        self.git_raw(args, DEFAULT_EXEC_TIMEOUT)?.checked(what)
    }

    /// Run git; returns stdout on success.
    fn git_out(&self, args: &str) -> Result<String, String> {
        Ok(self.git_checked(args, "git")?.stdout_str())
    }

    fn git_stdin(&self, args: &str, stdin: Vec<u8>, what: &str) -> Result<ExecOutput, String> {
        let cmd = self.command(args);
        self.block(
            self.session
                .exec_with_stdin(&cmd, Some(stdin), DEFAULT_EXEC_TIMEOUT),
        )?
        .checked(what)
    }

    /// `git archive`: binary stdout, long timeout, hard cap.
    fn git_archive_bytes(&self, args: &str) -> Result<Vec<u8>, String> {
        let out = self
            .git_raw(args, LONG_EXEC_TIMEOUT)?
            .checked("git archive")?;
        if out.stdout.len() > ARCHIVE_CAP {
            return Err("archive exceeds 512 MiB".to_string());
        }
        Ok(out.stdout)
    }

    /// Probe a path's existence on the device (`[ -e ]`).
    pub fn path_exists(&self, abs_path: &str) -> bool {
        let cmd = format!("[ -e {} ]", shell_quote(abs_path));
        self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))
            .map(|o| o.exit_code == 0)
            .unwrap_or(false)
    }

    fn head_sha(&self) -> Result<Option<String>, String> {
        let out = self.git_raw("rev-parse -q --verify HEAD", DEFAULT_EXEC_TIMEOUT)?;
        Ok(match out.exit_code {
            0 => Some(out.stdout_str().trim().to_string()),
            _ => None,
        })
    }

    fn current_branch(&self) -> Result<Option<String>, String> {
        let out = self.git_raw("rev-parse --abbrev-ref HEAD", DEFAULT_EXEC_TIMEOUT)?;
        let name = out.stdout_str().trim().to_string();
        Ok(match out.exit_code {
            0 if name != "HEAD" => Some(name),
            _ => None,
        })
    }

    // ---------- status ----------

    /// Sequencer/merge/bisect state probe: one exec, output lines
    /// `MERGE_HEAD` / `CHERRY_PICK_HEAD` / `REVERT_HEAD` / `REBASING` /
    /// `BISECTING`.
    fn state_probe(&self) -> Result<Vec<String>, String> {
        let cmd = format!(
            "d=$(git -C {} rev-parse --absolute-git-dir 2>/dev/null) || exit 0; \
             for n in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD; do \
               [ -f \"$d/$n\" ] && echo \"$n\"; done; \
             {{ [ -d \"$d/rebase-merge\" ] || [ -d \"$d/rebase-apply\" ]; }} && echo REBASING; \
             [ -f \"$d/BISECT_LOG\" ] && echo BISECTING; \
             exit 0",
            shell_quote(&self.root)
        );
        let out = self
            .block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("state probe")?;
        Ok(out
            .stdout_str()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    pub fn status(&self) -> Result<RepoStatus, String> {
        let raw = self.git_out("status --porcelain=v2 -z --branch")?;
        let (header, entries) = parse_status(&raw);
        let states = self.state_probe()?;
        Ok(RepoStatus {
            branch: header_branch(&header),
            head: header_oid(&header),
            detached: header.head == "(detached)",
            ahead: header.ahead,
            behind: header.behind,
            merging: states.iter().any(|s| s == "MERGE_HEAD"),
            rebasing: states.iter().any(|s| s == "REBASING"),
            sequencer: states
                .iter()
                .any(|s| s == "CHERRY_PICK_HEAD" || s == "REVERT_HEAD"),
            entries,
        })
    }

    /// Cheap change-detection snapshot for the poller: HEAD sha + a hash of
    /// the porcelain status (string; the caller compares).
    pub fn change_fingerprint(&self) -> Result<Option<(String, String)>, String> {
        let status = self.git_out("status --porcelain=v2 --branch")?;
        let head = self.head_sha()?;
        Ok(Some((head.unwrap_or_default(), status)))
    }

    // ---------- refs / log / blame / diff ----------

    pub fn refs(&self) -> Result<Vec<(String, String)>, String> {
        let mut out =
            parse_ref_pairs(&self.git_out("for-each-ref --format=%(refname)%09%(objectname)")?);
        if let Some(head) = self.head_sha()? {
            out.push(("HEAD".to_string(), head));
        }
        Ok(out)
    }

    pub fn log(
        &self,
        filter: &LogFilter,
        limit: usize,
        after: Option<&str>,
    ) -> Result<(Vec<CommitInfo>, Option<String>), String> {
        let args = log_args(filter, limit + 1, after);
        let raw = self.git_out(&args)?;
        let mut commits = parse_log_records(&raw);
        let next_cursor = if commits.len() > limit {
            commits.truncate(limit);
            commits.last().map(|c| c.sha.clone())
        } else {
            None
        };
        Ok((commits, next_cursor))
    }

    pub fn blame(&self, path: &str, from: Option<&str>) -> Result<Vec<BlameLine>, String> {
        let range = match from {
            Some(r) => format!("{} --", shell_quote(r)),
            None => "--".to_string(),
        };
        let raw = self
            .git_checked(
                &format!("blame --porcelain {range} {}", shell_quote(path)),
                "blame",
            )?
            .stdout_str();
        let mut lines = parse_blame(&raw);
        lines.sort_by_key(|l| l.line_no);
        Ok(lines)
    }

    pub fn diff(
        &self,
        old: &DiffSide,
        new: &DiffSide,
        paths: Option<&[String]>,
    ) -> Result<Vec<FileDiff>, String> {
        let range = diff_range(old, new)?;
        let pathspec = path_spec(paths);
        // 1) stats via numstat (authoritative counts + renames + binary).
        let stats_raw = self.git_out(&format!("diff --numstat -z {range}{pathspec}"))?;
        let rows = parse_numstat(&stats_raw);
        // 2) hunks via unified patch.
        let patch_raw = self.git_out(&format!(
            "diff --no-color --no-ext-diff -U3 --no-prefix {range}{pathspec}"
        ))?;
        // --no-prefix makes a/ b/ plain paths (simpler header parsing).
        let patches = split_patch(&patch_raw);
        let mut out = Vec::new();
        for row in &rows {
            let patch = patches.iter().find(|p| p.path == row.path);
            let mut hunks = Vec::new();
            if let Some(patch) = patch {
                for raw_hunk in &patch.hunks {
                    if let Some(hunk) = parse_hunk(raw_hunk) {
                        hunks.push(hunk);
                    }
                }
            }
            out.push(FileDiff {
                path: row.path.clone(),
                old_path: row.old_path.clone(),
                binary: row.binary,
                is_image: is_image_path(&row.path),
                additions: row.additions,
                deletions: row.deletions,
                hunks,
            });
        }
        Ok(out)
    }

    // ---------- staging / discard / commit ----------

    pub fn stage(&self, request: &StageRequest) -> Result<(), String> {
        // Whole-file targets go through add/reset; hunk/line targets via
        // patch surgery on the index (`git apply --cached`).
        let mut add_paths = Vec::new();
        let mut reset_paths = Vec::new();
        let mut hunks: Vec<&StageTarget> = Vec::new();
        for target in &request.targets {
            match target {
                StageTarget::File(path) => {
                    if request.unstage {
                        reset_paths.push(path.clone());
                    } else {
                        add_paths.push(path.clone());
                    }
                }
                StageTarget::Hunk { .. } | StageTarget::Lines { .. } => hunks.push(target),
            }
        }
        if !add_paths.is_empty() {
            self.git_checked(&format!("add -q --{}", quoted_paths(&add_paths)), "stage")?;
        }
        if !reset_paths.is_empty() {
            self.git_checked(
                &format!("reset -q HEAD --{}", quoted_paths(&reset_paths)),
                "unstage",
            )?;
        }
        for group in group_hunks_by_path(&hunks) {
            self.apply_hunk_patch(&group.0, &group.1, request.unstage, true)?;
        }
        Ok(())
    }

    /// Stage/discard selected hunks (and line ranges) by building a reduced
    /// patch from the worktree-vs-index diff and feeding `git apply`.
    fn apply_hunk_patch(
        &self,
        path: &str,
        indices: &[HunkSelection],
        reverse: bool,
        cached: bool,
    ) -> Result<(), String> {
        // indices: (hunk index within the file's unstaged diff, optional
        // new-side line ranges).
        let patch_raw = self.git_out(&format!(
            "diff --no-color --no-ext-diff -U3 --no-prefix -- {}",
            shell_quote(path)
        ))?;
        let files = split_patch(&patch_raw);
        let file = files
            .iter()
            .find(|f| f.path == path)
            .ok_or_else(|| format!("no unstaged diff for {path}"))?;
        let mut body = file.header.clone();
        for (idx, ranges) in indices {
            let raw = file
                .hunks
                .get(*idx as usize)
                .ok_or_else(|| format!("hunk {idx} not found in diff for {path}"))?;
            match ranges {
                None => body.push_str(raw),
                Some(ranges) => body.push_str(&filter_hunk_lines(raw, ranges)),
            }
        }
        if body.trim().is_empty() {
            return Ok(());
        }
        let mut args = String::from("apply --whitespace=nofix --unidiff-zero");
        if reverse {
            args.push_str(" --reverse");
        }
        if cached {
            args.push_str(" --cached");
        }
        self.git_stdin(&args, body.into_bytes(), "apply")?;
        Ok(())
    }

    pub fn discard(&self, targets: &[StageTarget]) -> Result<(), String> {
        let mut paths = Vec::new();
        let mut hunks: Vec<&StageTarget> = Vec::new();
        for target in targets {
            match target {
                StageTarget::File(path) => paths.push(path.clone()),
                StageTarget::Hunk { .. } | StageTarget::Lines { .. } => hunks.push(target),
            }
        }
        if !paths.is_empty() {
            self.git_checked(
                &format!("checkout -q --{}", quoted_paths(&paths)),
                "discard",
            )?;
        }
        for group in group_hunks_by_path(&hunks) {
            self.apply_hunk_patch(&group.0, &group.1, true, false)?;
        }
        Ok(())
    }

    pub fn commit(&self, opts: &CommitOptions) -> Result<String, String> {
        let mut args = String::from("commit -q -F -");
        if opts.amend {
            args.push_str(" --amend");
        }
        if opts.no_verify {
            args.push_str(" --no-verify");
        }
        if opts.allow_empty {
            args.push_str(" --allow-empty");
        }
        if let Some(author) = &opts.author {
            args.push_str(&format!(
                " --author={} --date=@{}",
                shell_quote(&format!("{} <{}>", author.name, author.email)),
                author.time.max(0)
            ));
        }
        self.git_stdin(&args, opts.message.clone().into_bytes(), "commit")?;
        self.head_sha()?
            .ok_or_else(|| "commit succeeded but HEAD is missing".to_string())
    }

    pub fn signing_info(&self) -> Result<SigningInfo, String> {
        let config = |key: &str| -> Option<String> {
            self.git_raw(
                &format!("config --get {}", shell_quote(key)),
                DEFAULT_EXEC_TIMEOUT,
            )
            .ok()
            .filter(|o| o.exit_code == 0)
            .map(|o| o.stdout_str().trim().to_string())
            .filter(|v| !v.is_empty())
        };
        let format = config("gpg.format").unwrap_or_else(|| "openpgp".to_string());
        let active = config("commit.gpgsign").as_deref() == Some("true");
        Ok(SigningInfo {
            active,
            key_id: config("user.signingkey"),
            format,
        })
    }

    pub fn hooks(&self) -> Result<Vec<HookInfo>, String> {
        // One shell pass: resolve the hooks dir (core.hooksPath aware), then
        // report presence + executability per known hook kind.
        let kinds = KNOWN_HOOKS
            .iter()
            .map(|k| shell_quote(k))
            .collect::<Vec<_>>()
            .join(" ");
        let cmd = format!(
            "d=$(git -C {} rev-parse --git-path hooks) || exit 1; \
             for k in {kinds}; do \
               if [ -e \"$d/$k\" ]; then \
                 if [ -x \"$d/$k\" ]; then echo \"$k 1 1\"; else echo \"$k 1 0\"; fi; \
               else echo \"$k 0 0\"; fi; \
             done",
            shell_quote(&self.root)
        );
        let out = self
            .block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("hooks probe")?;
        Ok(out
            .stdout_str()
            .lines()
            .filter_map(|line| {
                let mut f = line.split(' ');
                Some(HookInfo {
                    kind: f.next()?.to_string(),
                    present: f.next() == Some("1"),
                    executable: f.next() == Some("1"),
                })
            })
            .collect())
    }

    // ---------- branches / tags / remotes ----------

    pub fn branches(&self) -> Result<Vec<BranchInfo>, String> {
        Ok(parse_branches(&self.git_out(
            "for-each-ref refs/heads --format=%(refname:short)%09%(objectname)%09%(HEAD)%09%(upstream:short)%09%(upstream:track)",
        )?))
    }

    pub fn branch_create(
        &self,
        name: &str,
        from: Option<&str>,
        checkout: bool,
    ) -> Result<(), String> {
        let from_arg = from
            .map(|f| format!(" {}", shell_quote(f)))
            .unwrap_or_default();
        self.git_checked(
            &format!("branch {}{}", shell_quote(name), from_arg),
            "branch create",
        )?;
        if checkout {
            self.git_checked(&format!("switch -q {}", shell_quote(name)), "switch")?;
        }
        Ok(())
    }

    pub fn branch_switch(&self, name: &str, force: bool) -> Result<(), String> {
        let flag = if force { " -f" } else { "" };
        self.git_checked(&format!("switch{flag} -q {}", shell_quote(name)), "switch")?;
        Ok(())
    }

    pub fn branch_is_merged(&self, name: &str, into: &str) -> Result<bool, String> {
        let out = self.git_raw(
            &format!(
                "merge-base --is-ancestor {} {}",
                shell_quote(name),
                shell_quote(into)
            ),
            DEFAULT_EXEC_TIMEOUT,
        )?;
        match out.exit_code {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(format!("merge-base failed: {}", out.stderr_str().trim())),
        }
    }

    pub fn branch_delete(&self, name: &str, force: bool) -> Result<(), String> {
        // Safety net: snapshot under the trash ref before deleting (same
        // model as engine/trash.rs).
        let sha = self
            .git_out(&format!("rev-parse --verify {}", shell_quote(name)))?
            .trim()
            .to_string();
        let trash_id = format!("{}-{}", sanitize_ref_component(name), unix_millis());
        self.git_checked(
            &format!(
                "update-ref {} {}",
                shell_quote(&format!("{TRASH_PREFIX}{trash_id}")),
                shell_quote(&sha)
            ),
            "trash branch",
        )?;
        let flag = if force { "-D" } else { "-d" };
        self.git_checked(
            &format!("branch {flag} {}", shell_quote(name)),
            "branch delete",
        )?;
        Ok(())
    }

    pub fn branch_rename(&self, old: &str, new: &str) -> Result<(), String> {
        self.git_checked(
            &format!("branch -m {} {}", shell_quote(old), shell_quote(new)),
            "branch rename",
        )?;
        Ok(())
    }

    pub fn branch_checkout_remote(
        &self,
        remote: &str,
        name: &str,
        new_local: Option<&str>,
    ) -> Result<String, String> {
        let local = new_local.unwrap_or(name);
        self.git_checked(
            &format!(
                "switch -q -c {} --track {}/{}",
                shell_quote(local),
                shell_quote(remote),
                shell_quote(name)
            ),
            "checkout remote branch",
        )?;
        Ok(local.to_string())
    }

    pub fn remote_branches(&self) -> Result<Vec<RemoteBranchInfo>, String> {
        let mut out: Vec<RemoteBranchInfo> = parse_remote_branches(
            &self.git_out("for-each-ref refs/remotes --format=%(refname)%09%(objectname)")?,
        )
        .into_iter()
        .map(|(remote, name, sha)| RemoteBranchInfo {
            remote,
            name,
            sha,
            tracked_by: None,
        })
        .collect();
        // Map local branches' upstreams onto the tracking info.
        for branch in self.branches()? {
            if let Some(upstream) = branch.upstream {
                let upstream_full = format!("refs/remotes/{upstream}");
                for info in out.iter_mut() {
                    let full = format!("refs/remotes/{}/{}", info.remote, info.name);
                    if full == upstream_full {
                        info.tracked_by = Some(branch.name.clone());
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn tag_create(
        &self,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> Result<(), String> {
        let target_arg = target
            .map(|t| format!(" {}", shell_quote(t)))
            .unwrap_or_default();
        let args = match message {
            Some(m) => format!(
                "tag -a {} -m{}{}",
                shell_quote(name),
                shell_quote(m),
                target_arg
            ),
            None => format!("tag {}{}", shell_quote(name), target_arg),
        };
        self.git_checked(&args, "tag create")?;
        Ok(())
    }

    pub fn tag_create_signed(
        &self,
        name: &str,
        target: Option<&str>,
        message: &str,
    ) -> Result<(), String> {
        let target_arg = target
            .map(|t| format!(" {}", shell_quote(t)))
            .unwrap_or_default();
        self.git_checked(
            &format!(
                "tag -s {} -m{}{}",
                shell_quote(name),
                shell_quote(message),
                target_arg
            ),
            "signed tag create",
        )?;
        Ok(())
    }

    pub fn tag_delete(&self, name: &str) -> Result<(), String> {
        self.git_checked(&format!("tag -d {}", shell_quote(name)), "tag delete")?;
        Ok(())
    }

    pub fn tag_list(&self) -> Result<Vec<TagInfo>, String> {
        Ok(parse_tags(&self.git_out(
            "for-each-ref refs/tags --format=%(refname:short)%09%(objectname)%09%(objecttype)%09%(*objectname)%09%(taggername)\u{1f}%(taggeremail)\u{1f}%(taggerdate:unix)\u{1f}%(taggerdate:iso-strict-local)\u{1f}%(contents:subject)",
        )?))
    }

    pub fn remotes(&self) -> Result<Vec<RemoteInfo>, String> {
        let raw = self.git_out("remote -v")?;
        let mut out: Vec<RemoteInfo> = Vec::new();
        for line in raw.lines() {
            let mut f = line.split('\t');
            let name = f.next().unwrap_or("").to_string();
            let Some(url_part) = f.next() else { continue };
            let (url, kind) = match url_part.split_once(' ') {
                Some((u, k)) => (u, k),
                None => (url_part, ""),
            };
            match out.iter_mut().find(|r| r.name == name) {
                Some(entry) => {
                    if kind == "(push)" {
                        entry.push_url = Some(url.to_string());
                    }
                }
                None => out.push(RemoteInfo {
                    name,
                    url: url.to_string(),
                    push_url: if kind == "(push)" {
                        Some(url.to_string())
                    } else {
                        None
                    },
                }),
            }
        }
        Ok(out)
    }

    pub fn remote_add(&self, name: &str, url: &str) -> Result<(), String> {
        self.git_checked(
            &format!("remote add {} {}", shell_quote(name), shell_quote(url)),
            "remote add",
        )?;
        Ok(())
    }

    pub fn remote_remove(&self, name: &str) -> Result<(), String> {
        self.git_checked(&format!("remote rm {}", shell_quote(name)), "remote remove")?;
        Ok(())
    }

    pub fn remote_set_url(&self, name: &str, url: &str, push: bool) -> Result<(), String> {
        let flag = if push { " --push" } else { "" };
        self.git_checked(
            &format!(
                "remote set-url{flag} {} {}",
                shell_quote(name),
                shell_quote(url)
            ),
            "remote set-url",
        )?;
        Ok(())
    }

    // ---------- network ----------

    pub fn fetch(
        &self,
        opts: &FetchOptions,
        progress: &mut (dyn FnMut(FetchProgress) + Send),
    ) -> Result<NetStats, String> {
        let mut args = String::from("fetch --progress");
        if opts.prune {
            args.push_str(" --prune");
        }
        if let Some(depth) = opts.depth {
            args.push_str(&format!(" --depth={depth}"));
        }
        args.push_str(&format!(" {}", shell_quote(&opts.remote)));
        for r in &opts.refs {
            args.push(' ');
            args.push_str(&shell_quote(r));
        }
        let cmd = self.command(&args);
        let (output, transfer) = self.stream_with_progress(&cmd, progress)?;
        let refs = parse_updated_refs(&output.stderr_str());
        Ok(NetStats {
            received_bytes: transfer.bytes,
            objects: transfer.objects_total,
            updated_refs: refs,
        })
    }

    pub fn pull(
        &self,
        opts: &PullOptions,
        progress: &mut (dyn FnMut(FetchProgress) + Send),
    ) -> Result<NetStats, String> {
        let mut args = String::from("pull --progress");
        if opts.rebase {
            args.push_str(" --rebase");
        }
        if opts.ff_only {
            args.push_str(" --ff-only");
        }
        args.push_str(&format!(" {}", shell_quote(&opts.remote)));
        if !opts.branch.is_empty() {
            args.push(' ');
            args.push_str(&shell_quote(&opts.branch));
        }
        let cmd = self.command(&args);
        let (output, transfer) = self.stream_with_progress(&cmd, progress)?;
        Ok(NetStats {
            received_bytes: transfer.bytes,
            objects: transfer.objects_total,
            updated_refs: parse_updated_refs(&output.stderr_str()),
        })
    }

    pub fn push(
        &self,
        opts: &PushOptions,
        progress: &mut (dyn FnMut(PushProgress) + Send),
    ) -> Result<NetStats, String> {
        let mut args = String::from("push --progress");
        if opts.force {
            args.push_str(" --force");
        } else if opts.force_with_lease {
            args.push_str(" --force-with-lease");
        }
        if opts.delete {
            args.push_str(" --delete");
        }
        if opts.tags {
            args.push_str(" --tags");
        }
        args.push_str(&format!(" {}", shell_quote(&opts.remote)));
        if !opts.refs.is_empty() {
            for r in &opts.refs {
                args.push(' ');
                args.push_str(&shell_quote(r));
            }
        } else {
            let branch = if opts.branch.is_empty() {
                self.current_branch()?
                    .ok_or_else(|| "no current branch to push".to_string())?
            } else {
                opts.branch.clone()
            };
            args.push(' ');
            args.push_str(&shell_quote(&branch));
            if opts.set_upstream {
                args.push_str(" --set-upstream");
            }
        }
        let cmd = self.command(&args);
        let mut transfer = TransferProgress::default();
        let output = self.block(self.session.exec_stream(
            &cmd,
            LONG_EXEC_TIMEOUT,
            |_is_stderr, line| {
                if let Some(p) = parse_transfer_line(line) {
                    transfer = p;
                    progress(PushProgress {
                        current: p.objects_received,
                        total: p.objects_total,
                        bytes: p.bytes,
                        message: "pushing".to_string(),
                    });
                }
                true
            },
        ))?;
        Ok(NetStats {
            received_bytes: transfer.bytes,
            objects: transfer.objects_total,
            updated_refs: parse_updated_refs(&output.stderr_str()),
        })
    }

    /// Shared fetch/pull streaming loop with live progress emission.
    /// Returns the buffered output plus the LAST parsed transfer-progress
    /// line: git's progress output carries the cumulative totals, and the
    /// CLI prints no end-of-transfer summary, so the final line is what the
    /// `NetStats` report.
    fn stream_with_progress(
        &self,
        cmd: &str,
        progress: &mut (dyn FnMut(FetchProgress) + Send),
    ) -> Result<(ExecOutput, TransferProgress), String> {
        let mut last = TransferProgress::default();
        let output = self.block(self.session.exec_stream(
            cmd,
            LONG_EXEC_TIMEOUT,
            |_is_stderr, line| {
                if let Some(p) = parse_transfer_line(line) {
                    last = p;
                    progress(FetchProgress {
                        bytes: p.bytes,
                        objects_total: p.objects_total,
                        objects_received: p.objects_received,
                        done: p.done,
                    });
                }
                true
            },
        ))?;
        Ok((output, last))
    }

    // ---------- stash ----------

    pub fn stash_list(&self) -> Result<Vec<StashInfo>, String> {
        let raw = self.git_out("stash list --format=%gd%x09%H%x09%an%x09%ae%x09%at%x09%gs")?;
        Ok(parse_stash_list(&raw)
            .into_iter()
            .map(|(index, sha, message, author)| StashInfo {
                index,
                sha,
                message,
                author,
            })
            .collect())
    }

    pub fn stash_push(
        &self,
        message: Option<&str>,
        keep_index: bool,
        include_untracked: bool,
    ) -> Result<(), String> {
        let mut args = String::from("stash push -q");
        if let Some(m) = message {
            args.push_str(&format!(" -m{}", shell_quote(m)));
        }
        if keep_index {
            args.push_str(" --keep-index");
        }
        if include_untracked {
            args.push_str(" --include-untracked");
        }
        self.git_checked(&args, "stash push")?;
        Ok(())
    }

    pub fn stash_apply(&self, index: u32) -> Result<(), String> {
        self.git_checked(&format!("stash apply stash@{{{index}}}"), "stash apply")?;
        Ok(())
    }

    pub fn stash_drop(&self, index: u32) -> Result<(), String> {
        self.git_checked(&format!("stash drop stash@{{{index}}}"), "stash drop")?;
        Ok(())
    }

    pub fn stash_branch(&self, name: &str, index: u32) -> Result<(), String> {
        self.git_checked(
            &format!("stash branch {} 'stash@{{{index}}}'", shell_quote(name)),
            "stash branch",
        )?;
        Ok(())
    }

    // ---------- merge / conflicts / cherry-pick / revert / reset ----------

    pub fn merge_branch(&self, ref_name: &str, opts: &MergeOptions) -> Result<MergeResult, String> {
        let before = self.head_sha()?;
        let mut args = String::from("merge -q");
        if opts.no_ff {
            args.push_str(" --no-ff");
        }
        if opts.squash {
            args.push_str(" --squash");
        }
        if opts.no_commit {
            args.push_str(" --no-commit");
        }
        args.push_str(match opts.favor {
            MergeFavor::Ours => " -X ours",
            MergeFavor::Theirs => " -X theirs",
            MergeFavor::None => "",
        });
        args.push(' ');
        args.push_str(&shell_quote(ref_name));
        // Up-to-date probe BEFORE merging: `merge-base --is-ancestor` is
        // plumbing, so the answer is locale-independent (git's human
        // "Already up to date" message is localized on the device and must
        // never be parsed).
        let up_to_date = self
            .git_raw(
                &format!("merge-base --is-ancestor {} HEAD", shell_quote(ref_name)),
                DEFAULT_EXEC_TIMEOUT,
            )
            .map(|o| o.exit_code == 0)
            .unwrap_or(false);
        if up_to_date {
            return Ok(MergeResult {
                outcome: MergeOutcome::UpToDate,
                conflicts: Vec::new(),
                new_head: before.clone(),
            });
        }
        self.git_raw(&args, DEFAULT_EXEC_TIMEOUT)?;
        let conflicts = self.conflict_files()?;
        if !conflicts.is_empty() {
            return Ok(MergeResult {
                outcome: MergeOutcome::Conflicted,
                conflicts,
                new_head: self.head_sha()?,
            });
        }
        let after = self.head_sha()?;
        if opts.squash {
            return Ok(MergeResult {
                outcome: MergeOutcome::Squashed,
                conflicts,
                new_head: after.clone(),
            });
        }
        if opts.no_commit {
            return Ok(MergeResult {
                outcome: MergeOutcome::NoCommit,
                conflicts,
                new_head: after.clone(),
            });
        }
        let outcome = match (&before, &after) {
            (Some(b), Some(a)) if b == a => MergeOutcome::UpToDate,
            _ => {
                // New HEAD: two parents = real merge commit, else fast-forward.
                let parents = self
                    .git_out(&format!(
                        "rev-list --parents -n1 {}",
                        shell_quote(after.as_deref().unwrap_or("HEAD"))
                    ))?
                    .split_whitespace()
                    .count();
                if parents > 2 {
                    MergeOutcome::Merged
                } else {
                    MergeOutcome::FastForward
                }
            }
        };
        Ok(MergeResult {
            outcome,
            conflicts,
            new_head: after,
        })
    }

    pub fn merge_abort(&self) -> Result<(), String> {
        self.git_checked("merge --abort", "merge abort")?;
        Ok(())
    }

    pub fn sequencer_abort(&self) -> Result<(), String> {
        // cherry-pick --abort also covers revert sequences.
        self.git_checked("cherry-pick --abort", "sequencer abort")?;
        Ok(())
    }

    /// Conflicted paths with per-stage blob availability + the active
    /// sequencer kind.
    fn conflict_files(&self) -> Result<Vec<ConflictFile>, String> {
        let raw = self.git_out("ls-files -u -z")?;
        let mut out: Vec<ConflictFile> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for field in raw.split('\0').filter(|f| !f.is_empty()) {
            // <mode> <sha> <stage>\t<path>
            let Some((meta, path)) = field.split_once('\t') else {
                continue;
            };
            let stage: u32 = meta
                .split_whitespace()
                .nth(2)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            match seen.iter().position(|p| p == path) {
                Some(idx) => {
                    let entry = &mut out[idx];
                    match stage {
                        1 => entry.has_base = true,
                        2 => entry.has_ours = true,
                        3 => entry.has_theirs = true,
                        _ => {}
                    }
                }
                None => {
                    seen.push(path.to_string());
                    out.push(ConflictFile {
                        path: path.to_string(),
                        has_base: stage == 1,
                        has_ours: stage == 2,
                        has_theirs: stage == 3,
                        source: self.sequencer_source()?,
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn conflicts(&self) -> Result<Vec<ConflictFile>, String> {
        self.conflict_files()
    }

    fn sequencer_source(&self) -> Result<String, String> {
        let states = self.state_probe()?;
        Ok(if states.iter().any(|s| s == "MERGE_HEAD") {
            "merge".to_string()
        } else if states.iter().any(|s| s == "CHERRY_PICK_HEAD") {
            "cherry-pick".to_string()
        } else if states.iter().any(|s| s == "REVERT_HEAD") {
            "revert".to_string()
        } else if states.iter().any(|s| s == "REBASING") {
            "rebase".to_string()
        } else {
            String::new()
        })
    }

    pub fn conflict_resolve(
        &self,
        path: &str,
        resolution: ConflictResolution,
        custom_content: Option<Vec<u8>>,
    ) -> Result<(), String> {
        if let Some(content) = custom_content {
            self.write_file_bytes(path, &content)?;
        } else {
            let side = match resolution {
                ConflictResolution::Ours => "--ours",
                ConflictResolution::Theirs => "--theirs",
                // "both": keep the conflict-marker file as the user sees it.
                ConflictResolution::Both => {
                    self.git_checked(&format!("add -q --{}", shell_quote(path)), "resolve")?;
                    return Ok(());
                }
            };
            self.git_checked(
                &format!("checkout -q {side} -- {}", shell_quote(path)),
                "resolve",
            )?;
        }
        self.git_checked(&format!("add -q --{}", shell_quote(path)), "resolve")?;
        Ok(())
    }

    pub fn cherry_pick(&self, shas: &[String], no_commit: bool) -> Result<MergeResult, String> {
        self.sequencer_run("cherry-pick", shas, no_commit)
    }

    pub fn revert(&self, shas: &[String], no_commit: bool) -> Result<MergeResult, String> {
        self.sequencer_run("revert", shas, no_commit)
    }

    fn sequencer_run(
        &self,
        cmd: &str,
        shas: &[String],
        no_commit: bool,
    ) -> Result<MergeResult, String> {
        let mut args = format!("{cmd} -q");
        if no_commit {
            args.push_str(" --no-commit");
        }
        for sha in shas {
            args.push(' ');
            args.push_str(&shell_quote(sha));
        }
        let output = self.git_raw(&args, DEFAULT_EXEC_TIMEOUT)?;
        let conflicts = self.conflict_files()?;
        if !conflicts.is_empty() {
            return Ok(MergeResult {
                outcome: MergeOutcome::Conflicted,
                conflicts,
                new_head: self.head_sha()?,
            });
        }
        if output.exit_code != 0 {
            return Err(format!("{cmd} failed: {}", output.stderr_str().trim()));
        }
        Ok(MergeResult {
            outcome: if no_commit {
                MergeOutcome::NoCommit
            } else {
                MergeOutcome::Merged
            },
            conflicts,
            new_head: self.head_sha()?,
        })
    }

    pub fn reset(&self, kind: ResetKind, to: &str) -> Result<(), String> {
        let flag = match kind {
            ResetKind::Soft => "--soft",
            ResetKind::Mixed => "--mixed",
            ResetKind::Hard => "--hard",
            ResetKind::Keep => "--keep",
            ResetKind::Merge => "--merge",
        };
        self.git_checked(&format!("reset -q {flag} {}", shell_quote(to)), "reset")?;
        Ok(())
    }

    // ---------- rebase ----------

    pub fn rebase_start(
        &self,
        plan: &[RebaseStep],
        onto: Option<&str>,
    ) -> Result<RebaseState, String> {
        for step in plan {
            if step.action == "reword" {
                return Err("reword during rebase is not supported on remote projects".to_string());
            }
            if step.action == "exec" {
                return Err("exec steps are not supported on remote projects".to_string());
            }
        }
        if plan.is_empty() {
            return Err("empty rebase plan".to_string());
        }
        let todo_lines: Vec<String> = plan
            .iter()
            .map(|s| {
                format!(
                    "{} {} {}",
                    s.action,
                    s.sha,
                    s.new_message.as_deref().unwrap_or("")
                )
            })
            .collect();
        // Base: explicit onto, else the parent of the earliest planned commit.
        let base = match onto {
            Some(o) => shell_quote(o),
            None => format!("{}^", shell_quote(&plan[0].sha)),
        };
        let script: String = shell_quote_all(todo_lines.iter().map(|s| s.as_str()));
        let cmd = format!(
            "t=$(mktemp /tmp/mygitui-todo.XXXXXX) || exit 1; \
             printf '%s\\n' {script} > \"$t\"; \
             GIT_SEQUENCE_EDITOR=\"cp \\\"$t\\\"\" git -C {} rebase -i {base}; \
             rc=$?; rm -f \"$t\"; exit $rc",
            shell_quote(&self.root)
        );
        let output = self
            .block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("rebase start")?;
        let _ = output;
        self.rebase_state()
    }

    pub fn rebase_state(&self) -> Result<RebaseState, String> {
        let probe = self.state_probe()?;
        if !probe.iter().any(|s| s == "REBASING") {
            return Ok(RebaseState {
                active: false,
                plan: Vec::new(),
                current: 0,
                paused_for_edit: false,
                paused_for_exec: false,
                exec_error: None,
                rewritten: Vec::new(),
            });
        }
        let gdir = self
            .git_out("rev-parse --absolute-git-dir")?
            .trim()
            .to_string();
        let read_remote_file = |rel: &str| -> String {
            let cmd = format!(
                "[ -f {} ] && cat {} || true",
                shell_quote(&format!("{gdir}/{rel}")),
                shell_quote(&format!("{gdir}/{rel}"))
            );
            self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))
                .ok()
                .map(|o| o.stdout_str())
                .unwrap_or_default()
        };
        let done = parse_rebase_todo(&read_remote_file("rebase-merge/done"));
        let todo = parse_rebase_todo(&read_remote_file("rebase-merge/git-rebase-todo"));
        let stopped = read_remote_file("rebase-merge/stopped-sha")
            .trim()
            .to_string();
        let amend = !read_remote_file("rebase-merge/amend").trim().is_empty();
        let mut plan: Vec<RebaseStep> = done
            .into_iter()
            .map(|(action, sha, message)| RebaseStep {
                sha,
                action,
                new_message: if message.is_empty() {
                    None
                } else {
                    Some(message)
                },
            })
            .collect();
        plan.extend(todo.into_iter().map(|(action, sha, message)| RebaseStep {
            sha,
            action,
            new_message: if message.is_empty() {
                None
            } else {
                Some(message)
            },
        }));
        let current = plan
            .iter()
            .position(|s| !stopped.is_empty() && s.sha.starts_with(&stopped))
            .unwrap_or_else(|| plan.len().saturating_sub(1));
        Ok(RebaseState {
            active: true,
            plan,
            current,
            paused_for_edit: amend,
            paused_for_exec: false,
            exec_error: None,
            rewritten: Vec::new(),
        })
    }

    pub fn rebase_continue(&self) -> Result<RebaseState, String> {
        // Never hang waiting for an editor on the device.
        match self.git_checked("rebase --continue", "rebase continue") {
            Ok(_) => {}
            // "No rebase in progress" = the walk was already complete.
            Err(e) if e.contains("No rebase in progress") => {}
            Err(e) => return Err(e),
        }
        self.rebase_state()
    }

    pub fn rebase_abort(&self) -> Result<(), String> {
        self.git_checked("rebase --abort", "rebase abort")?;
        Ok(())
    }

    // ---------- bisect ----------

    pub fn bisect_start(
        &self,
        bad: Option<&str>,
        good: Option<&str>,
    ) -> Result<BisectState, String> {
        let mut args = String::from("bisect start");
        if let Some(b) = bad {
            args.push(' ');
            args.push_str(&shell_quote(b));
        }
        if let Some(g) = good {
            args.push(' ');
            args.push_str(&shell_quote(g));
        }
        self.git_checked(&args, "bisect start")?;
        self.bisect_state()
    }

    pub fn bisect_mark(&self, mark: BisectMark, rev: Option<&str>) -> Result<BisectState, String> {
        let word = match mark {
            BisectMark::Good => "good",
            BisectMark::Bad => "bad",
            BisectMark::Skip => "skip",
        };
        let rev_arg = rev
            .map(|r| format!(" {}", shell_quote(r)))
            .unwrap_or_default();
        self.git_checked(&format!("bisect {word}{rev_arg}"), "bisect mark")?;
        self.bisect_state()
    }

    pub fn bisect_reset(&self) -> Result<(), String> {
        self.git_checked("bisect reset", "bisect reset")?;
        Ok(())
    }

    pub fn bisect_log(&self) -> Result<Vec<BisectLogEntry>, String> {
        let raw = self.git_out("bisect log")?;
        Ok(parse_bisect_log(&raw)
            .into_iter()
            .map(|(mark, sha)| BisectLogEntry {
                mark,
                sha,
                at: 0, // `git bisect log` carries no timestamps
            })
            .collect())
    }

    pub fn bisect_state(&self) -> Result<BisectState, String> {
        let probe = self.state_probe()?;
        if !probe.iter().any(|s| s == "BISECTING") {
            return Ok(BisectState::default());
        }
        let marks = self.bisect_log()?;
        let bad = marks
            .iter()
            .rev()
            .find(|m| m.mark == "bad")
            .map(|m| m.sha.clone())
            .unwrap_or_default();
        let good = marks
            .iter()
            .rev()
            .find(|m| m.mark == "good")
            .map(|m| m.sha.clone());
        let skipped: Vec<String> = marks
            .iter()
            .filter(|m| m.mark == "skip")
            .map(|m| m.sha.clone())
            .collect();
        let current = self.head_sha()?;
        let remaining = match (&good, &bad) {
            // g/b come from the device's BISECT_LOG — quoted like any
            // remote-derived value, not trusted as rev syntax.
            (Some(g), b) if !b.is_empty() => self
                .git_out(&format!(
                    "rev-list --count {}..{}",
                    shell_quote(g),
                    shell_quote(b)
                ))?
                .trim()
                .parse()
                .unwrap_or(0),
            _ => 0,
        };
        // BISECT_START holds the ref (branch name or sha) HEAD had at start.
        let gdir = self
            .git_out("rev-parse --absolute-git-dir")?
            .trim()
            .to_string();
        let start = {
            let cmd = format!(
                "[ -f {} ] && cat {} || true",
                shell_quote(&format!("{gdir}/BISECT_START")),
                shell_quote(&format!("{gdir}/BISECT_START"))
            );
            self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))
                .ok()
                .map(|o| o.stdout_str().trim().to_string())
                .unwrap_or_default()
        };
        let is_sha = start.len() == 40 && start.chars().all(|c| c.is_ascii_hexdigit());
        Ok(BisectState {
            active: true,
            bad,
            good,
            current,
            remaining,
            skipped,
            orig_head: start.clone(),
            orig_branch: if start.is_empty() || is_sha {
                None
            } else {
                Some(start)
            },
            log: marks,
        })
    }

    // ---------- checkpoints + previews ----------

    pub fn checkpoint_create(&self, reason: &str) -> Result<CheckpointInfo, String> {
        let branch = self.current_branch()?;
        let branch_label = branch.clone().unwrap_or_else(|| "detached".to_string());
        let reason = reason.chars().take(48).collect::<String>();
        // Snapshot the FULL workdir (incl. untracked, respecting
        // .gitignore) via a temporary index — the local engine's
        // workdir_snapshot_tree, done with plumbing commands.
        let cmd = format!(
            "tmpidx=$(mktemp /tmp/mygitui-idx.XXXXXX) || exit 1; \
             export GIT_INDEX_FILE=\"$tmpidx\"; \
             git -C {root} read-tree HEAD 2>/dev/null; \
             git -C {root} add -A >/dev/null || {{ rm -f \"$tmpidx\"; exit 1; }}; \
             tree=$(git -C {root} write-tree) || {{ rm -f \"$tmpidx\"; exit 1; }}; \
             rm -f \"$tmpidx\"; \
             if [ -n \"$(git -C {root} rev-parse -q --verify HEAD)\" ]; then \
               git -C {root} commit-tree \"$tree\" -p HEAD -m {msg}; \
             else \
               git -C {root} commit-tree \"$tree\" -m {msg}; \
             fi",
            root = shell_quote(&self.root),
            msg = shell_quote(&format!(
                "checkpoint: {reason}\n\n{CHECKPOINT_MARKER}\nbranch: {branch_label}\nworktree: yes\n"
            )),
        );
        let out = self
            .block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("checkpoint create")?;
        let sha = out.stdout_str().trim().to_string();
        if sha.is_empty() {
            return Err("checkpoint snapshot failed".to_string());
        }
        let id = format!("{}-{}", sanitize_ref_component(&reason), unix_millis());
        let ref_name = format!("{CHECKPOINT_PREFIX}{id}");
        self.git_checked(
            &format!(
                "update-ref {} {}",
                shell_quote(&ref_name),
                shell_quote(&sha)
            ),
            "checkpoint ref",
        )?;
        Ok(CheckpointInfo {
            id,
            reason: reason.to_string(),
            ref_name,
            created_at: unix_secs(),
            branch,
            has_worktree_state: true,
        })
    }

    pub fn checkpoints(&self) -> Result<Vec<CheckpointInfo>, String> {
        let raw = self.git_out(&format!(
            "for-each-ref {CHECKPOINT_PREFIX}* --format=%(refname)%09%(objectname)%09%(committerdate:unix)%09%(contents)",
        ))?;
        let mut out: Vec<CheckpointInfo> = raw
            .lines()
            .filter_map(|line| {
                let mut f = line.splitn(4, '\t');
                let ref_name = f.next()?.strip_prefix(CHECKPOINT_PREFIX)?.to_string();
                let _sha = f.next()?.to_string();
                let created_at: i64 = f.next()?.parse().ok()?;
                let message = f.next().unwrap_or("");
                let (reason, branch) = parse_checkpoint_message(message);
                Some(CheckpointInfo {
                    id: ref_name.clone(),
                    reason,
                    ref_name,
                    created_at,
                    branch,
                    has_worktree_state: true,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.ref_name.cmp(&a.ref_name))
        });
        Ok(out)
    }

    pub fn checkpoint_restore(&self, id: &str) -> Result<(), String> {
        let ref_name = format!("{CHECKPOINT_PREFIX}{id}");
        let sha = self
            .git_out(&format!("rev-parse --verify {}", shell_quote(&ref_name)))?
            .trim()
            .to_string();
        if sha.is_empty() {
            return Err(format!("checkpoint `{id}` not found"));
        }
        // Safety first: snapshot the about-to-be-destroyed state.
        self.checkpoint_create("pre-restore")?;
        // index + worktree := snapshot tree (force), then drop files created
        // after the snapshot (exact-restore semantics, mirrors
        // force + recreate_missing + remove_untracked).
        let cmd = format!(
            "git -C {root} read-tree --reset -u {sha}^{{tree}} && git -C {root} clean -qfd",
            root = shell_quote(&self.root),
            sha = shell_quote(&sha)
        );
        self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("checkpoint restore")?;
        Ok(())
    }

    pub fn checkpoint_gc(&self, older_than_days: u32) -> Result<u32, String> {
        let entries = self.checkpoints()?;
        let cutoff = unix_secs() - i64::from(older_than_days) * 86_400;
        let mut deleted = 0u32;
        for (index, ckpt) in entries.iter().enumerate() {
            // The newest checkpoint always survives (safety floor).
            if index == 0 || ckpt.created_at >= cutoff {
                continue;
            }
            self.git_checked(
                &format!("update-ref -d {}", shell_quote(&ckpt.ref_name)),
                "checkpoint gc",
            )?;
            deleted += 1;
        }
        Ok(deleted)
    }

    pub fn ops_preview(&self, kind: &str, params: &Value) -> Result<PreviewInfo, String> {
        let param = |key: &str| -> Option<String> {
            params.get(key).and_then(|v| v.as_str()).map(String::from)
        };
        match kind {
            "reset_hard" => {
                let target = param("target").unwrap_or_else(|| "HEAD".to_string());
                let raw = self.git_out(&format!(
                    "diff --name-status --no-prefix -z {}",
                    shell_quote(&target)
                ))?;
                Ok(PreviewInfo {
                    kind: kind.to_string(),
                    summary: format!("Hard reset to {target}"),
                    files: name_status_rows(&raw),
                })
            }
            "clean" => {
                let raw = self.git_out("clean -nd")?;
                let files: Vec<PreviewFile> = raw
                    .lines()
                    .filter_map(|l| l.strip_prefix("Would remove "))
                    .map(|p| PreviewFile {
                        path: p.trim().to_string(),
                        change: "deleted".to_string(),
                    })
                    .collect();
                Ok(PreviewInfo {
                    kind: kind.to_string(),
                    summary: format!("Remove {} untracked files", files.len()),
                    files,
                })
            }
            "checkout_force" => {
                let target = param("target").unwrap_or_else(|| "HEAD".to_string());
                let raw = self.git_out(&format!(
                    "diff --name-status --no-prefix -z HEAD {}",
                    shell_quote(&target)
                ))?;
                Ok(PreviewInfo {
                    kind: kind.to_string(),
                    summary: format!("Force checkout {target}"),
                    files: name_status_rows(&raw),
                })
            }
            "branch_delete" => {
                let branch = param("branch").ok_or("branch_delete preview needs branch")?;
                let into = param("into").unwrap_or_else(|| "HEAD".to_string());
                let raw = self
                    .git_out(&format!(
                        "log --format=%s {}..{}",
                        shell_quote(&into),
                        shell_quote(&branch)
                    ))?
                    .lines()
                    .take(100)
                    .map(|s| PreviewFile {
                        path: s.to_string(),
                        change: s.to_string(),
                    })
                    .collect();
                Ok(PreviewInfo {
                    kind: kind.to_string(),
                    summary: format!("Commits lost from {branch}"),
                    files: raw,
                })
            }
            other => Err(format!("unknown preview kind `{other}`")),
        }
    }

    /// Checkpoint safety guard used by destructive IPC commands.
    pub fn guard_checkpoint(&self, reason: &str) -> Result<CheckpointInfo, String> {
        self.checkpoint_create(reason)
    }

    // ---------- health / maintenance / archive / sparse / lfs ----------

    pub fn repo_health(&self) -> Result<RepoHealth, String> {
        let cmd = format!(
            "d=$(git -C {root} rev-parse --absolute-git-dir) || exit 1; \
             git -C {root} count-objects -v; \
             echo \"GIT_SIZE $(du -sk \"$d\" | cut -f1)\"; \
             echo \"WT_SIZE $(du -sk {root} | cut -f1)\"; \
             [ -f \"$d/objects/info/commit-graph\" ] && echo \"CG_SIZE $(wc -c < \"$d/objects/info/commit-graph\")\"; \
             [ -f \"$d/packed-refs\" ] && echo PACKED_REFS; \
             [ -f \"$d/gc.log\" ] && echo \"GC_TIME $(stat -c %Y \"$d/gc.log\" 2>/dev/null || stat -f %m \"$d/gc.log\" 2>/dev/null)\"; \
             exit 0",
            root = shell_quote(&self.root)
        );
        let out = self
            .block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("health probe")?;
        let text = out.stdout_str();
        let (loose, _loose_bytes, packs, pack_bytes, _in_pack) = parse_count_objects(&text);
        let git_size = find_line_value(&text, "GIT_SIZE");
        let wt_total = find_line_value(&text, "WT_SIZE");
        let health = RepoHealth {
            git_size_bytes: git_size * 1024,
            worktree_size_bytes: wt_total.saturating_sub(git_size) * 1024,
            loose_objects: loose,
            packed_objects: Some(pack_bytes),
            pack_files: packs,
            has_commit_graph: text.contains("CG_SIZE"),
            commit_graph_bytes: find_line_value(&text, "CG_SIZE"),
            packed_refs: text.contains("PACKED_REFS"),
            last_gc: i64::try_from(find_line_value(&text, "GC_TIME")).ok(),
            fsck_dangling: None,
            fsck_samples: Vec::new(),
        };
        Ok(health)
    }

    pub fn fsck_dangling(&self) -> Result<(u64, Vec<String>), String> {
        let raw = self.git_out("fsck --no-progress --dangling")?;
        let samples: Vec<String> = parse_fsck_dangling(&raw).into_iter().take(10).collect();
        let count = raw.lines().filter(|l| l.starts_with("dangling ")).count() as u64;
        Ok((count, samples))
    }

    pub fn maintenance_run(&self, op: &str) -> Result<String, String> {
        let args = match op {
            "gc" => "gc --quiet",
            "prune" => "prune --verbose",
            "fsck" => "fsck --no-progress",
            other => return Err(format!("unknown maintenance op `{other}`")),
        };
        let out = self.git_raw(args, LONG_EXEC_TIMEOUT)?;
        Ok(format!(
            "exit {} {}",
            out.exit_code,
            out.stdout_str().trim()
        ))
    }

    /// `git archive` to a LOCAL destination file (bytes stream over SSH).
    pub fn archive(
        &self,
        spec: &str,
        format: &str,
        destination: &std::path::Path,
    ) -> Result<(), String> {
        if !format
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("invalid archive format".to_string());
        }
        let bytes = self.git_archive_bytes(&format!(
            "archive --format={} {}",
            shell_quote(format),
            shell_quote(spec)
        ))?;
        std::fs::write(destination, bytes).map_err(|e| format!("write archive: {e}"))?;
        Ok(())
    }

    pub fn sparse_info(&self) -> Result<SparseInfo, String> {
        let enabled = self
            .git_raw(
                "config --get --bool core.sparseCheckout",
                DEFAULT_EXEC_TIMEOUT,
            )?
            .stdout_str()
            .trim()
            == "true";
        let mut cone = false;
        let mut patterns = Vec::new();
        if enabled {
            let out = self.git_raw("sparse-checkout list", DEFAULT_EXEC_TIMEOUT)?;
            if out.exit_code == 0 {
                for line in out.stdout_str().lines() {
                    let l = line.trim();
                    if l.is_empty() {
                        continue;
                    }
                    if l == "/*/" || l == "!" || l.ends_with("/**") {
                        // cone-ish marker; keep the literal line either way
                    }
                    if l == "/*/" {
                        cone = true;
                    }
                    patterns.push(l.to_string());
                }
            }
        }
        Ok(SparseInfo {
            enabled,
            cone,
            patterns,
        })
    }

    pub fn sparse_apply(&self, patterns: &[String], add: bool) -> Result<(), String> {
        let sub = if add { "add" } else { "set" };
        let args = format!(
            "sparse-checkout {sub} --{}",
            patterns
                .iter()
                .map(|p| format!(" {}", shell_quote(p)))
                .collect::<String>()
        );
        self.git_checked(&args, "sparse-checkout")?;
        Ok(())
    }

    pub fn lfs_status(&self) -> Result<LfsStatus, String> {
        let installed = self
            .git_raw("lfs version", DEFAULT_EXEC_TIMEOUT)
            .map(|o| o.exit_code == 0)
            .unwrap_or(false);
        let version = self
            .git_out("lfs version")
            .ok()
            .map(|v| v.trim().to_string());
        let mut tracked = Vec::new();
        if installed {
            // Patterns declared `filter=lfs` in tracked .gitattributes files.
            let cmd = self.command("ls-files -- '*.gitattributes'");
            let attrs = self
                .block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))
                .ok()
                .filter(|o| o.exit_code == 0)
                .map(|o| o.stdout_str())
                .unwrap_or_default();
            for file in attrs.lines().filter(|l| !l.trim().is_empty()) {
                let cmd = format!(
                    "grep -h \"filter=lfs\" {} 2>/dev/null || true",
                    shell_quote(&format!("{}/{}", self.root, file.trim()))
                );
                if let Ok(out) = self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT)) {
                    for line in out.stdout_str().lines() {
                        if let Some(pattern) = line.split_whitespace().next() {
                            let pattern = pattern.trim_matches('"').to_string();
                            if !pattern.is_empty() && !tracked.contains(&pattern) {
                                tracked.push(pattern);
                            }
                        }
                    }
                }
            }
        }
        Ok(LfsStatus {
            installed,
            version,
            tracked_patterns: tracked,
        })
    }

    pub fn lfs_run(&self, subcommand: &str) -> Result<String, String> {
        const ALLOWED: [&str; 4] = ["fetch", "pull", "push", "checkout"];
        if !ALLOWED.contains(&subcommand) {
            return Err(format!("unsupported git-lfs subcommand `{subcommand}`"));
        }
        let out = self.git_raw(&format!("lfs {subcommand}"), LONG_EXEC_TIMEOUT)?;
        Ok(out.stdout_str().trim().to_string())
    }

    // ---------- worktrees / submodules / reflog / trash ----------

    pub fn worktrees(&self) -> Result<Vec<WorktreeInfo>, String> {
        Ok(parse_worktrees(&self.git_out("worktree list --porcelain")?))
    }

    pub fn worktree_add(
        &self,
        path: &str,
        branch: Option<&str>,
        new_branch: Option<&str>,
    ) -> Result<(), String> {
        let mut args = String::from("worktree add");
        if let Some(nb) = new_branch {
            args.push_str(&format!(" -b {}", shell_quote(nb)));
        }
        args.push(' ');
        args.push_str(&shell_quote(path));
        if let Some(b) = branch {
            args.push(' ');
            args.push_str(&shell_quote(b));
        }
        self.git_checked(&args, "worktree add")?;
        Ok(())
    }

    pub fn worktree_remove(&self, name: &str, force: bool) -> Result<(), String> {
        let flag = if force { " --force" } else { "" };
        self.git_checked(
            &format!("worktree remove{} {}", flag, shell_quote(name)),
            "worktree remove",
        )?;
        Ok(())
    }

    pub fn worktree_prune(&self) -> Result<(), String> {
        self.git_checked("worktree prune -v", "worktree prune")?;
        Ok(())
    }

    pub fn submodules(&self) -> Result<Vec<SubmoduleInfo>, String> {
        let raw = self.git_out("submodule status --recursive")?;
        let urls = self
            .git_out("config --get-regexp '^submodule\\..*\\.url$'")
            .ok();
        let mut out = Vec::new();
        for (status, sha, path) in parse_submodule_status(&raw) {
            let name = path.clone();
            let url = urls
                .as_deref()
                .and_then(|u| {
                    u.lines().find_map(|l| {
                        let (k, v) = l.split_once(' ')?;
                        let key = k.strip_prefix("submodule.")?.strip_suffix(".url")?;
                        // `submodule status` paths are relative; config keys
                        // use the submodule NAME (usually == path).
                        if key == name || path.ends_with(key) {
                            Some(v.to_string())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_default();
            let initialized = status != '-';
            let head_sha = if initialized { Some(sha.clone()) } else { None };
            let status_text = match status {
                '+' => "new commits",
                'U' => "merge conflicts",
                'T' => "modified content",
                ' ' => "",
                _ => "",
            }
            .to_string();
            out.push(SubmoduleInfo {
                path,
                name,
                url,
                head_sha,
                recorded_sha: sha,
                initialized,
                status: status_text,
            })
        }
        Ok(out)
    }

    pub fn submodule_update(&self, init: bool, recursive: bool) -> Result<(), String> {
        let mut args = String::from("submodule update");
        if init {
            args.push_str(" --init");
        }
        if recursive {
            args.push_str(" --recursive");
        }
        self.git_checked(&args, "submodule update")?;
        Ok(())
    }

    pub fn submodule_sync(&self, recursive: bool) -> Result<(), String> {
        let flag = if recursive { " --recursive" } else { "" };
        self.git_checked(&format!("submodule sync{flag}"), "submodule sync")?;
        Ok(())
    }

    pub fn reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>, String> {
        let raw = self.git_out(&format!(
            "reflog --format=%H%x09%at%x09%an%x09%ae%x09%gs -n {limit}"
        ))?;
        Ok(parse_reflog(&raw)
            .into_iter()
            .map(|(old_sha, new_sha, signature, message)| ReflogEntry {
                old_sha,
                new_sha,
                signature,
                message,
            })
            .collect())
    }

    pub fn branch_trash_list(&self) -> Result<Vec<BranchTrashEntry>, String> {
        let raw = self.git_out(&format!(
            "for-each-ref {TRASH_PREFIX}* --format=%(refname)%09%(objectname)%09%(committerdate:unix)"
        ))?;
        Ok(raw
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\t');
                let full = f.next()?;
                let id = full.strip_prefix(TRASH_PREFIX)?;
                let sha = f.next()?.to_string();
                let deleted_at: i64 = f.next()?.parse().ok()?;
                let name = strip_trash_suffix(id).to_string();
                Some(BranchTrashEntry {
                    id: id.to_string(),
                    name,
                    sha,
                    deleted_at,
                })
            })
            .collect())
    }

    pub fn branch_trash_restore(&self, id: &str) -> Result<(), String> {
        let ref_name = format!("{TRASH_PREFIX}{id}");
        let sha = self
            .git_out(&format!("rev-parse --verify {}", shell_quote(&ref_name)))?
            .trim()
            .to_string();
        if sha.is_empty() {
            return Err(format!("trash entry `{id}` not found"));
        }
        let name = strip_trash_suffix(id).to_string();
        self.git_checked(
            &format!("branch {} {}", shell_quote(&name), shell_quote(&sha)),
            "trash restore",
        )?;
        self.git_checked(
            &format!("update-ref -d {}", shell_quote(&ref_name)),
            "trash cleanup",
        )?;
        Ok(())
    }

    // ---------- stats / describe / signatures / gitignore / clean ----------

    pub fn commit_activity(
        &self,
        max_days: u32,
        author: Option<&str>,
    ) -> Result<Vec<DayCount>, String> {
        // Committer-local day buckets come straight from the device's
        // `--date=short` (device timezone, matching the local engine's
        // committer-tz bucketing closely enough for the activity chart).
        let raw = self.git_out(&format!(
            "log --all --since={max_days}.days.ago --date=short --format=%cd%x09%an%x09%ae"
        ))?;
        let needle = author.map(|a| a.to_ascii_lowercase());
        let mut days: Vec<(String, u32)> = Vec::new();
        for line in raw.lines() {
            let mut f = line.split('\t');
            let Some(day) = f.next() else { continue };
            let name = f.next().unwrap_or("");
            let email = f.next().unwrap_or("");
            if let Some(needle) = &needle {
                let hay = format!("{name} {email}").to_ascii_lowercase();
                if !hay.contains(needle) {
                    continue;
                }
            }
            match days.iter_mut().find(|(d, _)| d == day) {
                Some((_, count)) => *count += 1,
                None => days.push((day.to_string(), 1)),
            }
        }
        days.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(days
            .into_iter()
            .map(|(day, count)| DayCount { day, count })
            .collect())
    }

    pub fn contributor_stats(&self, max_days: u32) -> Result<Vec<Contributor>, String> {
        let raw = self.git_out(&format!(
            "log --all --since={max_days}.days.ago --date=short --format=%an%x09%ae%x09%cd"
        ))?;
        let mut out: Vec<Contributor> = Vec::new();
        for line in raw.lines() {
            let mut f = line.split('\t');
            let Some(name) = f.next() else { continue };
            let Some(email) = f.next() else { continue };
            let Some(day) = f.next() else { continue };
            match out.iter_mut().find(|c| c.name == name && c.email == email) {
                Some(c) => {
                    c.count += 1;
                    if day < c.first_day.as_str() {
                        c.first_day = day.to_string();
                    }
                    if day > c.last_day.as_str() {
                        c.last_day = day.to_string();
                    }
                }
                None => out.push(Contributor {
                    name: name.to_string(),
                    email: email.to_string(),
                    count: 1,
                    first_day: day.to_string(),
                    last_day: day.to_string(),
                }),
            }
        }
        out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn describe(&self, spec: &str) -> Result<String, String> {
        let out = self.git_raw(
            &format!("describe --tags {}", shell_quote(spec)),
            DEFAULT_EXEC_TIMEOUT,
        )?;
        if out.exit_code == 0 {
            let d = out.stdout_str().trim().to_string();
            if !d.is_empty() {
                return Ok(d);
            }
        }
        // Fallback: short sha (matches the local engine).
        self.git_out(&format!("rev-parse --short {}", shell_quote(spec)))
            .map(|s| s.trim().to_string())
    }

    pub fn autosquash_plan(&self, base: &str) -> Result<Vec<RebaseStep>, String> {
        let raw = self.git_out(&format!(
            "log --reverse --format=%H%x09%s {}..HEAD",
            shell_quote(base)
        ))?;
        Ok(raw
            .lines()
            .filter_map(|l| {
                let (sha, subject) = l.split_once('\t')?;
                let action = if subject.starts_with("fixup!") {
                    "fixup"
                } else if subject.starts_with("squash!") {
                    "squash"
                } else {
                    return None;
                };
                Some(RebaseStep {
                    sha: sha.to_string(),
                    action: action.to_string(),
                    new_message: None,
                })
            })
            .collect())
    }

    pub fn commit_signature(&self, sha: &str) -> Result<CommitSignature, String> {
        let raw = self
            .git_out(&format!("cat-file commit {}", shell_quote(sha)))?
            .to_string();
        let signed = raw.contains("\ngpgsig ") || raw.starts_with("gpgsig ");
        let (kind, valid, detail) = if !signed {
            (None, None, String::new())
        } else if raw.contains("SSH SIGNATURE") {
            let out = self.git_raw(
                &format!("verify-commit {}", shell_quote(sha)),
                DEFAULT_EXEC_TIMEOUT,
            )?;
            (
                Some(crate::engine::types::SignatureKind::Ssh),
                Some(out.exit_code == 0),
                out.stderr_str().trim().to_string(),
            )
        } else {
            let out = self.git_raw(
                &format!("verify-commit {}", shell_quote(sha)),
                DEFAULT_EXEC_TIMEOUT,
            )?;
            (
                Some(crate::engine::types::SignatureKind::Gpg),
                Some(out.exit_code == 0),
                out.stderr_str().trim().to_string(),
            )
        };
        Ok(CommitSignature {
            signed,
            kind,
            valid,
            detail,
        })
    }

    // ---------- files (SFTP-backed primitives used by IPC + actions) ----------

    /// Replace a workdir file's content over SFTP (create/truncate + write).
    /// The path is untrusted (FE-supplied, e.g. the conflict editor), so the
    /// same gates as every other remote writer apply: worktree-relative, no
    /// `..`, never inside `.git`.
    fn write_file_bytes(&self, rel_path: &str, content: &[u8]) -> Result<(), String> {
        let rel = super::workspace::validate_rel_posix(rel_path)?;
        if rel.split('/').any(|c| c == ".git") {
            return Err(format!("refusing to write inside .git: {rel_path}"));
        }
        let abs = format!("{}/{}", self.root, rel);
        self.block(async {
            let sftp = self.session.sftp().await?;
            let mut file = sftp
                .create(&abs)
                .await
                .map_err(|e| format!("sftp open {abs}: {e}"))?;
            use tokio::io::AsyncWriteExt;
            file.write_all(content)
                .await
                .map_err(|e| format!("sftp write {abs}: {e}"))?;
            file.flush()
                .await
                .map_err(|e| format!("sftp flush {abs}: {e}"))?;
            Ok::<(), String>(())
        })
    }

    pub fn gitignore_add(&self, pattern: &str) -> Result<(), String> {
        // Idempotent append: exact-line dedupe.
        let cmd = format!(
            "f={}; touch \"$f\"; grep -qxF {} \"$f\" || printf '%s\\n' {} >> \"$f\"",
            shell_quote(&format!("{}/.gitignore", self.root)),
            shell_quote(pattern),
            shell_quote(pattern)
        );
        self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?
            .checked("gitignore add")?;
        Ok(())
    }

    /// Append a whole template block to .gitignore (exact-line dedupe per
    /// line).
    pub fn gitignore_append_block(&self, text: &str) -> Result<(), String> {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            self.gitignore_add(line.trim())?;
        }
        Ok(())
    }

    pub fn repo_clean(&self, paths: &[String]) -> Result<u32, String> {
        let args = format!("clean -qfd --{}", quoted_paths(paths));
        let out = self.git_raw(&args, DEFAULT_EXEC_TIMEOUT)?;
        if out.exit_code != 0 {
            return Err(format!("clean failed: {}", out.stderr_str().trim()));
        }
        // -q suppresses "Removing" lines; count via dry-run first would race.
        // Return 0 — the FE shows success/failure, count is best-effort.
        let _ = out;
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// free helpers (pure, unit-testable)
// ---------------------------------------------------------------------------

/// Build a `git` command line: `git --no-optional-locks -C '<root>' <args>`.
/// `--no-optional-locks` stops status/diff from opportunistically writing
/// the index on the device (never fight the user's own git there).
pub fn git_command(root: &str, args: &str) -> String {
    format!("git --no-optional-locks -C {} {}", shell_quote(root), args)
}

fn unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Sanitize a reason/branch name for use inside a ref component.
fn sanitize_ref_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "checkpoint".to_string()
    } else {
        trimmed
    }
}

/// `trash/<name>-<millis>` → name (strip the `-<13-digit>` suffix).
fn strip_trash_suffix(id: &str) -> &str {
    let bytes = id.as_bytes();
    let mut cut = id.len();
    while cut > 0 && bytes[cut - 1].is_ascii_digit() {
        cut -= 1;
    }
    if cut > 0 && bytes[cut - 1] == b'-' {
        cut -= 1;
    }
    if cut == 0 {
        id
    } else {
        &id[..cut]
    }
}

fn header_branch(header: &StatusHeader) -> Option<String> {
    match header.head.as_str() {
        "(detached)" | "" => None,
        name => Some(name.to_string()),
    }
}

fn header_oid(header: &StatusHeader) -> Option<String> {
    match header.oid.as_str() {
        "(initial)" | "" => None,
        oid => Some(oid.to_string()),
    }
}

/// ` -- 'a' 'b'` pathspec suffix (empty when no paths).
fn path_spec(paths: Option<&[String]>) -> String {
    match paths {
        Some(p) if !p.is_empty() => format!(" --{}", quoted_paths(p)),
        _ => String::new(),
    }
}

fn quoted_paths(paths: &[String]) -> String {
    paths
        .iter()
        .map(|p| format!(" {}", shell_quote(p)))
        .collect()
}

/// Map a DiffSide pair onto `git diff` argument shapes. `Worktree` has no
/// tree-ish; `Index` is expressed with `--cached`; `-R` reverses when the
/// FE asks for the unnatural direction.
pub fn diff_range(old: &DiffSide, new: &DiffSide) -> Result<String, String> {
    use DiffSide::{Commit, Head, Index, Worktree};
    let args = match (old, new) {
        (Index, Worktree) => String::new(),
        (Worktree, Index) => "-R".to_string(),
        (Head, Index) => "--cached".to_string(),
        (Index, Head) => "--cached -R".to_string(),
        (Head, Worktree) => "HEAD".to_string(),
        (Worktree, Head) => "-R HEAD".to_string(),
        (Commit(a), Worktree) => shell_quote(a),
        (Worktree, Commit(a)) => format!("-R {}", shell_quote(a)),
        (Commit(a), Index) => format!("--cached {}", shell_quote(a)),
        (Index, Commit(a)) => format!("--cached -R {}", shell_quote(a)),
        (Commit(a), Head) => format!("{} HEAD", shell_quote(a)),
        (Head, Commit(a)) => format!("-R HEAD {}", shell_quote(a)),
        (Commit(a), Commit(b)) => format!("{} {}", shell_quote(a), shell_quote(b)),
        _ => return Err("identical diff sides".to_string()),
    };
    Ok(args)
}

/// Log args: format + filters + pagination (`limit` = page size; `after` =
/// last sha of the previous page).
pub fn log_args(filter: &LogFilter, limit: usize, after: Option<&str>) -> String {
    let mut args = String::from(
        "log --format=%H%x00%P%x00%an%x00%ae%x00%at%x00%ai%x00%cn%x00%ce%x00%ct%x00%ci%x00%D%x00%B%x01",
    );
    if filter.regex {
        args.push_str(" -E");
    } else {
        args.push_str(" -F");
    }
    if let Some(text) = &filter.text {
        args.push_str(&format!(" --grep={}", shell_quote(text)));
    }
    if let Some(author) = &filter.author {
        args.push_str(&format!(" --author={}", shell_quote(author)));
    }
    if let Some(after_unix) = filter.after_unix {
        args.push_str(&format!(" --since=@{after_unix}"));
    }
    if let Some(before_unix) = filter.before_unix {
        args.push_str(&format!(" --until=@{before_unix}"));
    }
    if let Some(p) = &filter.pickaxe {
        args.push_str(&format!(" -S{}", shell_quote(p)));
    }
    if let Some(p) = &filter.pickaxe_regex {
        args.push_str(&format!(" -G{}", shell_quote(p)));
    }
    if !filter.refs.is_empty() {
        for r in &filter.refs {
            args.push(' ');
            args.push_str(&shell_quote(r));
        }
    } else {
        args.push_str(" --all");
    }
    if let Some(cursor) = after {
        args.push_str(&format!(" ^{}", shell_quote(cursor)));
    }
    args.push_str(&format!(" -n {limit}"));
    if let Some(path) = &filter.path {
        if filter.follow {
            args.push_str(" --follow");
        }
        args.push_str(&format!(" -- {}", shell_quote(path)));
    }
    args
}

/// Parse one raw unified hunk into a DiffHunk (line numbers tracked from
/// the @@ header).
pub fn parse_hunk(raw: &str) -> Option<crate::engine::types::DiffHunk> {
    use crate::engine::types::{DiffHunk, DiffLine};
    let header_end = raw.find('\n')?;
    let header = &raw[..header_end];
    let body = &raw[header_end + 1..];
    let ranges: Vec<&str> = header
        .trim_start_matches("@@")
        .trim_end_matches("@@")
        .trim()
        .split(' ')
        .filter(|r| r.starts_with('-') || r.starts_with('+'))
        .collect();
    if ranges.len() < 2 {
        return None;
    }
    let parse_start = |r: &str| -> u32 {
        r[1..]
            .split(',')
            .next()
            .unwrap_or("0")
            .trim()
            .parse()
            .unwrap_or(0)
    };
    let (mut old_no, mut new_no) = (parse_start(ranges[0]), parse_start(ranges[1]));
    let mut lines = Vec::new();
    for line in body.split('\n').filter(|l| !l.is_empty()) {
        match line.chars().next() {
            Some('+') => {
                lines.push(DiffLine {
                    old_no: None,
                    new_no: Some(new_no),
                    origin: '+',
                    text: line[1..].to_string(),
                    highlights: Vec::new(),
                });
                new_no += 1;
            }
            Some('-') => {
                lines.push(DiffLine {
                    old_no: Some(old_no),
                    new_no: None,
                    origin: '-',
                    text: line[1..].to_string(),
                    highlights: Vec::new(),
                });
                old_no += 1;
            }
            Some(' ') => {
                lines.push(DiffLine {
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                    origin: ' ',
                    text: line[1..].to_string(),
                    highlights: Vec::new(),
                });
                old_no += 1;
                new_no += 1;
            }
            Some('\\') => {} // "\ No newline at end of file"
            _ => {}
        }
    }
    Some(DiffHunk {
        old_start: parse_start(ranges[0]),
        new_start: parse_start(ranges[1]),
        lines,
    })
}

/// Reduce a raw hunk to the lines covered by new-side `ranges`, with a
/// recomputed header (partial-hunk staging).
pub fn filter_hunk_lines(raw: &str, ranges: &[(u32, u32)]) -> String {
    let Some(parsed) = parse_hunk(raw) else {
        return String::new();
    };
    let in_range = |no: Option<u32>| -> bool {
        no.map(|n| ranges.iter().any(|(s, e)| n >= *s && n <= *e))
            .unwrap_or(false)
    };
    let kept: Vec<_> = parsed
        .lines
        .iter()
        .filter(|l| match l.origin {
            '+' => in_range(l.new_no),
            '-' => true, // removals always accompany their context selection
            _ => in_range(l.new_no) || in_range(l.old_no),
        })
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    let old_count = kept.iter().filter(|l| l.origin != '+').count();
    let new_count = kept.iter().filter(|l| l.origin != '-').count();
    let old_start = kept
        .iter()
        .find(|l| l.origin != '+')
        .and_then(|l| l.old_no)
        .unwrap_or(parsed.old_start.max(1));
    let new_start = kept
        .iter()
        .find(|l| l.origin != '-')
        .and_then(|l| l.new_no)
        .unwrap_or(parsed.new_start.max(1));
    let mut out = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n");
    for l in kept {
        out.push_str(&format!("{}{}\n", l.origin, l.text));
    }
    out
}

/// One per-file hunk selection: (hunk index within the file's unstaged
/// diff, optional new-side line ranges).
type HunkSelection = (u32, Option<Vec<(u32, u32)>>);

/// Group hunk targets by path, preserving hunk order per path.
fn group_hunks_by_path<'a>(targets: &'a [&'a StageTarget]) -> Vec<(String, Vec<HunkSelection>)> {
    let mut out: Vec<(String, Vec<HunkSelection>)> = Vec::new();
    for t in targets {
        match t {
            StageTarget::Hunk { path, hunk } => match out.iter_mut().find(|(p, _)| p == path) {
                Some((_, list)) => list.push((*hunk, None)),
                None => out.push((path.clone(), vec![(*hunk, None)])),
            },
            StageTarget::Lines { path, hunk, ranges } => {
                let ranges: Vec<(u32, u32)> = ranges.iter().map(|r| (r.start, r.end)).collect();
                match out.iter_mut().find(|(p, _)| p == path) {
                    Some((_, list)) => list.push((*hunk, Some(ranges))),
                    None => out.push((path.clone(), vec![(*hunk, Some(ranges))])),
                }
            }
            StageTarget::File(_) => {}
        }
    }
    out
}

/// `--name-status -z` records → PreviewFiles.
fn name_status_rows(raw: &str) -> Vec<PreviewFile> {
    let fields: Vec<&str> = raw.split('\0').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let status = fields[i];
        if status.is_empty() {
            i += 1;
            continue;
        }
        let change = match status {
            "A" => "added",
            "D" => "deleted",
            _ => "modified",
        };
        if status == "R" || status == "C" {
            if let (Some(old), Some(new)) = (fields.get(i + 1), fields.get(i + 2)) {
                out.push(PreviewFile {
                    path: new.to_string(),
                    change: change.to_string(),
                });
                let _ = old;
            }
            i += 3;
        } else {
            if let Some(p) = fields.get(i + 1) {
                out.push(PreviewFile {
                    path: p.to_string(),
                    change: change.to_string(),
                });
            }
            i += 2;
        }
    }
    out
}

/// First `KEY <value>` line in a probe dump.
fn find_line_value(text: &str, key: &str) -> u64 {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key} "))?.trim().parse().ok())
        .unwrap_or(0)
}

/// `checkpoint: reason\n\nmarker\nbranch: b\nworktree: yes` → (reason, branch).
fn parse_checkpoint_message(message: &str) -> (String, Option<String>) {
    let mut reason = String::new();
    let mut branch = None;
    for (i, line) in message.lines().enumerate() {
        if i == 0 {
            reason = line
                .strip_prefix("checkpoint: ")
                .unwrap_or(line)
                .to_string();
        }
        if let Some(b) = line.strip_prefix("branch: ") {
            branch = match b {
                "detached" => None,
                other => Some(other.to_string()),
            };
        }
    }
    (reason, branch)
}
