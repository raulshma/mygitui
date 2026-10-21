//! Bounded project-type + script detection.
//!
//! `project_detect` maps one open repository to the runnable commands its
//! ecosystem exposes, so the Actions panel can surface "Run", "Watch",
//! "Test", "Build", and "Lint" without asking the user to type commands.
//!
//! Scope rules keep this cheap enough to call on panel mount:
//! - the repository root plus its immediate child directories are scanned;
//! - well-known build/vendor directories (`node_modules`, `target`, `bin`,
//!   `obj`, `.git`, `.venv`, …) are skipped;
//! - each detector reads a handful of manifest files, never the whole tree;
//! - commands are plain argv strings (executed by the existing action
//!   runner's shell), using project flags (`--project`, `--prefix`, `-C`)
//!   instead of changing the working directory.
//!
//! First-class ecosystems: Node/JS/TS, Rust, Python, .NET, Android/Gradle.
//! Cheap extras: Go, Ruby, PHP/Composer, and Makefile targets.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

/// One runnable script exposed by a detected project.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ProjectScript {
    /// Stable within the project (`<project id>:<slug>`).
    pub id: String,
    /// Human label ("dev", "test", "assembleDebug", "Run", "Watch", …).
    pub label: String,
    /// Exact command line (runs from the repository root).
    pub command: String,
    /// `run` | `watch` | `test` | `build` | `lint` | `install`.
    pub kind: String,
    /// True for long-lived watchers (dev servers, `--watch`, `--continuous`).
    pub watch: bool,
}

/// One detected project inside the repository.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ProjectInfo {
    /// `node` | `rust` | `python` | `dotnet` | `android` | `go` | `ruby` |
    /// `php` | `make`.
    pub kind: String,
    /// Manifest package/module name when available, else the directory name.
    pub name: String,
    /// Repository-relative directory (`"."` for the root), forward slashes.
    pub path: String,
    /// Manifest files that produced this entry (`package.json`, …).
    pub detected_from: Vec<String>,
    pub scripts: Vec<ProjectScript>,
}

const SKIP_DIRS: [&str; 18] = [
    ".git",
    ".hg",
    ".svn",
    ".idea",
    ".vscode",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "bin",
    "obj",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
    ".next",
    "vendor",
];

/// Maximum number of immediate child directories visited (defensive cap for
/// huge monorepos; the root is always included).
const MAX_CHILD_DIRS: usize = 64;

/// Detect projects/scripts under `root` (root + immediate children).
pub fn detect_sync(root: &Path) -> Vec<ProjectInfo> {
    let dirs = candidate_dirs(root);
    let mut out = Vec::new();

    for dir in &dirs {
        detect_node(root, dir, &mut out);
        detect_rust(root, dir, &mut out);
        detect_go(root, dir, &mut out);
    }
    // Root-only ecosystems (their manifests describe the whole repo).
    detect_python(root, &mut out);
    detect_dotnet(root, &mut out);
    detect_android(root, &mut out);
    detect_ruby(root, &mut out);
    detect_php(root, &mut out);
    detect_make(root, &mut out);

    out.sort_by(|a, b| {
        let a_root = a.path == ".";
        let b_root = b.path == ".";
        b_root
            .cmp(&a_root)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    out
}

fn candidate_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(root) {
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !entry.file_type().ok()?.is_dir() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if SKIP_DIRS.contains(&name.as_str()) {
                    return None;
                }
                Some(path)
            })
            .collect();
        children.sort();
        dirs.extend(children.into_iter().take(MAX_CHILD_DIRS));
    }

    // Monorepo workspace members (`packages/web`, `apps/api`, …) live one
    // level below the immediate children. Root manifests tell us where to
    // look without walking the tree.
    for member in workspace_dirs(root) {
        if !dirs.contains(&member) && dirs.len() < MAX_CHILD_DIRS + 32 {
            dirs.push(member);
        }
    }
    dirs
}

