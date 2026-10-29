//! Workdir filesystem operations for the file browser (Files panel,
//! viewer/editor, quick-open).
//!
//! Reads are pure filesystem work: they never take the repo mutex and run
//! on the blocking pool exactly like [`crate::ipc_commands::repo_read_file`]
//! — the one exception is [`repo_list_dir`], which reads the gitlink
//! entries from the index (submodule marking) under the repo lock through
//! the serial queue. Mutations go through [`enqueue_fs_mutation`]: the
//! per-repo op queue serializes them with every other mutation and bumps
//! the generation, so the existing watcher → `repo-changed` →
//! `refreshStatus` pipeline refreshes the UI for free.
//!
//! Every path argument is workdir-relative and validated by
//! [`validate_rel_path`] (rejects absolute forms, `..` components, Windows
//! prefixes, and anything escaping the root — the same rules
//! `read_workdir_file` has always enforced). Mutating ops additionally
//! refuse paths inside `.git` (V1 safety: the metadata store is not
//! user-editable) and deleting a submodule checkout (a dir with its own
//! `.git`) — open the submodule as a repo instead.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use crate::engine::types::RepoId;
use crate::repo::{RepoHandle, RepoManager};

/// Entries per `repo_list_dir` response (contracts.md).
const MAX_LIST_ENTRIES: usize = 5000;
/// Entries per `repo_file_index` response (contracts.md).
const MAX_INDEX_ENTRIES: usize = 50_000;
/// Leading bytes sniffed for NUL when classifying binary content.
const BINARY_SNIFF_LEN: usize = 8192;

/// File extensions treated as images (mirrors `IMAGE_EXTENSIONS` in the
/// diff engine so the browser and the diff viewer agree).
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif",
];

// -- IPC types (contracts.md "Commands (workspace files)") --------------------

/// One row of a directory listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DirEntryKind {
    File,
    Dir,
    /// Symlink (not followed by the browser in V1).
    Symlink,
    /// Directory that is a git submodule checkout (gitlink in the index).
    Submodule,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DirEntryInfo {
    pub name: String,
    /// Repository-relative path, forward slashes.
    pub path: String,
    pub kind: DirEntryKind,
    /// File size in bytes; `null` for dirs.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DirListing {
    /// Repository-relative directory path ("" = workdir root).
    pub path: String,
    pub entries: Vec<DirEntryInfo>,
    /// True when the listing hit [`MAX_LIST_ENTRIES`] and entries are
    /// missing (error-free truncation).
    pub truncated: bool,
}

/// Content classification for one file (`repo_stat_file`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FileContentKind {
    Text,
    Image,
    Binary,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileMeta {
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    pub kind: FileContentKind,
    pub readonly: bool,
    /// Modification time, milliseconds since the UNIX epoch (0 when the
    /// mtime predates the epoch — saves compare this against their copy).
    pub mtime_ms: u128,
}

/// One row of the quick-open index (gitignore-respecting walk).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileIndexEntry {
    /// Repository-relative path, forward slashes.
    pub path: String,
    pub is_dir: bool,
    /// Sizes are not collected during the walk (a stat per file would
    /// dominate the walk time); always `null` in V1.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileIndex {
    pub entries: Vec<FileIndexEntry>,
    /// True when the walk hit [`MAX_INDEX_ENTRIES`] (huge repos stay
    /// responsive; the quick-open still searches what was collected).
    pub truncated: bool,
}

/// Kind of entry to create (`repo_create_entry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum NewEntryKind {
    File,
    Dir,
}

// -- Path validation -----------------------------------------------------------

/// Validate a workdir-relative path (no absolute form, no `..`, no Windows
/// prefixes, must stay inside `root`) and return the joined absolute path.
/// Shared by every workspace command and `read_workdir_file`.
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

/// Mutating ops refuse to touch `.git` internals: any path with a `.git`
/// component is metadata, not user content.
fn contains_git_component(path: &str) -> bool {
    path.split('/').any(|component| component == ".git")
}

/// Repository-relative string form of an absolute path under `root`
/// (forward slashes; the browser's path vocabulary everywhere).
fn rel_string(full: &Path, root: &Path) -> String {
    match full.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => String::new(),
    }
}

// -- Sorting --------------------------------------------------------------------

