//! Pure history model: streamed log accumulation, lane bookkeeping, filter
//! mapping and relative timestamps. The backend already assigns graph
//! lanes (`graph::types::layout_page`); the UI only appends pages and
//! measures the widest lane column.

use crate::engine::types::{CommitInfo, LogFilter};
use crate::graph::types::GraphRow;
use crate::service::log::LogPage;

/// Leading shas remembered to skip an overlapping page on re-streams.
const TAIL_SIZE: usize = 16;

#[derive(Debug, Clone, Default)]
pub struct HistoryState {
    pub commits: Vec<CommitInfo>,
    pub rows: Vec<GraphRow>,
    pub request_id: u64,
    pub generation: u64,
    pub loading: bool,
    pub complete: bool,
    /// Widest lane count seen so far (at least 2, for a stable column).
    pub max_lanes: u16,
    pub selected: Option<String>,
    pub scroll: f32,
    pub viewport: f32,
    tail: Vec<String>,
}

impl HistoryState {
    /// Start a new stream: drop the previous rows and remember its request.
    pub fn begin(&mut self, request_id: u64) {
        *self = Self {
            request_id,
            loading: true,
            max_lanes: 2,
            ..Self::default()
        };
    }

    pub fn reset(&mut self) {
        let request_id = self.request_id;
        let selected = self.selected.take();
        self.begin(request_id);
        self.selected = selected;
    }

    /// Append one page. Returns false for stale generations.
    pub fn accept(&mut self, page: LogPage) -> bool {
        if page.generation < self.generation {
            return false;
        }
        if page.generation > self.generation {
            self.generation = page.generation;
        }

        // A re-lived stream may replay a page whose leading commits we
        // already have; skip that aligned prefix.
        let skip = page
            .commits
            .iter()
            .take(TAIL_SIZE)
            .take_while(|commit| self.tail.iter().any(|sha| sha == &commit.sha))
            .count();

        self.commits.extend(page.commits.into_iter().skip(skip));
        self.rows.extend(page.rows.into_iter().skip(skip));

        for row in &self.rows {
            self.max_lanes = self.max_lanes.max(row.lane_count).max(row.lane + 1);
        }
        self.tail = self
            .commits
            .iter()
            .rev()
            .take(TAIL_SIZE)
            .map(|commit| commit.sha.clone())
            .collect();
        self.tail.reverse();

        self.loading = page.next_cursor.is_some();
        self.complete = page.next_cursor.is_none();
        true
    }

    pub fn find(&self, sha: &str) -> Option<usize> {
        self.commits.iter().position(|commit| commit.sha == sha)
    }

    pub fn selected_commit(&self) -> Option<&CommitInfo> {
        self.selected
            .as_deref()
            .and_then(|sha| self.commits.iter().find(|commit| commit.sha == sha))
    }

    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }

    pub fn len(&self) -> usize {
        self.commits.len()
    }
}

/// Port of the Svelte `toLogFilter`: trimmed empty fields become absent, and
/// day inputs are expanded to inclusive UTC second ranges.
pub fn to_log_filter(fields: &FilterFields) -> LogFilter {
    let text = fields.text.trim();
    let author = fields.author.trim();
    let path = fields.path.trim();
    let pickaxe = fields.pickaxe.trim();
    let pickaxe_regex = fields.pickaxe_regex.trim();
    let after = fields.after.trim();
    let before = fields.before.trim();
    LogFilter {
        text: (!text.is_empty()).then(|| text.to_string()),
        regex: fields.regex,
        author: (!author.is_empty()).then(|| author.to_string()),
        path: (!path.is_empty()).then(|| path.to_string()),
        after_unix: if after.is_empty() {
            None
        } else {
            day_to_unix_seconds(after)
        },
        before_unix: if before.is_empty() {
            None
        } else {
            day_to_unix_seconds(before).map(|start| start + 86_399)
        },
        refs: Vec::new(),
        follow: fields.follow,
        pickaxe: (!pickaxe.is_empty()).then(|| pickaxe.to_string()),
        pickaxe_regex: (!pickaxe_regex.is_empty()).then(|| pickaxe_regex.to_string()),
    }
}

/// Filter form state (what the history filter popover edits).
#[derive(Debug, Clone, Default)]
pub struct FilterFields {
    pub text: String,
    pub regex: bool,
    pub author: String,
    pub path: String,
    /// `YYYY-MM-DD`.
    pub after: String,
    /// `YYYY-MM-DD`.
    pub before: String,
    pub pickaxe: String,
    pub pickaxe_regex: String,
    pub follow: bool,
}