/// Workspace member directories declared by `package.json` `workspaces` or
/// `pnpm-workspace.yaml` (glob expansion is one directory level).
fn workspace_dirs(root: &Path) -> Vec<PathBuf> {
    let mut patterns: Vec<String> = Vec::new();
    if let Ok(text) = fs::read_to_string(root.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let workspaces = json.get("workspaces");
            let list = workspaces
                .and_then(serde_json::Value::as_array)
                .or_else(|| {
                    workspaces
                        .and_then(|value| value.get("packages"))
                        .and_then(serde_json::Value::as_array)
                });
            if let Some(list) = list {
                patterns.extend(
                    list.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
    }
    // Cargo workspace members (`crates/*`/explicit paths). The simple line
    // scanner handles the usual one-entry-per-line and inline-array forms.
    if let Ok(text) = fs::read_to_string(root.join("Cargo.toml")) {
        let mut in_workspace = false;
        let mut in_members = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_workspace = trimmed == "[workspace]";
                in_members = false;
                continue;
            }
            if !in_workspace {
                continue;
            }
            if !in_members && trimmed.starts_with("members") {
                in_members = true;
            }
            if in_members {
                let mut rest = trimmed;
                while let Some(start) = rest.find('"') {
                    rest = &rest[start + 1..];
                    let Some(end) = rest.find('"') else {
                        break;
                    };
                    patterns.push(rest[..end].to_string());
                    rest = &rest[end + 1..];
                }
                if trimmed.contains(']') {
                    in_members = false;
                }
            }
        }
    }
    if let Ok(text) = fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        let mut in_packages = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("packages:") {
                in_packages = true;
                continue;
            }
            if !in_packages {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix('-') {
                patterns.push(rest.trim().trim_matches('"').trim_matches('\'').to_string());
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                in_packages = false;
            }
        }
    }

    let mut out = Vec::new();
    for pattern in patterns {
        let pattern = pattern.trim().trim_start_matches("./");
        if pattern.is_empty() || pattern.starts_with('!') {
            continue;
        }
        let path = Path::new(pattern);
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            continue;
        }
        if let Some(star) = pattern.find('*') {
            let base = pattern[..star].trim_end_matches('/');
            if base.is_empty() {
                continue;
            }
            let Ok(entries) = fs::read_dir(root.join(base)) else {
                continue;
            };
            let mut members: Vec<PathBuf> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if !entry.file_type().ok()?.is_dir() {
                        return None;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    (!SKIP_DIRS.contains(&name.as_str())).then_some(path)
                })
                .collect();
            members.sort();
            out.extend(members);
        } else {
            let dir = root.join(pattern);
            if dir.is_dir() {
                out.push(dir);
            }
        }
    }
    out.sort();
    out.dedup();
    out.truncate(MAX_CHILD_DIRS);
    out
}

// ---------------------------------------------------------------------------
// Node / JavaScript / TypeScript
// ---------------------------------------------------------------------------

fn detect_node(root: &Path, dir: &Path, out: &mut Vec<ProjectInfo>) {
    let manifest = dir.join("package.json");
    if !manifest.is_file() {
        return;
    }
    let Ok(text) = fs::read_to_string(&manifest) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let manager = package_manager(root, dir);
    let project_path = rel_path(root, dir);
    let name = json
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| dir_name(dir));
    let project_id = format!("node:{project_path}");
    let mut scripts = Vec::new();

    if let Some(map) = json.get("scripts").and_then(serde_json::Value::as_object) {
        let sorted: BTreeMap<&str, &str> = map
            .iter()
            .filter_map(|(key, value)| Some((key.as_str(), value.as_str()?)))
            .collect();
        for (script_name, script_cmd) in sorted {
            let kind = node_script_kind(script_name, script_cmd);
            let watch = node_script_watch(script_name, script_cmd);
            scripts.push(make_script(
                &project_id,
                script_name,
                node_command(manager, &project_path, script_name),
                kind,
                watch,
            ));
        }
    }

    scripts.push(make_script(
        &project_id,
        "Install dependencies",
        node_install_command(manager, &project_path),
        "install",
        false,
    ));

    out.push(ProjectInfo {
        kind: "node".to_string(),
        name,
        path: project_path,
        detected_from: vec!["package.json".to_string()],
        scripts,
    });
}

fn package_manager(root: &Path, dir: &Path) -> &'static str {
    if find_up(root, dir, "pnpm-lock.yaml") || find_up(root, dir, "pnpm-workspace.yaml") {
        "pnpm"
    } else if find_up(root, dir, "yarn.lock") {
        "yarn"
    } else if find_up(root, dir, "bun.lockb") || find_up(root, dir, "bun.lock") {
        "bun"
    } else {
        "npm"
    }
}

fn find_up(root: &Path, start: &Path, name: &str) -> bool {
    let mut dir = Some(start);
    while let Some(current) = dir {
        if current.join(name).exists() {
            return true;
        }
        if current == root {
            break;
        }
        dir = current.parent();
    }
    false
}

fn node_command(manager: &str, dir: &str, script: &str) -> String {
    match manager {
        "pnpm" => format!("pnpm -C {} run {script}", q(dir)),
        "yarn" => format!("yarn --cwd {} run {script}", q(dir)),
        "bun" => format!("bun --cwd {} run {script}", q(dir)),
        _ => format!("npm --prefix {} run {script}", q(dir)),
    }
}

fn node_install_command(manager: &str, dir: &str) -> String {
    match manager {
        "pnpm" => format!("pnpm -C {} install", q(dir)),
        "yarn" => format!("yarn --cwd {} install", q(dir)),
        "bun" => format!("bun --cwd {} install", q(dir)),
        _ => format!("npm --prefix {} install", q(dir)),
    }
}

fn node_script_kind(name: &str, command: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    let command = command.to_ascii_lowercase();
    if name.contains("test") || name.contains("spec") || command.contains("vitest") {
        "test"
    } else if name.contains("lint")
        || name.contains("format")
        || name.contains("typecheck")
        || name.contains("type-check")
        || command.contains("eslint")
        || command.contains("prettier")
        || command.contains("tsc")
    {
        "lint"
    } else if name.contains("build") || name.contains("compile") {
        "build"
    } else {
        "run"
    }
}

