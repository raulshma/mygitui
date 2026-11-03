use std::path::Path;

use serde::Serialize;

use crate::engine::types::{RepoId, SubmoduleInfo};
use crate::service::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle, AppServices,
};

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
pub async fn repo_read_file(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
) -> Result<Vec<u8>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Remote repos read the workdir over SFTP through the workspace
        // (no engine call needed locally, none needed remotely either).
        return finish_op(enqueue_remote_read(
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
        .await;
    }
    let root = handle.root.clone();
    crate::rt::spawn_blocking(move || read_workdir_file(&root, &path))
        .await
        .map_err(|e| format!("read_file task failed: {e}"))?
}

/// Synchronous body of [`repo_read_file`] (validation + disk read). Path
/// validation is shared with the workspace file-browser commands
/// (`workspace::validate_rel_path`).
fn read_workdir_file(root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let full = crate::workspace::validate_rel_path(root, path)?;
    match std::fs::metadata(&full) {
        // Missing file → empty content (documented "no preview written").
        Err(_) => Ok(Vec::new()),
        Ok(meta) if !meta.is_file() => Err(format!("`{path}` is not a regular file")),
        Ok(_) => std::fs::read(&full).map_err(|e| format!("reading `{path}` failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// M4 (lane F3): submodule management + gitignore quick-add.
//
// `submodules` is a queued read (`enqueue_read`); `submodule_update` /
// `submodule_sync` are mutations on the serial op queue (contracts.md M4:
// queue kind `submodule`); `gitignore_add` is a pure workdir file write (no
// git objects touched — the FS-only twin of `repo_read_file`'s no-mutex
// shortcut), and `gitignore_templates` is a static builtin list with no
// repo involved.
// ---------------------------------------------------------------------------

/// One curated builtin gitignore template (the `gitignore_templates` reply,
/// mirroring the contracts.md shape `{name, description, patterns}`).
/// `patterns` is ONE newline-joined `.gitignore` snippet (no trailing
/// newline); the FE applies it line-by-line via `gitignore_add`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitignoreTemplate {
    pub name: String,
    pub description: String,
    pub patterns: String,
}

/// The curated template list. Small and accurate on purpose: real
/// `.gitignore` patterns only (GitHub gitignore / gitignore.io canon).
pub fn gitignore_templates_list() -> Vec<GitignoreTemplate> {
    let entry = |name: &str, description: &str, lines: &[&str]| GitignoreTemplate {
        name: name.to_owned(),
        description: description.to_owned(),
        patterns: lines.join("\n"),
    };
    vec![
        entry(
            "Node",
            "Node.js / npm / yarn / pnpm projects",
            &[
                "node_modules/",
                "npm-debug.log*",
                "yarn-debug.log*",
                "yarn-error.log*",
                "pnpm-debug.log*",
                ".npm",
                ".eslintcache",
                "coverage/",
            ],
        ),
        entry(
            "Rust",
            "Cargo projects (build output, backups)",
            &["/target", "**/*.rs.bk"],
        ),
        entry(
            "Python",
            "CPython bytecode, packaging, tool caches",
            &[
                "__pycache__/",
                "*.py[cod]",
                "*.egg-info/",
                ".eggs/",
                "build/",
                "dist/",
                ".venv/",
                "venv/",
                ".pytest_cache/",
                ".mypy_cache/",
                ".ruff_cache/",
            ],
        ),
        entry(
            "C++",
            "Object files, binaries, build directories",
            &[
                "*.o", "*.obj", "*.out", "*.a", "*.lib", "*.so", "*.so.*", "*.dylib", "*.dll",
                "*.exe", "build/", "bin/", "Debug/", "Release/", "x64/", "x86/",
            ],
        ),
        entry(
            "Go",
            "Compiled binaries, test artifacts, vendor tree",
            &[
                "*.exe", "*.exe~", "*.dll", "*.so", "*.dylib", "*.test", "*.out", "vendor/",
            ],
        ),
        entry(
            "Java",
            "Class/jar output, Maven + Gradle artifacts",
            &[
                "*.class",
                "*.jar",
                "*.war",
                "*.ear",
                "target/",
                "build/",
                "out/",
                ".gradle/",
                ".settings/",
                ".classpath",
                ".project",
            ],
        ),
        entry(
            "macOS",
            "Finder metadata and resource forks",
            &[
                ".DS_Store",
                ".AppleDouble",
                ".LSOverride",
                "._*",
                ".Spotlight-V100",
                ".Trashes",
                ".fseventsd",
            ],
        ),
        entry(
            "Windows",
            "Explorer metadata and the recycle bin",
            &[
                "Thumbs.db",
                "Thumbs.db:encryptable",
                "ehthumbs.db",
                "ehthumbs_vista.db",
                "Desktop.ini",
                "$RECYCLE.BIN/",
                "*.lnk",
            ],
        ),
        entry(
            "VS Code",
            "Editor settings except shared workspace files",
            &[
                ".vscode/*",
                "!.vscode/settings.json",
                "!.vscode/tasks.json",
                "!.vscode/launch.json",
                "!.vscode/extensions.json",
                "*.code-workspace",
            ],
        ),
        entry(
            "JetBrains",
            "IDE workspace files and build output",
            &[".idea/", "*.iml", "*.ipr", "*.iws", "out/"],
        ),
        entry(
            "Svelte",
            "SvelteKit / Svelte build and cache dirs",
            &[".svelte-kit/", "build/", "dist/", ".vercel", ".netlify"],
        ),
        entry(
            "Vite",
            "Vite output, caches, local env files",
            &["dist/", "dist-ssr/", "*.local", ".vite/"],
        ),
        entry(
            "Terraform",
            "State, variable values, crash logs, overrides",
            &[
                "*.tfstate",
                "*.tfstate.*",
                "*.tfvars",
                "*.tfvars.json",
                ".terraform/",
                "crash.log",
                "crash.*.log",
                "override.tf",
                "override.tf.json",
                "*_override.tf",
                "*_override.tf.json",
                ".terraformrc",
                "terraform.rc",
            ],
        ),
        entry(
            "Unreal",
            "Unreal Engine generated content",
            &[
                "Build/",
                "Binaries/",
                "DerivedDataCache/",
                "Intermediate/",
                "Saved/",
                ".vs/",
            ],
        ),
        entry(
            "Unity",
            "Unity library, temp, logs and build output",
            &[
                "[Ll]ibrary/",
                "[Tt]emp/",
                "[Oo]bj/",
                "[Bb]uild/",
                "[Bb]uilds/",
                "[Ll]ogs/",
                "[Uu]serSettings/",
                "MemoryCaptures/",
                "sysinfo.txt",
                "crashlytics-build.properties",
            ],
        ),
    ]
}

/// Append `pattern` to the repo-root `.gitignore` (creating the file when
/// missing). Idempotent: an existing exact line match is left untouched.
/// Validation: the pattern must be non-empty and a single line.
fn gitignore_add_sync(root: &Path, pattern: &str) -> Result<(), String> {
    if pattern.contains('\n') || pattern.contains('\r') {
        return Err("gitignore pattern must be a single line".to_string());
    }
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("gitignore pattern is empty".to_string());
    }
    let file = root.join(".gitignore");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    if existing.lines().any(|line| line == pattern) {
        return Ok(()); // already present (exact line match)
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(pattern);
    updated.push('\n');
    std::fs::write(&file, updated).map_err(|e| format!("writing .gitignore failed: {e}"))
}

/// Append one pattern to the repo-root `.gitignore` (the status panel's
/// quick-add). Pure workdir file write — no git objects touched, so like
/// the other FS-only commands it does not take the op queue; the watcher /
/// poller picks the change up. Remote repos write through the engine
/// (idempotent exact-line dedupe, same rule).
pub async fn gitignore_add(
    services: &AppServices,
    repo_id: RepoId,
    pattern: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "gitignore",
            move |git, _ws| git.gitignore_add(&pattern),
        ))
        .await;
    }
    let root = handle.root.clone();
    gitignore_add_sync(Path::new(&root), &pattern)
}

