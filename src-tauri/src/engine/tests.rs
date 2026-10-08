//! Engine tests against read-only fixture repos (scripts/make-fixtures.sh)
//! plus self-contained temp repos for anything that needs to write.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository};

use super::git_engine::{EngineError, GitEngine};
use super::libgit2::Libgit2Engine;
use super::types::{
    ChangeKind, CommitOptions, DiffSide, GitSignature, LineRange, LogFilter, StageRequest,
    StageTarget,
};

const ENGINE: Libgit2Engine = Libgit2Engine;

fn fixtures_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    [
        manifest.join("../fixtures"),
        manifest.join("../../fixtures"),
    ]
    .into_iter()
    .find(|dir| dir.join("basic").is_dir())
    .unwrap_or_else(|| manifest.join("../fixtures"))
}

fn fixture(name: &str) -> PathBuf {
    let dir = fixtures_dir().join(name);
    assert!(
        dir.is_dir(),
        "fixture `{name}` missing under {} — run `bash scripts/make-fixtures.sh`",
        fixtures_dir().display()
    );
    dir
}

fn open_fixture(name: &str) -> Repository {
    Repository::open(fixture(name)).expect("open fixture repo")
}

fn find_by_path<'a>(files: &'a [super::types::FileDiff], path: &str) -> &'a super::types::FileDiff {
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no diff entry for {path}"))
}

// ---------------------------------------------------------------------------
// basic fixture
// ---------------------------------------------------------------------------

#[test]
fn status_reports_untracked_and_branch() {
    let repo = open_fixture("basic");
    let status = ENGINE.status(&repo).expect("status");

    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(!status.detached);
    assert!(status.head.is_some());
    assert_eq!((status.ahead, status.behind), (0, 0)); // no upstream configured
    assert!(!status.merging && !status.rebasing && !status.sequencer);

    let notes = status
        .entries
        .iter()
        .find(|e| e.path == "notes.txt")
        .expect("untracked notes.txt in status");
    assert_eq!(notes.worktree, ChangeKind::Untracked);
    assert_eq!(notes.index, ChangeKind::Unmodified);
    assert_eq!(
        status.entries.len(),
        1,
        "basic fixture has exactly one change"
    );
}

#[test]
fn diff_worktree_vs_head_and_index() {
    let repo = open_fixture("basic");

    // Worktree vs HEAD: only the untracked file.
    let worktree = ENGINE
        .diff(&repo, &DiffSide::Head, &DiffSide::Worktree, None)
        .expect("diff head..worktree");
    assert_eq!(worktree.len(), 1);
    let notes = find_by_path(&worktree, "notes.txt");
    assert_eq!(notes.additions, 1);
    assert_eq!(notes.deletions, 0);
    assert!(!notes.binary);
    assert!(!notes.is_image);
    let hunk = &notes.hunks[0];
    assert_eq!(hunk.new_start, 1);
    assert_eq!(hunk.lines.len(), 1);
    assert_eq!(hunk.lines[0].origin, '+');
    assert_eq!(hunk.lines[0].text, "scratch notes");
    assert_eq!(hunk.lines[0].new_no, Some(1));

    // Clean index vs HEAD.
    let staged = ENGINE
        .diff(&repo, &DiffSide::Head, &DiffSide::Index, None)
        .expect("diff head..index");
    assert!(staged.is_empty(), "index matches HEAD in basic fixture");
}

#[test]
fn diff_between_commits_counts_additions() {
    let repo = open_fixture("basic");
    let files = ENGINE
        .diff(
            &repo,
            &DiffSide::Commit("HEAD~1".into()),
            &DiffSide::Commit("HEAD".into()),
            None,
        )
        .expect("diff HEAD~1..HEAD");
    assert_eq!(files.len(), 1);
    let util = find_by_path(&files, "src/util.py");
    assert_eq!(util.additions, 2);
    assert_eq!(util.deletions, 0);
    assert_eq!(util.hunks.len(), 1);
    assert!(util.hunks[0].lines.iter().all(|l| l.origin == '+'));
}

#[test]
fn diff_path_filter_is_component_prefix() {
    let repo = open_fixture("basic");
    let old = DiffSide::Commit("HEAD~2".into());
    let new = DiffSide::Commit("HEAD".into());

    let src = ENGINE
        .diff(&repo, &old, &new, Some(&["src".to_string()]))
        .expect("diff src filter");
    assert!(src.len() == 2, "src/main.py + src/util.py, got {src:?}");
    assert!(src.iter().all(|f| f.path.starts_with("src/")));

    let readme = ENGINE
        .diff(&repo, &old, &new, Some(&["README.md".to_string()]))
        .expect("diff README filter");
    assert!(
        readme.is_empty(),
        "README unchanged between HEAD~2 and HEAD"
    );
}

#[test]
fn log_paginates_full_history_with_decorations() {
    let repo = open_fixture("basic");

    // Walk everything (HEAD + all refs): 3 main commits + feature/login tip.
    let mut shas = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let (page, next) = ENGINE
            .log(&repo, &LogFilter::default(), 2, cursor.as_deref())
            .expect("log page");
        if page.is_empty() {
            assert!(next.is_none(), "empty terminal page must end the walk");
            break;
        }
        assert!(page.len() <= 2);
        for commit in &page {
            shas.push(commit.sha.clone());
        }
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(shas.len(), 4, "all refs: {shas:?}");
    let unique: HashSet<&String> = shas.iter().collect();
    assert_eq!(unique.len(), 4, "no duplicates across pages");

    // Main-only walk: 3 commits, children before parents, parent chain intact.
    let filter = LogFilter {
        refs: vec!["main".into()],
        ..Default::default()
    };
    let (main_page1, next1) = ENGINE.log(&repo, &filter, 2, None).expect("main log p1");
    assert_eq!(main_page1.len(), 2);
    let (main_page2, next2) = ENGINE
        .log(&repo, &filter, 2, next1.as_deref())
        .expect("main log p2");
    assert_eq!(main_page2.len(), 1);
    assert!(next2.is_none());
    let main_chain = [main_page1, main_page2].concat();
    assert_eq!(main_chain.len(), 3);
    assert_eq!(main_chain[0].summary, "feat: util helper");
    assert_eq!(main_chain[1].summary, "feat: main entrypoint");
    assert_eq!(main_chain[2].summary, "init: readme");
    for window in main_chain.windows(2) {
        assert_eq!(
            window[0].parents.first().map(String::as_str),
            Some(window[1].sha.as_str()),
            "topological order broken: {} then {}",
            window[0].summary,
            window[1].summary
        );
    }

    // Decorations on the main tip: HEAD -> main, main, tag v1.0.0.
    let tip_refs = &main_chain[0].refs;
    assert!(tip_refs.contains(&"main".to_string()), "{tip_refs:?}");
    assert!(tip_refs.contains(&"v1.0.0".to_string()), "{tip_refs:?}");
    assert!(
        tip_refs.contains(&"HEAD -> main".to_string()),
        "{tip_refs:?}"
    );

    // feature/login tip carries its branch decoration.
    let (all, _) = ENGINE.log(&repo, &LogFilter::default(), 4, None).unwrap();
    let feature_tip = all
        .iter()
        .find(|c| c.summary == "feat(login): stub")
        .expect("feature commit in walk");
    assert!(
        feature_tip.refs.iter().any(|r| r == "feature/login"),
        "{}",
        feature_tip.summary
    );
}