fn node_script_watch(name: &str, command: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let command = command.to_ascii_lowercase();
    name.contains("watch")
        || name == "dev"
        || name.starts_with("dev:")
        || name == "serve"
        || name.starts_with("serve:")
        || command.contains("--watch")
        || command.contains("nodemon")
        || (command.contains("vite") && !command.contains("vitest"))
        || command.contains("next dev")
        || command.contains("webpack serve")
        || command.contains("react-scripts start")
}

// ---------------------------------------------------------------------------
// Rust / Cargo
// ---------------------------------------------------------------------------

fn detect_rust(root: &Path, dir: &Path, out: &mut Vec<ProjectInfo>) {
    let manifest = dir.join("Cargo.toml");
    if !manifest.is_file() {
        return;
    }
    let Ok(text) = fs::read_to_string(&manifest) else {
        return;
    };
    let project_path = rel_path(root, dir);
    let name = cargo_package_name(&text).unwrap_or_else(|| dir_name(dir));
    let manifest_arg = quoted_path(&project_path_with_file(&project_path, "Cargo.toml"));
    let project_id = format!("rust:{project_path}");
    let has_main = dir.join("src").join("main.rs").is_file();
    let is_workspace = text.lines().any(|line| line.trim() == "[workspace]");
    let mut scripts = Vec::new();

    if has_main {
        scripts.push(make_script(
            &project_id,
            "Run",
            format!("cargo run --manifest-path {manifest_arg}"),
            "run",
            false,
        ));
    }
    scripts.push(make_script(
        &project_id,
        "Test",
        format!("cargo test --manifest-path {manifest_arg}"),
        "test",
        false,
    ));
    scripts.push(make_script(
        &project_id,
        "Build",
        format!("cargo build --manifest-path {manifest_arg}"),
        "build",
        false,
    ));
    scripts.push(make_script(
        &project_id,
        "Clippy",
        format!("cargo clippy --manifest-path {manifest_arg} --all-targets"),
        "lint",
        false,
    ));
    if has_main && project_path == "." {
        scripts.push(make_script(
            &project_id,
            "Watch (cargo-watch)",
            "cargo watch -x run".to_string(),
            "watch",
            true,
        ));
    }
    if is_workspace && project_path == "." {
        scripts.push(make_script(
            &project_id,
            "Test workspace",
            "cargo test --workspace".to_string(),
            "test",
            false,
        ));
    }

    out.push(ProjectInfo {
        kind: "rust".to_string(),
        name,
        path: project_path,
        detected_from: vec!["Cargo.toml".to_string()],
        scripts,
    });
}

fn cargo_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = line.strip_prefix("name") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn detect_python(root: &Path, out: &mut Vec<ProjectInfo>) {
    let signals = [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "Pipfile",
        "manage.py",
        "main.py",
        "app.py",
    ];
    let mut detected_from = Vec::new();
    for signal in signals {
        if root.join(signal).is_file() {
            detected_from.push(signal.to_string());
        }
    }
    if detected_from.is_empty() {
        return;
    }

    let name = read_toml_name(&root.join("pyproject.toml"))
        .or_else(|| {
            fs::read_to_string(root.join("setup.py"))
                .ok()
                .and_then(|text| quoted_value_after(&text, "name="))
        })
        .unwrap_or_else(|| dir_name(root));
    let project_id = "python:.";
    let mut scripts = Vec::new();
    let has_manage = root.join("manage.py").is_file();
    let has_main = root.join("main.py").is_file();
    let has_app = root.join("app.py").is_file();
    let has_pyproject = root.join("pyproject.toml").is_file();
    let has_tests = root.join("tests").is_dir() || root.join("test").is_dir();
    let manifest_hint = fs::read_to_string(root.join("pyproject.toml"))
        .unwrap_or_default()
        .to_ascii_lowercase()
        + &fs::read_to_string(root.join("requirements.txt"))
            .unwrap_or_default()
            .to_ascii_lowercase();

    if has_manage {
        scripts.push(make_script(
            project_id,
            "Django dev server",
            "python manage.py runserver".to_string(),
            "run",
            true,
        ));
    } else if has_main {
        scripts.push(make_script(
            project_id,
            "Run main.py",
            "python main.py".to_string(),
            "run",
            false,
        ));
    } else if has_app {
        scripts.push(make_script(
            project_id,
            "Run app.py",
            "python app.py".to_string(),
            "run",
            false,
        ));
    }

    if has_tests || manifest_hint.contains("pytest") {
        scripts.push(make_script(
            project_id,
            "Test (pytest)",
            "python -m pytest".to_string(),
            "test",
            false,
        ));
    }
    if has_pyproject {
        scripts.push(make_script(
            project_id,
            "Build package",
            "python -m build".to_string(),
            "build",
            false,
        ));
    }
    if manifest_hint.contains("watchfiles") && (has_main || has_app) {
        let file = if has_main { "main.py" } else { "app.py" };
        scripts.push(make_script(
            project_id,
            "Watch (watchfiles)",
            format!("watchfiles \"python {file}\""),
            "watch",
            true,
        ));
    }

    let install = if root.join("uv.lock").is_file() {
        Some("uv sync")
    } else if root.join("poetry.lock").is_file() {
        Some("poetry install")
    } else if root.join("Pipfile").is_file() {
        Some("pipenv install")
    } else if root.join("requirements.txt").is_file() {
        Some("pip install -r requirements.txt")
    } else if has_pyproject {
        Some("pip install -e .")
    } else {
        None
    };
    if let Some(install) = install {
        scripts.push(make_script(
            project_id,
            "Install dependencies",
            install.to_string(),
            "install",
            false,
        ));
    }

    out.push(ProjectInfo {
        kind: "python".to_string(),
        name,
        path: ".".to_string(),
        detected_from,
        scripts,
    });
}