/// Applies a builtin gitignore template by name: appends every pattern
/// (idempotent per line, same rule as `gitignore_add`) under a
/// `# <name>` comment header in ONE write.
pub async fn gitignore_apply_template(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    let template = gitignore_templates_list()
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| format!("no gitignore template named `{name}`"))?;
    if let Some(remote) = handle.remote() {
        // The remote engine dedupes per line (exact match), so re-applying
        // a template is idempotent remotely too — header line included.
        let block = format!("# {}\n{}", template.name, template.patterns);
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "gitignore",
            move |git, _ws| git.gitignore_append_block(&block),
        ))
        .await;
    }
    let root = handle.root.clone();
    let file = std::path::Path::new(&root).join(".gitignore");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let mut updated = existing.clone();
    let fresh: Vec<&str> = template
        .patterns
        .lines()
        .filter(|line| !existing.lines().any(|old| old == *line))
        .collect();
    if fresh.is_empty() {
        return Ok(()); // everything already ignored
    }
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&format!("# {}", template.name));
    updated.push('\n');
    for line in fresh {
        updated.push_str(line);
        updated.push('\n');
    }
    std::fs::write(&file, updated).map_err(|e| format!("writing .gitignore failed: {e}"))
}

/// List every submodule with checked-out vs recorded state (contracts.md:
/// `SubmoduleInfo[]`; status strings mirror `git status` submodule wording).
pub async fn submodules(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<SubmoduleInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodules(),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| engine.submodules(repo),
    ))
    .await
}