/// Case-insensitive natural (digit-aware) string compare: `file2` sorts
/// before `file10`, mirroring what users expect from a file tree.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.char_indices().peekable();
    let mut bi = b.char_indices().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some((_, ca)), Some((_, cb))) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    // Whole numeric run on each side, compared as numbers
                    // (leading zeros: fewer digits after trimming wins ties).
                    let a_num = take_digits(&mut ai);
                    let b_num = take_digits(&mut bi);
                    let a_trim = a_num.trim_start_matches('0');
                    let b_trim = b_num.trim_start_matches('0');
                    let ord = a_trim
                        .len()
                        .cmp(&b_trim.len())
                        .then_with(|| a_trim.cmp(b_trim));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    let ord = ca
                        .to_lowercase()
                        .cmp(cb.to_lowercase())
                        // Stable tiebreak for names differing only in case.
                        .then_with(|| ca.cmp(&cb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

/// Consume the peeked digit run, returning it as a string.
fn take_digits(
    iter: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> String {
    let mut out = String::new();
    while let Some((_, c)) = iter.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        out.push(c);
        iter.next();
    }
    out
}

/// Dirs (incl. submodule checkouts) first, then natural order by name.
pub(crate) fn compare_entries(a: &DirEntryInfo, b: &DirEntryInfo) -> Ordering {
    let a_dir = matches!(a.kind, DirEntryKind::Dir | DirEntryKind::Submodule);
    let b_dir = matches!(b.kind, DirEntryKind::Dir | DirEntryKind::Submodule);
    b_dir.cmp(&a_dir).then_with(|| natural_cmp(&a.name, &b.name))
}

// -- Reads -----------------------------------------------------------------------

/// Classify file content: image by extension, binary by a NUL byte in the
/// first [`BINARY_SNIFF_LEN`] bytes, text otherwise.
fn classify(path: &Path) -> FileContentKind {
    let image = path
        .extension()
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().to_string_lossy().as_ref()))
        .unwrap_or(false);
    if image {
        return FileContentKind::Image;
    }
    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buf = [0u8; BINARY_SNIFF_LEN];
        let mut read = 0usize;
        loop {
            match file.read(&mut buf[read..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    read += n;
                    if read >= BINARY_SNIFF_LEN {
                        break;
                    }
                }
            }
        }
        if buf[..read].contains(&0) {
            return FileContentKind::Binary;
        }
    }
    FileContentKind::Text
}

