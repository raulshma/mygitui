//! M12 lane B: commit signature verification via the git CLI.
//!
//! The commit object is inspected through `git cat-file commit` (sanitized
//! CLI) to detect a `gpgsig` / `gpgsig-sha256` header, and signed commits are
//! verified with `git verify-commit --raw` — the same gpg/ssh-agent plumbing
//! the terminal uses, honoring `gpg.ssh.allowedSignersFile` for SSH
//! signatures. The contract lives in [`super::git_engine::GitEngineM12`].
//!
//! WIRING NOTE (M2-lane convention): the crate's single
//! `impl GitEngineM12 for Libgit2Engine` lives in trash.rs (Rust coherence);
//! its `commit_signature` forwards to [`Libgit2Engine::commit_signature_impl`]
//! here.

use std::path::Path;
use std::process::Command;

use git2::Repository;

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{CommitSignature, SignatureKind};

impl Libgit2Engine {
    /// Verification status of one commit's signature. Three outcomes for a
    /// signed commit: valid (`Some(true)`), bad (`Some(false)`), or "signed
    /// but not verifiable here" (`None` — missing key / no allowed_signers).
    /// Unsigned commits report `signed: false` and never touch the CLI
    /// verifier.
    pub(crate) fn commit_signature_impl(
        &self,
        repo: &Repository,
        sha: &str,
    ) -> EngineResult<CommitSignature> {
        // Resolve first so callers get the engine's usual Invalid-ref error
        // (and we verify against the full 40-char sha, not user input).
        let commit = repo
            .revparse_single(sha)
            .map_err(|e| EngineError::Invalid(format!("cannot resolve `{sha}`: {e}")))?
            .peel_to_commit()
            .map_err(|e| EngineError::Invalid(format!("`{sha}` is not a commit: {e}")))?;
        let sha_hex = commit.id().to_string();

        let raw = run_git(repo, &["cat-file".into(), "commit".into(), sha_hex.clone()])?;
        let Some(kind) = detect_signature_kind(&raw) else {
            return Ok(CommitSignature {
                signed: false,
                kind: None,
                valid: None,
                detail: "unsigned".into(),
            });
        };

        // Signed: `verify-commit --raw` speaks for gpg and ssh alike.
        let (ok, output) = run_git_captured(
            repo,
            &[
                "verify-commit".to_string(),
                "--raw".to_string(),
                sha_hex.clone(),
            ],
        );
        if ok {
            return Ok(CommitSignature {
                signed: true,
                kind: Some(kind),
                valid: Some(true),
                detail: output.trim().to_string(),
            });
        }
        // Nonzero exit: either a genuinely bad signature or "we lack the
        // material to check" (no public key / no allowed_signers entry). The
        // latter must NOT read as tampering — the user may just need to
        // import a key — so it maps to valid: None.
        if let Some(reason) = not_verifiable_reason(&output) {
            return Ok(CommitSignature {
                signed: true,
                kind: Some(kind),
                valid: None,
                detail: reason,
            });
        }
        Ok(CommitSignature {
            signed: true,
            kind: Some(kind),
            valid: Some(false),
            detail: output.trim().to_string(),
        })
    }
}

/// Detect the signature kind from a raw commit object: the `gpgsig` header's
/// first fold line carries the armor marker. `gpgsig-sha256` (dual-signed
/// commits) is OpenPGP-only — SSH has no sha256 variant.
fn detect_signature_kind(raw_commit: &str) -> Option<SignatureKind> {
    for line in raw_commit.lines() {
        if line.is_empty() {
            break; // end of headers
        }
        if line.starts_with(' ') {
            continue; // folded continuation of the previous header
        }
        if let Some(first) = line.strip_prefix("gpgsig ") {
            return Some(if first.contains("SSH SIGNATURE") {
                SignatureKind::Ssh
            } else {
                SignatureKind::Gpg
            });
        }
        if line.starts_with("gpgsig-sha256 ") {
            return Some(SignatureKind::Gpg);
        }
    }
    None
}

