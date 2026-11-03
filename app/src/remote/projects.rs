//! Remote project/script detection.
//!
//! `project_detect` is a filesystem scan, so a remote repo's handle root (a
//! POSIX path on the device) cannot be walked with `std::fs` on the host.
//! One bounded `find` over the exec channel snapshots the directory tree and
//! manifest names; the manifests the detectors read are then pulled over
//! SFTP, and the result is fed to the same detection rules local repos use
//! (`crate::projects::detect_with`). Buttons therefore surface the same
//! scripts for a device repo as for the same repo opened locally.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::join_all;

use super::quoting::shell_quote;
use super::transport::{DeviceSession, DEFAULT_EXEC_TIMEOUT};
use crate::projects::{detect_with, ProjectFs, ProjectInfo};

/// Directory scan depth (root + children + workspace members + .NET's
/// `src/<Project>/`); files one deeper so those projects' manifests appear.
const MAX_DIR_DEPTH: usize = 3;
const MAX_FILE_DEPTH: usize = 4;

/// Bound on manifest reads per scan (monorepos) and per-file size.
const MAX_MANIFEST_FILES: usize = 256;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_TOTAL_MANIFEST_BYTES: usize = 8 * 1024 * 1024;

/// Mirrors `projects::SKIP_DIRS` (fixed literal — no user input).
const PRUNE_DIRS: &str = "-name .git -o -name .hg -o -name .svn -o -name .idea \
    -o -name .vscode -o -name node_modules -o -name target -o -name dist \
    -o -name build -o -name out -o -name bin -o -name obj -o -name .venv \
    -o -name venv -o -name __pycache__ -o -name .gradle -o -name .next \
    -o -name vendor";

/// Every manifest / signal file the detectors look at, by name.
const INTERESTING_FILES: &str = "-name package.json -o -name Cargo.toml \
    -o -name go.mod -o -name pnpm-workspace.yaml -o -name pnpm-lock.yaml \
    -o -name yarn.lock -o -name bun.lockb -o -name bun.lock \
    -o -name pyproject.toml -o -name requirements.txt -o -name setup.py \
    -o -name Pipfile -o -name manage.py -o -name main.py -o -name app.py \
    -o -name uv.lock -o -name poetry.lock -o -name main.rs -o -name Gemfile \
    -o -name Rakefile -o -name rails -o -name composer.json -o -name Makefile \
    -o -name makefile -o -name GNUmakefile -o -name build.gradle \
    -o -name build.gradle.kts -o -name settings.gradle \
    -o -name settings.gradle.kts -o -name gradlew -o -name gradlew.bat \
    -o -name '*.sln' -o -name '*.slnx' -o -name '*.csproj' \
    -o -name '*.fsproj' -o -name '*.vbproj'";

/// Detect the projects/scripts of the repo at `root` (absolute POSIX path on
/// the device).
pub fn detect(session: &Arc<DeviceSession>, root: &str) -> Result<Vec<ProjectInfo>, String> {
    let cmd = format!(
        "cd {} || exit 64; \
         find . -maxdepth {MAX_DIR_DEPTH} \\( {PRUNE_DIRS} \\) -prune \
           -o -type d -print | awk '{{print \"D\\t\" $0}}'; \
         find . -maxdepth {MAX_FILE_DEPTH} \\( {PRUNE_DIRS} \\) -prune \
           -o -type f \\( {INTERESTING_FILES} \\) -print \
           | awk '{{print \"F\\t\" $0}}'; \
         exit 0",
        shell_quote(root)
    );
    let out = crate::rt::block_on(session.exec(&cmd, DEFAULT_EXEC_TIMEOUT))?;
    if out.exit_code == 64 {
        return Err(format!("repository path not found on device: {root}"));
    }
    let out = out.checked("project scan")?;
    let mut snapshot = SnapshotFs::from_listing(&out.stdout_str(), root);
    snapshot.contents = fetch_contents(session, root, &snapshot.content_paths())?;
    // The device root doubles as the detection root so root-level project
    // names (Python/Ruby/Make) match the local scan's directory-name default.
    Ok(detect_with(&snapshot, Path::new(root)))
}