/// mtime as epoch milliseconds (`0` when the clock says "before epoch" or
/// the stat fails — callers treat 0 as "unknown/never written").
pub(crate) fn mtime_ms_of(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Synchronous body of [`repo_stat_file`].
pub(crate) fn stat_file_impl(root: &Path, path: &str) -> Result<FileMeta, String> {
    let full = validate_rel_path(root, path)?;
    let meta = std::fs::metadata(&full).map_err(|e| format!("`{path}`: {e}"))?;
    if !meta.is_file() {
        return Err(format!("`{path}` is not a regular file"));
    }
    Ok(FileMeta {
        path: path.to_string(),
        size: meta.len(),
        kind: classify(&full),
        readonly: meta.permissions().readonly(),
        mtime_ms: mtime_ms_of(&full),
    })
}

/// Synchronous body of [`repo_list_dir`]: one bounded, gitignore-aware
/// `read_dir` level. `submodules` carries the index's gitlink paths
/// (repository-relative) so their directories surface as `Submodule`.
pub(crate) fn list_dir_impl(
    root: &Path,
    path: &str,
    include_ignored: bool,
    submodules: &[String],
    cap: usize,
) -> Result<DirListing, String> {
    // "" lists the workdir root itself; anything else must be relative.
    let full = if path.is_empty() {
        root.to_path_buf()
    } else {
        validate_rel_path(root, path)?
    };
    let meta = std::fs::metadata(&full).map_err(|e| format!("`{path}`: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("`{path}` is not a directory"));
    }

    let mut builder = ignore::WalkBuilder::new(&full);
    builder.max_depth(Some(1));
    // Dotfiles (.gitignore, .github, …) are first-class browser content.
    builder.hidden(false);
    if include_ignored {
        // Show what git would hide: every filter off.
        builder.git_ignore(false).git_exclude(false).git_global(false);
        builder.ignore(false).parents(false).require_git(false);
    } else {
        // A repo is always a git repo here; requiring it keeps the
        // .gitignore chain (root → nested → exclude file) active.
        builder.require_git(false);
    }
    // `.git` itself never browses (metadata store, not content).
    builder.filter_entry(|entry| entry.file_name() != ".git");

    let mut entries: Vec<DirEntryInfo> = Vec::new();
    let mut truncated = false;
    for entry in builder.build().flatten() {
        // Depth 0 is the walked dir itself.
        if entry.depth() == 0 {
            continue;
        }
        if entries.len() >= cap {
            truncated = true;
            break;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = rel_string(entry.path(), root);
        let kind = if file_type.is_symlink() {
            DirEntryKind::Symlink
        } else if file_type.is_dir() {
            if submodules.iter().any(|sub| sub == &rel) {
                DirEntryKind::Submodule
            } else {
                DirEntryKind::Dir
            }
        } else {
            DirEntryKind::File
        };
        let size = match kind {
            DirEntryKind::File => entry.metadata().ok().map(|m| m.len()),
            _ => None,
        };
        entries.push(DirEntryInfo {
            name,
            path: rel,
            kind,
            size,
        });
    }
    entries.sort_by(compare_entries);
    Ok(DirListing {
        path: path.to_string(),
        entries,
        truncated,
    })
}

/// Gitlink (mode 160000) index paths — submodule checkout directories.
fn collect_gitlinks(repo: &git2::Repository) -> Vec<String> {
    let Ok(index) = repo.index() else {
        return Vec::new();
    };
    index
        .iter()
        .filter(|entry| entry.mode == 0o160000)
        .map(|entry| String::from_utf8_lossy(&entry.path).into_owned())
        .collect()
}

/// Synchronous body of [`repo_file_index`]: gitignore-respecting parallel
/// walk capped at [`MAX_INDEX_ENTRIES`]. Sizes are not collected (a stat
/// per file would dominate); quick-open only needs paths.
pub(crate) fn file_index_impl(root: &Path) -> Result<FileIndex, String> {
    file_index_impl_with_cap(root, MAX_INDEX_ENTRIES)
}

/// Cap-injectable body (tests pass a small cap to exercise truncation).
pub(crate) fn file_index_impl_with_cap(root: &Path, cap: usize) -> Result<FileIndex, String> {
    if !root.is_dir() {
        return Err("workdir is not a directory".into());
    }
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).require_git(false);
    builder.filter_entry(|entry| entry.file_name() != ".git");

    let entries: Arc<Mutex<Vec<FileIndexEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let truncated = Arc::new(AtomicBool::new(false));
    let walker = builder.build_parallel();
    {
        let entries = entries.clone();
        let stop = stop.clone();
        let truncated = truncated.clone();
        // One visitor per walk thread; the shared cap accounting makes
        // whichever thread hits the cap flip `stop` for everyone.
        walker.run(move || {
            let entries = entries.clone();
            let stop = stop.clone();
            let truncated = truncated.clone();
            Box::new(move |entry| match entry {
                Err(_) => ignore::WalkState::Continue,
                Ok(entry) => {
                    if stop.load(AtomicOrdering::Relaxed) {
                        return ignore::WalkState::Quit;
                    }
                    // Depth 0 is the workdir itself.
                    if entry.depth() == 0 {
                        return ignore::WalkState::Continue;
                    }
                    let Some(file_type) = entry.file_type() else {
                        return ignore::WalkState::Continue;
                    };
                    let path = match entry.path().strip_prefix(root) {
                        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                        Err(_) => return ignore::WalkState::Continue,
                    };
                    if path.is_empty() {
                        return ignore::WalkState::Continue;
                    }
                    let mut guard = entries.lock().unwrap();
                    if guard.len() >= cap {
                        truncated.store(true, AtomicOrdering::Relaxed);
                        stop.store(true, AtomicOrdering::Relaxed);
                        return ignore::WalkState::Quit;
                    }
                    guard.push(FileIndexEntry {
                        path,
                        is_dir: file_type.is_dir(),
                        size: None,
                    });
                    ignore::WalkState::Continue
                }
            })
        });
    }
    let mut entries = Arc::try_unwrap(entries)
        .map_err(|_| "index collector still shared".to_string())?
        .into_inner()
        .unwrap();
    entries.sort_by(|a, b| natural_cmp(&a.path, &b.path));
    Ok(FileIndex {
        entries,
        truncated: truncated.load(AtomicOrdering::Relaxed),
    })
}

// -- Mutations ---------------------------------------------------------------------

/// Synchronous body of [`repo_write_file`]: mtime-guarded, atomic (temp
/// file + rename in the same dir), parent dirs created on demand.
/// `expected_mtime` semantics: `Some(stored)` must equal the file's current
/// mtime (or the file must be missing with `Some(0)` = "create"), otherwise
/// the file changed externally and the write is refused; `None` overwrites
/// unconditionally (unused by the editor, available for tooling).
pub(crate) fn write_file_impl(
    root: &Path,
    path: &str,
    content: &[u8],
    expected_mtime: Option<u128>,
) -> Result<FileMeta, String> {
    if contains_git_component(path) {
        return Err(format!("refusing to write inside `.git`: `{path}`"));
    }
    let full = validate_rel_path(root, path)?;
    let current = mtime_ms_of(&full);
    let exists = full.is_file();
    if let Some(expected) = expected_mtime {
        let conflict = if exists {
            current != expected
        } else {
            expected != 0
        };
        if conflict {
            return Err(format!(
                "`{path}` changed on disk (external modification); reload before saving"
            ));
        }
    }
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating parent dirs: {e}"))?;
    }
    // Atomic-ish write: never leave a truncated file on failure.
    let tmp = full.with_extension(format!(
        "{}mygitui-tmp",
        full.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, content).map_err(|e| format!("writing `{path}`: {e}"))?;
    if let Err(err) = std::fs::rename(&tmp, &full) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("saving `{path}`: {err}"));
    }
    Ok(FileMeta {
        path: path.to_string(),
        size: content.len() as u64,
        kind: classify(&full),
        readonly: false,
        mtime_ms: mtime_ms_of(&full),
    })
}

