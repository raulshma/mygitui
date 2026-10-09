//! External mergetool + signed-tag support via the real `git` CLI.
//!
//! Same rules as `cli.rs`: system `git`, `cwd` = repo workdir, sanitized
//! environment ([`crate::cli::sanitize_env`]), no shell anywhere. The
//! built-in 3-way editor stays the default conflict UI; launching the
//! user's configured tool (`merge.tool` / `merge.guitool`) is opt-in per
//! repo. `git mergetool --no-prompt` owns markers/backup handling and
//! honors `mergetool.<tool>.trustExitCode`; mygitui never guesses whether
//! the file resolved — the FE re-reads `conflicts()` after the tool exits
//! and offers to stage the workdir bytes via `conflict_resolve`.

use std::path::Path;
use std::process::Command;

use git2::Repository;

use crate::engine::types::{MergetoolInfo, MergetoolResult};

/// Read `merge.tool` / `merge.guitool` plus the resolved tool's
/// `mergetool.<tool>.path` / `.cmd` from the repo+global config (pure
/// introspection; no process spawned).
///
/// Resolution follows `git mergetool --gui` semantics: `merge.guitool` wins
/// (the FE conflict dialog launches the GUI tool), falling back to
/// `merge.tool`; `path`/`cmd` are read for that effective tool and stay
/// `None` when unset.
pub fn mergetool_info(repo: &Repository) -> MergetoolInfo {
    let config = repo.config().ok();
    let get = |key: &str| -> Option<String> {
        config
            .as_ref()
            .and_then(|c| c.get_string(key).ok())
            .filter(|v| !v.trim().is_empty())
    };
    let tool = get("merge.tool");
    let gui_tool = get("merge.guitool");
    let effective = gui_tool.clone().or_else(|| tool.clone());
    let (path, cmd) = match effective {
        Some(tool) => (
            get(&format!("mergetool.{tool}.path")),
            get(&format!("mergetool.{tool}.cmd")),
        ),
        None => (None, None),
    };
    MergetoolInfo {
        tool,
        gui_tool,
        path,
        cmd,
    }
}

/// Launch `git mergetool` for one conflicted path. `tool` overrides
/// `merge.tool` for this run (`git mergetool -t <tool>`).
pub fn mergetool_run(
    workdir: &Path,
    path: &str,
    tool: Option<&str>,
) -> Result<MergetoolResult, String> {
    let mut args = vec!["mergetool".to_string(), "--no-prompt".to_string()];
    if let Some(tool) = tool.filter(|t| !t.trim().is_empty()) {
        args.push("--tool".to_string());
        args.push(tool.to_string());
    }
    args.push("--".to_string());
    args.push(path.to_string());

    let mut cmd = Command::new("git");
    cmd.current_dir(workdir).args(&args);
    crate::cli::apply_sanitized_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|err| format!("failed to spawn git mergetool: {err}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    Ok(MergetoolResult {
        success: output.status.success(),
        output: text,
    })
}

/// Create a signed annotated tag via `git tag -s` (libgit2 cannot sign).
/// The message travels via `-F` temp file (quoting-proof), same as commit.
pub fn tag_sign_via_cli(
    workdir: &Path,
    name: &str,
    target: Option<&str>,
    message: &str,
) -> Result<(), String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file = std::env::temp_dir().join(format!("mygitui-tag-{}-{nanos}.msg", std::process::id()));
    std::fs::write(&file, message)
        .map_err(|err| format!("cannot write tag message file: {err}"))?;
    let result = (|| {
        let mut args = vec![
            "tag".to_string(),
            "-s".to_string(),
            "-F".to_string(),
            file.to_string_lossy().into_owned(),
            name.to_string(),
        ];
        if let Some(target) = target.filter(|t| !t.trim().is_empty()) {
            args.push(target.to_string());
        }
        let mut cmd = Command::new("git");
        cmd.current_dir(workdir).args(&args);
        crate::cli::apply_sanitized_env(&mut cmd);
        let output = cmd
            .output()
            .map_err(|err| format!("failed to spawn git tag: {err}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!("git tag -s failed: {detail}"));
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mergetool_info_reads_config() {
        let dir = std::env::temp_dir().join(format!("mygitui-mt-info-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        let info = mergetool_info(&repo);
        assert_eq!(info.tool, None);
        assert_eq!(info.gui_tool, None);
        assert_eq!(info.path, None, "no tool resolved ⇒ no path/cmd");
        assert_eq!(info.cmd, None);
        repo.config()
            .unwrap()
            .set_str("merge.tool", "kdiff3")
            .unwrap();
        let info = mergetool_info(&repo);
        assert_eq!(info.tool.as_deref(), Some("kdiff3"));
        drop(repo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mergetool_info_reads_path_and_cmd_of_resolved_tool() {
        let dir = std::env::temp_dir().join(format!("mygitui-mt-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("merge.tool", "kdiff3").unwrap();
            cfg.set_str("mergetool.kdiff3.path", "/usr/bin/kdiff3")
                .unwrap();
            cfg.set_str("mergetool.kdiff3.cmd", "\"$base\" \"$local\" \"$remote\"")
                .unwrap();
        }
        let info = mergetool_info(&repo);
        assert_eq!(info.tool.as_deref(), Some("kdiff3"));
        assert_eq!(info.path.as_deref(), Some("/usr/bin/kdiff3"));
        assert_eq!(
            info.cmd.as_deref(),
            Some("\"$base\" \"$local\" \"$remote\"")
        );

        // An unrelated tool's mergetool.* entries are ignored.
        repo.config()
            .unwrap()
            .set_str("mergetool.vimdiff.path", "/usr/bin/vim")
            .unwrap();
        let info = mergetool_info(&repo);
        assert_eq!(info.path.as_deref(), Some("/usr/bin/kdiff3"));
        drop(repo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mergetool_info_prefers_guitool_for_resolution() {
        let dir = std::env::temp_dir().join(format!("mygitui-mt-gui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("merge.tool", "kdiff3").unwrap();
            cfg.set_str("merge.guitool", "mgui").unwrap();
            cfg.set_str("mergetool.kdiff3.path", "/kdiff3-path")
                .unwrap();
            cfg.set_str("mergetool.mgui.cmd", "mgui --do-it").unwrap();
        }
        let info = mergetool_info(&repo);
        assert_eq!(info.gui_tool.as_deref(), Some("mgui"));
        // Effective tool = guitool ⇒ its cmd wins, kdiff3's path is ignored.
        assert_eq!(info.path, None);
        assert_eq!(info.cmd.as_deref(), Some("mgui --do-it"));
        drop(repo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mergetool_run_missing_tool_errors() {
        // `--no-prompt` with an unknown tool fails fast; the error text names
        // the tool. Requires git on PATH (CI guarantees it).
        let dir = std::env::temp_dir().join(format!("mygitui-mt-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let result =
            mergetool_run(&dir, "some.txt", Some("definitely-not-a-tool")).expect("spawn works");
        assert!(!result.success);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