fn read_toml_name(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_name_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_name_section = line == "[project]" || line == "[tool.poetry]";
            continue;
        }
        if in_name_section {
            if let Some(rest) = line.strip_prefix("name") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_string());
            }
        }
    }
    None
}

fn quoted_value_after(text: &str, needle: &str) -> Option<String> {
    let idx = text.find(needle)? + needle.len();
    let rest = text[idx..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)?;
    Some(rest[1..1 + end].to_string())
}

// ---------------------------------------------------------------------------
// .NET
// ---------------------------------------------------------------------------

fn dotnet_entry_project(root: &Path, projects: &[PathBuf], solution: &Path) -> Option<String> {
    let stem = file_stem(solution);
    let by_name = projects.iter().find(|path| file_stem(path) == stem);
    let executable = || {
        projects.iter().find(|path| {
            fs::read_to_string(path)
                .map(|text| {
                    text.contains("<OutputType>Exe") || text.contains("Microsoft.NET.Sdk.Web")
                })
                .unwrap_or(false)
        })
    };
    by_name
        .or_else(executable)
        .or_else(|| projects.first())
        .map(|path| rel_path(root, path))
}

fn detect_dotnet(root: &Path, out: &mut Vec<ProjectInfo>) {
    let solution = manifest_files(root, &["sln", "slnx"]).into_iter().next();
    // .NET solutions usually keep projects under `src/<Project>/`; scan a
    // bounded depth instead of only the immediate children.
    let projects = find_manifests_bounded(root, &["csproj", "fsproj", "vbproj"], 3, 64);

    if let Some(solution) = solution {
        let project_path = rel_path(root, &solution);
        let solution_arg = q(&project_path);
        let project = dotnet_entry_project(root, &projects, &solution);
        let project_id = format!("dotnet:{project_path}");
        let mut scripts = Vec::new();
        if let Some(project) = &project {
            let project_arg = q(project);
            scripts.push(make_script(
                &project_id,
                "Run",
                format!("dotnet run --project {project_arg}"),
                "run",
                false,
            ));
            scripts.push(make_script(
                &project_id,
                "Watch (dotnet watch)",
                format!("dotnet watch --project {project_arg} run"),
                "watch",
                true,
            ));
        }
        scripts.push(make_script(
            &project_id,
            "Build",
            format!("dotnet build {solution_arg}"),
            "build",
            false,
        ));
        scripts.push(make_script(
            &project_id,
            "Test",
            format!("dotnet test {solution_arg}"),
            "test",
            false,
        ));
        out.push(ProjectInfo {
            kind: "dotnet".to_string(),
            name: file_stem(&solution),
            path: solution
                .parent()
                .map(|p| rel_path(root, p))
                .unwrap_or_else(|| ".".into()),
            detected_from: vec![file_name(&solution)],
            scripts,
        });
        return;
    }

    for project in projects {
        let project_path = rel_path(root, &project);
        let project_arg = q(&project_path);
        let project_id = format!("dotnet:{project_path}");
        let scripts = vec![
            make_script(
                &project_id,
                "Run",
                format!("dotnet run --project {project_arg}"),
                "run",
                false,
            ),
            make_script(
                &project_id,
                "Build",
                format!("dotnet build {project_arg}"),
                "build",
                false,
            ),
            make_script(
                &project_id,
                "Test",
                format!("dotnet test {project_arg}"),
                "test",
                false,
            ),
            make_script(
                &project_id,
                "Watch (dotnet watch)",
                format!("dotnet watch --project {project_arg} run"),
                "watch",
                true,
            ),
        ];
        out.push(ProjectInfo {
            kind: "dotnet".to_string(),
            name: file_stem(&project),
            path: project
                .parent()
                .map(|p| rel_path(root, p))
                .unwrap_or_else(|| ".".into()),
            detected_from: vec![file_name(&project)],
            scripts,
        });
    }
}

// ---------------------------------------------------------------------------
// Android / Gradle
// ---------------------------------------------------------------------------

/// True when a Gradle script applies an Android plugin. Covers Groovy/Kotlin
/// `com.android.*` ids and version-catalog aliases such as
/// `alias(libs.plugins.android.application)`.
fn android_marker(text: &str) -> bool {
    text.contains("com.android")
        || text.contains("libs.plugins.android")
        || (text.contains("android {") && text.contains("compileSdk"))
}

