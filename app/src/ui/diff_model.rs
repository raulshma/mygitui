//! Pure diff row model and virtualizer. Ported 1:1 from the former Svelte
//! `rowModel.ts` / `virtualizer.ts`: rows reference `FileDiff` by index so
//! the model stays cheap to rebuild when the diff or the view options change.

use std::collections::HashSet;

use crate::engine::types::{ChangeKind, DiffHunk, FileDiff};

pub const FILE_HEADER_HEIGHT: f32 = 40.0;
pub const HUNK_HEADER_HEIGHT: f32 = 26.0;
pub const LINE_HEIGHT: f32 = 20.0;
pub const BINARY_HEIGHT: f32 = 56.0;
pub const IMAGE_HEIGHT: f32 = 384.0;

pub fn is_context(origin: char) -> bool {
    origin == ' ' || origin == '='
}

/// One rendered row of a diff. Indices resolve against the diff payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    FileHeader {
        file: usize,
        collapsed: bool,
    },
    HunkHeader {
        file: usize,
        hunk: usize,
        old_start: u32,
        old_count: u32,
        new_start: u32,
        new_count: u32,
    },
    /// Unified `+`/`-` line.
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// Unified or split context line.
    Context {
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// Split pairing; either side may be missing.
    Pair {
        file: usize,
        hunk: usize,
        left: Option<usize>,
        right: Option<usize>,
    },
    Binary {
        file: usize,
    },
    Image {
        file: usize,
    },
}