/// Substrings in a failed `verify-commit` output that mean "cannot check",
/// not "bad". Returns the matching trimmed lines (the tooltip detail).
fn not_verifiable_reason(output: &str) -> Option<String> {
    const MARKERS: [&str; 4] = [
        "No public key",
        "can't check signature: No public key",
        "gpg.ssh.allowedSignersFile",
        "allowed_signers",
    ];
    let mut hits = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if MARKERS.iter().any(|m| trimmed.contains(m)) {
            hits.push(trimmed.to_string());
        }
    }
    if hits.is_empty() {
        None
    } else {
        Some(hits.join("\n"))
    }
}

/// Workdir to run git in (a bare repo still works — git discovers the
/// object store from its gitdir).
fn workdir(repo: &Repository) -> EngineResult<&Path> {
    repo.workdir()
        .or_else(|| repo.path().parent())
        .ok_or_else(|| EngineError::Invalid("repository has no readable path".into()))
}

/// `git <args>` with the sanitized environment (same spawn pattern as the
/// tag signer in mergetool.rs). Success returns trimmed stdout.
fn run_git(repo: &Repository, args: &[String]) -> EngineResult<String> {
    let (ok, output) = run_git_captured(repo, args);
    if !ok {
        return Err(EngineError::Invalid(format!(
            "git {} failed: {output}",
            args.first().map(String::as_str).unwrap_or_default()
        )));
    }
    Ok(output)
}

