//! Per-file history: streamed commit list restricted to one path.
//!
//! Self-contained read surface mirroring the shell's history list, but for
//! a single `path`. The shell opens it with `Msg::Opened { path }`, routes
//! the returned `Action::Load` into `service::log::repo_file_history`
//! (which streams `LogPage`s tagged with a request id) and feeds every page
//! back as `Msg::PageLoaded` — pages accumulate exactly like
//! [`super::history_model::HistoryState`] does, with sha-level dedupe.
//!
//! v1 note: the rows of a `LogPage` are accepted and stored for shape
//! compatibility, but the list renders flat (sha, summary, age) — no graph
//! lane canvas. `Msg::LoadMore` supports lazy paging when the shell only
//! forwards pages on demand.

// Registered for gates before the shell wires this module in; drop this
// allow once `app.rs` routes `Msg`/`Action`.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::CommitInfo;
use crate::graph::types::GraphRow;

use super::history_model::{relative_age, short_summary};
use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// Fixed height of one commit row.
pub const FILE_HISTORY_ROW_HEIGHT: f32 = 24.0;

/// View state: paged commits for one path plus selection.
#[derive(Debug, Default)]
pub struct FileHistoryView {
    pub open: bool,
    pub path: Option<String>,
    pub loading: bool,
    pub commits: Vec<CommitInfo>,
    /// Graph rows parallel to `commits` (stored, not rendered — see the
    /// module note).
    pub rows: Vec<GraphRow>,
    pub next_cursor: Option<String>,
    pub error: Option<String>,
    /// Selected commit sha; `None` clears.
    pub selected: Option<String>,
    /// Shas already accumulated; guards against duplicate pages.
    seen: HashSet<String>,
}

/// View messages (shell → [`FileHistoryView::update`]).
#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    /// The shell opened file history for `path`; resets and asks for a load.
    Opened {
        path: String,
    },
    /// One streamed `LogPage` (fields of `service::log::LogPage`); pages
    /// accumulate with sha-level dedupe.
    PageLoaded {
        commits: Vec<CommitInfo>,
        rows: Vec<GraphRow>,
        next_cursor: Option<String>,
    },
    /// Fetch the next page (only while `next_cursor` is set and idle).
    LoadMore,
    /// Select a commit by sha; `None` clears.
    SelectCommit(Option<String>),
    /// "View commit" on the selected row; the shell opens commit detail.
    CommitDetails(String),
    /// "Diff" on a row; the shell opens the diff for `path` at that commit.
    OpenDiff(String),
    /// The stream ended without a final page (engine error, cancelled, or
    /// repo generation moved); finalizes the load state.
    Failed(String),
}

/// Intents returned by [`FileHistoryView::update`] for the shell to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start `service::log::repo_file_history` for `path` with a fresh
    /// request id; forward each `LogPage` as `Msg::PageLoaded`.
    Load { path: String },
    /// Request the next page for the current stream.
    LoadMore,
    /// The shell routes this to its existing commit-detail flow.
    SelectCommit(String),
    /// Open the working diff for `path` at `sha`; the shell decides how.
    OpenDiff { path: String, sha: String },
}

/// Display model for one commit row (pure mapping, unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistoryRow {
    /// Position in the commit list.
    pub index: usize,
    /// Short (7-char) sha.
    pub sha: String,
    /// First line of the commit message, clipped for dense rows.
    pub summary: String,
    /// Compact relative age.
    pub age: String,
}

