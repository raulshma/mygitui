//! Two-ref compare: diff summary between arbitrary commit-ishes.
//!
//! Self-contained read surface. The shell opens it (`open = true`), the
//! user picks two refs (defaults `HEAD~1` → `HEAD`), and pressing Compare
//! (or swapping refs) makes [`CompareView::update`] emit
//! `Action::Load { old_ref, new_ref }`. The shell executes
//! `service::diff::repo_diff` with `DiffSide::Commit(old_ref)` /
//! `DiffSide::Commit(new_ref)` and feeds the result back as
//! `Msg::Loaded`. This view renders the file summary list only — the
//! selected file's diff is rendered by the shell through its existing
//! `DiffPane` (`diff_view`), driven by `Action::OpenFileDiff` /
//! [`CompareView::selected_file`].
//!
//! The loaded payload keeps a derived [`DiffRowModel`]
//! (`diff_model::build_row_model` is public and reused — not duplicated)
//! so the shell can lay out or pre-virtualize the result without a second
//! pass.

// Registered for gates before the shell wires this module in; drop this
// allow once `app.rs` routes `Msg`/`Action`.

use std::collections::HashSet;

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::FileDiff;

use super::diff_model::{build_row_model, file_change_kind, file_kind_letter, DiffRowModel};
use super::theme::{self, Tokens};

/// View state: ref inputs, loaded diff summary and layout options.
#[derive(Debug)]
pub struct CompareView {
    pub open: bool,
    /// "Before" side; prefilled to `HEAD~1`.
    pub old_ref: String,
    /// "After" side; prefilled to `HEAD`.
    pub new_ref: String,
    /// Loaded diff (files + derived row model). Empty until a compare runs.
    pub diff: CompareDiff,
    pub error: Option<String>,
    /// Split-side rendering flag for the downstream diff pane.
    pub split: bool,
    /// Index into [`CompareDiff::files`] of the highlighted file.
    pub selected: Option<usize>,
}

impl Default for CompareView {
    fn default() -> Self {
        Self {
            open: false,
            old_ref: "HEAD~1".to_string(),
            new_ref: "HEAD".to_string(),
            diff: CompareDiff::default(),
            error: None,
            split: false,
            selected: None,
        }
    }
}

/// Loaded compare payload: the `FileDiff`s plus a derived row model.
#[derive(Debug, Clone, Default)]
pub struct CompareDiff {
    pub files: Vec<FileDiff>,
    /// Flat row model over `files` (`diff_model::build_row_model`); totals
    /// reflect the whole compare, not just the selected file.
    pub rows: DiffRowModel,
}

impl CompareDiff {
    /// Build the payload for one compare result.
    pub fn build(files: Vec<FileDiff>, split: bool) -> Self {
        let rows = build_row_model(&files, split, &HashSet::new());
        Self { files, rows }
    }

    /// `(additions, deletions)` totals across the compare.
    pub fn totals(&self) -> (u32, u32) {
        (self.rows.total_additions, self.rows.total_deletions)
    }
}

/// View messages (shell → [`CompareView::update`]).
#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    OldRefChanged(String),
    NewRefChanged(String),
    /// Swap the two refs; re-runs the compare when both refs are set.
    Swapped,
    /// Run the compare for the current refs.
    ComparePressed,
    /// Result of `service::diff::repo_diff` for the two Commit sides.
    Loaded(Result<Vec<FileDiff>, String>),
    /// Click on the file at `index`; also emits `Action::OpenFileDiff`.
    SelectFile(usize),
    ToggleSplit,
}

/// Intents returned by [`CompareView::update`] for the shell to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run `service::diff::repo_diff` with `DiffSide::Commit(old_ref)` /
    /// `DiffSide::Commit(new_ref)`; feed the result back as `Msg::Loaded`.
    Load { old_ref: String, new_ref: String },
    /// Load the selected file's diff for the current refs (a one-path
    /// `repo_diff` with the same two Commit sides); the shell renders it
    /// through its existing `DiffPane`.
    OpenFileDiff { path: String },
}