#[test]
fn log_filters_text_author_path_date() {
    let repo = open_fixture("basic");

    let by_text = |text: &str| LogFilter {
        text: Some(text.into()),
        ..Default::default()
    };
    let (page, next) = ENGINE
        .log(&repo, &by_text("util"), 10, None)
        .expect("text filter");
    assert!(next.is_none());
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].summary, "feat: util helper");

    // Case-insensitive.
    let (page, _) = ENGINE.log(&repo, &by_text("UTIL"), 10, None).expect("ci");
    assert_eq!(page.len(), 1);

    // Author matches on name.
    let author = LogFilter {
        author: Some("Fixture Bot".into()),
        ..Default::default()
    };
    let (page, _) = ENGINE.log(&repo, &author, 10, None).expect("author match");
    assert_eq!(page.len(), 4);

    let author_none = LogFilter {
        author: Some("nobody".into()),
        ..Default::default()
    };
    let (page, _) = ENGINE
        .log(&repo, &author_none, 10, None)
        .expect("author miss");
    assert!(page.is_empty());

    // Path filter.
    let by_path = LogFilter {
        path: Some("README.md".into()),
        ..Default::default()
    };
    let (page, _) = ENGINE.log(&repo, &by_path, 10, None).expect("path filter");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].summary, "init: readme");

    // Date range on committer time.
    let after_all = LogFilter {
        after_unix: Some(0),
        before_unix: Some(i64::MAX),
        ..Default::default()
    };
    let (page, _) = ENGINE.log(&repo, &after_all, 10, None).expect("date all");
    assert_eq!(page.len(), 4);
    let after_none = LogFilter {
        after_unix: Some(i64::MAX),
        ..Default::default()
    };
    let (page, _) = ENGINE.log(&repo, &after_none, 10, None).expect("date none");
    assert!(page.is_empty());
}

#[test]
fn log_regex_filter_is_reported_unsupported() {
    let repo = open_fixture("basic");
    let filter = LogFilter {
        text: Some(".*".into()),
        regex: true,
        ..Default::default()
    };
    // `.*` matches everything (M9: regex filtering is implemented).
    let (commits, _) = ENGINE.log(&repo, &filter, 10, None).expect("regex log");
    assert!(!commits.is_empty());
}

#[test]
fn refs_list_branches_and_peeled_tag() {
    let repo = open_fixture("basic");
    let refs = ENGINE.refs(&repo).expect("refs");

    let get = |name: &str| {
        refs.iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("ref {name} missing from {refs:?}"))
            .1
            .clone()
    };
    let main_sha = get("main");
    let feature_sha = get("feature/login");
    let tag_sha = get("v1.0.0");

    assert_ne!(main_sha, feature_sha);
    assert_eq!(tag_sha, main_sha, "tag v1.0.0 sits on main's tip");

    // main ref matches HEAD.
    let status = ENGINE.status(&repo).unwrap();
    assert_eq!(status.head.as_deref(), Some(main_sha.as_str()));
}

#[test]
fn blame_reports_line_origins() {
    let repo = open_fixture("basic");
    let lines = ENGINE
        .blame(&repo, "src/main.py", None)
        .expect("blame workdir");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].line_no, 1);
    assert_eq!(lines[0].text, "def main():");
    assert_eq!(lines[1].line_no, 2);
    assert_eq!(lines[1].text, "    print(\"hello\")");

    // Both lines come from the "feat: main entrypoint" commit.
    let filter = LogFilter {
        refs: vec!["main".into()],
        ..Default::default()
    };
    let (commits, _) = ENGINE.log(&repo, &filter, 10, None).unwrap();
    let entrypoint = commits
        .iter()
        .find(|c| c.summary == "feat: main entrypoint")
        .unwrap();
    for line in &lines {
        assert_eq!(line.sha, entrypoint.sha);
        assert_eq!(line.final_sha, entrypoint.sha);
        assert_eq!(line.signature.name, "Fixture Bot");
        assert_eq!(line.final_signature.name, "Fixture Bot");
    }

    // Explicit commit-ish gives the same answer for a clean worktree file.
    let from_head = ENGINE
        .blame(&repo, "src/main.py", Some("HEAD"))
        .expect("blame HEAD");
    assert_eq!(from_head.len(), lines.len());
    assert_eq!(from_head[0].sha, lines[0].sha);
}

// ---------------------------------------------------------------------------
// conflicted fixture
// ---------------------------------------------------------------------------

#[test]
fn conflicted_status_shows_merge_state() {
    let repo = open_fixture("conflicted");
    let status = ENGINE.status(&repo).expect("status");

    assert!(status.merging, "MERGE_HEAD present");
    assert!(!status.rebasing);
    assert!(!status.sequencer);
    assert_eq!(status.branch.as_deref(), Some("main"));

    let app = status
        .entries
        .iter()
        .find(|e| e.path == "app.txt")
        .expect("conflicted app.txt");
    assert_eq!(app.index, ChangeKind::Conflicted);

    // The conflict must also be visible as a diffable side pair.
    let files = ENGINE
        .diff(&repo, &DiffSide::Head, &DiffSide::Worktree, None)
        .expect("diff in conflicted repo");
    assert!(
        files.iter().any(|f| f.path == "app.txt"),
        "app.txt in workdir diff: {files:?}"
    );
}