/// Read the manifests detection parses. Concurrent over the shared SFTP
/// session; individual failures are skipped (a missing/unreadable manifest
/// only hides that ecosystem), while a dead session errors the whole scan.
fn fetch_contents(
    session: &Arc<DeviceSession>,
    root: &str,
    paths: &[String],
) -> Result<BTreeMap<String, String>, String> {
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let sftp = crate::rt::block_on(session.sftp())?;
    let reads = paths.iter().map(|rel| {
        let sftp = sftp.clone();
        let abs = format!("{}/{rel}", root.trim_end_matches('/'));
        let rel = rel.clone();
        async move {
            let meta = sftp.metadata(&abs).await.ok()?;
            if meta.len() > MAX_MANIFEST_BYTES as u64 {
                return None;
            }
            let bytes = sftp.read(&abs).await.ok()?;
            Some((rel, String::from_utf8_lossy(&bytes).into_owned()))
        }
    });
    let mut out = BTreeMap::new();
    let mut total = 0usize;
    for result in crate::rt::block_on(join_all(reads)) {
        let Some((rel, text)) = result else {
            continue;
        };
        total += text.len();
        if total > MAX_TOTAL_MANIFEST_BYTES {
            break;
        }
        out.insert(rel, text);
    }
    Ok(out)
}

/// A remote detection snapshot: keys are repository-relative POSIX paths
/// (`""` = root). Paths arriving from detection are either root-relative or
/// rooted at the device workdir, so both normalize to the same key.
struct SnapshotFs {
    /// Absolute device workdir (no trailing slash), used as the prefix to
    /// strip from detection's `root.join(…)` paths.
    root: String,
    dirs: BTreeSet<String>,
    files: BTreeSet<String>,
    contents: BTreeMap<String, String>,
}

impl SnapshotFs {
    fn from_listing(raw: &str, root: &str) -> Self {
        let mut dirs = BTreeSet::new();
        let mut files = BTreeSet::new();
        for line in raw.lines() {
            let Some((kind, path)) = line.split_once('\t') else {
                continue;
            };
            let key = normalize(path);
            match kind {
                "D" => {
                    dirs.insert(key);
                }
                "F" => {
                    files.insert(key);
                }
                _ => {}
            }
        }
        Self {
            root: root.trim_end_matches('/').to_string(),
            dirs,
            files,
            contents: BTreeMap::new(),
        }
    }

    /// Snapshot key for a detection path (absolute-under-root or relative).
    fn key(&self, path: &Path) -> String {
        let raw = path.to_string_lossy().replace('\\', "/");
        let prefix = if self.root.is_empty() {
            "/".to_string()
        } else {
            format!("{}/", self.root)
        };
        let rel = if raw == self.root {
            ""
        } else {
            raw.strip_prefix(&prefix).unwrap_or(&raw)
        };
        normalize(rel)
    }

    /// Files detection reads the contents of, sorted + capped (the rest are
    /// existence checks only).
    fn content_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .files
            .iter()
            .filter(|path| wants_content(path))
            .take(MAX_MANIFEST_FILES)
            .cloned()
            .collect();
        out.sort();
        out
    }
}

impl ProjectFs for SnapshotFs {
    fn read_dir(&self, dir: &Path) -> Vec<(PathBuf, bool)> {
        let parent = self.key(dir);
        let mut out = Vec::new();
        for (paths, is_dir) in [(&self.dirs, true), (&self.files, false)] {
            for path in paths {
                if is_child_of(&parent, path) {
                    out.push((PathBuf::from(path), is_dir));
                }
            }
        }
        out.sort();
        out
    }

    fn read_file(&self, path: &Path) -> Option<String> {
        self.contents.get(&self.key(path)).cloned()
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains(&self.key(path))
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains(&self.key(path))
    }

    /// Device repos are POSIX: prefer `./gradlew` even from a Windows host.
    fn windows(&self) -> bool {
        false
    }
}