impl DiffRow {
    pub fn height(self) -> f32 {
        match self {
            Self::FileHeader { .. } => FILE_HEADER_HEIGHT,
            Self::HunkHeader { .. } => HUNK_HEADER_HEIGHT,
            Self::Line { .. } | Self::Context { .. } | Self::Pair { .. } => LINE_HEIGHT,
            Self::Binary { .. } => BINARY_HEIGHT,
            Self::Image { .. } => IMAGE_HEIGHT,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiffRowModel {
    pub rows: Vec<DiffRow>,
    pub has_split_rows: bool,
    /// Longest raw text length (code points) across every line.
    pub max_text_chars: usize,
    pub total_additions: u32,
    pub total_deletions: u32,
}

/// `(old, new)` line counts of one hunk.
pub fn hunk_counts(hunk: &DiffHunk) -> (u32, u32) {
    let old = hunk
        .lines
        .iter()
        .filter(|line| line.old_no.is_some())
        .count() as u32;
    let new = hunk
        .lines
        .iter()
        .filter(|line| line.new_no.is_some())
        .count() as u32;
    (old, new)
}

fn line_counts(file: &FileDiff) -> (u32, u32) {
    let mut additions = 0;
    let mut deletions = 0;
    for hunk in &file.hunks {
        for line in &hunk.lines {
            match (line.old_no, line.new_no) {
                (None, Some(_)) => additions += 1,
                (Some(_), None) => deletions += 1,
                _ => {}
            }
        }
    }
    (additions, deletions)
}

/// Change kind of one file, derived from the hunks when the diff does not
/// carry a rename or the engine counts disagree.
pub fn file_change_kind(file: &FileDiff) -> ChangeKind {
    if file.old_path.as_deref().is_some_and(|old| old != file.path) {
        return ChangeKind::Renamed;
    }
    let (additions, deletions) = line_counts(file);
    let (additions, deletions) = if file.hunks.is_empty() && additions == 0 && deletions == 0 {
        (file.additions, file.deletions)
    } else {
        (additions, deletions)
    };
    // A whole-file add/delete shows no context lines; with context the file
    // already existed and the hunk is a pure-insertion/deletion edit.
    let has_context = file
        .hunks
        .iter()
        .any(|hunk| hunk.lines.iter().any(|line| is_context(line.origin)));
    if !has_context && additions == 0 && deletions > 0 {
        return ChangeKind::Deleted;
    }
    if !has_context && deletions == 0 && additions > 0 {
        return ChangeKind::Added;
    }
    ChangeKind::Modified
}

pub fn file_kind_letter(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        _ => "M",
    }
}

/// Build the flat row list. `collapsed` holds file indices whose bodies are
/// hidden; totals still count every file.
pub fn build_row_model(
    files: &[FileDiff],
    split: bool,
    collapsed: &HashSet<usize>,
) -> DiffRowModel {
    let mut model = DiffRowModel::default();

    for (file_index, file) in files.iter().enumerate() {
        let is_collapsed = collapsed.contains(&file_index);
        model.rows.push(DiffRow::FileHeader {
            file: file_index,
            collapsed: is_collapsed,
        });

        let (additions, deletions) = line_counts(file);
        model.total_additions += additions;
        model.total_deletions += deletions;
        for hunk in &file.hunks {
            for line in &hunk.lines {
                model.max_text_chars = model.max_text_chars.max(line.text.chars().count());
            }
        }

        if is_collapsed {
            continue;
        }
        if file.is_image {
            model.rows.push(DiffRow::Image { file: file_index });
            continue;
        }
        if file.binary {
            model.rows.push(DiffRow::Binary { file: file_index });
            continue;
        }

        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            let (old_count, new_count) = hunk_counts(hunk);
            model.rows.push(DiffRow::HunkHeader {
                file: file_index,
                hunk: hunk_index,
                old_start: hunk.old_start,
                old_count,
                new_start: hunk.new_start,
                new_count,
            });

            if split {
                let lines = &hunk.lines;
                let mut cursor = 0;
                while cursor < lines.len() {
                    if is_context(lines[cursor].origin) {
                        model.rows.push(DiffRow::Context {
                            file: file_index,
                            hunk: hunk_index,
                            line: cursor,
                        });
                        cursor += 1;
                        continue;
                    }
                    let start = cursor;
                    while cursor < lines.len() && !is_context(lines[cursor].origin) {
                        cursor += 1;
                    }
                    let block = &lines[start..cursor];
                    let deletions: Vec<usize> = block
                        .iter()
                        .enumerate()
                        .filter(|(_, line)| line.origin == '-')
                        .map(|(offset, _)| start + offset)
                        .collect();
                    let additions: Vec<usize> = block
                        .iter()
                        .enumerate()
                        .filter(|(_, line)| line.origin == '+')
                        .map(|(offset, _)| start + offset)
                        .collect();
                    let pairs = deletions.len().max(additions.len());
                    for pair in 0..pairs {
                        model.rows.push(DiffRow::Pair {
                            file: file_index,
                            hunk: hunk_index,
                            left: deletions.get(pair).copied(),
                            right: additions.get(pair).copied(),
                        });
                        model.has_split_rows = true;
                    }
                }
            } else {
                for (line_index, line) in hunk.lines.iter().enumerate() {
                    if is_context(line.origin) {
                        model.rows.push(DiffRow::Context {
                            file: file_index,
                            hunk: hunk_index,
                            line: line_index,
                        });
                    } else {
                        model.rows.push(DiffRow::Line {
                            file: file_index,
                            hunk: hunk_index,
                            line: line_index,
                        });
                    }
                }
            }
        }
    }

    model
}

/// One maximal run of equal-height rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub first_row: usize,
    pub count: usize,
    pub row_height: f32,
    pub top: f32,
}

#[derive(Debug, Clone, Default)]
pub struct RowLayout {
    pub buckets: Vec<Bucket>,
    pub total_height: f32,
}

pub fn build_layout(rows: &[DiffRow]) -> RowLayout {
    let mut layout = RowLayout::default();
    let mut top = 0.0;
    let mut start = 0usize;
    while start < rows.len() {
        let height = rows[start].height();
        let mut end = start + 1;
        while end < rows.len() && rows[end].height() == height {
            end += 1;
        }
        layout.buckets.push(Bucket {
            first_row: start,
            count: end - start,
            row_height: height,
            top,
        });
        top += (end - start) as f32 * height;
        start = end;
    }
    layout.total_height = top;
    layout
}

