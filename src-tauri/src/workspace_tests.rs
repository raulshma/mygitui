//! Tests for the workdir file-browser layer (`workspace.rs`). Every test
//! builds its own temp workdir; gitignore filtering relies on the `ignore`
//! crate running with `require_git(false)`, so no real repo is needed.

use std::fs;
use std::path::Path;

use crate::workspace::{
    compare_entries, create_entry_impl, delete_entry_impl, file_index_impl,
    file_index_impl_with_cap, list_dir_impl, natural_cmp, rename_entry_impl, stat_file_impl,
    validate_rel_path, write_file_impl, DirEntryInfo, DirEntryKind, FileContentKind,
    NewEntryKind,
};

/// Fresh, empty temp workdir with a `.git` marker dir (so helpers that
/// look for a repo behave like real workdirs).
fn temp_root(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mygitui-workspace-{}-{tag}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".git")).expect("mkdir root");
    dir
}

fn touch(root: &Path, rel: &str, content: &[u8]) {
    let full = root.join(rel);
    fs::create_dir_all(full.parent().unwrap()).expect("mkdir parents");
    fs::write(full, content).expect("write");
}

fn mkdir(root: &Path, rel: &str) {
    fs::create_dir_all(root.join(rel)).expect("mkdir");
}

// -- path validation -----------------------------------------------------------