/// One editable filter field (routes `FilterFields` updates).
#[derive(Debug, Clone, Copy)]
pub enum HistoryFilterField {
    Text,
    Author,
    Path,
    After,
    Before,
    Pickaxe,
    PickaxeRegex,
}

impl FilterFields {
    pub fn field_mut(&mut self, field: HistoryFilterField) -> &mut String {
        match field {
            HistoryFilterField::Text => &mut self.text,
            HistoryFilterField::Author => &mut self.author,
            HistoryFilterField::Path => &mut self.path,
            HistoryFilterField::After => &mut self.after,
            HistoryFilterField::Before => &mut self.before,
            HistoryFilterField::Pickaxe => &mut self.pickaxe,
            HistoryFilterField::PickaxeRegex => &mut self.pickaxe_regex,
        }
    }

    /// The active pickaxe pattern, whichever mode it is in.
    pub fn pickaxe_value(&self) -> &str {
        if self.pickaxe_regex.is_empty() {
            &self.pickaxe
        } else {
            &self.pickaxe_regex
        }
    }

    pub fn set_pickaxe(&mut self, value: String) {
        if self.pickaxe_regex.is_empty() {
            self.pickaxe = value;
        } else {
            self.pickaxe_regex = value;
        }
    }

    pub fn pickaxe_regex_enabled(&self) -> bool {
        !self.pickaxe_regex.is_empty()
    }

    /// Switch the pickaxe input between literal and regex storage.
    pub fn set_pickaxe_regex(&mut self, regex: bool) {
        if regex {
            if self.pickaxe_regex.is_empty() {
                self.pickaxe_regex = std::mem::take(&mut self.pickaxe);
            }
        } else if !self.pickaxe_regex.is_empty() {
            if self.pickaxe.is_empty() {
                self.pickaxe = std::mem::take(&mut self.pickaxe_regex);
            } else {
                self.pickaxe_regex.clear();
            }
        }
    }

    /// True when any field is set; `regex` alone does not count.
    pub fn is_active(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.author.trim().is_empty()
            || !self.path.trim().is_empty()
            || !self.after.trim().is_empty()
            || !self.before.trim().is_empty()
            || !self.pickaxe.trim().is_empty()
            || !self.pickaxe_regex.trim().is_empty()
    }
}