/// Visible portion of one bucket, in row coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slice {
    pub first_row: usize,
    pub count: usize,
    pub top: f32,
    pub height: f32,
}

pub fn visible_slices(
    layout: &RowLayout,
    scroll_top: f32,
    viewport_height: f32,
    overscan: f32,
) -> Vec<Slice> {
    if layout.buckets.is_empty() {
        return Vec::new();
    }
    let y_top = (scroll_top - overscan).max(0.0);
    let y_bottom = scroll_top.max(0.0) + viewport_height.max(0.0) + overscan;

    // Last bucket whose top <= y_top.
    let first_bucket = layout
        .buckets
        .partition_point(|bucket| bucket.top <= y_top)
        .saturating_sub(1);

    let mut slices = Vec::new();
    for bucket in &layout.buckets[first_bucket..] {
        let bucket_bottom = bucket.top + bucket.count as f32 * bucket.row_height;
        if bucket_bottom <= y_top {
            continue;
        }
        if bucket.top >= y_bottom {
            break;
        }
        let start = (((y_top - bucket.top) / bucket.row_height).floor() as usize).min(bucket.count);
        let end = (((y_bottom - bucket.top) / bucket.row_height).ceil() as usize).min(bucket.count);
        if end > start {
            slices.push(Slice {
                first_row: bucket.first_row + start,
                count: end - start,
                top: bucket.top + start as f32 * bucket.row_height,
                height: (end - start) as f32 * bucket.row_height,
            });
        }
    }
    slices
}

/// New-side line selection for line-granular staging. Identity is
/// `(file, hunk, new_no)`; only lines with a new-side number are selectable.
#[derive(Debug, Clone, Default)]
pub struct LineSelection {
    pub anchor: Option<(usize, usize, u32)>,
    pub lines: HashSet<(usize, usize, u32)>,
}

impl LineSelection {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.anchor = None;
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn contains(&self, file: usize, hunk: usize, new_no: u32) -> bool {
        self.lines.contains(&(file, hunk, new_no))
    }

    /// Click one gutter; with `extend` and an anchor in the same hunk the
    /// whole new-side range is selected.
    pub fn toggle(&mut self, file: usize, hunk: usize, new_no: u32, extend: bool) {
        if extend {
            if let Some((anchor_file, anchor_hunk, anchor_line)) = self.anchor {
                if anchor_file == file && anchor_hunk == hunk {
                    let (lo, hi) = if anchor_line <= new_no {
                        (anchor_line, new_no)
                    } else {
                        (new_no, anchor_line)
                    };
                    for line in lo..=hi {
                        self.lines.insert((file, hunk, line));
                    }
                    return;
                }
            }
        }
        if !self.lines.remove(&(file, hunk, new_no)) {
            self.lines.insert((file, hunk, new_no));
        }
        self.anchor = Some((file, hunk, new_no));
    }

    /// Merged consecutive new-side ranges for one hunk.
    pub fn ranges(&self, file: usize, hunk: usize) -> Vec<(u32, u32)> {
        let mut lines: Vec<u32> = self
            .lines
            .iter()
            .filter(|(f, h, _)| *f == file && *h == hunk)
            .map(|(_, _, line)| *line)
            .collect();
        lines.sort_unstable();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for line in lines {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == line => *end = line,
                _ => ranges.push((line, line)),
            }
        }
        ranges
    }
}