/// Synchronous body of [`repo_create_entry`].
pub(crate) fn create_entry_impl(
    root: &Path,
    path: &str,
    kind: NewEntryKind,
) -> Result<(), String> {
    if contains_git_component(path) {
        return Err(format!("refusing to create inside `.git`: `{path}`"));
    }
    let full = validate_rel_path(root, path)?;
    if full.exists() {
        return Err(format!("`{path}` already exists"));
    }
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating parent dirs: {e}"))?;
    }
    match kind {
        NewEntryKind::File => std::fs::write(&full, b"")
            .map_err(|e| format!("creating `{path}`: {e}")),
        NewEntryKind::Dir => std::fs::create_dir(&full)
            .map_err(|e| format!("creating `{path}`: {e}")),
    }
}

/// Synchronous body of [`repo_rename_entry`].
pub(crate) fn rename_entry_impl(
    root: &Path,
    path: &str,
    new_path: &str,
) -> Result<(), String> {
    if contains_git_component(path) || contains_git_component(new_path) {
        return Err("refusing to rename into or out of `.git`".into());
    }
    let from = validate_rel_path(root, path)?;
    let to = validate_rel_path(root, new_path)?;
    if !from.exists() {
        return Err(format!("`{path}` does not exist"));
    }
    if to.exists() {
        return Err(format!("`{new_path}` already exists"));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating parent dirs: {e}"))?;
    }
    std::fs::rename(&from, &to).map_err(|e| format!("renaming `{path}`: {e}"))
}

/// Synchronous body of [`repo_delete_entry`] (after the checkpoint guard).
/// Refuses `.git` internals and submodule checkouts (a dir with its own
/// `.git` — open it as a repo instead).
pub(crate) fn delete_entry_impl(root: &Path, path: &str) -> Result<(), String> {
    if contains_git_component(path) {
        return Err(format!("refusing to delete inside `.git`: `{path}`"));
    }
    let full = validate_rel_path(root, path)?;
    let meta = std::fs::metadata(&full).map_err(|e| format!("`{path}`: {e}"))?;
    if meta.is_dir() {
        if full.join(".git").exists() {
            return Err(format!(
                "`{path}` is a submodule checkout; open it as a repo instead of deleting"
            ));
        }
        std::fs::remove_dir_all(&full).map_err(|e| format!("deleting `{path}`: {e}"))
    } else {
        std::fs::remove_file(&full).map_err(|e| format!("deleting `{path}`: {e}"))
    }
}