impl CompareView {
    /// Pure state transition; returns the intent for the shell to run.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::Close => {
                self.open = false;
                None
            }
            Msg::OldRefChanged(value) => {
                self.old_ref = value;
                None
            }
            Msg::NewRefChanged(value) => {
                self.new_ref = value;
                None
            }
            Msg::Swapped => {
                std::mem::swap(&mut self.old_ref, &mut self.new_ref);
                self.request_compare()
            }
            Msg::ComparePressed => self.request_compare(),
            Msg::Loaded(Ok(files)) => {
                self.error = None;
                self.selected = None;
                self.diff = CompareDiff::build(files, self.split);
                None
            }
            Msg::Loaded(Err(err)) => {
                self.error = Some(err);
                self.selected = None;
                self.diff = CompareDiff::default();
                None
            }
            Msg::SelectFile(index) => {
                // Out-of-bounds clicks are ignored.
                let path = self.diff.files.get(index).map(|file| file.path.clone());
                match path {
                    Some(path) => {
                        self.selected = Some(index);
                        Some(Action::OpenFileDiff { path })
                    }
                    None => None,
                }
            }
            Msg::ToggleSplit => {
                self.split = !self.split;
                self.diff.rows = build_row_model(&self.diff.files, self.split, &HashSet::new());
                None
            }
        }
    }

    /// Compare intent from the current inputs; requires both refs set.
    fn request_compare(&mut self) -> Option<Action> {
        let old = self.old_ref.trim();
        let new = self.new_ref.trim();
        if old.is_empty() || new.is_empty() {
            return None;
        }
        self.error = None;
        Some(Action::Load {
            old_ref: old.to_string(),
            new_ref: new.to_string(),
        })
    }

    /// The highlighted file, if any.
    /// The selected file (tests / shell introspection).
    #[allow(dead_code)]
    pub fn selected_file(&self) -> Option<&FileDiff> {
        let index = self.selected?;
        self.diff.files.get(index)
    }

    /// Compare card, or `None` while closed (the shell decides placement).
    pub fn view(&self, tokens: Tokens) -> Option<Element<'_, Msg>> {
        if !self.open {
            return None;
        }

        let header = row![
            text("Compare")
                .size(theme::TEXT_BODY)
                .font(theme::semibold()),
            Space::new().width(Length::Fill),
            theme::tip(
                tokens,
                button(text(theme::icon::CLOSE).size(theme::TEXT_BODY))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::Close),
                "Close compare",
            ),
        ]
        .align_y(Alignment::Center);

        let refs = row![
            text("Old")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
            text_input("HEAD~1", self.old_ref.as_str())
                .on_input(Msg::OldRefChanged)
                .on_submit(Msg::ComparePressed)
                .size(theme::TEXT_SMALL)
                .padding([4, 6])
                .width(Length::Fixed(150.0)),
            theme::tip(
                tokens,
                button(text(theme::icon::SWAP).size(theme::TEXT_SMALL))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::Swapped),
                "Swap old and new refs",
            ),
            text("New")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
            text_input("HEAD", self.new_ref.as_str())
                .on_input(Msg::NewRefChanged)
                .on_submit(Msg::ComparePressed)
                .size(theme::TEXT_SMALL)
                .padding([4, 6])
                .width(Length::Fixed(150.0)),
            button(text("Compare").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                .padding([3, 10])
                .on_press(Msg::ComparePressed),
            Space::new().width(Length::Fill),
            button(text(if self.split { "Split" } else { "Unified" }).size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([3, 8])
                .on_press(Msg::ToggleSplit),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let mut card = column![header, refs].spacing(6);

        if let Some(err) = &self.error {
            // Compare re-runs the diff for the current refs, so it doubles
            // as the retry.
            let retry_note: Element<'_, Msg> = row![
                theme::note(tokens, err.as_str(), true, theme::NoteKind::Inline),
                theme::mini_button(
                    tokens,
                    "Retry",
                    "Run the compare again",
                    Some(Msg::ComparePressed),
                ),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into();
            card = card.push(retry_note);
        }

        if !self.diff.files.is_empty() {
            let (additions, deletions) = self.diff.totals();
            card = card.push(
                text(format!(
                    "{} files · +{} −{}",
                    self.diff.files.len(),
                    additions,
                    deletions
                ))
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
            );
            card = card.push(file_list(self, tokens));
        } else if self.error.is_none() {
            card = card.push(theme::note(
                tokens,
                "Pick two refs and press Compare.",
                false,
                theme::NoteKind::Empty,
            ));
        }

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

/// Display model for one file summary row (pure mapping, unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSummary {
    pub path: String,
    /// Change letter: `A`/`D`/`R`/`M` (see `diff_model::file_kind_letter`).
    pub kind_letter: &'static str,
    pub additions: u32,
    pub deletions: u32,
    pub hunk_count: usize,
    pub binary: bool,
    pub image: bool,
}

/// Map one [`FileDiff`] to its summary row (pure).
pub fn file_summary(file: &FileDiff) -> FileSummary {
    let kind = file_change_kind(file);
    FileSummary {
        path: file.path.clone(),
        kind_letter: file_kind_letter(kind),
        additions: file.additions,
        deletions: file.deletions,
        hunk_count: file.hunks.len(),
        binary: file.binary,
        image: file.is_image,
    }
}

fn file_list<'a>(view: &'a CompareView, tokens: Tokens) -> Element<'a, Msg> {
    let mut content = column![].spacing(0).width(Length::Fill);
    for (index, file) in view.diff.files.iter().enumerate() {
        let selected = view.selected == Some(index);
        content = content.push(file_row_view(index, file_summary(file), selected, tokens));
    }
    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn file_row_view<'a>(
    index: usize,
    model: FileSummary,
    selected: bool,
    tokens: Tokens,
) -> Element<'a, Msg> {
    let kind_color = match model.kind_letter {
        "A" => tokens.added,
        "D" => tokens.deleted,
        "R" => tokens.renamed,
        _ => tokens.modified,
    };

    let mut line = row![
        text(model.kind_letter)
            .size(theme::TEXT_META)
            .font(theme::mono())
            .color(kind_color)
            .width(Length::Fixed(16.0)),
        text(model.path)
            .size(theme::TEXT_SMALL)
            .font(theme::mono())
            .color(tokens.foreground)
            .width(Length::Fill),
        text(format!("+{}", model.additions))
            .size(theme::TEXT_META)
            .color(tokens.added)
            .width(Length::Fixed(52.0))
            .align_x(Alignment::End),
        text(format!("−{}", model.deletions))
            .size(theme::TEXT_META)
            .color(tokens.deleted)
            .width(Length::Fixed(52.0))
            .align_x(Alignment::End),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    if model.binary {
        line = line.push(theme::chip(
            "binary",
            tokens.muted_foreground,
            theme::ChipShape::Pill,
        ));
    }
    if model.image {
        line = line.push(theme::chip(
            "image",
            tokens.muted_foreground,
            theme::ChipShape::Pill,
        ));
    }
    line = line.push(
        text(format!("{} hunks", model.hunk_count))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(56.0))
            .align_x(Alignment::End),
    );

    let background = if selected {
        theme::with_alpha(tokens.primary, 0.14)
    } else {
        tokens.card
    };

    container(
        button(container(line).width(Length::Fill).padding([0, 4]))
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
            .padding(0)
            .width(Length::Fill)
            .on_press(Msg::SelectFile(index)),
    )
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{DiffHunk, DiffLine};

    fn line(origin: char, old_no: Option<u32>, new_no: Option<u32>) -> DiffLine {
        DiffLine {
            old_no,
            new_no,
            origin,
            text: "x".to_string(),
            highlights: Vec::new(),
        }
    }

    fn file(path: &str, additions: u32, deletions: u32) -> FileDiff {
        let mut lines = Vec::new();
        for i in 0..additions {
            lines.push(line('+', None, Some(i + 1)));
        }
        for i in 0..deletions {
            lines.push(line('-', Some(i + 1), None));
        }
        FileDiff {
            path: path.to_string(),
            old_path: None,
            binary: false,
            is_image: false,
            additions,
            deletions,
            hunks: if lines.is_empty() {
                Vec::new()
            } else {
                vec![DiffHunk {
                    old_start: 1,
                    new_start: 1,
                    lines,
                }]
            },
        }
    }

    fn fixture_files() -> Vec<FileDiff> {
        vec![file("src/added.rs", 4, 0), file("src/edited.rs", 2, 3), {
            let mut binary = file("assets/logo.png", 0, 0);
            binary.binary = true;
            binary.is_image = true;
            binary
        }]
    }

    #[test]
    fn refs_default_to_head_minus_one_and_head() {
        let view = CompareView::default();
        assert_eq!(view.old_ref, "HEAD~1");
        assert_eq!(view.new_ref, "HEAD");
        assert!(!view.open);
        assert!(!view.split);
        assert_eq!(view.selected, None);
    }

    #[test]
    fn compare_pressed_requires_both_refs() {
        let mut view = CompareView {
            open: true,
            new_ref: "   ".to_string(),
            ..CompareView::default()
        };
        assert_eq!(view.old_ref, "HEAD~1");
        assert_eq!(view.update(Msg::ComparePressed), None);

        view.new_ref = "HEAD".to_string();
        assert_eq!(
            view.update(Msg::ComparePressed),
            Some(Action::Load {
                old_ref: "HEAD~1".to_string(),
                new_ref: "HEAD".to_string(),
            })
        );
    }

    #[test]
    fn ref_inputs_trim_and_swap_reloads() {
        let mut view = CompareView::default();
        let _ = view.update(Msg::OldRefChanged(" main~2 ".to_string()));
        let _ = view.update(Msg::NewRefChanged("feature".to_string()));
        assert_eq!(view.old_ref, " main~2 ");
        assert_eq!(view.new_ref, "feature");

        let action = view.update(Msg::Swapped);
        assert_eq!(view.old_ref, "feature");
        assert_eq!(view.new_ref, " main~2 ");
        // Swapped refs are trimmed in the emitted load.
        assert_eq!(
            action,
            Some(Action::Load {
                old_ref: "feature".to_string(),
                new_ref: "main~2".to_string(),
            })
        );
    }

    #[test]
    fn swap_stays_idle_without_refs() {
        let mut view = CompareView {
            old_ref: "HEAD".to_string(),
            new_ref: String::new(),
            ..CompareView::default()
        };
        assert_eq!(view.update(Msg::Swapped), None);
        assert_eq!(view.old_ref, "");
        assert_eq!(view.new_ref, "HEAD");
    }

    #[test]
    fn loaded_builds_row_model_and_totals() {
        let mut view = CompareView::default();
        let _ = view.update(Msg::Loaded(Ok(fixture_files())));

        assert_eq!(view.diff.files.len(), 3);
        // Row model is derived: one header per file plus hunk/line rows.
        assert!(view.diff.rows.rows.len() > 3);
        assert_eq!(view.diff.totals(), (6, 3));
        assert!(view.error.is_none());
        assert_eq!(view.selected, None);
    }

    #[test]
    fn loaded_error_resets_payload() {
        let mut view = CompareView::default();
        let _ = view.update(Msg::Loaded(Ok(fixture_files())));
        let _ = view.update(Msg::SelectFile(0));
        assert!(view.selected_file().is_some());

        let _ = view.update(Msg::Loaded(Err("bad ref".to_string())));
        assert_eq!(view.error.as_deref(), Some("bad ref"));
        assert!(view.diff.files.is_empty());
        assert_eq!(view.diff.totals(), (0, 0));
        assert!(view.selected_file().is_none());
    }

    #[test]
    fn select_file_maps_summary_and_emits_open_intent() {
        let mut view = CompareView::default();
        let _ = view.update(Msg::Loaded(Ok(fixture_files())));

        assert_eq!(view.update(Msg::SelectFile(99)), None);
        assert_eq!(view.selected, None);

        assert_eq!(
            view.update(Msg::SelectFile(1)),
            Some(Action::OpenFileDiff {
                path: "src/edited.rs".to_string(),
            })
        );
        assert_eq!(view.selected, Some(1));
        assert_eq!(
            view.selected_file().map(|f| f.path.as_str()),
            Some("src/edited.rs")
        );
    }

    #[test]
    fn toggle_split_flips_and_rebuilds_rows() {
        let mut view = CompareView::default();
        let _ = view.update(Msg::Loaded(Ok(fixture_files())));
        let unified_rows = view.diff.rows.rows.len();
        assert!(!view.diff.rows.has_split_rows);

        let _ = view.update(Msg::ToggleSplit);
        assert!(view.split);
        assert!(view.diff.rows.has_split_rows);
        // Split pairing changes the row count for uneven hunks.
        assert_ne!(view.diff.rows.rows.len(), unified_rows);
    }

    #[test]
    fn file_summary_maps_the_fixture() {
        let summary = file_summary(&file("src/edited.rs", 2, 3));
        assert_eq!(summary.path, "src/edited.rs");
        assert_eq!(summary.kind_letter, "M");
        assert_eq!(summary.additions, 2);
        assert_eq!(summary.deletions, 3);
        assert_eq!(summary.hunk_count, 1);
        assert!(!summary.binary && !summary.image);

        let added = file_summary(&file("src/new.rs", 4, 0));
        assert_eq!(added.kind_letter, "A");

        let mut renamed = file("src/moved.rs", 1, 1);
        renamed.old_path = Some("src/old.rs".to_string());
        assert_eq!(file_summary(&renamed).kind_letter, "R");
    }

    #[test]
    fn file_summary_flags_binary_and_image() {
        let mut binary = file("assets/logo.png", 0, 0);
        binary.binary = true;
        binary.is_image = true;
        let summary = file_summary(&binary);
        assert!(summary.binary);
        assert!(summary.image);
        assert_eq!(summary.hunk_count, 0);
    }

    #[test]
    fn close_is_pure() {
        let mut view = CompareView {
            open: true,
            ..CompareView::default()
        };
        assert_eq!(view.update(Msg::Close), None);
        assert!(!view.open);
    }
}
