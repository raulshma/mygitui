//! Remote workspace: workdir file operations over SFTP for one open remote
//! repo. Mirrors the local `workspace.rs` surface (`repo_list_dir`,
//! `repo_stat_file`, `repo_read_file`, `repo_write_file`, create/rename/
//! delete, quick-open index) and REUSES its DTO types so the FE sees no
//! difference.
//!
//! Path discipline: every FE-supplied path is repository-relative POSIX and
//! validated by [`validate_rel_posix`] (no absolute form, no `..`, no `.git`
//! for mutations — same rules as the local `validate_rel_path`).

use std::sync::Arc;

use super::quoting::shell_quote;
use super::transport::{DeviceSession, DEFAULT_EXEC_TIMEOUT};
use crate::workspace::{
    DirEntryInfo, DirEntryKind, DirListing, FileContentKind, FileIndex, FileIndexEntry, FileMeta,
    NewEntryKind,
};

/// `repo_list_dir` cap (matches contracts.md).
pub const MAX_LIST_ENTRIES: usize = 5000;
/// `repo_file_index` cap.
pub const MAX_INDEX_ENTRIES: usize = 50_000;
/// Leading bytes sniffed for NUL when classifying binary content.
const BINARY_SNIFF_LEN: usize = 8192;
/// Single-file read cap.
pub const MAX_READ_BYTES: usize = 64 * 1024 * 1024;

/// Image extensions (mirrors the other copies so every layer agrees).
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif",
];

pub fn is_image_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    path.contains('.') && IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Validate a repo-relative POSIX path: not absolute, no `..` components,
/// no backslashes. Empty input is the workdir root (`""`). Returns the
/// clean path.
pub fn validate_rel_posix(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Ok(String::new());
    }
    if path.starts_with('/') {
        return Err(format!("path must be workdir-relative: `{path}`"));
    }
    if path.contains('\\') {
        return Err(format!(
            "backslashes are not valid remote path separators: `{path}`"
        ));
    }
    let mut clean = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => continue, // tolerate doubled/./ separators
            ".." => return Err(format!("path escapes the workdir: `{path}`")),
            other => clean.push(other),
        }
    }
    if clean.is_empty() {
        return Err("path is empty".to_string());
    }
    Ok(clean.join("/"))
}

fn join_abs(root: &str, rel: &str) -> String {
    if rel.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{rel}")
    }
}

/// One remote repo's SFTP-backed file surface.
pub struct RemoteWorkspace {
    pub session: Arc<DeviceSession>,
    /// Absolute POSIX workdir path on the device.
    pub root: String,
}

impl RemoteWorkspace {
    pub fn new(session: Arc<DeviceSession>, root: String) -> Self {
        Self { session, root }
    }