// ---------------------------------------------------------------------------
// worktree fixture (open the MAIN repo path only)
// ---------------------------------------------------------------------------

#[test]
fn worktree_main_repo_opens_cleanly() {
    let repo = open_fixture("worktree/repo");
    let status = ENGINE.status(&repo).expect("status");
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.entries.is_empty(), "main worktree is clean");

    // All-ref walk sees the linked worktree's commit (feature branch).
    let (commits, _) = ENGINE
        .log(&repo, &LogFilter::default(), 10, None)
        .expect("log across worktrees");
    assert!(commits.len() >= 2);
    assert!(
        commits
            .iter()
            .any(|c| c.summary == "feature: from linked worktree"),
        "{commits:?}"
    );

    let refs = ENGINE.refs(&repo).unwrap();
    assert!(refs.iter().any(|(n, _)| n == "feature"));
}

// ---------------------------------------------------------------------------
// temp repos (self-written; safe to mutate)
// ---------------------------------------------------------------------------

struct TempRepo {
    _dir: PathBuf,
    repo: Repository,
}

impl TempRepo {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("mygitui-engine-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let repo = Repository::init(&dir).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Engine Test")?;
                c.set_str("user.email", "engine@test.local")
            })
            .expect("configure identity");
        TempRepo { _dir: dir, repo }
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.repo.workdir().unwrap().join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(file, content).expect("write file");
    }

    fn write_bytes(&self, path: &str, content: &[u8]) {
        let file = self.repo.workdir().unwrap().join(path);
        std::fs::write(file, content).expect("write file");
    }

    fn add_all_and_commit(&self, message: &str) -> git2::Oid {
        let mut index = self.repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("write index");
        self.commit_tree(message)
    }

    fn commit_tree(&self, message: &str) -> git2::Oid {
        let mut index = self.repo.index().expect("index");
        let tree_oid = index.write_tree().expect("write tree");
        index.write().expect("write index");
        let tree = self.repo.find_tree(tree_oid).expect("tree");
        let sig = self.repo.signature().expect("signature");
        let mut parents = Vec::new();
        if let Ok(head) = self.repo.head() {
            parents.push(head.peel_to_commit().expect("head commit"));
        }
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
    }

    fn stage(&self, path: &str) {
        let mut index = self.repo.index().expect("index");
        index.add_path(Path::new(path)).expect("stage add");
        index.write().expect("stage write");
    }

    fn stage_removal(&self, path: &str) {
        let mut index = self.repo.index().expect("index");
        index.remove_path(Path::new(path)).expect("stage remove");
        index.write().expect("stage write");
    }

    fn head_sha(&self) -> String {
        self.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
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
        // Repository must close before we could delete the dir; on Windows the
        // workdir watcher would otherwise hold locks. Deleting is best-effort.
        let dir = self._dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

#[test]
fn staged_and_worktree_kinds_map_correctly() {
    let temp = TempRepo::new("status-kinds");
    temp.write("kept.txt", "kept\n");
    temp.write("gone.txt", "gone\n");
    temp.write("mod.txt", "one\n");
    temp.add_all_and_commit("base");

    // Stage a modification + a new file, delete one file in the worktree only,
    // and modify another file after staging.
    temp.write("mod.txt", "one\nstaged\n");
    temp.stage("mod.txt");
    temp.write("mod.txt", "one\nstaged\nunstaged\n");

    temp.write("new.txt", "new\n");
    temp.stage("new.txt");

    std::fs::remove_file(temp.workdir().unwrap().join("gone.txt")).expect("rm gone");

    let status = ENGINE.status(&temp).expect("status");

    let entry = |path: &str| {
        status
            .entries
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("{path} missing from {:?}", status.entries))
    };
    let mod_entry = entry("mod.txt");
    assert_eq!(mod_entry.index, ChangeKind::Modified);
    assert_eq!(mod_entry.worktree, ChangeKind::Modified);
    assert_eq!(entry("new.txt").index, ChangeKind::Added);
    assert_eq!(entry("new.txt").worktree, ChangeKind::Unmodified);
    assert_eq!(entry("gone.txt").index, ChangeKind::Unmodified);
    assert_eq!(entry("gone.txt").worktree, ChangeKind::Deleted);

    // The staged pair sees only staged changes; the worktree pair sees the rest.
    let staged = ENGINE
        .diff(&temp, &DiffSide::Head, &DiffSide::Index, None)
        .unwrap();
    let staged_paths: Vec<&str> = staged.iter().map(|f| f.path.as_str()).collect();
    assert!(staged_paths.contains(&"mod.txt"), "{staged:?}");
    assert!(staged_paths.contains(&"new.txt"), "{staged:?}");
    assert!(!staged_paths.contains(&"gone.txt"), "{staged:?}");

    let unstaged = ENGINE
        .diff(&temp, &DiffSide::Index, &DiffSide::Worktree, None)
        .unwrap();
    let unstaged_paths: Vec<&str> = unstaged.iter().map(|f| f.path.as_str()).collect();
    assert!(unstaged_paths.contains(&"mod.txt"), "{unstaged:?}");
    assert!(unstaged_paths.contains(&"gone.txt"), "{unstaged:?}");
    assert!(!unstaged_paths.contains(&"new.txt"), "{unstaged:?}");
}

#[test]
fn rename_in_status_reports_old_path() {
    let temp = TempRepo::new("status-rename");
    temp.write("old.txt", "alpha\nbeta\ngamma\n");
    temp.add_all_and_commit("add old");

    // Same content under a new name in the index: rename detection material.
    temp.write("renamed.txt", "alpha\nbeta\ngamma\n");
    std::fs::remove_file(temp.workdir().unwrap().join("old.txt")).expect("rm old");
    temp.stage("renamed.txt");
    temp.stage_removal("old.txt");

    let status = ENGINE.status(&temp).expect("status");
    let entry = status
        .entries
        .iter()
        .find(|e| e.path == "renamed.txt")
        .unwrap_or_else(|| panic!("renamed entry missing: {:?}", status.entries));
    assert_eq!(entry.index, ChangeKind::Renamed);
    assert_eq!(entry.old_path.as_deref(), Some("old.txt"));

    // And in the diff side pair.
    let files = ENGINE
        .diff(&temp, &DiffSide::Head, &DiffSide::Index, None)
        .unwrap();
    let file = find_by_path(&files, "renamed.txt");
    assert_eq!(file.old_path.as_deref(), Some("old.txt"));
}

