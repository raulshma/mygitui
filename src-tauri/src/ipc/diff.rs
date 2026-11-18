//! Diffs, blame, and raw file/blob reads.
//!
//! `repo_diff_stream` pushes pages of [`DIFF_PAGE_SIZE`] files over a
//! `tauri::ipc::Channel`; large diffs stream instead of materializing
//! whole-hunk JSON up front. `repo_read_file` / `repo_blob` ship raw bytes
//! as `tauri::ipc::Response` ArrayBuffers (the conflict editor and image
//! diffs), both bounded by [`READ_FILE_MAX_BYTES`] — the cap the blame
//! engine shares.

use std::path::Path;

use tauri::ipc::Channel;
use tauri::State;

use crate::engine::types::{BlameLine, BlameQuery, DiffQuery, DiffSide, FileDiff, RepoId};
use crate::repo::{RepoManager, StreamKind};

use super::{engine_err, enqueue_read, enqueue_remote_read, finish_op, get_handle};

/// Files per `repo_diff_stream` page.
const DIFF_PAGE_SIZE: usize = 50;

/// Blame one file. `query` carries the full option surface
/// ([`BlameQuery`]); the legacy standalone `from` argument is kept for
/// backward compatibility — `query.from` wins when both arrive.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_blame(
    repo_id: RepoId,
    path: String,
    from: Option<String>,
    query: Option<BlameQuery>,
    state: State<'_, RepoManager>,
) -> Result<Vec<BlameLine>, String> {
    let handle = get_handle(&state, &repo_id)?;
    // Legacy `from` merges into the query; an explicit query wins.
    let mut blame = query.unwrap_or_default();
    if blame.from.is_none() {
        blame.from = from;
    }
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "blame",
            move |git, _ws| git.blame(&path, &blame),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "blame", move |_ctx, repo| {
        engine.blame(repo, &path, &blame)
    }))
    .await
}

/// One-shot diff for small/medium changes; large diffs use
/// [`repo_diff_stream`].
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_diff(
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    query: Option<DiffQuery>,
    state: State<'_, RepoManager>,
) -> Result<Vec<FileDiff>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let (paths, ignore_ws) = diff_query_parts(query);
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "diff",
            move |git, _ws| git.diff(&old, &new, paths.as_deref(), ignore_ws),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "diff", move |_ctx, repo| {
        engine.diff(repo, &old, &new, paths.as_deref(), ignore_ws)
    }))
    .await
}

/// Split a diff query into the engine-shaped parts: `None`/empty paths is
/// "all paths", and the whitespace flag defaults to false.
fn diff_query_parts(query: Option<DiffQuery>) -> (Option<Vec<String>>, bool) {
    match query {
        Some(q) => (
            if q.paths.is_empty() {
                None
            } else {
                Some(q.paths)
            },
            q.ignore_whitespace,
        ),
        None => (None, false),
    }
}