fn detect_android(root: &Path, out: &mut Vec<ProjectInfo>) {
    // Settings/root scripts plus the usual `app/` module and any immediate
    // child module (version catalogs often declare Android outside `app/`).
    let mut gradle_files: Vec<PathBuf> = [
        "build.gradle",
        "build.gradle.kts",
        "app/build.gradle",
        "app/build.gradle.kts",
    ]
    .iter()
    .map(|rel| root.join(rel))
    .filter(|path| path.is_file())
    .collect();
    if let Ok(entries) = fs::read_dir(root) {
        let mut visited_dirs = 0usize;
        for entry in entries.flatten() {
            if !entry.file_type().ok().is_some_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            visited_dirs += 1;
            if visited_dirs > MAX_CHILD_DIRS {
                break;
            }
            for manifest in ["build.gradle", "build.gradle.kts"] {
                let path = entry.path().join(manifest);
                if path.is_file() {
                    gradle_files.push(path);
                }
            }
        }
    }
    gradle_files.sort();
    gradle_files.dedup();

    let android_files: Vec<PathBuf> = gradle_files
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .map(|text| android_marker(&text))
                .unwrap_or(false)
        })
        .collect();
    if android_files.is_empty() {
        // A bare wrapper without an Android plugin is just Gradle, not a
        // recognizable Android app; keep detection honest.
        return;
    }

    let wrapper = if cfg!(windows) && root.join("gradlew.bat").is_file() {
        "gradlew.bat"
    } else if root.join("gradlew").is_file() {
        "./gradlew"
    } else {
        "gradle"
    };
    let project_id = "android:.";
    let mut detected_from = Vec::new();
    for path in &android_files {
        detected_from.push(rel_path(root, path));
    }
    for rel in ["settings.gradle", "settings.gradle.kts"] {
        if root.join(rel).is_file() {
            detected_from.push(rel.to_string());
        }
    }
    detected_from.sort();
    detected_from.dedup();
    detected_from.truncate(6);
    let scripts = vec![
        make_script(
            project_id,
            "Build debug",
            format!("{wrapper} assembleDebug"),
            "build",
            false,
        ),
        make_script(
            project_id,
            "Install debug",
            format!("{wrapper} installDebug"),
            "run",
            false,
        ),
        make_script(
            project_id,
            "Unit tests",
            format!("{wrapper} testDebugUnitTest"),
            "test",
            false,
        ),
        make_script(
            project_id,
            "Lint",
            format!("{wrapper} lintDebug"),
            "lint",
            false,
        ),
        make_script(
            project_id,
            "Watch build (Gradle continuous)",
            format!("{wrapper} --continuous assembleDebug"),
            "watch",
            true,
        ),
    ];
    out.push(ProjectInfo {
        kind: "android".to_string(),
        name: dir_name(root),
        path: ".".to_string(),
        detected_from,
        scripts,
    });
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn detect_go(root: &Path, dir: &Path, out: &mut Vec<ProjectInfo>) {
    let manifest = dir.join("go.mod");
    if !manifest.is_file() {
        return;
    }
    let Ok(text) = fs::read_to_string(&manifest) else {
        return;
    };
    let project_path = rel_path(root, dir);
    let name = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .map(str::to_owned)
        .unwrap_or_else(|| dir_name(dir));
    let dir_arg = q(&project_path);
    let project_id = format!("go:{project_path}");
    let scripts = vec![
        make_script(
            &project_id,
            "Run",
            format!("go -C {dir_arg} run ."),
            "run",
            false,
        ),
        make_script(
            &project_id,
            "Test",
            format!("go -C {dir_arg} test ./..."),
            "test",
            false,
        ),
        make_script(
            &project_id,
            "Build",
            format!("go -C {dir_arg} build ./..."),
            "build",
            false,
        ),
        make_script(
            &project_id,
            "Vet",
            format!("go -C {dir_arg} vet ./..."),
            "lint",
            false,
        ),
    ];
    out.push(ProjectInfo {
        kind: "go".to_string(),
        name,
        path: project_path,
        detected_from: vec!["go.mod".to_string()],
        scripts,
    });
}

// ---------------------------------------------------------------------------
// Ruby
// ---------------------------------------------------------------------------

fn detect_ruby(root: &Path, out: &mut Vec<ProjectInfo>) {
    let gemfile = root.join("Gemfile");
    if !gemfile.is_file() {
        return;
    }
    let project_id = "ruby:.";
    let rails = root.join("bin").join("rails").is_file();
    let rspec = root.join("spec").is_dir();
    let mut scripts = Vec::new();
    if rails {
        scripts.push(make_script(
            project_id,
            "Rails server",
            "bundle exec rails server".to_string(),
            "run",
            true,
        ));
        scripts.push(make_script(
            project_id,
            "Rails tests",
            "bundle exec rails test".to_string(),
            "test",
            false,
        ));
    } else if rspec {
        scripts.push(make_script(
            project_id,
            "RSpec",
            "bundle exec rspec".to_string(),
            "test",
            false,
        ));
    } else if root.join("Rakefile").is_file() {
        scripts.push(make_script(
            project_id,
            "Rake",
            "bundle exec rake".to_string(),
            "test",
            false,
        ));
    }
    scripts.push(make_script(
        project_id,
        "Install dependencies",
        "bundle install".to_string(),
        "install",
        false,
    ));
    out.push(ProjectInfo {
        kind: "ruby".to_string(),
        name: dir_name(root),
        path: ".".to_string(),
        detected_from: vec!["Gemfile".to_string()],
        scripts,
    });
}

