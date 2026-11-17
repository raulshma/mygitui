//! Workdir-relative path validation shared by the engine's filesystem
//! writers and the workspace IPC layer: every IPC-supplied path joined
//! against the repository root must be relative, must not escape the root,
//! and (on Windows) must carry no drive/UNC prefix.

use std::path::{Path, PathBuf};

use super::git_engine::{EngineError, EngineResult};

/// Validate a workdir-relative path (no absolute form, no `..`, no Windows
/// prefixes, must stay inside `root`) and return the joined absolute path.
/// Single source of truth: the workspace layer's mutating commands and the
/// engine's workdir writers both call this so the rules cannot diverge.
pub(crate) fn validate_rel_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("path is empty".into());
    }
    let relative = Path::new(path);
    if relative.is_absolute() || has_windows_prefix(relative) {
        return Err(format!("path must be workdir-relative: `{path}`"));
    }
    if relative
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(format!("path must not contain `..`: `{path}`"));
    }
    let full = root.join(relative);
    // Belt and braces: a drive-relative form like `C:foo` slips past the
    // absolute check but `join` replaces the base with it — the containment
    // check catches every replacement case.
    if full.strip_prefix(root).is_err() {
        return Err(format!("path escapes the repository: `{path}`"));
    }
    Ok(full)
}

/// Engine-facing wrapper for the workdir writers: same rules, mapped onto
/// [`EngineError::Invalid`].
pub(crate) fn validate_workdir_path(root: &Path, path: &str) -> EngineResult<PathBuf> {
    validate_rel_path(root, path).map_err(EngineError::Invalid)
}

/// True when `path` carries a Windows prefix (`C:\`, `\\?\C:\`, `\\server`).
pub(crate) fn has_windows_prefix(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        matches!(
            path.components().next(),
            Some(Component::Prefix(_)) | Some(Component::RootDir)
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::validate_rel_path;

    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mygitui-paths-{}", std::process::id()))
    }

    #[test]
    fn plain_relative_path_passes() {
        let joined = validate_rel_path(&root(), "src/lib/main.rs").expect("valid");
        assert_eq!(joined, root().join("src/lib/main.rs"));
    }

    #[test]
    fn empty_path_is_rejected() {
        assert!(validate_rel_path(&root(), "").is_err());
    }

    #[test]
    fn absolute_path_is_rejected() {
        assert!(validate_rel_path(&root(), "/etc/passwd").is_err());
    }

    #[test]
    fn parent_components_are_rejected() {
        assert!(validate_rel_path(&root(), "../outside").is_err());
        assert!(validate_rel_path(&root(), "foo/../../bar").is_err());
        assert!(validate_rel_path(&root(), "foo/./bar/../baz").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_and_unc_forms_are_rejected() {
        use std::path::Path;
        let root = Path::new(r"C:\repo");
        for bad in ["C:\\evil", "C:evil", "\\\\server\\share\\x", "\\repo\\x"] {
            assert!(validate_rel_path(root, bad).is_err(), "{bad}");
        }
    }
}