#[test]
fn word_highlights_mark_changed_words() {
    let temp = TempRepo::new("word-highlights");
    temp.write("story.txt", "hello world\n");
    temp.add_all_and_commit("original");

    let first = temp.head_sha();
    temp.write("story.txt", "hello brave\n");
    temp.add_all_and_commit("braver");

    let files = ENGINE
        .diff(
            &temp,
            &DiffSide::Commit(first),
            &DiffSide::Commit(temp.head_sha()),
            None,
        )
        .unwrap();
    let story = find_by_path(&files, "story.txt");
    assert_eq!(story.additions, 1);
    assert_eq!(story.deletions, 1);
    let hunk = &story.hunks[0];
    let minus = hunk.lines.iter().find(|l| l.origin == '-').unwrap();
    let plus = hunk.lines.iter().find(|l| l.origin == '+').unwrap();
    assert_eq!(minus.text, "hello world");
    assert_eq!(plus.text, "hello brave");

    // Only the changed word ("world" -> "brave") is highlighted; the shared
    // prefix "hello " stays unhighlighted on both sides.
    assert_eq!(minus.highlights, vec![(6, 11)], "{:?}", minus.highlights);
    assert_eq!(plus.highlights, vec![(6, 11)], "{:?}", plus.highlights);
    let (s, e) = plus.highlights[0];
    assert_eq!(&plus.text[s as usize..e as usize], "brave");
}

#[test]
fn binary_and_image_files_are_flagged() {
    let temp = TempRepo::new("binary-image");
    temp.write("plain.txt", "text\n");
    temp.add_all_and_commit("text base");

    temp.write_bytes("img.png", b"\x89PNG\r\n\x1a\n\x00\x00binary\x00bytes");
    temp.write("icon.svg", "<svg></svg>\n");
    temp.add_all_and_commit("add images");

    let files = ENGINE
        .diff(
            &temp,
            &DiffSide::Commit("HEAD~1".into()),
            &DiffSide::Commit("HEAD".into()),
            None,
        )
        .unwrap();
    // plain.txt is unchanged between the two commits: exactly the two images.
    assert_eq!(files.len(), 2, "{files:?}");

    let png = find_by_path(&files, "img.png");
    assert!(png.binary, "png with NUL bytes must be binary");
    assert!(png.is_image);
    assert!(png.hunks.is_empty());

    let svg = find_by_path(&files, "icon.svg");
    assert!(!svg.binary);
    assert!(svg.is_image);
    assert_eq!(svg.additions, 1);
}

#[test]
fn blame_defaults_to_head_state() {
    let temp = TempRepo::new("blame-evolve");
    temp.write("doc.md", "first\n");
    temp.add_all_and_commit("c1");
    let c1 = temp.head_sha();

    temp.write("doc.md", "first\nsecond\n");
    temp.add_all_and_commit("c2");
    let c2 = temp.head_sha();

    let from_commit = ENGINE.blame(&temp, "doc.md", Some(&c2)).unwrap();
    assert_eq!(from_commit.len(), 2);
    assert_eq!(from_commit[0].sha, c1);
    assert_eq!(from_commit[0].text, "first");
    assert_eq!(from_commit[1].sha, c2);
    assert_eq!(from_commit[1].text, "second");
    assert_eq!(from_commit[1].final_sha, c2);

    // Default (`from` = None) targets HEAD: an uncommitted workdir line does
    // not leak into the blame, and the reported text is HEAD's content.
    temp.write("doc.md", "first\nsecond\nthird (uncommitted)\n");
    let from_head = ENGINE.blame(&temp, "doc.md", None).unwrap();
    assert_eq!(from_head.len(), 2);
    assert_eq!(from_head[1].text, "second");
    assert_eq!(from_head[1].sha, c2);
}

#[test]
fn log_follows_renames_for_single_path() {
    let temp = TempRepo::new("log-follow");
    temp.write("a.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n");
    temp.add_all_and_commit("add a");

    // Rename (identical content) with the follow filter on.
    temp.write("b.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n");
    std::fs::remove_file(temp.workdir().unwrap().join("a.txt")).expect("rm a");
    temp.stage("b.txt");
    temp.stage_removal("a.txt");
    temp.commit_tree("rename a to b");

    temp.write(
        "b.txt",
        "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n",
    );
    temp.stage("b.txt");
    temp.commit_tree("edit b");

    let follow = LogFilter {
        path: Some("b.txt".into()),
        follow: true,
        ..Default::default()
    };
    let (commits, _) = ENGINE.log(&temp, &follow, 10, None).expect("follow log");
    let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, vec!["edit b", "rename a to b", "add a"]);
}

// ---------------------------------------------------------------------------
// M2 mutations: staging / commit engine (lane C1)
// ---------------------------------------------------------------------------

fn stage_req(targets: Vec<StageTarget>, unstage: bool) -> StageRequest {
    StageRequest { targets, unstage }
}

fn file_target(path: &str) -> StageTarget {
    StageTarget::File(path.to_string())
}

fn line_ranges(ranges: &[(u32, u32)]) -> Vec<LineRange> {
    ranges
        .iter()
        .map(|(start, end)| LineRange {
            start: *start,
            end: *end,
        })
        .collect()
}

fn commit_opts(message: &str) -> CommitOptions {
    CommitOptions {
        message: message.to_string(),
        amend: false,
        no_verify: false,
        allow_empty: false,
        author: None,
    }
}

/// Content of a path as currently staged in the index.
fn index_blob(repo: &Repository, path: &str) -> String {
    let index = repo.index().expect("index");
    let entry = index
        .get_path(Path::new(path), 0)
        .unwrap_or_else(|| panic!("`{path}` missing from index"));
    let blob = repo.find_blob(entry.id).expect("index blob");
    String::from_utf8_lossy(blob.content()).into_owned()
}

fn workdir_file(repo: &Repository, path: &str) -> String {
    std::fs::read_to_string(repo.workdir().unwrap().join(path)).expect("read workdir file")
}