/// Spawn variant that keeps both streams and the exit status: signature
/// verification FAILS on purpose all the time (that is its data).
fn run_git_captured(repo: &Repository, args: &[String]) -> (bool, String) {
    let mut cmd = Command::new("git");
    cmd.current_dir(workdir(repo).unwrap_or_else(|_| Path::new(".")));
    cmd.args(args);
    crate::cli::apply_sanitized_env(&mut cmd);
    match cmd.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            (out.status.success(), text)
        }
        Err(err) => (false, format!("failed to spawn git: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use git2::{Repository, RepositoryInitOptions};

    use super::*;
    use crate::engine::git_engine::GitEngineM12;

    const ENGINE: Libgit2Engine = Libgit2Engine;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-signing-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let dir = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }

    fn init_repo(name: &str) -> (Repository, TempDir) {
        let dir = TempDir::new(name);
        let mut opts = RepositoryInitOptions::new();
        opts.bare(false).initial_head("main");
        let repo = Repository::init_opts(dir.path(), &opts).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Signing Test")?;
                c.set_str("user.email", "signing@test.local")?;
                c.set_bool("core.autocrlf", false)?;
                c.set_bool("commit.gpgsign", false)
            })
            .expect("configure identity");
        (repo, dir)
    }

    /// One `git <args>` with the sanitized env; returns (ok, stdout+stderr).
    fn git(dir: &Path, args: &[&str]) -> (bool, String) {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut cmd = Command::new("git");
        cmd.current_dir(dir).args(&owned);
        crate::cli::apply_sanitized_env(&mut cmd);
        match cmd.output() {
            Ok(out) => {
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                (out.status.success(), text)
            }
            Err(err) => (false, format!("failed to spawn git: {err}")),
        }
    }

    fn head_sha(repo: &Repository) -> String {
        repo.head()
            .expect("head")
            .peel_to_commit()
            .expect("commit")
            .id()
            .to_string()
    }

    // ---- pure header classification --------------------------------------

    #[test]
    fn detect_signature_kind_reads_armor_markers() {
        let gpg = "tree 1111\nauthor x <x@y> 0 +0000\ncommitter x <x@y> 0 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n iQEcBAABCgAG\n =abcd\n -----END PGP SIGNATURE-----\n\nmessage\n";
        assert!(matches!(
            detect_signature_kind(gpg),
            Some(SignatureKind::Gpg)
        ));
        // Dual-signed: the sha256 arm is OpenPGP as well.
        let dual = gpg.replace("gpgsig -----BEGIN", "gpgsig-sha256 -----BEGIN");
        assert!(matches!(
            detect_signature_kind(&dual),
            Some(SignatureKind::Gpg)
        ));
        let ssh = "tree 1111\ncommitter x <x@y> 0 +0000\ngpgsig -----BEGIN SSH SIGNATURE-----\n b25zaG90\n -----END SSH SIGNATURE-----\n\nmessage\n";
        assert!(matches!(
            detect_signature_kind(ssh),
            Some(SignatureKind::Ssh)
        ));
        let unsigned = "tree 1111\ncommitter x <x@y> 0 +0000\n\nmessage\n";
        assert_eq!(detect_signature_kind(unsigned), None);
    }

    #[test]
    fn not_verifiable_reason_matches_tool_wording() {
        // Missing GPG key: not verifiable, and NOT a bad signature.
        let out = "gpg: can't check signature: No public key\nerror: could not verify the tag 'x'";
        let reason = not_verifiable_reason(out).expect("no public key is not-verifiable");
        assert!(reason.contains("No public key"), "{reason}");
        // SSH without allowed_signers: same class.
        let out = "error: gpg.ssh.allowedSignersFile needs to be configured and exist for ssh signature verification";
        assert!(not_verifiable_reason(out).is_some());
        // Anything else (BAD signature) must NOT be downgraded to
        // not-verifiable.
        assert_eq!(
            not_verifiable_reason("gpg: BAD signature from Tester"),
            None
        );
        assert_eq!(not_verifiable_reason(""), None);
    }

    // ---- end-to-end through the engine ------------------------------------

    #[test]
    fn unsigned_commit_reports_unsigned() {
        let (repo, _dir) = init_repo("unsigned");
        let (ok, out) = git(
            repo.workdir().unwrap(),
            &["commit", "--allow-empty", "-m", "plain"],
        );
        assert!(ok, "git commit failed: {out}");
        let sig = ENGINE
            .commit_signature_impl(&repo, &head_sha(&repo))
            .expect("commit_signature");
        assert!(!sig.signed);
        assert_eq!(sig.kind, None);
        assert_eq!(sig.valid, None);
        assert_eq!(sig.detail, "unsigned");

        // Same answer through the trait method (trash.rs forwarder).
        let sig = GitEngineM12::commit_signature(&ENGINE, &repo, &head_sha(&repo))
            .expect("trait commit_signature");
        assert!(!sig.signed);
        assert_eq!(sig.detail, "unsigned");
    }

    #[test]
    fn unknown_sha_is_invalid() {
        let (repo, _dir) = init_repo("bad-sha");
        match ENGINE.commit_signature_impl(&repo, "deadbeef") {
            Err(EngineError::Invalid(msg)) => assert!(msg.contains("cannot resolve"), "{msg}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// ssh-keygen availability probe (it ships with git; skip the SSH
    /// end-to-end test where it is missing).
    fn ssh_keygen_available() -> bool {
        Command::new("ssh-keygen")
            .arg("-h")
            .output()
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false)
    }

    /// Create an SSH-signing fixture: ed25519 key pair + repo-local config
    /// (gpg.format=ssh, commit.gpgsign=true) + an allowed_signers file that
    /// trusts `signer@test.local` (the repo's committer email). Returns the
    /// allowed_signers path so tests can revoke it. Key material lives in an
    /// untracked `keys/` subdir of the throwaway worktree (nothing in these
    /// tests asserts status), so it is cleaned up with the repo.
    fn setup_ssh_signing(repo: &Repository, name: &str) -> PathBuf {
        let dir = repo.workdir().unwrap().join("keys");
        std::fs::create_dir_all(&dir).expect("mkdir keys");
        let key = dir.join(format!("signing-{name}-key"));
        let pub_key = dir.join(format!("signing-{name}-key.pub"));
        let mut cmd = Command::new("ssh-keygen");
        cmd.arg("-q")
            .arg("-t")
            .arg("ed25519")
            .arg("-N")
            .arg("")
            .arg("-C")
            .arg("signer@test.local")
            .arg("-f")
            .arg(&key);
        crate::cli::apply_sanitized_env(&mut cmd);
        let out = cmd.output().expect("run ssh-keygen");
        assert!(out.status.success(), "ssh-keygen failed: {:?}", out.stderr);

        let pub_text = std::fs::read_to_string(&pub_key).expect("read pub key");
        let signers = dir.join(format!("signing-{name}-signers"));
        std::fs::write(&signers, format!("signer@test.local {pub_text}")).expect("write signers");

        // NOTE: paths stay as-is (already absolute via temp_dir) —
        // `canonicalize` would hand git `\\?\` verbatim paths it rejects, and
        // `user.signingkey` must name the PUBLIC key (or its file), never the
        // private half.
        let mut repo_cfg = repo.config().expect("config");
        repo_cfg.set_str("gpg.format", "ssh").unwrap();
        repo_cfg
            .set_str("user.signingkey", pub_key.to_string_lossy().as_ref())
            .unwrap();
        repo_cfg.set_bool("commit.gpgsign", true).unwrap();
        repo_cfg
            .set_str(
                "gpg.ssh.allowedSignersFile",
                signers.to_string_lossy().as_ref(),
            )
            .unwrap();
        signers
    }

    #[test]
    fn ssh_signed_commit_verifies() {
        if !ssh_keygen_available() {
            eprintln!("skipping ssh_signed_commit_verifies: ssh-keygen not available");
            return;
        }
        let (repo, _dir) = init_repo("ssh-ok");
        setup_ssh_signing(&repo, "ok");
        let (ok, out) = git(
            repo.workdir().unwrap(),
            &["commit", "--allow-empty", "-m", "signed via ssh"],
        );
        assert!(ok, "signed commit failed: {out}");

        let sig = ENGINE
            .commit_signature_impl(&repo, &head_sha(&repo))
            .expect("commit_signature");
        assert!(sig.signed);
        assert_eq!(sig.kind, Some(SignatureKind::Ssh));
        assert_eq!(sig.valid, Some(true), "detail: {}", sig.detail);
        assert!(!sig.detail.is_empty(), "stdout of verify-commit is kept");
    }

    #[test]
    fn ssh_signed_commit_without_matching_signer_is_not_verified() {
        if !ssh_keygen_available() {
            eprintln!(
                "skipping ssh_signed_commit_without_matching_signer: ssh-keygen not available"
            );
            return;
        }
        let (repo, _dir) = init_repo("ssh-empty-signers");
        let signers = setup_ssh_signing(&repo, "revoked");
        // Revoke trust: an allowed_signers file with no matching principal.
        std::fs::write(&signers, "").expect("truncate signers");
        let (ok, out) = git(
            repo.workdir().unwrap(),
            &["commit", "--allow-empty", "-m", "signed but untrusted"],
        );
        assert!(ok, "signed commit failed: {out}");

        let sig = ENGINE
            .commit_signature_impl(&repo, &head_sha(&repo))
            .expect("commit_signature");
        assert!(sig.signed);
        assert_eq!(sig.kind, Some(SignatureKind::Ssh));
        // DOCUMENTED BEHAVIOR: which side of "not verifiable here" (None) vs
        // "bad" (Some(false)) a signature that fails trust lookup lands on
        // depends on the git/gpg.ssh wording of the host toolchain — both are
        // accepted here; the invariant is that it is NEVER Some(true), and
        // the detail always carries the tool output for the tooltip.
        assert_ne!(sig.valid, Some(true), "detail: {}", sig.detail);
        assert!(!sig.detail.is_empty(), "failure detail is preserved");
    }
}