#[test]
fn validate_accepts_simple_relative_paths() {
    let root = temp_root("validate-ok");
    let joined = validate_rel_path(&root, "src/lib/main.rs").expect("valid");
    assert_eq!(joined, root.join("src/lib/main.rs"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn validate_rejects_escape_and_absolute_forms() {
    let root = temp_root("validate-bad");
    for bad in [
        "",
        "../outside.txt",
        "src/../../outside.txt",
        if cfg!(windows) { "C:\\abs.txt" } else { "/abs.txt" },
        if cfg!(windows) { "C:drivrel.txt" } else { "/dev/null" },
        if cfg!(windows) { "\\\\server\\share" } else { "/tmp" },
    ] {
        assert!(
            validate_rel_path(&root, bad).is_err(),
            "`{bad}` must be rejected"
        );
    }
    let _ = fs::remove_dir_all(&root);
}

// -- sorting ---------------------------------------------------------------------

#[test]
fn natural_sort_numbers_within_names() {
    let mut names = vec!["f10.txt", "f2.txt", "f1.txt"];
    names.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(names, vec!["f1.txt", "f2.txt", "f10.txt"]);
}

#[test]
fn natural_sort_case_insensitive() {
    assert_eq!(natural_cmp("Apple", "banana"), std::cmp::Ordering::Less);
    assert_eq!(natural_cmp("BANANA", "apple"), std::cmp::Ordering::Greater);
}

#[test]
fn entries_sort_dirs_first_then_natural() {
    let entry = |name: &str, kind: DirEntryKind| DirEntryInfo {
        name: name.to_string(),
        path: name.to_string(),
        kind,
        size: None,
    };
    let mut entries = vec![
        entry("zebra.txt", DirEntryKind::File),
        entry("B", DirEntryKind::Dir),
        entry("a.txt", DirEntryKind::File),
        entry("sub", DirEntryKind::Submodule),
    ];
    entries.sort_by(compare_entries);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["B", "sub", "a.txt", "zebra.txt"]);
}

// -- list_dir ----------------------------------------------------------------------

#[test]
fn list_dir_respects_gitignore_until_included() {
    let root = temp_root("list-ignore");
    touch(&root, ".gitignore", b"build/\n*.log\n");
    touch(&root, "src.rs", b"");
    touch(&root, "debug.log", b"");
    mkdir(&root, "build");
    touch(&root, "build/out.txt", b"");

    let listing = list_dir_impl(&root, "", false, &[], 100).expect("listing");
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec![".gitignore", "src.rs"], "gitignored entries hidden");

    let listing = list_dir_impl(&root, "", true, &[], 100).expect("listing");
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"debug.log") && names.contains(&"build"),
        "include_ignored reveals everything: {names:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn list_dir_marks_submodules_and_sizes() {
    let root = temp_root("list-sub");
    mkdir(&root, "vendor/lib");
    fs::write(root.join("vendor/lib/.git"), b"gitdir: ../.git/modules/lib").unwrap();
    touch(&root, "vendor/lib/inner.txt", b"abc");
    touch(&root, "plain.txt", b"12345");

    // Root level: `vendor` is a plain dir; the submodule is one level down.
    let root_listing = list_dir_impl(&root, "", false, &["vendor/lib".to_string()], 100)
        .expect("listing");
    fn find<'a>(
        listing: &'a crate::workspace::DirListing,
        name: &str,
    ) -> &'a crate::workspace::DirEntryInfo {
        listing
            .entries
            .iter()
            .find(|e| e.name == name)
            .expect("entry present")
    }
    assert_eq!(find(&root_listing, "vendor").kind, DirEntryKind::Dir);
    assert_eq!(find(&root_listing, "plain.txt").size, Some(5));
    assert_eq!(find(&root_listing, "vendor").size, None, "dirs carry no size");

    // Listing `vendor` marks `lib` (rel path `vendor/lib`) as a submodule.
    let vendor_listing =
        list_dir_impl(&root, "vendor", false, &["vendor/lib".to_string()], 100)
            .expect("vendor listing");
    assert_eq!(find(&vendor_listing, "lib").kind, DirEntryKind::Submodule);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn list_dir_skips_git_and_reports_truncation() {
    let root = temp_root("list-cap");
    touch(&root, "keep.txt", b"");
    for i in 0..8 {
        touch(&root, &format!("f{i:02}.txt"), b"");
    }
    let listing = list_dir_impl(&root, "", false, &[], 5).expect("listing");
    assert!(listing.truncated);
    assert_eq!(listing.entries.len(), 5);
    assert!(listing.entries.iter().all(|e| e.name != ".git"));

    let full = list_dir_impl(&root, "", false, &[], 100).expect("listing");
    assert!(!full.truncated);
    assert_eq!(full.entries.len(), 9, "8 f-files + keep.txt (no .gitignore here)");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn list_dir_rejects_files_and_bad_paths() {
    let root = temp_root("list-bad");
    touch(&root, "file.txt", b"");
    assert!(list_dir_impl(&root, "file.txt", false, &[], 10).is_err());
    assert!(list_dir_impl(&root, "../elsewhere", false, &[], 10).is_err());
    let _ = fs::remove_dir_all(&root);
}

// -- stat_file ---------------------------------------------------------------------

#[test]
fn stat_file_classifies_image_binary_text() {
    let root = temp_root("stat-kinds");
    touch(&root, "pic.png", b"not really png");
    touch(&root, "blob.bin", &[0u8, 1, 2, 0]);
    touch(&root, "code.rs", b"fn main() {}\n");
    touch(&root, "nullless.bin", b"plain bytes");

    let png = stat_file_impl(&root, "pic.png").unwrap();
    assert_eq!(png.kind, FileContentKind::Image);
    let blob = stat_file_impl(&root, "blob.bin").unwrap();
    assert_eq!(blob.kind, FileContentKind::Binary);
    let code = stat_file_impl(&root, "code.rs").unwrap();
    assert_eq!(code.kind, FileContentKind::Text);
    assert_eq!(code.size, 13);
    assert!(!code.readonly);
    assert!(code.mtime_ms > 0);
    // No NUL in the sniff window → text despite the .bin extension.
    let nullless = stat_file_impl(&root, "nullless.bin").unwrap();
    assert_eq!(nullless.kind, FileContentKind::Text);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn stat_file_rejects_dirs_and_missing() {
    let root = temp_root("stat-bad");
    mkdir(&root, "adir");
    assert!(stat_file_impl(&root, "adir").is_err());
    assert!(stat_file_impl(&root, "missing.txt").is_err());
    assert!(stat_file_impl(&root, "../x").is_err());
    let _ = fs::remove_dir_all(&root);
}

// -- write_file ----------------------------------------------------------------------

#[test]
fn write_file_creates_and_reports_meta() {
    let root = temp_root("write-new");
    let meta = write_file_impl(&root, "deep/dir/file.txt", b"hello", Some(0)).unwrap();
    assert_eq!(fs::read(root.join("deep/dir/file.txt")).unwrap(), b"hello");
    assert_eq!(meta.size, 5);
    assert_eq!(meta.kind, FileContentKind::Text);
    assert!(meta.mtime_ms > 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn write_file_mtime_guard_conflicts_on_external_change() {
    let root = temp_root("write-conflict");
    touch(&root, "f.txt", b"v1");
    let meta = stat_file_impl(&root, "f.txt").unwrap();

    // Matching mtime: ok.
    write_file_impl(&root, "f.txt", b"v2", Some(meta.mtime_ms)).unwrap();
    assert_eq!(fs::read(root.join("f.txt")).unwrap(), b"v2");

    // Stale mtime: refused, content untouched.
    let stale = meta.mtime_ms.saturating_sub(1);
    assert!(write_file_impl(&root, "f.txt", b"v3", Some(stale)).is_err());
    assert_eq!(fs::read(root.join("f.txt")).unwrap(), b"v2");

    // Missing file with nonzero expectation (deleted externally): refused.
    fs::remove_file(root.join("f.txt")).unwrap();
    assert!(write_file_impl(&root, "f.txt", b"v3", Some(meta.mtime_ms)).is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn write_file_refuses_git_paths() {
    let root = temp_root("write-git");
    assert!(write_file_impl(&root, ".git/config", b"x", None).is_err());
    assert!(write_file_impl(&root, ".git/hooks/h", b"x", None).is_err());
    let _ = fs::remove_dir_all(&root);
}

// -- create / rename / delete ---------------------------------------------------------

#[test]
fn create_entry_makes_files_and_dirs_and_rejects_duplicates() {
    let root = temp_root("create");
    create_entry_impl(&root, "a/b/new.txt", NewEntryKind::File).unwrap();
    assert!(root.join("a/b/new.txt").is_file());
    create_entry_impl(&root, "a/b/sub", NewEntryKind::Dir).unwrap();
    assert!(root.join("a/b/sub").is_dir());
    assert!(create_entry_impl(&root, "a/b/new.txt", NewEntryKind::File).is_err());
    assert!(create_entry_impl(&root, ".git/x", NewEntryKind::File).is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rename_entry_moves_and_validates() {
    let root = temp_root("rename");
    touch(&root, "old.txt", b"data");
    mkdir(&root, "destdir");
    touch(&root, "destdir/taken.txt", b"");

    rename_entry_impl(&root, "old.txt", "destdir/renamed.txt").unwrap();
    assert!(!root.join("old.txt").exists());
    assert_eq!(
        fs::read(root.join("destdir/renamed.txt")).unwrap(),
        b"data"
    );
    assert!(rename_entry_impl(&root, "missing.txt", "x.txt").is_err());
    assert!(rename_entry_impl(&root, "destdir/renamed.txt", "destdir/taken.txt").is_err());
    assert!(rename_entry_impl(&root, "destdir", "../escape").is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn delete_entry_removes_files_and_trees() {
    let root = temp_root("delete");
    touch(&root, "file.txt", b"");
    mkdir(&root, "tree/nested/deep");
    touch(&root, "tree/nested/deep/x.txt", b"");

    delete_entry_impl(&root, "file.txt").unwrap();
    assert!(!root.join("file.txt").exists());
    delete_entry_impl(&root, "tree").unwrap();
    assert!(!root.join("tree").exists());
    assert!(delete_entry_impl(&root, "file.txt").is_err(), "gone already");
    assert!(delete_entry_impl(&root, ".git").is_err(), ".git protected");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn delete_entry_refuses_submodule_checkout() {
    let root = temp_root("delete-sub");
    mkdir(&root, "sub");
    fs::write(root.join("sub/.git"), b"gitdir: ../.git/modules/sub").unwrap();
    assert!(delete_entry_impl(&root, "sub").is_err());
    assert!(root.join("sub/.git").exists(), "submodule untouched");
    let _ = fs::remove_dir_all(&root);
}

// -- file_index -----------------------------------------------------------------------

#[test]
fn file_index_walks_respecting_gitignore() {
    let root = temp_root("index");
    touch(&root, ".gitignore", b"ignored-dir/\n");
    touch(&root, "src/a.rs", b"");
    touch(&root, "src/b.rs", b"");
    mkdir(&root, "ignored-dir");
    touch(&root, "ignored-dir/x.rs", b"");

    let index = file_index_impl(&root).expect("index");
    assert!(!index.truncated);
    let paths: Vec<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"src/a.rs"), "paths: {paths:?}");
    assert!(paths.contains(&"src"));
    assert!(
        !paths.iter().any(|p| p.contains("ignored-dir")),
        "gitignored tree skipped: {paths:?}"
    );
    assert!(!paths.iter().any(|p| *p == ".git" || p.starts_with(".git/")));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn file_index_caps_entries() {
    let root = temp_root("index-cap");
    for i in 0..40 {
        touch(&root, &format!("f{i:03}.txt"), b"");
    }
    let index = file_index_impl_with_cap(&root, 10).expect("index");
    assert!(index.truncated);
    assert_eq!(index.entries.len(), 10);
    // Under the cap: exact, no truncation.
    let index = file_index_impl_with_cap(&root, 100).expect("index");
    assert_eq!(index.entries.len(), 40);
    assert!(!index.truncated);
    let _ = fs::remove_dir_all(&root);
}