/// A 24-line base file plus three independent workdir changes:
/// modify line 2, insert two lines after line 12, delete line 23 — far
/// enough apart to land in three separate diff hunks.
fn three_change_file(temp: &TempRepo) -> (&'static str, String, String) {
    let path = "multi.txt";
    let base: String = (1..=24).map(|n| format!("L{n:02}\n")).collect();
    temp.write(path, &base);
    temp.add_all_and_commit("base multi");
    let changed: String = {
        let mut lines: Vec<String> = base.lines().map(str::to_owned).collect();
        lines[1] = "L02-mod".to_string();
        lines.splice(12..12, ["INS-A".to_string(), "INS-B".to_string()]);
        lines.retain(|l| *l != "L23");
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    temp.write(path, &changed);
    (path, base, changed)
}

#[test]
fn stage_and_unstage_modified_file() {
    let temp = TempRepo::new("stage-mod");
    temp.write("a.txt", "v1\n");
    temp.add_all_and_commit("base");

    temp.write("a.txt", "v1\nv2\n");
    ENGINE
        .stage(&temp, &stage_req(vec![file_target("a.txt")], false))
        .expect("stage modified");
    assert_eq!(index_blob(&temp, "a.txt"), "v1\nv2\n", "staged content");
    let staged = ENGINE
        .diff(&temp, &DiffSide::Head, &DiffSide::Index, None)
        .unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].path, "a.txt");

    ENGINE
        .stage(&temp, &stage_req(vec![file_target("a.txt")], true))
        .expect("unstage modified");
    assert_eq!(index_blob(&temp, "a.txt"), "v1\n", "index back to HEAD");
    // Workdir keeps the change through both directions.
    assert_eq!(workdir_file(&temp, "a.txt"), "v1\nv2\n");
}

#[test]
fn stage_and_unstage_untracked_and_deleted_files() {
    let temp = TempRepo::new("stage-untracked-deleted");
    temp.write("kept.txt", "kept\n");
    temp.write("gone.txt", "gone\n");
    temp.add_all_and_commit("base");

    temp.write("new.txt", "brand new\n");
    ENGINE
        .stage(&temp, &stage_req(vec![file_target("new.txt")], false))
        .expect("stage untracked");
    assert_eq!(index_blob(&temp, "new.txt"), "brand new\n");
    ENGINE
        .stage(&temp, &stage_req(vec![file_target("new.txt")], true))
        .expect("unstage untracked");
    assert!(
        temp.index()
            .unwrap()
            .get_path(Path::new("new.txt"), 0)
            .is_none(),
        "untracked entry removed from index"
    );
    assert_eq!(workdir_file(&temp, "new.txt"), "brand new\n");

    std::fs::remove_file(temp.workdir().unwrap().join("gone.txt")).expect("rm gone");
    ENGINE
        .stage(&temp, &stage_req(vec![file_target("gone.txt")], false))
        .expect("stage deletion");
    assert!(
        temp.index()
            .unwrap()
            .get_path(Path::new("gone.txt"), 0)
            .is_none(),
        "deletion staged"
    );
    ENGINE
        .stage(&temp, &stage_req(vec![file_target("gone.txt")], true))
        .expect("unstage deletion");
    assert_eq!(
        index_blob(&temp, "gone.txt"),
        "gone\n",
        "deletion unstaged back to HEAD"
    );
    assert!(!temp.workdir().unwrap().join("gone.txt").exists());
}

#[test]
fn unstage_file_on_unborn_head_removes_entry() {
    let temp = TempRepo::new("stage-unborn");
    temp.write("first.txt", "one\n");
    temp.write("second.txt", "two\n");
    temp.stage("first.txt");
    temp.stage("second.txt");

    ENGINE
        .stage(&temp, &stage_req(vec![file_target("first.txt")], true))
        .expect("unstage on unborn HEAD");
    let index = temp.index().unwrap();
    assert!(index.get_path(Path::new("first.txt"), 0).is_none());
    assert!(index.get_path(Path::new("second.txt"), 0).is_some());
}

#[test]
fn stage_all_roundtrip_stages_and_unstages_everything() {
    let temp = TempRepo::new("stage-all");
    temp.write("a.txt", "one\n");
    temp.write("b.txt", "two\n");
    temp.add_all_and_commit("base");

    temp.write("a.txt", "one\nedited\n");
    temp.write("c.txt", "untracked\n");
    std::fs::remove_file(temp.workdir().unwrap().join("b.txt")).expect("rm b");

    ENGINE.stage_all(&temp, false).expect("stage all");
    let staged = ENGINE
        .diff(&temp, &DiffSide::Head, &DiffSide::Index, None)
        .unwrap();
    let paths: Vec<&str> = staged.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"a.txt"), "{staged:?}");
    assert!(paths.contains(&"b.txt"), "deletion staged: {staged:?}");
    assert!(paths.contains(&"c.txt"), "untracked staged: {staged:?}");
    assert_eq!(paths.len(), 3, "{staged:?}");
    assert!(!temp.workdir().unwrap().join("b.txt").exists());

    ENGINE.stage_all(&temp, true).expect("unstage all");
    let staged = ENGINE
        .diff(&temp, &DiffSide::Head, &DiffSide::Index, None)
        .unwrap();
    assert!(
        staged.is_empty(),
        "index matches HEAD after unstage-all: {staged:?}"
    );
    // Workdir untouched by unstage-all.
    assert_eq!(workdir_file(&temp, "a.txt"), "one\nedited\n");
    assert_eq!(workdir_file(&temp, "c.txt"), "untracked\n");
}

#[test]
fn stage_all_unstage_on_unborn_head_clears_index() {
    let temp = TempRepo::new("stage-all-unborn");
    temp.write("x.txt", "x\n");
    temp.write("y.txt", "y\n");
    temp.stage("x.txt");
    temp.stage("y.txt");

    ENGINE.stage_all(&temp, true).expect("unstage all unborn");
    assert!(
        temp.index().unwrap().is_empty(),
        "index cleared on unborn HEAD"
    );
}