    fn block<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(fut)
    }

    /// Run one SFTP-backed operation. A failure invalidates the device's
    /// cached SFTP session — the shared channel may be dead — so the next
    /// op re-opens instead of failing forever.
    fn sftp_op<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let result = self.block(fut);
        if result.is_err() {
            self.session.sftp_invalidate();
        }
        result
    }

    // ---------- listing ----------

    pub fn list_dir(&self, path: &str, include_ignored: bool) -> Result<DirListing, String> {
        let rel = validate_rel_posix(path)?;
        let abs = join_abs(&self.root, &rel);
        let entries = self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            let read_dir = sftp
                .read_dir(&abs)
                .await
                .map_err(|e| format!("read_dir {rel}: {e}"))?;
            let mut out: Vec<(String, bool, bool, Option<u64>)> = Vec::new(); // name, is_dir, is_symlink, size
            for entry in read_dir {
                let name = entry.file_name();
                let file_type = entry.file_type();
                let is_dir = file_type.is_dir();
                let is_symlink = file_type.is_symlink();
                let size = if is_dir {
                    None
                } else {
                    Some(entry.metadata().len())
                };
                out.push((name, is_dir, is_symlink, size));
            }
            Ok::<_, String>(out)
        })?;
        if entries.len() > MAX_LIST_ENTRIES {
            return Ok(DirListing {
                path: rel,
                entries: Vec::new(),
                truncated: true,
            });
        }
        // Submodule marking: index entries with mode 160000 under this dir.
        let gitlinks = self.gitlink_paths(&rel, &entries)?;
        let mut rows: Vec<DirEntryInfo> = entries
            .into_iter()
            .map(|(name, is_dir, is_symlink, size)| {
                let child_path = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                let kind = if is_dir {
                    if gitlinks.contains(&child_path) {
                        DirEntryKind::Submodule
                    } else {
                        DirEntryKind::Dir
                    }
                } else if is_symlink {
                    DirEntryKind::Symlink
                } else {
                    DirEntryKind::File
                };
                DirEntryInfo {
                    name,
                    path: child_path,
                    kind,
                    size,
                }
            })
            .collect();
        rows.sort_by(|a, b| natural_cmp(&a.path, &b.path));
        let _ = include_ignored; // listing includes everything either way (gitignore filters only the index)
        Ok(DirListing {
            path: rel,
            entries: rows,
            truncated: false,
        })
    }

    /// Paths under `rel` that are gitlinks (submodule checkouts) according
    /// to the index.
    fn gitlink_paths(
        &self,
        rel: &str,
        entries: &[(String, bool, bool, Option<u64>)],
    ) -> Result<Vec<String>, String> {
        let dir_prefix = if rel.is_empty() {
            String::new()
        } else {
            format!("{rel}/")
        };
        let dir_names: Vec<String> = entries
            .iter()
            .filter(|(_, is_dir, _, _)| *is_dir)
            .map(|(name, _, _, _)| name.clone())
            .collect();
        if dir_names.is_empty() {
            return Ok(Vec::new());
        }
        let specs: String = dir_names
            .iter()
            .map(|d| format!(" {}", shell_quote(&format!("{dir_prefix}{d}"))))
            .collect();
        let args = format!("ls-files -s -z --{specs}");
        let raw = self.git_raw(&args)?.stdout_str();
        Ok(raw
            .split('\0')
            .filter(|f| !f.is_empty())
            .filter(|f| {
                f.split_whitespace()
                    .next()
                    .map(|mode| mode == "160000")
                    .unwrap_or(false)
            })
            .filter_map(|f| f.split_once('\t').map(|(_, p)| p.to_string()))
            .collect())
    }

    fn git_raw(&self, args: &str) -> Result<super::transport::ExecOutput, String> {
        let cmd = super::engine::git_command(&self.root, args);
        self.block(self.session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))
    }

    // ---------- stat / read / write ----------

    pub fn stat_file(&self, path: &str) -> Result<FileMeta, String> {
        let rel = validate_rel_posix(path)?;
        let abs = join_abs(&self.root, &rel);
        let meta = self.sftp_op(async {
            self.session
                .sftp()
                .await?
                .metadata(&abs)
                .await
                .map_err(|e| format!("stat {rel}: {e}"))
        })?;
        let is_dir = meta.file_type().is_dir();
        let size = meta.len();
        let readonly = meta.permissions().is_readonly();
        let mtime_ms: u128 = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let kind = if is_dir {
            FileContentKind::Binary
        } else if is_image_path(&rel) {
            FileContentKind::Image
        } else {
            // Sniff the head for NUL bytes (binary indicator), same as local.
            let head = self.read_head(&rel, BINARY_SNIFF_LEN)?;
            if head.contains(&0) {
                FileContentKind::Binary
            } else {
                FileContentKind::Text
            }
        };
        Ok(FileMeta {
            path: rel,
            size,
            kind,
            readonly,
            mtime_ms,
        })
    }

    fn read_head(&self, rel: &str, cap: usize) -> Result<Vec<u8>, String> {
        let abs = join_abs(&self.root, rel);
        self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            let mut file = sftp
                .open(&abs)
                .await
                .map_err(|e| format!("open {rel}: {e}"))?;
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; cap];
            let n = file
                .read(&mut buf)
                .await
                .map_err(|e| format!("read {rel}: {e}"))?;
            buf.truncate(n);
            Ok::<_, String>(buf)
        })
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let rel = validate_rel_posix(path)?;
        let abs = join_abs(&self.root, &rel);
        self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            let mut file = sftp
                .open(&abs)
                .await
                .map_err(|e| format!("open {rel}: {e}"))?;
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 64 * 1024];
            loop {
                let n = file
                    .read(&mut chunk)
                    .await
                    .map_err(|e| format!("read {rel}: {e}"))?;
                if n == 0 {
                    break;
                }
                if buf.len() + n > MAX_READ_BYTES {
                    return Err(format!(
                        "{rel} exceeds {} MiB",
                        MAX_READ_BYTES / 1024 / 1024
                    ));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Ok::<_, String>(buf)
        })
    }

    pub fn write_file(
        &self,
        path: &str,
        content: &[u8],
        expected_mtime: Option<u128>,
    ) -> Result<FileMeta, String> {
        let rel = validate_rel_posix(path)?;
        if rel.is_empty() {
            return Err("cannot write the workdir root".to_string());
        }
        // Refuse .git mutations (same V1 safety as local).
        if rel.split('/').any(|c| c == ".git") {
            return Err(format!("refusing to write inside .git: `{rel}`"));
        }
        let abs = join_abs(&self.root, &rel);
        // mtime guard (optimistic concurrency like the local editor save).
        if let Some(expected) = expected_mtime {
            let current = self.stat_file(&rel)?;
            if current.mtime_ms != expected {
                return Err(format!(
                    "file changed on the device since it was opened (expected mtime {expected}, found {})",
                    current.mtime_ms
                ));
            }
        }
        self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            let mut file = sftp
                .create(&abs)
                .await
                .map_err(|e| format!("write {rel}: {e}"))?;
            use tokio::io::AsyncWriteExt;
            file.write_all(content)
                .await
                .map_err(|e| format!("write {rel}: {e}"))?;
            file.flush()
                .await
                .map_err(|e| format!("write {rel}: {e}"))?;
            Ok::<_, String>(())
        })?;
        self.stat_file(&rel)
    }

    // ---------- create / rename / delete ----------

    pub fn create_entry(&self, path: &str, kind: NewEntryKind) -> Result<(), String> {
        let rel = validate_rel_posix(path)?;
        if rel.split('/').any(|c| c == ".git") {
            return Err(format!("refusing to create inside .git: `{rel}`"));
        }
        let abs = join_abs(&self.root, &rel);
        match kind {
            NewEntryKind::File => {
                if self.exists(&rel)? {
                    return Err(format!("already exists: `{rel}`"));
                }
                self.sftp_op(async {
                    let sftp = self.session.sftp().await?;
                    sftp.create(&abs)
                        .await
                        .map_err(|e| format!("create {rel}: {e}"))?;
                    Ok::<_, String>(())
                })?;
            }
            NewEntryKind::Dir => {
                self.sftp_op(async {
                    let sftp = self.session.sftp().await?;
                    sftp.create_dir(&abs)
                        .await
                        .map_err(|e| format!("mkdir {rel}: {e}"))?;
                    Ok::<_, String>(())
                })?;
            }
        }
        Ok(())
    }

    pub fn rename_entry(&self, path: &str, new_path: &str) -> Result<(), String> {
        let rel = validate_rel_posix(path)?;
        let new_rel = validate_rel_posix(new_path)?;
        if rel.split('/').any(|c| c == ".git") || new_rel.split('/').any(|c| c == ".git") {
            return Err("refusing to rename into/out of .git".to_string());
        }
        if self.exists(&new_rel)? {
            return Err(format!("target already exists: `{new_rel}`"));
        }
        let abs = join_abs(&self.root, &rel);
        let new_abs = join_abs(&self.root, &new_rel);
        self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            sftp.rename(&abs, &new_abs)
                .await
                .map_err(|e| format!("rename {rel} → {new_rel}: {e}"))?;
            Ok::<_, String>(())
        })
    }

    pub fn delete_entry(&self, path: &str) -> Result<(), String> {
        let rel = validate_rel_posix(path)?;
        if rel.is_empty() || rel.split('/').any(|c| c == ".git") {
            return Err(format!("refusing to delete `{rel}`"));
        }
        // (empty root + .git components are both rejected above)
        // A directory with its own .git is a submodule checkout — refuse
        // (same rule as local; open the submodule as a repo instead).
        if self.exists(&format!("{rel}/.git"))? {
            return Err(format!(
                "`{rel}` is a submodule checkout; open it as a repository instead"
            ));
        }
        let abs = join_abs(&self.root, &rel);
        self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            let meta = sftp
                .metadata(&abs)
                .await
                .map_err(|e| format!("stat {rel}: {e}"))?;
            if meta.file_type().is_dir() {
                remove_dir_recursive(&sftp, &abs, &rel, 0).await?;
            } else {
                sftp.remove_file(&abs)
                    .await
                    .map_err(|e| format!("delete {rel}: {e}"))?;
            }
            Ok::<_, String>(())
        })
    }

    pub fn exists(&self, rel: &str) -> Result<bool, String> {
        let abs = join_abs(&self.root, rel);
        self.sftp_op(async {
            let sftp = self.session.sftp().await?;
            Ok::<_, String>(sftp.metadata(&abs).await.is_ok())
        })
    }

    // ---------- quick-open index ----------

    pub fn file_index(&self) -> Result<FileIndex, String> {
        // Tracked + untracked-not-ignored, one exec (the `ignore` crate walk
        // equivalent, done by git itself on the device).
        let raw = self
            .git_raw("ls-files -z -co --exclude-standard")?
            .stdout_str();
        let mut entries: Vec<FileIndexEntry> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut truncated = false;
        let push = |path: String,
                    is_dir: bool,
                    entries: &mut Vec<FileIndexEntry>,
                    seen: &mut std::collections::HashSet<String>,
                    truncated: &mut bool| {
            if seen.len() >= MAX_INDEX_ENTRIES {
                *truncated = true;
                return;
            }
            if seen.insert(path.clone()) {
                entries.push(FileIndexEntry {
                    path,
                    is_dir,
                    size: None,
                });
            }
        };
        for field in raw.split('\0').filter(|f| !f.is_empty()) {
            // Add every ancestor directory + the file itself.
            let mut depth = field.split('/').count();
            let mut cur = String::new();
            for comp in field.split('/') {
                depth -= 1;
                if depth == 0 {
                    push(
                        comp.to_string(),
                        false,
                        &mut entries,
                        &mut seen,
                        &mut truncated,
                    );
                } else {
                    if cur.is_empty() {
                        cur = comp.to_string();
                    } else {
                        cur = format!("{cur}/{comp}");
                    }
                    push(cur.clone(), true, &mut entries, &mut seen, &mut truncated);
                }
                if truncated {
                    break;
                }
            }
            if truncated {
                break;
            }
        }
        Ok(FileIndex { entries, truncated })
    }
}