impl FileHistoryView {
    /// Pure state transition; returns the intent for the shell to run.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::Close => {
                self.open = false;
                None
            }
            Msg::Opened { path } => {
                self.open = true;
                self.path = Some(path.clone());
                self.commits.clear();
                self.rows.clear();
                self.seen.clear();
                self.next_cursor = None;
                self.error = None;
                self.selected = None;
                self.loading = true;
                Some(Action::Load { path })
            }
            Msg::PageLoaded {
                commits,
                rows,
                next_cursor,
            } => {
                self.loading = false;
                self.error = None;
                // Zip keeps commits and rows parallel even if a page were
                // to disagree on lengths; duplicates by sha are skipped.
                for (commit, row) in commits.into_iter().zip(rows) {
                    if self.seen.insert(commit.sha.clone()) {
                        self.commits.push(commit);
                        self.rows.push(row);
                    }
                }
                self.next_cursor = next_cursor;
                None
            }
            Msg::LoadMore => {
                if self.next_cursor.is_some() && !self.loading {
                    self.loading = true;
                    Some(Action::LoadMore)
                } else {
                    None
                }
            }
            Msg::SelectCommit(sha) => {
                match sha {
                    Some(sha) => {
                        // Clicking the selected commit toggles it off.
                        if self.selected.as_deref() == Some(sha.as_str()) {
                            self.selected = None;
                        } else {
                            self.selected = Some(sha);
                        }
                    }
                    None => self.selected = None,
                }
                None
            }
            Msg::CommitDetails(sha) => Some(Action::SelectCommit(sha)),
            Msg::OpenDiff(sha) => self.path.as_ref().map(|path| Action::OpenDiff {
                path: path.clone(),
                sha,
            }),
            Msg::Failed(err) => {
                self.loading = false;
                self.error = Some(err);
                None
            }
        }
    }

    /// The selected commit, if still present.
    /// The selected commit (tests / shell introspection).
    #[allow(dead_code)]
    pub fn selected_commit(&self) -> Option<&CommitInfo> {
        let sha = self.selected.as_deref()?;
        self.commits.iter().find(|commit| commit.sha == sha)
    }

    /// True when the walk is exhausted (no `next_cursor` and not loading).
    /// Whether every page of the stream arrived (tests / shell
    /// introspection; the stream pages itself to completion).
    #[allow(dead_code)]
    pub fn complete(&self) -> bool {
        self.next_cursor.is_none() && !self.loading
    }

    /// File-history card, or `None` while closed (the shell decides
    /// placement).
    pub fn view(&self, tokens: Tokens) -> Option<Element<'_, Msg>> {
        if !self.open {
            return None;
        }

        let mut header = row![
            text("File history").size(12).font(theme::semibold()),
            text(self.path.as_deref().unwrap_or_default())
                .size(11)
                .font(theme::mono())
                .color(tokens.muted_foreground),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        header = header.push(
            text(status_label(self))
                .size(10)
                .color(tokens.muted_foreground),
        );
        if self.error.is_some() {
            header = header.push(text("· failed").size(10).color(tokens.deleted));
        }
        header = header.push(Space::new().width(Length::Fill));
        if self.next_cursor.is_some() {
            header = header.push(
                button(text("Load more").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 6])
                    .on_press(Msg::LoadMore),
            );
        }
        header = header.push(
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::Close),
        );

        let body: Element<'_, Msg> = if let Some(err) = &self.error {
            if self.commits.is_empty() {
                note(tokens, err, true)
            } else {
                note(
                    tokens,
                    format!("{err} · showing {} commits so far", self.commits.len()),
                    true,
                )
            }
        } else if self.commits.is_empty() && self.loading {
            note(tokens, "Loading history…", false)
        } else if self.commits.is_empty() {
            note(tokens, "No commits touch this file.", false)
        } else {
            commit_list(self, tokens)
        };

        let card = column![header, body]
            .spacing(4)
            .width(Length::Fill)
            .height(Length::Fill);

        Some(
            container(card.padding([8, 10]))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_theme| container::Style {
                    background: Some(Background::Color(tokens.card)),
                    border: Border {
                        color: tokens.border,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..container::Style::default()
                })
                .into(),
        )
    }
}

/// Map one [`CommitInfo`] to its compact display row (pure).
pub fn file_history_row(index: usize, commit: &CommitInfo, now: i64) -> FileHistoryRow {
    FileHistoryRow {
        index,
        sha: short_sha(&commit.sha).to_string(),
        summary: clip(short_summary(commit).trim(), 72),
        age: relative_age(commit.author.time, now),
    }
}

fn status_label(view: &FileHistoryView) -> String {
    let count = if view.commits.is_empty() && view.loading {
        "loading…".to_string()
    } else if view.loading {
        format!("{} commits · loading…", view.commits.len())
    } else {
        format!("{} commits", view.commits.len())
    };
    if view.next_cursor.is_some() && !view.loading {
        return format!("{count} · more available");
    }
    count
}

