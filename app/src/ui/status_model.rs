//! Pure working-copy status model: sections, row keys, filtering and
//! selection. Ported 1:1 from the former Svelte `statusModel.ts` /
//! `statusSelection.ts` / `commitBarModel.ts` so the native panel keeps the
//! same row identity and action semantics.

use std::collections::HashSet;

use crate::engine::types::{ChangeKind, StatusEntry};

/// Display order of the two status sections.
pub const SECTION_ORDER: [SectionId; 2] = [SectionId::Staged, SectionId::Changes];

/// The sections of the working-copy panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SectionId {
    Staged,
    Changes,
}

impl SectionId {
    pub fn index(self) -> usize {
        match self {
            Self::Staged => 0,
            Self::Changes => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Changes => "changes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Staged => "Staged Changes",
            Self::Changes => "Changes",
        }
    }
}

/// Kinds that represent a real change (untracked/conflicted excluded).
fn is_change_kind(kind: ChangeKind) -> bool {
    matches!(
        kind,
        ChangeKind::Added
            | ChangeKind::Deleted
            | ChangeKind::Modified
            | ChangeKind::Renamed
            | ChangeKind::Copied
    )
}

fn is_untracked(kind: ChangeKind) -> bool {
    kind == ChangeKind::Untracked
}

/// `(staged, changed)` membership for one entry.
pub fn classify_entry(entry: &StatusEntry) -> (bool, bool) {
    let staged = is_change_kind(entry.index);
    let changes = entry.index == ChangeKind::Conflicted
        || entry.worktree == ChangeKind::Conflicted
        || is_change_kind(entry.worktree)
        || is_untracked(entry.index)
        || is_untracked(entry.worktree);
    (staged, changes)
}

/// One row of the status panel. A path with staged *and* unstaged edits
/// produces one row per section; the key carries the section so both rows
/// select independently.
#[derive(Debug, Clone)]
pub struct StatusRow {
    pub key: String,
    pub section: SectionId,
    pub path: String,
    pub old_path: Option<String>,
    pub index: ChangeKind,
    pub worktree: ChangeKind,
    pub kind: ChangeKind,
    pub dir: String,
    pub base: String,
    pub search: String,
}

#[derive(Debug, Clone, Default)]
pub struct StatusIndex {
    pub rows: Vec<StatusRow>,
    by_section: [Vec<usize>; 2],
}

impl StatusIndex {
    /// Row indices belonging to `section`, in document order.
    pub fn section(&self, section: SectionId) -> impl Iterator<Item = &StatusRow> {
        self.by_section[section.index()]
            .iter()
            .map(|&index| &self.rows[index])
    }

    pub fn section_len(&self, section: SectionId) -> usize {
        self.by_section[section.index()].len()
    }

    pub fn total(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, key: &str) -> Option<&StatusRow> {
        self.rows.iter().find(|row| row.key == key)
    }

    pub fn live_keys(&self) -> HashSet<String> {
        self.rows.iter().map(|row| row.key.clone()).collect()
    }
}

/// Split a repo-relative path into `(dir, base)`; the directory keeps its
/// trailing separator and is empty when the path has none.
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rfind(['/', '\\']) {
        Some(index) => (&path[..=index], &path[index + 1..]),
        None => ("", path),
    }
}

/// The kind shown for a row: the section's side wins, conflicts override.
pub fn row_kind(entry: &StatusEntry, section: SectionId) -> ChangeKind {
    match section {
        SectionId::Staged => entry.index,
        SectionId::Changes => {
            if entry.index == ChangeKind::Conflicted || entry.worktree == ChangeKind::Conflicted {
                ChangeKind::Conflicted
            } else if entry.worktree != ChangeKind::Unmodified
                && entry.worktree != ChangeKind::Ignored
            {
                entry.worktree
            } else if entry.index == ChangeKind::Untracked {
                entry.index
            } else {
                entry.worktree
            }
        }
    }
}

pub fn kind_letter(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Unmodified | ChangeKind::Ignored => "\u{b7}",
        ChangeKind::Added => "A",
        ChangeKind::Deleted => "D",
        ChangeKind::Modified => "M",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::Untracked => "U",
        ChangeKind::Conflicted => "!",
    }
}

pub fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Unmodified => "Unmodified",
        ChangeKind::Added => "Added",
        ChangeKind::Deleted => "Deleted",
        ChangeKind::Modified => "Modified",
        ChangeKind::Renamed => "Renamed",
        ChangeKind::Copied => "Copied",
        ChangeKind::Untracked => "Untracked",
        ChangeKind::Conflicted => "Conflicted",
        ChangeKind::Ignored => "Ignored",
    }
}