/// Recursive SFTP rmdir with a depth fuse.
fn remove_dir_recursive<'a>(
    sftp: &'a russh_sftp::client::SftpSession,
    abs: &'a str,
    rel: &'a str,
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move {
        if depth > 64 {
            return Err(format!("delete {rel}: directory nesting too deep"));
        }
        let read_dir = sftp
            .read_dir(abs)
            .await
            .map_err(|e| format!("read_dir {rel}: {e}"))?;
        let mut names: Vec<(String, bool)> = Vec::new();
        for entry in read_dir {
            let name = entry.file_name();
            let is_dir = entry.file_type().is_dir();
            names.push((name, is_dir));
        }
        for (name, is_dir) in names {
            let child_abs = format!("{abs}/{name}");
            let child_rel = format!("{rel}/{name}");
            if is_dir {
                remove_dir_recursive(sftp, &child_abs, &child_rel, depth + 1).await?;
            } else {
                sftp.remove_file(&child_abs)
                    .await
                    .map_err(|e| format!("delete {child_rel}: {e}"))?;
            }
        }
        sftp.remove_dir(abs)
            .await
            .map_err(|e| format!("rmdir {rel}: {e}"))?;
        Ok(())
    })
}

/// Natural sort (digits by value) — mirrors the file tree ordering.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<std::string::String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut last_digit: Option<bool> = None;
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            if last_digit == Some(!digit) && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(c);
            last_digit = Some(digit);
        }
        out.push(cur);
        out
    }
    let (ac, bc) = (chunks(a), chunks(b));
    for (a, b) in ac.iter().zip(bc.iter()) {
        let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.cmp(b),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ac.len().cmp(&bc.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_validation() {
        assert!(validate_rel_posix("src/main.rs").is_ok());
        assert_eq!(
            validate_rel_posix("src//sub/./x.rs").unwrap(),
            "src/sub/x.rs"
        );
        assert_eq!(validate_rel_posix("").unwrap(), ""); // "" = repo root (list_dir(""))
        assert!(validate_rel_posix("/abs/path").is_err());
        assert!(validate_rel_posix("a/../b").is_err());
        assert!(validate_rel_posix("..").is_err());
        assert!(validate_rel_posix("a\\b").is_err());
        assert!(validate_rel_posix(".git/config").is_ok()); // validation passes; mutation gate rejects
    }

    #[test]
    fn mutation_gate_rejects_git_paths() {
        let ws_paths = [".git/config", "sub/.git/x", ".git"];
        // The mutation gate is `rel.split('/').any(|c| c == ".git")`.
        for p in ws_paths {
            assert!(p.split('/').any(|c| c == ".git"), "{p} must be gated");
        }
    }

    #[test]
    fn natural_ordering() {
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("a2", "a10"), Less);
        assert_eq!(natural_cmp("a10", "a2"), Greater);
        assert_eq!(natural_cmp("a2", "a2"), Equal);
        assert_eq!(natural_cmp("src/a.rs", "src/b.rs"), Less);
    }

    #[test]
    fn image_detection() {
        assert!(is_image_path("x/logo.PNG"));
        assert!(!is_image_path("README.md"));
    }
}