// ---------------------------------------------------------------------------
// PHP / Composer
// ---------------------------------------------------------------------------

fn detect_php(root: &Path, out: &mut Vec<ProjectInfo>) {
    let manifest = root.join("composer.json");
    if !manifest.is_file() {
        return;
    }
    let Ok(text) = fs::read_to_string(&manifest) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let name = json
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| dir_name(root));
    let project_id = "php:.";
    let mut scripts = Vec::new();
    if let Some(map) = json.get("scripts").and_then(serde_json::Value::as_object) {
        let sorted: BTreeMap<&str, &str> = map
            .iter()
            .filter_map(|(key, value)| Some((key.as_str(), value.as_str()?)))
            .collect();
        for (script_name, script_cmd) in sorted {
            let kind = node_script_kind(script_name, script_cmd);
            let watch = node_script_watch(script_name, script_cmd);
            scripts.push(make_script(
                project_id,
                script_name,
                format!("composer run {script_name}"),
                kind,
                watch,
            ));
        }
    }
    scripts.push(make_script(
        project_id,
        "Install dependencies",
        "composer install".to_string(),
        "install",
        false,
    ));
    out.push(ProjectInfo {
        kind: "php".to_string(),
        name,
        path: ".".to_string(),
        detected_from: vec!["composer.json".to_string()],
        scripts,
    });
}

// ---------------------------------------------------------------------------
// Makefile (generic fallback)
// ---------------------------------------------------------------------------

fn detect_make(root: &Path, out: &mut Vec<ProjectInfo>) {
    let makefile = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());
    let Some(makefile) = makefile else {
        return;
    };
    let Ok(text) = fs::read_to_string(&makefile) else {
        return;
    };
    let project_id = "make:.";
    let mut seen = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('\t') || line.starts_with(' ') || line.starts_with('#') {
            continue;
        }
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if right.trim_start().starts_with('=') {
            // Variable assignment (`VAR := value`), not a target.
            continue;
        }
        if left.contains('=') || left.contains('$') || left.contains('%') {
            continue;
        }
        let Some(target) = left.split_whitespace().next() else {
            continue;
        };
        if target.is_empty()
            || target.starts_with('.')
            || target.contains('/')
            || target.contains('\\')
            || seen.contains_key(target)
        {
            continue;
        }
        if !target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'))
        {
            continue;
        }
        let kind = if target.contains("test") || target.contains("check") {
            "test"
        } else if target.contains("lint") {
            "lint"
        } else if target.contains("build") || target == "all" {
            "build"
        } else {
            "run"
        };
        let watch = target.contains("watch")
            || target.contains("dev")
            || target == "serve"
            || target == "run";
        seen.insert(
            target.to_string(),
            make_script(project_id, target, format!("make {target}"), kind, watch),
        );
        if seen.len() >= 20 {
            break;
        }
    }
    if seen.is_empty() {
        return;
    }
    out.push(ProjectInfo {
        kind: "make".to_string(),
        name: dir_name(root),
        path: ".".to_string(),
        detected_from: vec![file_name(&makefile)],
        scripts: seen.into_values().collect(),
    });
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_script(
    project_id: &str,
    label: &str,
    command: String,
    kind: &str,
    watch: bool,
) -> ProjectScript {
    ProjectScript {
        id: format!("{project_id}:{}", slug(label)),
        label: label.to_string(),
        command,
        kind: kind.to_string(),
        watch,
    }
}

fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let text = rel.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

fn project_path_with_file(dir: &str, file: &str) -> String {
    if dir == "." {
        file.to_string()
    } else {
        format!("{dir}/{file}")
    }
}

fn q(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "'"))
}

fn quoted_path(path: &str) -> String {
    q(path)
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| dir_name(path))
}

fn manifest_files(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
            extensions.contains(&ext.as_str()).then_some(path)
        })
        .collect();
    paths.sort();
    paths
}