// -- Queue plumbing --------------------------------------------------------------

/// Enqueue a filesystem mutation on the repo's serial queue: `f` runs
/// without the repo mutex (pure fs work), the generation is bumped after it
/// finishes so the watcher-driven UI refresh picks the change up.
fn enqueue_fs_mutation<T, F>(
    handle: &RepoHandle,
    kind: &'static str,
    f: F,
) -> tokio::sync::oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue(kind, move |_ctx| {
        let result = f();
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result
    });
    rx
}

fn get_handle(state: &RepoManager, repo_id: &RepoId) -> Result<Arc<RepoHandle>, String> {
    state
        .get(repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))
}

// -- Tauri commands ---------------------------------------------------------------

/// One directory level of the working tree (contracts.md). Queued read:
/// the index walk for submodule marking shares the repo lock with other ops.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_list_dir(
    repo_id: RepoId,
    path: String,
    include_ignored: bool,
    state: State<'_, RepoManager>,
) -> Result<DirListing, String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    let repo = handle.repo();
    let (_op_id, rx) = handle.ops().enqueue_silent("list_dir", move |_ctx| {
        let submodules = {
            let guard = repo.lock();
            collect_gitlinks(&guard)
        };
        list_dir_impl(&root, &path, include_ignored, &submodules, MAX_LIST_ENTRIES)
    });
    rx.await.map_err(|_| "op queue closed before completion".to_string())?
}

/// Size/content-classification/readonly/mtime for one file — lets the FE
/// decide how to open a file before transferring any bytes.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_stat_file(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<FileMeta, String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || stat_file_impl(&root, &path))
        .await
        .map_err(|e| format!("stat_file task failed: {e}"))?
}

/// Write file content to the worktree (the file editor's save). Queued
/// mutation: serialized with every other op, generation-bumped so the
/// watcher refreshes status/diffs.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_write_file(
    repo_id: RepoId,
    path: String,
    content: Vec<u8>,
    expected_mtime: Option<u128>,
    state: State<'_, RepoManager>,
) -> Result<FileMeta, String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    finish(enqueue_fs_mutation(&handle, "write_file", move || {
        write_file_impl(&root, &path, &content, expected_mtime)
    }))
    .await
}

/// Create an empty file or a directory (with parents).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_create_entry(
    repo_id: RepoId,
    path: String,
    kind: NewEntryKind,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    finish(enqueue_fs_mutation(&handle, "create_entry", move || {
        create_entry_impl(&root, &path, kind)
    }))
    .await
}

/// Rename/move a file or directory within the worktree.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_rename_entry(
    repo_id: RepoId,
    path: String,
    new_path: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    finish(enqueue_fs_mutation(&handle, "rename_entry", move || {
        rename_entry_impl(&root, &path, &new_path)
    }))
    .await
}

/// Delete a file or directory tree. A checkpoint is snapshotted BEFORE the
/// deletion (same ordering rule as discard/repo_clean) so the delete is
/// undoable from the Undo panel.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_delete_entry(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<crate::engine::types::CheckpointInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    let engine = handle.m3();
    let repo = handle.repo();
    finish(enqueue_fs_mutation(&handle, "delete_entry", move || {
        // Guard first: a failed delete must leave the repo untouched, and
        // the checkpoint must capture the pre-delete state.
        let checkpoint = {
            let guard = repo.lock();
            engine
                .checkpoint_create(&guard, &format!("delete {path}"))
                .map_err(|e| e.to_string())?
        };
        delete_entry_impl(&root, &path)?;
        Ok(checkpoint)
    }))
    .await
}

/// Flat, gitignore-respecting file index for quick-open (Ctrl+P).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_file_index(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<FileIndex, String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || file_index_impl(&root))
        .await
        .map_err(|e| format!("file_index task failed: {e}"))?
}

/// Await an enqueued op's result (worker done or queue closed).
async fn finish<T>(rx: tokio::sync::oneshot::Receiver<Result<T, String>>) -> Result<T, String> {
    rx.await.map_err(|_| "op queue closed before completion".to_string())?
}