#[test]
fn line_staging_stages_only_middle_change() {
    let temp = TempRepo::new("stage-lines-middle");
    let (path, base, changed) = three_change_file(&temp);

    let files = ENGINE
        .diff(&temp, &DiffSide::Index, &DiffSide::Worktree, None)
        .unwrap();
    let file = find_by_path(&files, path);
    assert_eq!(file.hunks.len(), 3, "three independent hunks");

    // Stage only the inserted lines (new-side numbers 13..14) of hunk 1.
    ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Lines {
                    path: path.to_string(),
                    hunk: 1,
                    ranges: line_ranges(&[(13, 14)]),
                }],
                false,
            ),
        )
        .expect("stage middle lines");

    let expected_middle: String = {
        let mut lines: Vec<String> = base.lines().map(str::to_owned).collect();
        lines.splice(12..12, ["INS-A".to_string(), "INS-B".to_string()]);
        format!("{}\n", lines.join("\n"))
    };
    assert_eq!(
        index_blob(&temp, path),
        expected_middle,
        "index = base + insertion only"
    );
    assert_eq!(workdir_file(&temp, path), changed, "worktree untouched");

    // Now stage the line-2 modification by number: the paired deletion must
    // come along (replacement staged as a unit).
    ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Lines {
                    path: path.to_string(),
                    hunk: 0,
                    ranges: line_ranges(&[(2, 2)]),
                }],
                false,
            ),
        )
        .expect("stage replacement line");
    let expected_two: String = {
        let mut lines: Vec<String> = expected_middle.lines().map(str::to_owned).collect();
        lines[1] = "L02-mod".to_string();
        format!("{}\n", lines.join("\n"))
    };
    assert_eq!(index_blob(&temp, path), expected_two, "mod + insertion");
    assert!(index_blob(&temp, path).contains("L23"), "deletion unstaged");
}

#[test]
fn line_unstaging_reverts_only_selected_change() {
    let temp = TempRepo::new("unstage-lines-middle");
    let (path, _base, _changed) = three_change_file(&temp);
    ENGINE.stage_all(&temp, false).expect("stage everything");

    // Unstage the inserted lines (new side = index side, numbers 13..14).
    ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Lines {
                    path: path.to_string(),
                    hunk: 1,
                    ranges: line_ranges(&[(13, 14)]),
                }],
                true,
            ),
        )
        .expect("unstage middle lines");

    let index = index_blob(&temp, path);
    assert!(!index.contains("INS-A"), "{index}");
    assert!(!index.contains("INS-B"), "{index}");
    assert!(index.contains("L02-mod"), "modification stays staged");
    assert!(!index.contains("L23"), "deletion stays staged");
}

#[test]
fn hunk_staging_and_unstaging_single_hunks() {
    let temp = TempRepo::new("stage-hunk");
    temp.write(
        "two.txt",
        "a1\na2\na3\na4\na5\na6\na7\na8\na9\na10\nb1\nb2\nb3\nb4\nb5\nb6\nb7\nb8\nb9\nb10\n",
    );
    temp.add_all_and_commit("base hunks");
    temp.write(
        "two.txt",
        "a1\nA2\na3\na4\na5\na6\na7\na8\na9\na10\nb1\nb2\nb3\nb4\nb5\nb6\nb7\nB8\nb9\nb10\n",
    );

    let files = ENGINE
        .diff(&temp, &DiffSide::Index, &DiffSide::Worktree, None)
        .unwrap();
    let file = find_by_path(&files, "two.txt");
    assert_eq!(file.hunks.len(), 2, "two separate hunks");

    ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Hunk {
                    path: "two.txt".to_string(),
                    hunk: 0,
                }],
                false,
            ),
        )
        .expect("stage hunk 0");
    let index = index_blob(&temp, "two.txt");
    assert!(index.contains("A2"), "{index}");
    assert!(!index.contains("B8"), "{index}");

    // Stage the rest, then unstage hunk 0 only.
    ENGINE.stage_all(&temp, false).expect("stage the rest");
    ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Hunk {
                    path: "two.txt".to_string(),
                    hunk: 0,
                }],
                true,
            ),
        )
        .expect("unstage hunk 0");
    let index = index_blob(&temp, "two.txt");
    assert!(!index.contains("A2"), "{index}");
    assert!(index.contains("B8"), "{index}");
}

#[test]
fn line_staging_untracked_file_subset() {
    let temp = TempRepo::new("stage-lines-untracked");
    temp.write("u.txt", "U1\nU2\nU3\nU4\nU5\n");

    ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Lines {
                    path: "u.txt".to_string(),
                    hunk: 0,
                    ranges: line_ranges(&[(2, 3)]),
                }],
                false,
            ),
        )
        .expect("stage untracked lines");
    assert_eq!(index_blob(&temp, "u.txt"), "U2\nU3\n");
    assert_eq!(workdir_file(&temp, "u.txt"), "U1\nU2\nU3\nU4\nU5\n");
}