/// Strict `YYYY-MM-DD` → UTC midnight seconds.
pub fn day_to_unix_seconds(day: &str) -> Option<i64> {
    let bytes = day.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = day.get(0..4)?.parse().ok()?;
    let month: u32 = day.get(5..7)?.parse().ok()?;
    let day_of_month: u32 = day.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || day_of_month == 0 {
        return None;
    }
    let max_day = days_in_month(year, month);
    if day_of_month > max_day {
        return None;
    }
    Some(days_from_civil(year, month, day_of_month) * 86_400)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's civil calendar algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month as i64 + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Compact relative age: `just now`, `5m ago`, `3h ago`, `2d ago`, `1w ago`,
/// `4mo ago`, `2y ago`.
pub fn relative_age(unix_seconds: i64, now_seconds: i64) -> String {
    let delta = (now_seconds - unix_seconds).max(0);
    if delta < 45 {
        return "just now".to_string();
    }
    let minutes = delta / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = delta / 3_600;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = delta / 86_400;
    if days < 7 {
        return format!("{days}d ago");
    }
    if days < 30 {
        return format!("{}w ago", days / 7);
    }
    if days < 365 {
        return format!("{}mo ago", days / 30);
    }
    format!("{}y ago", days / 365)
}

/// First line of a commit summary, trimmed, for dense rows.
pub fn short_summary(commit: &CommitInfo) -> &str {
    commit.summary.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::GitSignature;

    fn commit(sha: &str) -> CommitInfo {
        let signature = GitSignature {
            name: "A".to_string(),
            email: "a@example.com".to_string(),
            time: 0,
            offset_minutes: 0,
        };
        CommitInfo {
            sha: sha.to_string(),
            parents: Vec::new(),
            author: signature.clone(),
            committer: signature,
            message: format!("commit {sha}"),
            summary: format!("commit {sha}"),
            refs: Vec::new(),
        }
    }

    fn row(sha: &str, lane: u16, lanes: u16) -> GraphRow {
        GraphRow {
            sha: sha.to_string(),
            lane,
            edges: Vec::new(),
            lane_count: lanes,
        }
    }

    fn page(commits: Vec<CommitInfo>, rows: Vec<GraphRow>, next: Option<&str>) -> LogPage {
        LogPage {
            commits,
            rows,
            next_cursor: next.map(str::to_string),
            generation: 1,
        }
    }

    #[test]
    fn pages_append_and_track_completion() {
        let mut state = HistoryState::default();
        state.begin(7);
        assert!(state.loading);
        assert!(state.accept(page(
            vec![commit("a"), commit("b")],
            vec![row("a", 0, 2), row("b", 1, 3)],
            Some("b"),
        )));
        assert!(state.loading);
        assert_eq!(state.max_lanes, 3);
        assert!(state.accept(page(vec![commit("c")], vec![row("c", 0, 1)], None)));
        assert!(!state.loading);
        assert!(state.complete);
        assert_eq!(state.len(), 3);
        assert_eq!(state.find("b"), Some(1));
        assert_eq!(state.selected_commit().map(|c| c.sha.as_str()), None);
        state.selected = Some("c".to_string());
        assert_eq!(state.selected_commit().unwrap().sha, "c");
    }

    #[test]
    fn overlapping_page_skips_aligned_prefix() {
        let mut state = HistoryState::default();
        state.begin(1);
        state.accept(page(
            vec![commit("a"), commit("b")],
            vec![row("a", 0, 1), row("b", 0, 1)],
            Some("b"),
        ));
        state.accept(page(
            vec![commit("b"), commit("c")],
            vec![row("b", 0, 1), row("c", 0, 1)],
            None,
        ));
        let shas: Vec<&str> = state.commits.iter().map(|c| c.sha.as_str()).collect();
        assert_eq!(shas, vec!["a", "b", "c"]);
        assert_eq!(state.rows.len(), 3);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let mut state = HistoryState::default();
        state.begin(1);
        state.generation = 5;
        let mut stale = page(vec![commit("a")], vec![row("a", 0, 1)], None);
        stale.generation = 4;
        assert!(!state.accept(stale));
        assert!(state.is_empty());
    }

    #[test]
    fn filter_fields_trim_and_expand_dates() {
        let fields = FilterFields {
            text: "  fix ".to_string(),
            regex: true,
            after: "2024-02-29".to_string(),
            before: "2024-03-01".to_string(),
            ..FilterFields::default()
        };
        let filter = to_log_filter(&fields);
        assert_eq!(filter.text.as_deref(), Some("fix"));
        assert!(filter.regex);
        assert_eq!(filter.after_unix, Some(1_709_164_800));
        assert_eq!(filter.before_unix, Some(1_709_337_599));
        assert!(fields.is_active());
        assert!(!FilterFields::default().is_active());
        assert!(!FilterFields {
            regex: true,
            ..FilterFields::default()
        }
        .is_active());
    }

    #[test]
    fn strict_day_parsing() {
        assert_eq!(day_to_unix_seconds("1970-01-01"), Some(0));
        assert_eq!(day_to_unix_seconds("2024-02-29"), Some(1_709_164_800));
        assert_eq!(day_to_unix_seconds("2023-02-29"), None);
        assert_eq!(day_to_unix_seconds("2024-13-01"), None);
        assert_eq!(day_to_unix_seconds("2024-1-01"), None);
        assert_eq!(day_to_unix_seconds("2024-01-32"), None);
        assert_eq!(day_to_unix_seconds("nope"), None);
    }

    #[test]
    fn relative_ages_match_the_ported_scale() {
        let now = 1_000_000_000;
        assert_eq!(relative_age(now, now), "just now");
        assert_eq!(relative_age(now - 44, now), "just now");
        assert_eq!(relative_age(now - 45, now), "0m ago");
        assert_eq!(relative_age(now - 300, now), "5m ago");
        assert_eq!(relative_age(now - 3 * 3_600, now), "3h ago");
        assert_eq!(relative_age(now - 2 * 86_400, now), "2d ago");
        assert_eq!(relative_age(now - 10 * 86_400, now), "1w ago");
        assert_eq!(relative_age(now - 45 * 86_400, now), "1mo ago");
        assert_eq!(relative_age(now - 800 * 86_400, now), "2y ago");
        assert_eq!(relative_age(now + 500, now), "just now");
    }
}