/// Stream a diff in pages of [`DIFF_PAGE_SIZE`] files over `on_page`.
/// Carries a per-consumer stream slot (see [`repo_log_stream`]); stops
/// early when the channel send fails (client gone) or the stream is
/// cancelled (a new request for the same repo+kind+consumer).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_diff_stream(
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    query: Option<DiffQuery>,
    consumer_id: Option<String>,
    on_page: Channel<Vec<FileDiff>>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let (paths, ignore_ws) = diff_query_parts(query);
    let token = handle.begin_stream(StreamKind::Diff, consumer_id.as_deref());
    if let Some(remote) = handle.remote() {
        // The remote engine returns the complete diff in one shot: one
        // queued read, then page it out exactly like the local result.
        let diffs = finish_op(enqueue_remote_read(
            &handle,
            remote,
            "diff",
            move |git, _ws| git.diff(&old, &new, paths.as_deref(), ignore_ws),
        ))
        .await?;
        for page in chunk_vec(diffs, DIFF_PAGE_SIZE) {
            if token.is_cancelled() || handle.is_closed() {
                return Ok(());
            }
            if on_page.send(page).is_err() {
                break; // client dropped the channel
            }
        }
        return Ok(());
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let repo = repo.lock();
        // Page while patches build: the first page ships after
        // DIFF_PAGE_SIZE files instead of after the whole diff (every
        // hunk, every line) is materialized; a client that leaves mid-
        // stream also stops paying for the rest.
        let mut page: Vec<FileDiff> = Vec::with_capacity(DIFF_PAGE_SIZE);
        let mut cancelled = false;
        engine
            .diff_stream(
                &repo,
                &old,
                &new,
                paths.as_deref(),
                ignore_ws,
                &mut |file| {
                    // Per-page cancel check (same rule as the log walk): a
                    // superseded/closed stream stops building patches.
                    if token.is_cancelled() || handle.is_closed() {
                        cancelled = true;
                        return false;
                    }
                    page.push(file);
                    if page.len() < DIFF_PAGE_SIZE {
                        return true;
                    }
                    // Full page: ship it; a failed send = client gone.
                    on_page.send(std::mem::take(&mut page)).is_ok()
                },
            )
            .map_err(engine_err)?;
        if cancelled {
            return Ok(());
        }
        if !page.is_empty() {
            let _ = on_page.send(page);
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))??;
    Ok(())
}

/// Merge base of two commitishes — the three-dot compare's `A...B` anchor.
/// `None` for unrelated histories (`git merge-base` exits 1 there) or a
/// criss-cross criss: the first of several bases is reported (the compare
/// only anchors one side; see `GitEngine::merge_base`).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_merge_base(
    repo_id: RepoId,
    a: String,
    b: String,
    state: State<'_, RepoManager>,
) -> Result<Option<String>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "diff",
            move |git, _ws| git.merge_base(&a, &b),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "diff", move |_ctx, repo| {
        engine.merge_base(repo, &a, &b)
    }))
    .await
}

/// Split `items` into consecutive chunks of at most `size` (the last may be
/// smaller); empty input yields no pages.
pub fn chunk_vec<T>(items: Vec<T>, size: usize) -> Vec<Vec<T>> {
    assert!(size > 0, "chunk size must be positive");
    let mut pages: Vec<Vec<T>> = Vec::new();
    for item in items {
        match pages.last_mut() {
            Some(page) if page.len() < size => page.push(item),
            _ => pages.push(vec![item]),
        }
    }
    pages
}

// ---------------------------------------------------------------------------
// M3 E1: conflict editor file access
// ---------------------------------------------------------------------------

/// Raw bytes of one workdir file (`path` workdir-relative). Backs the
/// conflict editor: conflicted paths carry marker previews in the workdir
/// (see `engine::merge`) and no other command returns raw file content.
/// A path that escapes the workdir (`..` components, absolute or
/// drive-prefixed forms) or is not a regular file rejects; a file that does
/// not exist resolves to an empty vec (the editor treats that as "no
/// marker preview written yet"). Workdir reads need no engine call, so the
/// repo mutex is not taken.
///
/// Returns a raw IPC response (`tauri::ipc::Response`): the webview receives
/// an `ArrayBuffer` instead of a JSON number array — a 2 MB file used to
/// serialize as ~8 MB of JSON and a 2M-element JS array (~16 MB heap).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_read_file(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<tauri::ipc::Response, String> {
    let handle = get_handle(&state, &repo_id)?;
    let content = if let Some(remote) = handle.remote() {
        // Remote repos read the workdir over SFTP through the workspace
        // (no engine call needed locally, none needed remotely either).
        finish_op(enqueue_remote_read(
            &handle,
            remote,
            "read_file",
            move |_git, ws| {
                // Missing file → empty content (documented "no preview
                // written"), mirroring the local fs path.
                if !ws.exists(&path)? {
                    return Ok(Vec::new());
                }
                ws.read_file(&path)
            },
        ))
        .await?
    } else {
        let root = handle.root.clone();
        tauri::async_runtime::spawn_blocking(move || read_workdir_file(&root, &path))
            .await
            .map_err(|e| format!("read_file task failed: {e}"))??
    };
    Ok(tauri::ipc::Response::new(content))
}