/// `git submodule update [--init] [--recursive] <path>` for one submodule.
/// Network fetches (missing clone / missing target commit) route credential
/// prompts through the auth broker attributed to this repo.
pub async fn submodule_update(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
    init: bool,
    recursive: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine updates all submodules (no per-path argument);
        // the device session brings its own credentials, so no auth broker.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodule_update(init, recursive),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| {
            crate::auth::with_netop_context(&repo_id, || {
                engine.submodule_update(repo, &path, init, recursive)
            })
        },
    ))
    .await
}

/// `git submodule sync [path]`: rewrite the submodule URL from `.gitmodules`
/// into the repository config. `None`/empty `path` syncs all submodules.
pub async fn submodule_sync(
    services: &AppServices,
    repo_id: RepoId,
    path: Option<String>,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine syncs all submodules (no per-path argument);
        // `recursive: false` mirrors the local non-recursive sync.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "submodule",
            move |git, _ws| git.submodule_sync(false),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "submodule",
        move |_ctx, repo| engine.submodule_sync(repo, path.as_deref()),
    ))
    .await
}

/// The curated builtin gitignore templates (`{name, description, patterns}`).
pub fn gitignore_templates(_services: &AppServices) -> Vec<GitignoreTemplate> {
    gitignore_templates_list()
}

#[cfg(test)]
mod m4_gitignore_tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mygitui-gi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn gitignore_add_creates_appends_and_dedups() {
        let dir = scratch("ok");
        // Creates the file.
        gitignore_add_sync(&dir, "node_modules/").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "node_modules/\n"
        );
        // Appends, normalizing a missing trailing newline first.
        std::fs::write(dir.join(".gitignore"), "target").unwrap();
        gitignore_add_sync(&dir, "*.log").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "target\n*.log\n"
        );
        // Exact-line dedup is a no-op (whitespace-trimmed pattern).
        gitignore_add_sync(&dir, "  *.log  ").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "target\n*.log\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gitignore_add_rejects_empty_and_newlines() {
        let dir = scratch("bad");
        assert!(gitignore_add_sync(&dir, "   ").is_err());
        assert!(gitignore_add_sync(&dir, "a\nb").is_err());
        assert!(gitignore_add_sync(&dir, "a\r\nb").is_err());
        assert!(
            !dir.join(".gitignore").exists(),
            "nothing written on rejection"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn templates_are_curated_unique_and_wellformed() {
        let templates = gitignore_templates_list();
        assert!(templates.len() >= 15, "curated list: {}", templates.len());
        let mut names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "template names must be unique");
        for t in &templates {
            assert!(!t.description.is_empty(), "{}: description", t.name);
            assert!(!t.patterns.is_empty(), "{}: patterns", t.name);
            assert!(!t.patterns.contains('\r'), "{}: no CR", t.name);
            assert!(
                !t.patterns.ends_with('\n'),
                "{}: no trailing newline (FE joins lines)",
                t.name
            );
            assert!(
                t.patterns.lines().all(|l| !l.trim().is_empty()),
                "{}: no blank pattern lines",
                t.name
            );
        }
        // Serializes with the documented field names.
        let json = serde_json::to_value(&templates).unwrap();
        assert_eq!(json[0]["name"], templates[0].name);
        assert_eq!(json[0]["patterns"], templates[0].patterns);
    }
}