fn commit_list<'a>(view: &'a FileHistoryView, tokens: Tokens) -> Element<'a, Msg> {
    let now = now_seconds();
    let mut content = column![].spacing(0).width(Length::Fill);
    for (index, commit) in view.commits.iter().enumerate() {
        let selected = view.selected.as_deref() == Some(commit.sha.as_str());
        content = content.push(commit_row_view(
            file_history_row(index, commit, now),
            selected,
            commit,
            tokens,
        ));
    }
    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn commit_row_view<'a>(
    model: FileHistoryRow,
    selected: bool,
    commit: &'a CommitInfo,
    tokens: Tokens,
) -> Element<'a, Msg> {
    let background = if selected {
        theme::with_alpha(tokens.primary, 0.14)
    } else {
        tokens.card
    };

    let line = button(
        row![
            text(model.sha)
                .size(10)
                .font(theme::mono())
                .color(tokens.muted_foreground)
                .width(Length::Fixed(56.0)),
            text(model.summary)
                .size(11)
                .color(tokens.foreground)
                .width(Length::Fill),
            text(model.age)
                .size(9)
                .color(tokens.muted_foreground)
                .width(Length::Fixed(52.0))
                .align_x(Alignment::End),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .style(move |_theme, status| {
        let bg = match status {
            button::Status::Hovered => theme::mix(background, tokens.hover, 0.5),
            _ => background,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: tokens.foreground,
            border: Border::default(),
            shadow: iced::Shadow::default(),
            snap: true,
        }
    })
    .padding([0, 4])
    .width(Length::Fill)
    .height(Length::Fixed(FILE_HISTORY_ROW_HEIGHT))
    .on_press(Msg::SelectCommit(Some(commit.sha.clone())));

    let mut row_view = row![line]
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if selected {
        row_view = row_view
            .push(
                button(text("Diff").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 6])
                    .on_press(Msg::OpenDiff(commit.sha.clone())),
            )
            .push(
                button(text("View commit").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 6])
                    .on_press(Msg::CommitDetails(commit.sha.clone())),
            );
    }

    container(row_view).width(Length::Fill).into()
}