/// Depth-bounded manifest search (used by .NET where `src/<Project>/` is the
/// norm). Skips vendor/build directories and caps both visited directories
/// and returned manifests.
fn find_manifests_bounded(
    root: &Path,
    extensions: &[&str],
    max_depth: usize,
    cap: usize,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        if visited >= cap || out.len() >= cap {
            break;
        }
        visited += 1;
        out.extend(manifest_files(&dir, extensions));
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !entry.file_type().ok()?.is_dir() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if SKIP_DIRS.contains(&name.as_str()) {
                    return None;
                }
                Some(path)
            })
            .collect();
        children.sort();
        // LIFO: push reversed so the smallest path is visited first.
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1));
        }
    }
    out.sort();
    out.dedup();
    out.truncate(cap);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempRepo {
        root: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "mygitui-projects-{tag}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.root.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, text).unwrap();
            path
        }

        fn detect(&self) -> Vec<ProjectInfo> {
            detect_sync(&self.root)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn project<'a>(projects: &'a [ProjectInfo], kind: &str) -> &'a ProjectInfo {
        projects
            .iter()
            .find(|project| project.kind == kind)
            .unwrap_or_else(|| panic!("missing {kind} project: {projects:?}"))
    }

    #[test]
    fn detects_node_scripts_package_manager_and_watch() {
        let repo = TempRepo::new("node");
        repo.write("pnpm-lock.yaml", "");
        repo.write(
            "package.json",
            r#"{
              "name": "acme-web",
              "scripts": {
                "dev": "vite",
                "build": "vite build",
                "test": "vitest run",
                "lint": "eslint ."
              }
            }"#,
        );
        let projects = repo.detect();
        let node = project(&projects, "node");
        assert_eq!(node.name, "acme-web");
        assert_eq!(node.path, ".");
        assert_eq!(node.detected_from, ["package.json"]);
        let dev = node.scripts.iter().find(|s| s.label == "dev").unwrap();
        assert!(dev.watch);
        assert_eq!(dev.kind, "run");
        assert_eq!(dev.command, "pnpm -C \".\" run dev");
        let test = node.scripts.iter().find(|s| s.label == "test").unwrap();
        assert_eq!(test.kind, "test");
        assert!(!test.watch);
        assert!(node
            .scripts
            .iter()
            .any(|s| s.kind == "install" && s.command == "pnpm -C \".\" install"));
    }

    #[test]
    fn detects_child_node_packages_and_skips_node_modules() {
        let repo = TempRepo::new("monorepo");
        repo.write("package-lock.json", "{}");
        repo.write(
            "package.json",
            r#"{ "name": "acme-monorepo", "private": true, "workspaces": ["packages/*"] }"#,
        );
        repo.write(
            "packages/web/package.json",
            r#"{ "name": "@acme/web", "scripts": { "start": "next dev" } }"#,
        );
        repo.write(
            "node_modules/ignored/package.json",
            r#"{ "name": "ignored", "scripts": { "test": "false" } }"#,
        );
        let projects = repo.detect();
        let web = projects
            .iter()
            .find(|p| p.kind == "node" && p.path == "packages/web")
            .expect("workspace package detected");
        assert!(
            !projects.iter().any(|p| p.path.contains("node_modules")),
            "node_modules is skipped"
        );
        assert_eq!(
            web.scripts[0].command,
            "npm --prefix \"packages/web\" run start"
        );
        assert!(web.scripts[0].watch);
    }

    #[test]
    fn detects_rust_package_and_workspace_commands() {
        let repo = TempRepo::new("rust");
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"fastgit\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/*\"]\n",
        );
        repo.write("src/main.rs", "fn main() {}");
        repo.write(
            "crates/core/Cargo.toml",
            "[package]\nname = \"fastgit-core\"\nversion = \"0.1.0\"\n",
        );
        repo.write("crates/core/src/lib.rs", "");
        let projects = repo.detect();
        let rust = projects
            .iter()
            .find(|project| project.kind == "rust" && project.path == ".")
            .expect("root rust project");
        assert_eq!(rust.name, "fastgit");
        assert!(
            projects.iter().any(|project| project.kind == "rust"
                && project.path == "crates/core"
                && project.name == "fastgit-core"),
            "cargo workspace member detected: {projects:?}"
        );
        let run = rust.scripts.iter().find(|s| s.kind == "run").unwrap();
        assert_eq!(run.command, "cargo run --manifest-path \"Cargo.toml\"");
        assert!(rust
            .scripts
            .iter()
            .any(|s| s.label == "Watch (cargo-watch)" && s.watch));
        assert!(rust
            .scripts
            .iter()
            .any(|s| s.label == "Test workspace" && s.kind == "test"));
    }

    #[test]
    fn detects_python_django_and_test_tools() {
        let repo = TempRepo::new("python");
        repo.write(
            "pyproject.toml",
            "[project]\nname = \"widget-api\"\n\n[tool.poetry]\nname = \"ignored\"\n",
        );
        repo.write("requirements.txt", "django\npytest\n");
        repo.write("manage.py", "print('ok')\n");
        fs::create_dir_all(repo.root.join("tests")).unwrap();
        let projects = repo.detect();
        let python = project(&projects, "python");
        assert_eq!(python.name, "widget-api");
        let server = python
            .scripts
            .iter()
            .find(|s| s.label == "Django dev server")
            .unwrap();
        assert!(server.watch);
        assert_eq!(server.command, "python manage.py runserver");
        assert!(python
            .scripts
            .iter()
            .any(|s| s.kind == "test" && s.command == "python -m pytest"));
        assert!(python
            .scripts
            .iter()
            .any(|s| s.kind == "install" && s.command == "pip install -r requirements.txt"));
    }

    #[test]
    fn detects_dotnet_solution() {
        let repo = TempRepo::new("dotnet");
        repo.write("Acme.sln", "Microsoft Visual Studio Solution File");
        repo.write(
            "src/Acme/Acme.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>",
        );
        let projects = repo.detect();
        let dotnet = project(&projects, "dotnet");
        assert_eq!(dotnet.name, "Acme");
        assert!(dotnet
            .scripts
            .iter()
            .any(|s| s.command == "dotnet build \"Acme.sln\""));
        assert!(dotnet
            .scripts
            .iter()
            .any(|s| s.command == "dotnet run --project \"src/Acme/Acme.csproj\""));
        assert!(dotnet
            .scripts
            .iter()
            .any(|s| s.kind == "watch" && s.command.contains("dotnet watch")));
    }

    #[test]
    fn detects_android_gradle_wrapper() {
        let repo = TempRepo::new("android");
        repo.write("settings.gradle.kts", "include(\":app\")");
        repo.write(
            "app/build.gradle.kts",
            "plugins { id(\"com.android.application\") }",
        );
        repo.write("gradlew.bat", "");
        let projects = repo.detect();
        let android = project(&projects, "android");
        let wrapper = if cfg!(windows) {
            "gradlew.bat"
        } else {
            "./gradlew"
        };
        assert_eq!(android.path, ".");
        assert!(android
            .scripts
            .iter()
            .any(|s| s.command == format!("{wrapper} assembleDebug")));
        assert!(android
            .scripts
            .iter()
            .any(|s| s.watch && s.command.contains("--continuous")));
    }

    #[test]
    fn detects_android_version_catalog_modules() {
        let repo = TempRepo::new("android-catalog");
        repo.write("settings.gradle.kts", "include(\":mobile\")");
        repo.write(
            "build.gradle.kts",
            "plugins { alias(libs.plugins.android.application) apply false }",
        );
        repo.write(
            "mobile/build.gradle.kts",
            "plugins { alias(libs.plugins.android.application) }\nandroid { compileSdk = 36 }",
        );
        repo.write("gradlew", "");
        let projects = repo.detect();
        let android = project(&projects, "android");
        assert!(android
            .detected_from
            .iter()
            .any(|file| file == "mobile/build.gradle.kts"));
        assert!(android
            .scripts
            .iter()
            .any(|script| script.command.contains("assembleDebug")));
    }

    #[test]
    fn detects_go_ruby_php_and_make() {
        let repo = TempRepo::new("extras");
        repo.write("go.mod", "module example.com/acme\n");
        repo.write("Gemfile", "source \"https://rubygems.org\"\n");
        repo.write("spec/.keep", "");
        repo.write(
            "composer.json",
            r#"{ "name": "acme/api", "scripts": { "test": "phpunit", "serve": "php -S localhost:8000" } }"#,
        );
        repo.write(
            "Makefile",
            "VERSION := 1.0\nbuild:\n\t@echo build\ntest:\n\t@echo test\nwatch:\n\t@echo watch\n",
        );
        let projects = repo.detect();
        assert_eq!(project(&projects, "go").name, "example.com/acme");
        assert!(project(&projects, "ruby")
            .scripts
            .iter()
            .any(|s| s.command == "bundle exec rspec"));
        assert!(project(&projects, "php")
            .scripts
            .iter()
            .any(|s| s.label == "serve" && s.watch));
        let make = project(&projects, "make");
        assert_eq!(
            make.scripts
                .iter()
                .map(|s| s.label.as_str())
                .collect::<Vec<_>>(),
            ["build", "test", "watch"]
        );
        assert!(make.scripts.iter().any(|s| s.label == "watch" && s.watch));
    }

    #[test]
    fn dotnet_solution_prefers_a_runnable_project() {
        let repo = TempRepo::new("dotnet-entry");
        repo.write("Acme.sln", "Microsoft Visual Studio Solution File");
        repo.write(
            "src/Acme.Library/Acme.Library.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Library</OutputType></PropertyGroup></Project>",
        );
        repo.write(
            "src/Acme.App/Acme.App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType></PropertyGroup></Project>",
        );
        let projects = repo.detect();
        let dotnet = project(&projects, "dotnet");
        assert!(dotnet
            .scripts
            .iter()
            .any(|script| script.command.contains("Acme.App.csproj")));
        assert!(!dotnet
            .scripts
            .iter()
            .any(|script| script.command.contains("Acme.Library.csproj")));
    }

    #[test]
    fn empty_repo_yields_no_projects() {
        let repo = TempRepo::new("empty");
        assert!(repo.detect().is_empty());
    }

    #[test]
    fn script_ids_are_stable_and_unique() {
        let repo = TempRepo::new("ids");
        repo.write(
            "package.json",
            r#"{ "name": "ids", "scripts": { "dev": "vite", "build": "vite build" } }"#,
        );
        let node = project(&repo.detect(), "node").clone();
        let ids: Vec<&str> = node.scripts.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "two scripts + install");
        assert_eq!(new_set_len(&ids), ids.len());
        assert!(ids.iter().all(|id| id.starts_with("node:.:")));
    }

    fn new_set_len(values: &[&str]) -> usize {
        values
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}