/// Raw text of the selected lines, in diff order (files → hunks → lines).
/// Selection identity is the new-side number, so the copy carries added and
/// context lines — the same set line-granular staging would take.
pub fn selected_line_texts(files: &[FileDiff], selection: &LineSelection) -> Vec<String> {
    let mut texts = Vec::new();
    for (file_index, file) in files.iter().enumerate() {
        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            for line in &hunk.lines {
                let Some(new_no) = line.new_no else {
                    continue;
                };
                if selection.contains(file_index, hunk_index, new_no) {
                    texts.push(line.text.clone());
                }
            }
        }
    }
    texts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::DiffLine;

    fn line(origin: char, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            old_no: old,
            new_no: new,
            origin,
            text: text.to_string(),
            highlights: Vec::new(),
        }
    }

    fn file(name: &str, hunks: Vec<DiffHunk>) -> FileDiff {
        let additions = hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.new_no.is_some() && l.old_no.is_none())
            .count() as u32;
        let deletions = hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.old_no.is_some() && l.new_no.is_none())
            .count() as u32;
        FileDiff {
            path: name.to_string(),
            old_path: None,
            binary: false,
            is_image: false,
            additions,
            deletions,
            hunks,
        }
    }

    fn hunk(old_start: u32, new_start: u32, lines: Vec<DiffLine>) -> DiffHunk {
        DiffHunk {
            old_start,
            new_start,
            lines,
        }
    }

    #[test]
    fn unified_rows_and_totals() {
        let f = file(
            "a.txt",
            vec![hunk(
                1,
                1,
                vec![
                    line(' ', Some(1), Some(1), "ctx"),
                    line('-', Some(2), None, "old"),
                    line('+', None, Some(2), "new"),
                    line('+', None, Some(3), "new2"),
                ],
            )],
        );
        let model = build_row_model(&[f], false, &HashSet::new());
        assert_eq!(model.rows.len(), 6); // header + hunk + 4 lines
        assert!(!model.has_split_rows);
        assert_eq!(model.total_additions, 2);
        assert_eq!(model.total_deletions, 1);
        assert!(matches!(model.rows[2], DiffRow::Context { .. }));
        assert!(matches!(model.rows[3], DiffRow::Line { .. }));
    }

    #[test]
    fn collapsed_file_keeps_totals_but_hides_body() {
        let f = file(
            "a.txt",
            vec![hunk(1, 1, vec![line('+', None, Some(1), "new")])],
        );
        let collapsed = HashSet::from([0usize]);
        let model = build_row_model(&[f], false, &collapsed);
        assert_eq!(model.rows.len(), 1);
        assert_eq!(model.total_additions, 1);
        assert!(matches!(
            model.rows[0],
            DiffRow::FileHeader {
                collapsed: true,
                ..
            }
        ));
    }

    #[test]
    fn split_pairing_is_order_agnostic() {
        let f = file(
            "a",
            vec![hunk(
                1,
                1,
                vec![
                    line('+', None, Some(1), "added"),
                    line('-', Some(1), None, "deleted"),
                    line(' ', Some(2), Some(2), "ctx"),
                    line('+', None, Some(3), "second add"),
                ],
            )],
        );
        let model = build_row_model(&[f], true, &HashSet::new());
        let pairs: Vec<_> = model
            .rows
            .iter()
            .filter_map(|row| match row {
                DiffRow::Pair { left, right, .. } => Some((*left, *right)),
                _ => None,
            })
            .collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs[0].0.is_some() && pairs[0].1.is_some());
        assert_eq!(pairs[1].0, None);
        assert_eq!(pairs[1].1, Some(3));
        assert!(model.has_split_rows);
    }

    #[test]
    fn context_only_split_does_not_flag_split_rows() {
        let f = file(
            "a",
            vec![hunk(1, 1, vec![line(' ', Some(1), Some(1), "ctx")])],
        );
        let model = build_row_model(&[f], true, &HashSet::new());
        assert!(!model.has_split_rows);
    }

    #[test]
    fn image_wins_over_binary() {
        let mut f = file("img.png", vec![]);
        f.binary = true;
        f.is_image = true;
        let model = build_row_model(&[f], false, &HashSet::new());
        assert!(matches!(model.rows[1], DiffRow::Image { .. }));
    }

    #[test]
    fn layout_merges_equal_heights_and_slices_visible_rows() {
        let rows = vec![
            DiffRow::FileHeader {
                file: 0,
                collapsed: false,
            },
            DiffRow::HunkHeader {
                file: 0,
                hunk: 0,
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
            },
            DiffRow::Line {
                file: 0,
                hunk: 0,
                line: 0,
            },
            DiffRow::Line {
                file: 0,
                hunk: 0,
                line: 1,
            },
            DiffRow::FileHeader {
                file: 1,
                collapsed: false,
            },
        ];
        let layout = build_layout(&rows);
        assert_eq!(layout.buckets.len(), 4);
        assert_eq!(layout.total_height, 40.0 + 26.0 + 40.0 + 40.0);

        let slices = visible_slices(&layout, 0.0, 90.0, 0.0);
        assert_eq!(slices.len(), 3);
        assert_eq!(slices[0].first_row, 0);
        assert_eq!(slices[0].count, 1);
        assert_eq!(slices[1].first_row, 1);
        assert_eq!(slices[1].count, 1);
        assert_eq!(slices[2].first_row, 2);
        assert_eq!(slices[2].count, 2);

        let slices = visible_slices(&layout, 70.0, 40.0, 0.0);
        assert_eq!(slices[0].first_row, 2);
        assert_eq!(slices[0].top, 66.0);
    }

    #[test]
    fn file_kinds_cover_rename_and_fallbacks() {
        let added = file("new", vec![hunk(0, 1, vec![line('+', None, Some(1), "x")])]);
        assert_eq!(file_change_kind(&added), ChangeKind::Added);
        let deleted = file("old", vec![hunk(1, 0, vec![line('-', Some(1), None, "x")])]);
        assert_eq!(file_change_kind(&deleted), ChangeKind::Deleted);
        let mut renamed = file("new", vec![]);
        renamed.old_path = Some("old".to_string());
        assert_eq!(file_change_kind(&renamed), ChangeKind::Renamed);
        let mut empty = file("bin", vec![]);
        empty.additions = 0;
        empty.deletions = 0;
        assert_eq!(file_change_kind(&empty), ChangeKind::Modified);
        let insertion = file(
            "src.txt",
            vec![hunk(
                1,
                1,
                vec![
                    line(' ', Some(1), Some(1), "keep"),
                    line('+', None, Some(2), "new"),
                ],
            )],
        );
        assert_eq!(file_change_kind(&insertion), ChangeKind::Modified);
        assert_eq!(file_kind_letter(ChangeKind::Renamed), "R");
    }

    #[test]
    fn line_selection_merges_ranges_and_extends() {
        let mut selection = LineSelection::default();
        selection.toggle(0, 0, 3, false);
        selection.toggle(0, 0, 5, false);
        selection.toggle(0, 0, 4, false);
        assert_eq!(selection.ranges(0, 0), vec![(3, 5)]);

        let mut selection = LineSelection::default();
        selection.toggle(0, 0, 2, false);
        selection.toggle(0, 0, 5, true);
        assert_eq!(selection.ranges(0, 0), vec![(2, 5)]);
        assert_eq!(selection.ranges(0, 1), Vec::<(u32, u32)>::new());
    }

    #[test]
    fn selected_line_texts_copies_in_diff_order() {
        let files = vec![file(
            "a.txt",
            vec![hunk(
                1,
                1,
                vec![
                    line(' ', Some(1), Some(1), "ctx"),
                    line('-', Some(2), None, "old"),
                    line('+', None, Some(2), "new"),
                    line(' ', Some(3), Some(3), "ctx2"),
                ],
            )],
        )];
        let mut selection = LineSelection::default();
        selection.toggle(0, 0, 1, false);
        selection.toggle(0, 0, 2, false);
        // Removed lines have no new-side number and can never be selected.
        assert_eq!(
            selected_line_texts(&files, &selection),
            vec!["ctx".to_string(), "new".to_string()]
        );
        // Empty selection copies nothing.
        assert!(selected_line_texts(&files, &LineSelection::default()).is_empty());
    }
}