fn note<'a>(tokens: Tokens, label: impl Into<String>, is_error: bool) -> Element<'a, Msg> {
    let color = if is_error {
        tokens.deleted
    } else {
        tokens.muted_foreground
    };
    container(text(label.into()).size(10).color(color))
        .padding([8, 4])
        .into()
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::GitSignature;

    fn signature(time: i64) -> GitSignature {
        GitSignature {
            name: "Raul".to_string(),
            email: "raul@example.com".to_string(),
            time,
            offset_minutes: 0,
        }
    }

    fn commit(sha: &str, summary: &str, time: i64) -> CommitInfo {
        CommitInfo {
            sha: sha.to_string(),
            parents: Vec::new(),
            author: signature(time),
            committer: signature(time),
            message: summary.to_string(),
            summary: summary.to_string(),
            refs: Vec::new(),
        }
    }

    fn row(sha: &str) -> GraphRow {
        GraphRow {
            sha: sha.to_string(),
            lane: 0,
            edges: Vec::new(),
            lane_count: 1,
        }
    }

    fn page(commits: Vec<CommitInfo>, next_cursor: Option<&str>) -> Msg {
        Msg::PageLoaded {
            rows: commits.iter().map(|c| row(&c.sha)).collect(),
            commits,
            next_cursor: next_cursor.map(str::to_string),
        }
    }

    fn opened_view() -> FileHistoryView {
        let mut view = FileHistoryView::default();
        let _ = view.update(Msg::Opened {
            path: "src/main.rs".to_string(),
        });
        view
    }

    #[test]
    fn opened_resets_and_emits_load() {
        let mut view = FileHistoryView {
            open: false,
            path: Some("old".to_string()),
            loading: true,
            commits: vec![commit("a", "keep", 1)],
            rows: vec![row("a")],
            next_cursor: Some("a".to_string()),
            error: Some("stale".to_string()),
            selected: Some("a".to_string()),
            seen: HashSet::new(),
        };
        // Re-dirty `seen` to prove it is cleared.
        view.seen.insert("a".to_string());
        let action = view.update(Msg::Opened {
            path: "src/lib.rs".to_string(),
        });
        assert_eq!(
            action,
            Some(Action::Load {
                path: "src/lib.rs".to_string(),
            })
        );
        assert!(view.open);
        assert_eq!(view.path.as_deref(), Some("src/lib.rs"));
        assert!(view.loading);
        assert!(view.commits.is_empty() && view.rows.is_empty());
        assert!(view.next_cursor.is_none() && view.error.is_none());
        assert_eq!(view.selected, None);
        assert!(!view.seen.contains("a"));
    }

    #[test]
    fn pages_accumulate_with_sha_dedupe() {
        let mut view = opened_view();
        assert_eq!(
            view.update(page(
                vec![
                    commit("a", "first", 1_700_000_000),
                    commit("b", "second", 1_699_000_000),
                ],
                Some("b"),
            )),
            None
        );
        assert_eq!(view.commits.len(), 2);
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.next_cursor.as_deref(), Some("b"));
        assert!(!view.loading);

        // Overlapping page: the duplicated leading commit is skipped.
        let _ = view.update(page(
            vec![
                commit("b", "second", 1_699_000_000),
                commit("c", "third", 1_698_000_000),
            ],
            None,
        ));
        let shas: Vec<&str> = view.commits.iter().map(|c| c.sha.as_str()).collect();
        assert_eq!(shas, vec!["a", "b", "c"]);
        assert_eq!(view.rows.len(), 3);
        assert!(view.complete());
        // Rows stay parallel to commits after dedupe.
        assert_eq!(view.rows[2].sha, "c");
    }

    #[test]
    fn duplicates_inside_one_page_are_skipped() {
        let mut view = opened_view();
        let _ = view.update(page(
            vec![
                commit("a", "x", 1),
                commit("a", "x", 1),
                commit("b", "y", 1),
            ],
            None,
        ));
        assert_eq!(view.commits.len(), 2);
        assert_eq!(view.commits[1].sha, "b");
    }

    #[test]
    fn load_more_follows_cursor_and_loading_state() {
        let mut view = opened_view();
        let _ = view.update(page(vec![commit("a", "x", 1)], Some("a")));
        assert_eq!(view.update(Msg::LoadMore), Some(Action::LoadMore));
        assert!(view.loading);
        // No double-request while a page is in flight.
        assert_eq!(view.update(Msg::LoadMore), None);
        // Final page: no more loads.
        let _ = view.update(page(vec![commit("b", "y", 1)], None));
        assert_eq!(view.update(Msg::LoadMore), None);
        assert!(view.complete());
    }

    #[test]
    fn select_commit_sets_toggles_and_clears() {
        let mut view = opened_view();
        let _ = view.update(page(vec![commit("a", "x", 1), commit("b", "y", 1)], None));

        let _ = view.update(Msg::SelectCommit(Some("a".to_string())));
        assert_eq!(view.selected.as_deref(), Some("a"));
        assert_eq!(view.selected_commit().map(|c| c.sha.as_str()), Some("a"));

        // Same sha again toggles off.
        let _ = view.update(Msg::SelectCommit(Some("a".to_string())));
        assert_eq!(view.selected, None);

        let _ = view.update(Msg::SelectCommit(Some("b".to_string())));
        // Explicit None clears.
        let _ = view.update(Msg::SelectCommit(None));
        assert_eq!(view.selected, None);
        assert!(view.selected_commit().is_none());
    }

    #[test]
    fn detail_and_diff_intents() {
        let mut view = FileHistoryView::default();
        // Without a path there is nothing to diff.
        assert_eq!(view.update(Msg::OpenDiff("a".to_string())), None);

        let mut view = opened_view();

        let _ = view.update(Msg::SelectCommit(Some("a".to_string())));
        assert_eq!(
            view.update(Msg::CommitDetails("a".to_string())),
            Some(Action::SelectCommit("a".to_string()))
        );
        assert_eq!(
            view.update(Msg::OpenDiff("a".to_string())),
            Some(Action::OpenDiff {
                path: "src/main.rs".to_string(),
                sha: "a".to_string(),
            })
        );
    }

    #[test]
    fn failed_finalizes_without_dropping_commits() {
        let mut view = opened_view();
        let _ = view.update(page(vec![commit("a", "x", 1)], Some("a")));
        let _ = view.update(Msg::LoadMore);
        assert!(view.loading);
        assert_eq!(view.update(Msg::Failed("stream aborted".to_string())), None);
        assert!(!view.loading);
        assert_eq!(view.error.as_deref(), Some("stream aborted"));
        assert_eq!(view.commits.len(), 1);
        // A late page still recovers the stream.
        let _ = view.update(page(vec![commit("b", "y", 1)], None));
        assert!(view.error.is_none());
        assert_eq!(view.commits.len(), 2);
    }

    #[test]
    fn close_is_pure() {
        let mut view = opened_view();
        assert_eq!(view.update(Msg::Close), None);
        assert!(!view.open);
    }

    #[test]
    fn row_model_maps_the_fixture() {
        let commit = commit(
            "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11",
            "feat: add file history",
            1_700_000_000,
        );
        let model = file_history_row(3, &commit, 1_700_006_400);
        assert_eq!(model.index, 3);
        assert_eq!(model.sha, "aaaaaaa");
        assert_eq!(model.summary, "feat: add file history");
        assert_eq!(model.age, "1h ago");
    }

    #[test]
    fn row_model_clips_long_summaries() {
        let commit = commit("a", &"s".repeat(90), 1_700_000_000);
        let model = file_history_row(0, &commit, 1_700_000_000);
        assert_eq!(model.summary.chars().count(), 72);
        assert!(model.summary.ends_with('…'));
    }

    #[test]
    fn clip_keeps_short_text() {
        assert_eq!(clip("short", 10), "short");
    }
}
