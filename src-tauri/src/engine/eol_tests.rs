//! eol-awareness tests (phase 5 item 49): discard/checkout-style restores
//! and hunk staging under `core.autocrlf` + `.gitattributes`.
//!
//! Conversion is CONFIG-driven, not host-driven: every test sets
//! `core.autocrlf` explicitly in its temp repo and asserts the same bytes on
//! all platforms (git's autocrlf=true smudges CRLF on checkout everywhere;
//! only git's `text`-attribute native-eol rule is host-dependent, and that
//! rule is deliberately simplified — see `eol.rs`).

use std::path::{Path, PathBuf};

use git2::{Repository, RepositoryInitOptions};

use super::git_engine::GitEngine;
use super::libgit2::Libgit2Engine;
use super::types::{StageRequest, StageTarget};

static ENGINE: Libgit2Engine = Libgit2Engine::new();

struct TempRepo {
    _dir: PathBuf,
    repo: Repository,
}

impl TempRepo {
    fn new(name: &str, autocrlf: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("mygitui-eol-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let mut opts = RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(&dir, &opts).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Eol Test")?;
                c.set_str("user.email", "eol@test.local")?;
                c.set_str("core.autocrlf", autocrlf)
            })
            .expect("configure identity");
        TempRepo { _dir: dir, repo }
    }

    /// Write raw bytes into the workdir (no conversion — the test decides
    /// the exact bytes on disk).
    fn write_bytes(&self, path: &str, content: &[u8]) {
        let file = self.repo.workdir().unwrap().join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(file, content).expect("write file");
    }

    /// Stage one path through the index (`index.add_path` — the same filter
    /// path whole-file staging uses) and commit on HEAD.
    fn add_and_commit(&self, path: &str, message: &str) {
        let mut index = self.repo.index().expect("index");
        index.add_path(Path::new(path)).expect("stage add");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("tree");
        let tree = self.repo.find_tree(tree_oid).expect("tree");
        let sig = self.repo.signature().expect("signature");
        let mut parents = Vec::new();
        if let Ok(head) = self.repo.head() {
            parents.push(head.peel_to_commit().expect("head commit"));
        }
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit");
    }

    /// LF bytes of the index blob for `path` (what `git add` stored).
    fn index_blob(&self, path: &str) -> Vec<u8> {
        let index = self.repo.index().expect("index");
        let entry = index
            .get_path(Path::new(path), 0)
            .unwrap_or_else(|| panic!("{path} not in index"));
        self.repo
            .find_blob(entry.id)
            .expect("blob")
            .content()
            .to_vec()
    }

    fn workdir_bytes(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.repo.workdir().unwrap().join(path)).expect("read workdir file")
    }
}

impl std::ops::Deref for TempRepo {
    type Target = Repository;