fn wants_content(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "package.json"
            | "Cargo.toml"
            | "go.mod"
            | "pnpm-workspace.yaml"
            | "pyproject.toml"
            | "setup.py"
            | "requirements.txt"
            | "composer.json"
            | "Makefile"
            | "makefile"
            | "GNUmakefile"
            | "build.gradle"
            | "build.gradle.kts"
    ) || name.ends_with(".csproj")
        || name.ends_with(".fsproj")
        || name.ends_with(".vbproj")
}

/// `find -print` emits `./a/b` (and `.` for the root); normalize both forms
/// to the same key.
fn normalize(path: &str) -> String {
    let path = path.trim_end_matches('/');
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty() || path == "." {
        String::new()
    } else {
        path.to_string()
    }
}

fn is_child_of(parent: &str, path: &str) -> bool {
    if parent.is_empty() {
        return !path.is_empty() && !path.contains('/');
    }
    path.strip_prefix(parent)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(raw: &str) -> SnapshotFs {
        SnapshotFs::from_listing(raw, "/home/me/repo")
    }

    #[test]
    fn listing_parses_dirs_files_and_root() {
        let fs =
            snapshot("D\t.\nD\t./src\nD\t./packages/web\nF\t./package.json\nF\t./src/main.rs\n");
        assert!(fs.is_dir(Path::new("")));
        assert!(fs.is_dir(Path::new("/home/me/repo")));
        assert!(fs.is_dir(Path::new("src")));
        assert!(fs.is_file(Path::new("/home/me/repo/package.json")));
        assert!(fs.is_file(Path::new("src/main.rs")));
        assert!(!fs.is_file(Path::new("src")));
        assert!(!fs.is_dir(Path::new("package.json")));
    }

    #[test]
    fn read_dir_returns_direct_children_only() {
        let fs = snapshot("D\t.\nD\t./src\nD\t./src/deep\nF\t./package.json\nF\t./src/main.rs\n");
        let root: Vec<(String, bool)> = fs
            .read_dir(Path::new(""))
            .into_iter()
            .map(|(path, is_dir)| (path.to_string_lossy().replace('\\', "/"), is_dir))
            .collect();
        assert_eq!(
            root,
            [
                ("package.json".to_string(), false),
                ("src".to_string(), true)
            ]
        );
        let nested: Vec<(String, bool)> = fs
            .read_dir(Path::new("src"))
            .into_iter()
            .map(|(path, is_dir)| (path.to_string_lossy().replace('\\', "/"), is_dir))
            .collect();
        assert_eq!(
            nested,
            [
                ("src/deep".to_string(), true),
                ("src/main.rs".to_string(), false)
            ]
        );
    }

    #[test]
    fn manifest_contents_are_selected_by_name() {
        let fs = snapshot(
            "D\t.\nF\t./package.json\nF\t./Cargo.toml\nF\t./Gemfile\nF\t./src/App.csproj\nF\t./settings.gradle\n",
        );
        assert_eq!(
            fs.content_paths(),
            ["Cargo.toml", "package.json", "src/App.csproj"]
        );
    }

    #[test]
    fn detection_runs_against_a_snapshot() {
        let mut fs = snapshot(
            "D\t.\nD\t./packages/web\nF\t./package.json\nF\t./packages/web/package.json\nF\t./pnpm-lock.yaml\n",
        );
        fs.contents.insert(
            "package.json".into(),
            r#"{ "workspaces": ["packages/*"] }"#.into(),
        );
        fs.contents.insert(
            "packages/web/package.json".into(),
            r#"{ "name": "web", "scripts": { "dev": "vite", "test": "vitest run" } }"#.into(),
        );
        let projects = detect_with(&fs, Path::new("/home/me/repo"));
        let web = projects
            .iter()
            .find(|p| p.path == "packages/web")
            .expect("workspace package detected");
        assert!(web.scripts.iter().any(|s| s.label == "dev" && s.watch));
        assert!(web.scripts.iter().any(|s| s.kind == "test"));
        assert!(
            web.scripts
                .iter()
                .any(|s| s.command == "pnpm -C \"packages/web\" run dev"),
            "package manager resolved through the snapshot: {:?}",
            web.scripts
        );
    }
}