/// Build the two-section index. `key_prefix` scopes keys for submodules.
pub fn build_status_index(entries: &[StatusEntry], key_prefix: Option<&str>) -> StatusIndex {
    let mut index = StatusIndex::default();
    for entry in entries {
        let (staged, changes) = classify_entry(entry);
        for section in SECTION_ORDER {
            let present = match section {
                SectionId::Staged => staged,
                SectionId::Changes => changes,
            };
            if !present {
                continue;
            }
            let key = match key_prefix {
                Some(prefix) => format!("{prefix}:{}:{}", section.as_str(), entry.path),
                None => format!("{}:{}", section.as_str(), entry.path),
            };
            let search = match &entry.old_path {
                Some(old) => format!("{} {old}", entry.path),
                None => entry.path.clone(),
            }
            .to_lowercase();
            let (dir, base) = split_path(&entry.path);
            let row = StatusRow {
                key,
                section,
                path: entry.path.clone(),
                old_path: entry.old_path.clone(),
                index: entry.index,
                worktree: entry.worktree,
                kind: row_kind(entry, section),
                dir: dir.to_string(),
                base: base.to_string(),
                search,
            };
            index.by_section[section.index()].push(index.rows.len());
            index.rows.push(row);
        }
    }
    index
}

/// One filtered section. Sections with no matching row are omitted.
#[derive(Debug, Clone)]
pub struct SectionView<'a> {
    pub id: SectionId,
    pub rows: Vec<&'a StatusRow>,
}

pub fn filter_sections<'a>(index: &'a StatusIndex, needle: &str) -> Vec<SectionView<'a>> {
    let needle = needle.trim().to_lowercase();
    let mut sections = Vec::new();
    for id in SECTION_ORDER {
        let rows: Vec<&StatusRow> = index
            .section(id)
            .filter(|row| needle.is_empty() || row.search.contains(&needle))
            .collect();
        if !rows.is_empty() {
            sections.push(SectionView { id, rows });
        }
    }
    sections
}

/// Multi-selection state for the status list. Identity is the row key.
#[derive(Debug, Clone, Default)]
pub struct StatusSelection {
    pub selected: HashSet<String>,
    pub anchor: Option<String>,
}

impl StatusSelection {
    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.contains(key)
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Plain click selects only `key`; Ctrl/Cmd toggles; Shift extends the
    /// range from the anchor in document order (inclusive).
    pub fn click(&mut self, key: &str, rows: &[&StatusRow], toggle: bool, extend: bool) {
        if extend {
            if let Some(anchor) = self.anchor.as_deref() {
                let from = rows.iter().position(|row| row.key == anchor);
                let to = rows.iter().position(|row| row.key == key);
                if let (Some(from), Some(to)) = (from, to) {
                    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
                    self.selected.clear();
                    self.selected
                        .extend(rows[lo..=hi].iter().map(|row| row.key.clone()));
                    return;
                }
            }
            self.selected.clear();
            self.selected.insert(key.to_string());
            self.anchor = Some(key.to_string());
            return;
        }

        if toggle {
            if !self.selected.remove(key) {
                self.selected.insert(key.to_string());
            }
            self.anchor = Some(key.to_string());
            return;
        }

        self.selected.clear();
        self.selected.insert(key.to_string());
        self.anchor = Some(key.to_string());
    }

    pub fn select_all(&mut self, rows: &[&StatusRow]) {
        self.selected = rows.iter().map(|row| row.key.clone()).collect();
        self.anchor = rows.last().map(|row| row.key.clone());
    }

    pub fn select_none(&mut self) {
        self.clear();
    }

    pub fn prune(&mut self, live: &HashSet<String>) {
        self.selected.retain(|key| live.contains(key));
        if self
            .anchor
            .as_deref()
            .is_some_and(|anchor| !live.contains(anchor))
        {
            self.anchor = None;
        }
    }

    pub fn count(&self) -> usize {
        self.selected.len()
    }
}