#[test]
fn binary_hunk_staging_is_rejected() {
    let temp = TempRepo::new("stage-binary");
    temp.write("bin.dat", "text for now\n");
    temp.add_all_and_commit("base");
    temp.write_bytes("bin.dat", b"\x00\x01\x02binary now\x00");

    let err = ENGINE
        .stage(
            &temp,
            &stage_req(
                vec![StageTarget::Hunk {
                    path: "bin.dat".to_string(),
                    hunk: 0,
                }],
                false,
            ),
        )
        .expect_err("binary hunk staging must fail");
    match err {
        EngineError::Invalid(msg) => assert!(msg.contains("binary"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn commit_creates_and_amends_preserving_author() {
    let temp = TempRepo::new("commit-amend");
    temp.write("a.txt", "v1\n");
    temp.stage("a.txt");
    let sha1 = ENGINE.commit(&temp, &commit_opts("first")).expect("commit");
    assert_eq!(temp.head_sha(), sha1);

    let head = temp.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.author().name().unwrap(), "Engine Test");
    assert_eq!(head.message().unwrap(), "first");
    assert_eq!(head.parent_count(), 0);

    // Amend with a staged change and a new committer identity: the original
    // author must be preserved (git amend semantics).
    temp.write("a.txt", "v2\n");
    temp.stage("a.txt");
    temp.config()
        .unwrap()
        .set_str("user.name", "Committer Now")
        .unwrap();
    let sha2 = ENGINE
        .commit(
            &temp,
            &CommitOptions {
                amend: true,
                ..commit_opts("first (amended)")
            },
        )
        .expect("amend");
    assert_ne!(sha1, sha2);
    let amended = temp
        .find_commit(git2::Oid::from_str(&sha2).unwrap())
        .unwrap();
    assert_eq!(amended.parent_count(), 0, "root parents preserved");
    assert_eq!(amended.author().name().unwrap(), "Engine Test");
    assert_eq!(amended.committer().name().unwrap(), "Committer Now");
    assert_eq!(amended.message().unwrap(), "first (amended)");
    let tree = amended.tree().unwrap();
    let blob = temp
        .find_blob(tree.get_path(Path::new("a.txt")).unwrap().id())
        .unwrap();
    assert_eq!(String::from_utf8_lossy(blob.content()), "v2\n");
}

#[test]
fn commit_author_override_and_empty_guards() {
    let temp = TempRepo::new("commit-author-empty");
    temp.write("a.txt", "v1\n");
    temp.stage("a.txt");

    let override_author = GitSignature {
        name: "Override Author".to_string(),
        email: "override@test.local".to_string(),
        time: 1_600_000_000,
        offset_minutes: 60,
    };
    let sha = ENGINE
        .commit(
            &temp,
            &CommitOptions {
                author: Some(override_author),
                ..commit_opts("authored")
            },
        )
        .expect("commit with author override");
    let commit = temp
        .find_commit(git2::Oid::from_str(&sha).unwrap())
        .unwrap();
    assert_eq!(commit.author().name().unwrap(), "Override Author");
    assert_eq!(commit.author().email().unwrap(), "override@test.local");
    assert_eq!(commit.author().when().seconds(), 1_600_000_000);
    assert_eq!(commit.committer().name().unwrap(), "Engine Test");

    // Nothing new staged: refused, then allowed with allow_empty.
    match ENGINE.commit(&temp, &commit_opts("empty")) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("nothing to commit"), "{msg}"),
        other => panic!("expected Invalid nothing-to-commit, got {other:?}"),
    }
    let empty_sha = ENGINE
        .commit(
            &temp,
            &CommitOptions {
                allow_empty: true,
                ..commit_opts("explicitly empty")
            },
        )
        .expect("allow_empty commits");
    assert_ne!(empty_sha, sha);
    let empty = temp
        .find_commit(git2::Oid::from_str(&empty_sha).unwrap())
        .unwrap();
    assert_eq!(
        empty.parent_ids().next().unwrap().to_string(),
        sha,
        "empty commit on top of the previous one"
    );

    // Blank messages are refused outright.
    match ENGINE.commit(&temp, &commit_opts("   ")) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("empty"), "{msg}"),
        other => panic!("expected Invalid empty-message, got {other:?}"),
    }
}

#[test]
fn signing_info_reflects_config_and_routes_commits_to_cli() {
    let temp = TempRepo::new("signing");
    temp.write("a.txt", "v1\n");
    temp.stage("a.txt");

    let info = ENGINE.signing_info(&temp).expect("signing info default");
    assert!(!info.active);
    assert_eq!(info.format, "openpgp");
    assert_eq!(info.key_id, None);

    let mut config = temp.config().unwrap();
    config.set_str("commit.gpgsign", "true").unwrap();
    config.set_str("gpg.format", "ssh").unwrap();
    config.set_str("user.signingkey", "SHA256:abc123").unwrap();

    let info = ENGINE.signing_info(&temp).expect("signing info set");
    assert!(info.active);
    assert_eq!(info.format, "ssh");
    assert_eq!(info.key_id.as_deref(), Some("SHA256:abc123"));

    match ENGINE.commit(&temp, &commit_opts("signed")) {
        Err(EngineError::Unsupported(msg)) => assert!(msg.contains("signing"), "{msg}"),
        other => panic!("expected Unsupported signing, got {other:?}"),
    }

    config.set_str("commit.gpgsign", "false").unwrap();
    let info = ENGINE.signing_info(&temp).expect("signing info off");
    assert!(!info.active);
    ENGINE
        .commit(&temp, &commit_opts("unsigned ok"))
        .expect("unsigned commit succeeds");
}

#[test]
fn hooks_reported_and_commit_hooks_route_to_cli() {
    let temp = TempRepo::new("hooks");
    temp.write("a.txt", "v1\n");
    temp.stage("a.txt");

    let hooks = ENGINE.hooks(&temp).expect("hooks default");
    assert_eq!(hooks.len(), 5, "five known hook kinds");
    assert!(hooks.iter().all(|h| !h.present && !h.executable));

    let hooks_dir = temp.commondir().join("hooks");
    std::fs::create_dir_all(&hooks_dir).expect("mkdir hooks");
    let hook = hooks_dir.join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 0\n").expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&hook, perms).expect("chmod 644");
    }

    let hooks = ENGINE.hooks(&temp).expect("hooks with file");
    let pre = hooks
        .iter()
        .find(|h| h.kind == "pre-commit")
        .expect("pre-commit reported");
    assert!(pre.present);
    #[cfg(unix)]
    assert!(!pre.executable, "0644 has no exec bit on unix");
    #[cfg(windows)]
    assert!(pre.executable, "git-for-windows runs present hooks");

    // Unix: a non-executable hook does not force the CLI path...
    #[cfg(unix)]
    {
        ENGINE
            .commit(&temp, &commit_opts("no exec bit"))
            .expect("non-executable hook ignored");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook, perms).expect("chmod 755");
    }

    // ...but an executable one routes commits to the CLI engine.
    match ENGINE.commit(&temp, &commit_opts("hooked")) {
        Err(EngineError::Unsupported(msg)) => assert!(msg.contains("hooks"), "{msg}"),
        other => panic!("expected Unsupported hooks, got {other:?}"),
    }

    // no_verify bypasses the routing decision.
    temp.write("a.txt", "v2\n");
    temp.stage("a.txt");
    ENGINE
        .commit(
            &temp,
            &CommitOptions {
                no_verify: true,
                ..commit_opts("skipped hooks")
            },
        )
        .expect("no_verify bypasses hooks");
}

// ---------------------------------------------------------------------------
// discard (M9)
// ---------------------------------------------------------------------------