/// Largest file `repo_read_file` will ship over IPC. Anything bigger fails
/// with a clear error instead of freezing the bridge (and the editor) on a
/// multi-GB read; real source files are orders of magnitude smaller.
/// Shared with the blame engine, which materializes the same file (see
/// `engine::libgit2::blame`).
pub(crate) const READ_FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Synchronous body of [`repo_read_file`] (validation + disk read). Path
/// validation is shared with the workspace file-browser commands
/// (`workspace::validate_rel_path`).
fn read_workdir_file(root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let full = crate::workspace::validate_rel_path(root, path)?;
    match std::fs::metadata(&full) {
        // Missing file → empty content (documented "no preview written").
        Err(_) => Ok(Vec::new()),
        Ok(meta) if !meta.is_file() => Err(format!("`{path}` is not a regular file")),
        Ok(meta) if meta.len() > READ_FILE_MAX_BYTES => Err(format!(
            "`{path}` is {} MB — larger than the {} MB read limit",
            meta.len() / (1024 * 1024),
            READ_FILE_MAX_BYTES / (1024 * 1024)
        )),
        Ok(_) => std::fs::read(&full).map_err(|e| format!("reading `{path}` failed: {e}")),
    }
}

/// Raw bytes of one blob at `path` on diff `side` (`repo_blob`): image
/// diffs need both sides of commit-to-commit comparisons, and only the
/// worktree side lives on disk (`repo_read_file`). Missing paths,
/// traversal/absolute paths, and blobs above [`READ_FILE_MAX_BYTES`] all
/// reject — the FE renders the "unavailable" placeholder for them. Unlike
/// [`repo_read_file`] there is no missing-file fallback: a side without
/// the path (a deleted file's new side) IS the placeholder case.
///
/// Returns a raw IPC response (`tauri::ipc::Response`) like
/// [`repo_read_file`] — bytes ship as an `ArrayBuffer`, never a JSON
/// number array.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_blob(
    repo_id: RepoId,
    side: DiffSide,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<tauri::ipc::Response, String> {
    let handle = get_handle(&state, &repo_id)?;
    let content = if let Some(remote) = handle.remote() {
        finish_op(enqueue_remote_read(
            &handle,
            remote,
            "blob",
            move |git, ws| {
                // Worktree blobs are device files: the workspace's SFTP
                // reader (missing file → error, the placeholder case).
                if matches!(side, DiffSide::Worktree) {
                    ws.read_file(&path)
                } else {
                    git.blob(&side, &path)
                }
            },
        ))
        .await?
    } else {
        let engine = handle.engine();
        let repo = handle.repo()?;
        finish_op(enqueue_read(&handle, &repo, "blob", move |_ctx, repo| {
            engine.blob(repo, &side, &path)
        }))
        .await?
    };
    Ok(tauri::ipc::Response::new(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_vec_splits_into_pages() {
        let items: Vec<u32> = (0..120).collect();
        let pages = chunk_vec(items, 50);
        assert_eq!(pages.len(), 3);
        assert_eq!(
            [pages[0].len(), pages[1].len(), pages[2].len()],
            [50, 50, 20]
        );
        assert_eq!(pages[2][0], 100);
        let flat: Vec<u32> = pages.into_iter().flatten().collect();
        assert_eq!(flat, (0..120).collect::<Vec<u32>>());
    }

    #[test]
    fn chunk_vec_exact_multiple_and_empty() {
        assert!(chunk_vec(Vec::<u8>::new(), 50).is_empty());
        let pages = chunk_vec(vec![1u8; 50], 50);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].len(), 50);
    }
}