/// Selection keys that follow files across a stage/unstage refresh: a moved
/// row is kept only when the target row is live; otherwise the old key is
/// retained so an in-flight refresh does not drop the selection.
pub fn remap_selection(
    selected: &HashSet<String>,
    paths: &HashSet<String>,
    unstage: bool,
    rows: &[StatusRow],
    prefix: Option<&str>,
) -> HashSet<String> {
    let target = if unstage {
        SectionId::Changes
    } else {
        SectionId::Staged
    };
    let live: HashSet<&str> = rows.iter().map(|row| row.key.as_str()).collect();
    let mut next = HashSet::with_capacity(selected.len());
    for key in selected {
        let scope = match prefix {
            Some(prefix) => key.starts_with(&format!("{prefix}:")),
            None => true,
        };
        let path = match key.rfind(":staged:") {
            Some(position) => key[position + ":staged:".len()..].to_string(),
            None => match key.rfind(":changes:") {
                Some(position) => key[position + ":changes:".len()..].to_string(),
                None => {
                    if scope {
                        if live.contains(key.as_str()) {
                            next.insert(key.clone());
                        }
                    } else {
                        next.insert(key.clone());
                    }
                    continue;
                }
            },
        };
        if scope && paths.contains(&path) {
            let moved = match prefix {
                Some(prefix) => format!("{prefix}:{}:{path}", target.as_str()),
                None => format!("{}:{path}", target.as_str()),
            };
            if live.contains(moved.as_str()) {
                next.insert(moved);
                continue;
            }
            // Target section not ready yet (refresh in flight): keep the old
            // key so the selection survives the round trip.
            next.insert(key.clone());
        } else if live.contains(key.as_str()) {
            next.insert(key.clone());
        }
    }
    next
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionActions {
    pub unstage: Vec<String>,
    pub stage: Vec<String>,
    pub discard: Vec<String>,
}

/// Map selected rows to stage/unstage/discard path lists. Conflicts never
/// take part (they must be resolved first).
pub fn selection_actions(rows: &[&StatusRow]) -> SelectionActions {
    let mut actions = SelectionActions::default();
    for row in rows {
        if row.kind == ChangeKind::Conflicted
            || row.index == ChangeKind::Conflicted
            || row.worktree == ChangeKind::Conflicted
        {
            continue;
        }
        match row.section {
            SectionId::Staged => push_unique(&mut actions.unstage, &row.path),
            SectionId::Changes => {
                push_unique(&mut actions.stage, &row.path);
                push_unique(&mut actions.discard, &row.path);
            }
        }
    }
    actions
}

fn push_unique(list: &mut Vec<String>, path: &str) {
    if !list.iter().any(|item| item == path) {
        list.push(path.to_string());
    }
}

/// "Stage" / "Stage 3 files" style labels.
pub fn count_label(verb: &str, count: usize) -> String {
    if count > 1 {
        format!("{verb} {count} files")
    } else {
        verb.to_string()
    }
}

/// True when the index holds anything a commit would pick up.
pub fn has_staged_changes(entries: &[StatusEntry]) -> bool {
    entries.iter().any(|entry| is_change_kind(entry.index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, index: ChangeKind, worktree: ChangeKind) -> StatusEntry {
        StatusEntry {
            path: path.to_string(),
            old_path: None,
            index,
            worktree,
        }
    }

    #[test]
    fn classification_matches_sections() {
        assert_eq!(
            classify_entry(&entry("a", ChangeKind::Modified, ChangeKind::Unmodified)),
            (true, false)
        );
        assert_eq!(
            classify_entry(&entry("a", ChangeKind::Unmodified, ChangeKind::Modified)),
            (false, true)
        );
        assert_eq!(
            classify_entry(&entry("a", ChangeKind::Modified, ChangeKind::Modified)),
            (true, true)
        );
        assert_eq!(
            classify_entry(&entry("a", ChangeKind::Unmodified, ChangeKind::Untracked)),
            (false, true)
        );
        assert_eq!(
            classify_entry(&entry("a", ChangeKind::Conflicted, ChangeKind::Conflicted)),
            (false, true)
        );
        assert_eq!(
            classify_entry(&entry("a", ChangeKind::Ignored, ChangeKind::Ignored)),
            (false, false)
        );
    }

    #[test]
    fn index_builds_document_ordered_sections_with_stable_keys() {
        let entries = vec![
            entry("staged.txt", ChangeKind::Modified, ChangeKind::Unmodified),
            entry("both.txt", ChangeKind::Added, ChangeKind::Modified),
            entry("work.txt", ChangeKind::Unmodified, ChangeKind::Modified),
        ];
        let index = build_status_index(&entries, None);
        let staged: Vec<&str> = index
            .section(SectionId::Staged)
            .map(|r| r.path.as_str())
            .collect();
        let changes: Vec<&str> = index
            .section(SectionId::Changes)
            .map(|r| r.path.as_str())
            .collect();
        assert_eq!(staged, vec!["staged.txt", "both.txt"]);
        assert_eq!(changes, vec!["both.txt", "work.txt"]);
        assert_eq!(index.rows[0].key, "staged:staged.txt".to_string());
        assert_eq!(index.rows[2].key, "changes:both.txt");
        assert_eq!(index.total(), 4);
    }

    #[test]
    fn prefix_scopes_keys_and_remap() {
        let entries = vec![entry(
            "dir/a.txt",
            ChangeKind::Modified,
            ChangeKind::Unmodified,
        )];
        let index = build_status_index(&entries, Some("sub"));
        assert_eq!(index.rows[0].key, "sub:staged:dir/a.txt");

        let selected = HashSet::from([index.rows[0].key.clone()]);
        let paths = HashSet::from(["dir/a.txt".to_string()]);
        let rows = vec![
            index.rows[0].clone(),
            build_status_index(
                &[entry(
                    "dir/a.txt",
                    ChangeKind::Unmodified,
                    ChangeKind::Modified,
                )],
                None,
            )
            .rows[0]
                .clone(),
        ];
        let remapped = remap_selection(&selected, &paths, true, &rows, Some("sub"));
        // No prefixed target row yet: the old key is retained so the
        // selection survives the in-flight refresh.
        assert_eq!(
            remapped,
            HashSet::from(["sub:staged:dir/a.txt".to_string()])
        );
        let remapped = remap_selection(&selected, &paths, true, &rows, None);
        assert!(remapped.contains("changes:dir/a.txt"));
    }

    #[test]
    fn split_path_keeps_trailing_separator() {
        assert_eq!(split_path("src/ui/app.rs"), ("src/ui/", "app.rs"));
        assert_eq!(split_path("src\\ui\\app.rs"), ("src\\ui\\", "app.rs"));
        assert_eq!(split_path("README.md"), ("", "README.md"));
    }

    #[test]
    fn row_kind_prefers_worktree_and_reports_conflicts() {
        let staged = entry("a", ChangeKind::Added, ChangeKind::Modified);
        assert_eq!(row_kind(&staged, SectionId::Staged), ChangeKind::Added);
        assert_eq!(row_kind(&staged, SectionId::Changes), ChangeKind::Modified);
        let conflict = entry("a", ChangeKind::Conflicted, ChangeKind::Modified);
        assert_eq!(
            row_kind(&conflict, SectionId::Changes),
            ChangeKind::Conflicted
        );
    }

    #[test]
    fn filtering_is_case_insensitive_and_hides_empty_sections() {
        let entries = vec![
            entry("SRC/App.rs", ChangeKind::Modified, ChangeKind::Unmodified),
            entry("README.md", ChangeKind::Unmodified, ChangeKind::Modified),
        ];
        let index = build_status_index(&entries, None);
        let sections = filter_sections(&index, "  app ");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, SectionId::Staged);
        assert_eq!(sections[0].rows.len(), 1);
        assert_eq!(filter_sections(&index, "zzz").len(), 0);
        assert_eq!(filter_sections(&index, "").len(), 2);
    }

    #[test]
    fn selection_click_toggle_and_range() {
        let entries = vec![
            entry("a", ChangeKind::Modified, ChangeKind::Unmodified),
            entry("b", ChangeKind::Modified, ChangeKind::Unmodified),
            entry("c", ChangeKind::Modified, ChangeKind::Unmodified),
        ];
        let index = build_status_index(&entries, None);
        let rows: Vec<&StatusRow> = index.rows.iter().collect();
        let mut selection = StatusSelection::default();
        selection.click("staged:a", &rows, false, false);
        assert_eq!(selection.selected.len(), 1);
        selection.click("staged:c", &rows, false, true);
        assert_eq!(selection.selected.len(), 3);
        selection.click("staged:b", &rows, true, false);
        assert!(!selection.is_selected("staged:b"));
        selection.select_all(&rows);
        assert_eq!(selection.count(), 3);
        selection.prune(&HashSet::from(["staged:a".to_string()]));
        assert_eq!(selection.count(), 1);
    }

    #[test]
    fn selection_actions_split_by_section_and_skip_conflicts() {
        let entries = vec![
            entry("a", ChangeKind::Modified, ChangeKind::Unmodified),
            entry("b", ChangeKind::Unmodified, ChangeKind::Modified),
            entry("c", ChangeKind::Conflicted, ChangeKind::Conflicted),
        ];
        let index = build_status_index(&entries, None);
        let rows: Vec<&StatusRow> = index.rows.iter().collect();
        let actions = selection_actions(&rows);
        assert_eq!(actions.unstage, vec!["a"]);
        assert_eq!(actions.stage, vec!["b"]);
        assert_eq!(actions.discard, vec!["b"]);
    }

    #[test]
    fn staged_detection() {
        let entries = vec![
            entry("a", ChangeKind::Added, ChangeKind::Unmodified),
            entry("b", ChangeKind::Unmodified, ChangeKind::Untracked),
        ];
        assert!(has_staged_changes(&entries));
        assert!(!has_staged_changes(&entries[1..]));
    }

    #[test]
    fn count_labels() {
        assert_eq!(count_label("Stage", 1), "Stage");
        assert_eq!(count_label("Stage", 3), "Stage 3 files");
    }
}