#[test]
fn discard_file_restores_head_and_drops_untracked() {
    let temp = TempRepo::new("discard-file");
    temp.write("a.txt", "v1\n");
    temp.write("gone.txt", "x\n");
    temp.add_all_and_commit("base");

    // Tracked file: staged + unstaged changes both vanish.
    temp.write("a.txt", "v1\nstaged\n");
    temp.stage("a.txt");
    temp.write("a.txt", "v1\nstaged\nworkdir\n");
    // Untracked file.
    temp.write("new.txt", "brand new\n");

    ENGINE
        .discard(&temp, &[file_target("a.txt"), file_target("new.txt")])
        .expect("discard");

    assert_eq!(index_blob(&temp, "a.txt"), "v1\n", "index back to HEAD");
    assert_eq!(workdir_file(&temp, "a.txt"), "v1\n", "workdir back to HEAD");
    assert!(
        !temp.repo.workdir().unwrap().join("new.txt").exists(),
        "untracked file deleted"
    );
    assert!(
        temp.repo
            .index()
            .unwrap()
            .get_path(Path::new("new.txt"), 0)
            .is_none(),
        "index entry gone"
    );
    assert_eq!(workdir_file(&temp, "gone.txt"), "x\n", "untouched path");
}

#[test]
fn discard_file_deletes_newly_added_and_restores_deleted() {
    let temp = TempRepo::new("discard-add-del");
    temp.write("base.txt", "b\n");
    temp.add_all_and_commit("base");

    // Newly added (staged, not in HEAD): file + index entry gone.
    temp.write("added.txt", "a\n");
    temp.stage("added.txt");
    ENGINE
        .discard(&temp, &[file_target("added.txt")])
        .expect("discard added");
    assert!(!temp.repo.workdir().unwrap().join("added.txt").exists());

    // Deleted in workdir (not staged): restored from index/HEAD.
    let _ = std::fs::remove_file(temp.repo.workdir().unwrap().join("base.txt"));
    ENGINE
        .discard(&temp, &[file_target("base.txt")])
        .expect("discard delete");
    assert_eq!(workdir_file(&temp, "base.txt"), "b\n");
}

#[test]
fn discard_hunks_reverses_only_selected_changes() {
    let temp = TempRepo::new("discard-hunks");
    let (path, base, _changed) = three_change_file(&temp);

    // Discard only the first hunk (the L02 modification): the inserted
    // lines and the deleted L23 stay.
    ENGINE
        .discard(
            &temp,
            &[StageTarget::Hunk {
                path: path.to_string(),
                hunk: 0,
            }],
        )
        .expect("discard hunk 0");

    let after = workdir_file(&temp, path);
    assert!(after.contains("L02\n"), "line 2 back to base: {after}");
    assert!(after.contains("INS-A"), "insertion kept");
    assert!(!after.contains("L23"), "deletion kept");
    // Index untouched (still HEAD).
    assert_eq!(index_blob(&temp, path), base);
}

#[test]
fn discard_lines_reverses_selected_lines_only() {
    let temp = TempRepo::new("discard-lines");
    let (path, base, _changed) = three_change_file(&temp);

    // Discard just INS-B (new-side line of the insertion hunk).
    ENGINE
        .discard(
            &temp,
            &[StageTarget::Lines {
                path: path.to_string(),
                hunk: 1,
                ranges: line_ranges(&[(14, 14)]),
            }],
        )
        .expect("discard one line");

    let after = workdir_file(&temp, path);
    assert!(after.contains("INS-A"), "INS-A kept: {after}");
    assert!(!after.contains("INS-B"), "INS-B discarded");
    assert!(!after.contains("L23"), "other hunks untouched");
    assert_eq!(index_blob(&temp, path), base, "index untouched");
}

// ---------------------------------------------------------------------------
// regex log filter + diff Index→Commit (M9)
// ---------------------------------------------------------------------------

#[test]
fn log_regex_filter_matches_and_rejects_bad_patterns() {
    let temp = TempRepo::new("log-regex");
    temp.write("a.txt", "1\n");
    temp.add_all_and_commit("feat: add widget");
    temp.write("a.txt", "2\n");
    temp.add_all_and_commit("fix(widget): repair");
    temp.write("a.txt", "3\n");
    temp.add_all_and_commit("chore: cleanup");

    let filter = LogFilter {
        text: Some(r"^(feat|fix)\(widget\)".to_string()),
        regex: true,
        ..Default::default()
    };
    let (commits, _) = ENGINE.log(&temp, &filter, 10, None).expect("regex log");
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert!(commits[0].summary.contains("fix(widget)"));

    // Anchored pattern that matches nothing.
    let filter = LogFilter {
        text: Some(r"^nope$".to_string()),
        regex: true,
        ..Default::default()
    };
    let (commits, _) = ENGINE.log(&temp, &filter, 10, None).unwrap();
    assert!(commits.is_empty());

    // Invalid regex → friendly Invalid, not a panic.
    let filter = LogFilter {
        text: Some("(unclosed".to_string()),
        regex: true,
        ..Default::default()
    };
    match ENGINE.log(&temp, &filter, 10, None) {
        Err(EngineError::Invalid(msg)) => assert!(msg.contains("invalid regex"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn diff_index_against_commit_shows_staged_vs_target() {
    let temp = TempRepo::new("diff-index-commit");
    temp.write("a.txt", "one\n");
    temp.add_all_and_commit("c1");
    let first = temp.head_sha();
    temp.write("a.txt", "one\ntwo\n");
    temp.add_all_and_commit("c2");
    // Staged change diverging from HEAD.
    temp.write("a.txt", "one\ntwo\nthree\n");
    temp.stage("a.txt");

    // Staged content vs the first commit (old side = Index): "two" and
    // "three" exist only on the index side → deletions going index→commit.
    let diff = ENGINE
        .diff(
            &temp,
            &DiffSide::Index,
            &DiffSide::Commit(first.clone()),
            None,
        )
        .expect("index vs commit");
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].deletions, 2, "{:?}", diff[0].hunks);

    // Staged content vs HEAD: only "three" is index-only.
    let diff = ENGINE
        .diff(&temp, &DiffSide::Index, &DiffSide::Head, None)
        .expect("index vs HEAD");
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].deletions, 1);

    // Identical sides (index == HEAD's tree) → empty.
    temp.write("a.txt", "one\ntwo\n");
    temp.stage("a.txt");
    let diff = ENGINE
        .diff(&temp, &DiffSide::Index, &DiffSide::Head, None)
        .unwrap();
    assert!(diff.is_empty());
}