    fn deref(&self) -> &Repository {
        &self.repo
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let dir = self._dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

fn file_target(path: &str) -> StageTarget {
    StageTarget::File(path.to_string())
}

// ---------------------------------------------------------------------------
// discard (checkout-style restore)
// ---------------------------------------------------------------------------

#[test]
fn discard_file_restores_crlf_when_autocrlf_true() {
    let temp = TempRepo::new("discard-crlf", "true");
    // The workdir file is CRLF (what checkout writes) while the committed
    // blob is LF — `index.add_path` runs the same clean filter.
    temp.write_bytes("a.txt", b"a\r\nb\r\n");
    temp.add_and_commit("a.txt", "base");

    // Modify, then discard: the restore must smudge like checkout, not
    // write the raw LF blob bytes.
    temp.write_bytes("a.txt", b"a\r\nb\r\nstaged edit\r\n");
    ENGINE
        .discard(&temp, &[file_target("a.txt")])
        .expect("discard");

    assert_eq!(temp.workdir_bytes("a.txt"), b"a\r\nb\r\n".to_vec());
    assert_eq!(temp.index_blob("a.txt"), b"a\nb\n".to_vec());
}

#[test]
fn discard_file_keeps_lf_when_autocrlf_false() {
    let temp = TempRepo::new("discard-lf", "false");
    temp.write_bytes("a.txt", b"a\nb\n");
    temp.add_and_commit("a.txt", "base");

    temp.write_bytes("a.txt", b"a\nb\nedit\n");
    ENGINE
        .discard(&temp, &[file_target("a.txt")])
        .expect("discard");

    // No conversion configured: byte-exact blob restore (historical behavior).
    assert_eq!(temp.workdir_bytes("a.txt"), b"a\nb\n".to_vec());
}

#[test]
fn discard_file_untouched_for_binary() {
    let temp = TempRepo::new("discard-binary", "true");
    let blob_bytes: &[u8] = b"a\n\x00b\n";
    temp.write_bytes("a.bin", blob_bytes);
    temp.add_and_commit("a.bin", "base");

    temp.write_bytes("a.bin", b"a\n\x00b\nedit\n");
    ENGINE
        .discard(&temp, &[file_target("a.bin")])
        .expect("discard binary");

    // NUL sniff: no conversion, byte-exact blob restore.
    assert_eq!(temp.workdir_bytes("a.bin"), blob_bytes.to_vec());
}

#[test]
fn gitattributes_text_false_suppresses_conversion() {
    let temp = TempRepo::new("discard-attr-off", "true");
    temp.write_bytes(".gitattributes", b"*.txt -text\n");
    temp.add_and_commit(".gitattributes", "attrs");
    temp.write_bytes("a.txt", b"a\nb\n");
    temp.add_and_commit("a.txt", "base");

    temp.write_bytes("a.txt", b"a\nb\nedit\n");
    ENGINE
        .discard(&temp, &[file_target("a.txt")])
        .expect("discard");

    // `-text` wins over autocrlf=true: the blob bytes land verbatim.
    assert_eq!(temp.workdir_bytes("a.txt"), b"a\nb\n".to_vec());
}

#[test]
fn gitattributes_eol_lf_suppresses_conversion() {
    let temp = TempRepo::new("discard-attr-lf", "true");
    temp.write_bytes(".gitattributes", b"*.txt text eol=lf\n");
    temp.add_and_commit(".gitattributes", "attrs");
    temp.write_bytes("a.txt", b"a\nb\n");
    temp.add_and_commit("a.txt", "base");

    temp.write_bytes("a.txt", b"a\nb\nedit\n");
    ENGINE
        .discard(&temp, &[file_target("a.txt")])
        .expect("discard");

    // `eol=lf` overrides autocrlf=true: the workdir stays LF.
    assert_eq!(temp.workdir_bytes("a.txt"), b"a\nb\n".to_vec());
}

#[test]
fn discard_hunks_re_smudge_restored_lines_to_crlf() {
    let temp = TempRepo::new("discard-hunk-crlf", "true");
    temp.write_bytes("a.txt", b"a\nb\n");
    temp.add_and_commit("a.txt", "base");

    // Workdir is CRLF (checkout style) with an added line; the discard
    // surgery walks the raw workdir bytes but restores patch content that
    // comes out of the odb-normalized diff as LF — the restored context
    // lines must land back as CRLF or the file ends mixed.
    temp.write_bytes("a.txt", b"a\r\nb\r\nextra\r\n");
    ENGINE
        .discard(
            &temp,
            &[StageTarget::Hunk {
                path: "a.txt".to_string(),
                hunk: 0,
            }],
        )
        .expect("discard hunk");

    assert_eq!(
        temp.workdir_bytes("a.txt"),
        b"a\r\nb\r\n".to_vec(),
        "context lines re-smudged, addition dropped, no mixed endings"
    );
    assert_eq!(
        temp.index_blob("a.txt"),
        b"a\nb\n".to_vec(),
        "index untouched"
    );
}

// ---------------------------------------------------------------------------
// hunk staging (clean-style index writes)
// ---------------------------------------------------------------------------

#[test]
fn hunk_stage_of_untracked_crlf_file_stores_lf_blob() {
    let temp = TempRepo::new("stage-untracked-crlf", "true");
    temp.write_bytes("new.txt", b"l1\r\nl2\r\n");

    ENGINE
        .stage(
            &temp,
            &StageRequest {
                targets: vec![StageTarget::Hunk {
                    path: "new.txt".to_string(),
                    hunk: 0,
                }],
                unstage: false,
            },
        )
        .expect("stage hunk");

    // `git add` parity: the staged blob is LF-normalized, matching what
    // whole-file staging (index.add_path) stores for the same bytes.
    assert_eq!(temp.index_blob("new.txt"), b"l1\nl2\n".to_vec());
}

#[test]
fn whole_file_stage_of_crlf_workdir_stores_lf_blob() {
    let temp = TempRepo::new("stage-whole-crlf", "true");
    temp.write_bytes("new.txt", b"l1\r\nl2\r\n");

    // Regression guard for the libgit2 filter path (`index.add_path`):
    // whole-file staging normalizes the same way.
    temp.add_and_commit("new.txt", "base");
    assert_eq!(temp.index_blob("new.txt"), b"l1\nl2\n".to_vec());
}

#[test]
fn hunk_stage_of_tracked_crlf_file_stores_lf_blob() {
    let temp = TempRepo::new("stage-hunk-crlf", "true");
    temp.write_bytes("a.txt", b"a\nb\n");
    temp.add_and_commit("a.txt", "base");

    // CRLF workdir with an added line; the staged hunk must be LF even
    // though the workdir bytes are CRLF.
    temp.write_bytes("a.txt", b"a\r\nb\r\nextra\r\n");
    ENGINE
        .stage(
            &temp,
            &StageRequest {
                targets: vec![StageTarget::Hunk {
                    path: "a.txt".to_string(),
                    hunk: 0,
                }],
                unstage: false,
            },
        )
        .expect("stage hunk");

    assert_eq!(temp.index_blob("a.txt"), b"a\nb\nextra\n".to_vec());
}
