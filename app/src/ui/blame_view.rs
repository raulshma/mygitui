//! Blame view: per-line attribution for one file.
//!
//! Self-contained read surface: owns its messages ([`Msg`]) and intents
//! ([`Action`]). The shell opens it with `Msg::Opened { path }`, routes the
//! returned `Action::Load` into `service::repo::repo_blame` and feeds the
//! result back as `Msg::LinesLoaded`. `Action::SelectCommit` is routed to
//! the shell's existing commit-detail flow; `Action::CopySha` goes to the
//! clipboard. The view itself never mutates repo state.
//!
//! Rows are plain fixed-height widgets in a `scrollable` (blame output is
//! typically < 5k lines, so full-row rendering stays cheap) with stable
//! per-commit sha colours and age-heat tinting.

// Registered for gates before the shell wires this module in; drop this
// allow once `app.rs` routes `Msg`/`Action`.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::engine::types::BlameLine;

use super::history_model::relative_age;
use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// Fixed height of one blame row (kept constant so the list stays cheap).
pub const BLAME_ROW_HEIGHT: f32 = 20.0;

/// Size of the stable sha→colour palette (FNV-1a hash bucket).
pub const SHA_PALETTE_LEN: usize = 8;

/// Age steps (days) of the heat ramp: fresher than each step stays hotter.
const HEAT_STEPS: [i64; 6] = [1, 7, 30, 90, 180, 365];

/// View state: load state, blame lines and the selected commit sha.
#[derive(Debug, Default)]
pub struct BlameView {
    pub open: bool,
    pub path: Option<String>,
    pub loading: bool,
    pub lines: Option<Vec<BlameLine>>,
    pub error: Option<String>,
    /// Sha of the commit owning the selected line (footer strip).
    pub selected: Option<String>,
}

/// View messages (shell → [`BlameView::update`]).
#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    /// The shell opened blame for `path`; resets state and asks for a load.
    Opened {
        path: String,
    },
    /// Result of `service::repo::repo_blame`.
    LinesLoaded(Result<Vec<BlameLine>, String>),
    /// Click on the line at `index` (0-based, order of [`BlameView::lines`]).
    SelectLine(usize),
    CopySha(String),
    /// "View commit" on the selected line; the shell opens commit detail.
    SelectCommit(String),
}

/// Intents returned by [`BlameView::update`] for the shell to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Load blame for `path` via `service::repo::repo_blame`
    /// (`from: None`); feed the result back as `Msg::LinesLoaded`.
    Load { path: String },
    /// Copy the full sha to the clipboard.
    CopySha(String),
    /// The shell routes this to its existing commit-detail flow.
    SelectCommit(String),
}

/// Display model for one blame row (pure mapping, unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameRow {
    /// Position in the line list.
    pub index: usize,
    /// 1-based file line number.
    pub line_no: u32,
    /// Full sha of the commit that last touched the line.
    pub sha: String,
    pub author: String,
    /// Compact relative age of the commit.
    pub age: String,
    /// Whole days since the commit (drives [`heat_color`]).
    pub age_days: i64,
    pub text: String,
}

/// Footer digest of the commit owning the selected line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDigest {
    /// Short (7-char) sha.
    pub sha: String,
    pub author: String,
    /// Compact relative age of the commit.
    pub age: String,
    /// How many lines of the file this commit last touched.
    pub line_count: usize,
}

impl BlameView {
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
                self.loading = true;
                self.lines = None;
                self.error = None;
                self.selected = None;
                Some(Action::Load { path })
            }
            Msg::LinesLoaded(Ok(lines)) => {
                self.loading = false;
                self.error = None;
                // Drop a selection whose commit no longer appears (reload
                // against a different revision); keep it otherwise.
                let stale = self
                    .selected
                    .as_ref()
                    .is_some_and(|sha| !lines.iter().any(|line| &line.sha == sha));
                if stale {
                    self.selected = None;
                }
                self.lines = Some(lines);
                None
            }
            Msg::LinesLoaded(Err(err)) => {
                self.loading = false;
                self.error = Some(err);
                None
            }
            Msg::SelectLine(index) => {
                // Out-of-bounds clicks are ignored (clamped, never panic).
                if let Some(line) = self.lines.as_ref().and_then(|lines| lines.get(index)) {
                    self.selected = Some(line.sha.clone());
                }
                None
            }
            Msg::CopySha(sha) => Some(Action::CopySha(sha)),
            Msg::SelectCommit(sha) => Some(Action::SelectCommit(sha)),
        }
    }

    /// Digest of the commit owning the selected line, if any.
    pub fn selected_digest(&self, now: i64) -> Option<CommitDigest> {
        let sha = self.selected.as_deref()?;
        let lines = self.lines.as_deref()?;
        commit_digest(lines, sha, now)
    }

    /// Blame card, or `None` while closed (the shell decides placement).
    pub fn view(&self, tokens: Tokens) -> Option<Element<'_, Msg>> {
        if !self.open {
            return None;
        }

        let title = self
            .path
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| "Blame".to_string());

        let mut header = row![
            text("Blame").size(12).font(theme::semibold()),
            text(clip(&title, 64))
                .size(11)
                .font(theme::mono())
                .color(tokens.muted_foreground),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        header = header.push(match &self.lines {
            Some(lines) => text(format!("{} lines", lines.len()))
                .size(10)
                .color(tokens.muted_foreground),
            None => text(String::new()).size(10),
        });
        if self.loading {
            header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
        }
        header = header.push(Space::new().width(Length::Fill)).push(
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::Close),
        );

        let body: Element<'_, Msg> = match &self.lines {
            Some(lines) if !lines.is_empty() => blame_list(lines, &self.selected, tokens),
            Some(_) => note(tokens, "Empty file.", false),
            None if self.error.is_some() => {
                note(tokens, self.error.as_deref().unwrap_or_default(), true)
            }
            None if self.loading => note(tokens, "Loading blame…", false),
            None => note(tokens, "No blame loaded.", false),
        };

        let mut card = column![header].spacing(4);
        if let Some(err) = &self.error {
            card = card.push(
                text(err.as_str())
                    .size(10)
                    .color(tokens.deleted)
                    .width(Length::Fill),
            );
        }
        if let Some(digest) = self.selected_digest(now_seconds()) {
            card = card.push(footer(digest, tokens));
        }
        card = card.push(body);

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

/// Map one [`BlameLine`] to its display row (pure).
pub fn blame_row(index: usize, line: &BlameLine, now: i64) -> BlameRow {
    let age_days = ((now - line.signature.time) / 86_400).max(0);
    BlameRow {
        index,
        line_no: line.line_no,
        sha: line.sha.clone(),
        author: clip(line.signature.name.trim(), 16),
        age: relative_age(line.signature.time, now),
        age_days,
        text: line.text.trim_end().to_string(),
    }
}

/// Digest of one commit across a blame: author/age from its first line.
pub fn commit_digest(lines: &[BlameLine], sha: &str, now: i64) -> Option<CommitDigest> {
    let mut digest: Option<CommitDigest> = None;
    for line in lines {
        if line.sha != sha {
            continue;
        }
        let entry = digest.get_or_insert_with(|| CommitDigest {
            sha: short_sha(sha).to_string(),
            author: line.signature.name.trim().to_string(),
            age: relative_age(line.signature.time, now),
            line_count: 0,
        });
        entry.line_count += 1;
    }
    digest
}

/// Stable palette index for a sha: FNV-1a over its bytes, bucketed. Pure,
/// so a commit keeps its colour across reloads and restarts.
pub fn sha_palette_index(sha: &str) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in sha.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % SHA_PALETTE_LEN as u64) as usize
}

/// Consistent per-commit colour for the short-sha column.
pub fn sha_color(tokens: Tokens, sha: &str) -> Color {
    let palette = [
        tokens.primary,
        tokens.accent,
        tokens.added,
        tokens.deleted,
        tokens.renamed,
        tokens.modified,
        tokens.sidebar_primary,
        tokens.ring,
    ];
    palette[sha_palette_index(sha)]
}

/// Discrete heat ramp over commit age: day 0 is the hottest shade (pure
/// `tokens.primary`), one step cooler per [`HEAT_STEPS`] bucket, fully cold
/// (muted) past a year.
pub fn heat_color(tokens: Tokens, age_days: i64) -> Color {
    let step = HEAT_STEPS.iter().filter(|&&day| age_days >= day).count();
    let t = step as f32 / HEAT_STEPS.len() as f32;
    theme::mix(tokens.primary, tokens.muted_foreground, t)
}

fn blame_list<'a>(
    lines: &'a [BlameLine],
    selected: &Option<String>,
    tokens: Tokens,
) -> Element<'a, Msg> {
    let now = now_seconds();
    let mut content = column![].spacing(0).width(Length::Fill);
    for (index, line) in lines.iter().enumerate() {
        content = content.push(blame_line_view(
            blame_row(index, line, now),
            selected.as_deref() == Some(line.sha.as_str()),
            tokens,
        ));
    }
    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn blame_line_view<'a>(model: BlameRow, selected: bool, tokens: Tokens) -> Element<'a, Msg> {
    let line = row![
        text(short_sha(&model.sha).to_string())
            .size(10)
            .font(theme::mono())
            .color(sha_color(tokens, &model.sha))
            .width(Length::Fixed(52.0)),
        text(model.author)
            .size(10)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(96.0)),
        text(model.age)
            .size(9)
            .color(heat_color(tokens, model.age_days))
            .width(Length::Fixed(52.0))
            .align_x(Alignment::End),
        text(model.line_no.to_string())
            .size(9)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(40.0))
            .align_x(Alignment::End),
        text(model.text)
            .size(11)
            .font(theme::mono())
            .color(tokens.foreground)
            .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .height(Length::Fixed(BLAME_ROW_HEIGHT));

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
            .on_press(Msg::SelectLine(model.index)),
    )
    .width(Length::Fill)
    .into()
}

fn footer(digest: CommitDigest, tokens: Tokens) -> Element<'static, Msg> {
    let select_sha = digest.sha.clone();
    let copy_sha = digest.sha.clone();
    row![
        text(digest.sha)
            .size(10)
            .font(theme::mono())
            .color(tokens.primary),
        text(digest.author).size(10).color(tokens.foreground),
        text(digest.age).size(9).color(tokens.muted_foreground),
        text(format!("{} lines", digest.line_count))
            .size(9)
            .color(tokens.muted_foreground),
        Space::new().width(Length::Fill),
        button(text("View commit").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([2, 6])
            .on_press(Msg::SelectCommit(select_sha)),
        button(text("Copy SHA").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .on_press(Msg::CopySha(copy_sha)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([4, 0])
    .into()
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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

    fn line(line_no: u32, sha: &str, time: i64, text: &str) -> BlameLine {
        BlameLine {
            line_no,
            sha: sha.to_string(),
            signature: signature(time),
            final_sha: sha.to_string(),
            final_signature: signature(time),
            text: text.to_string(),
        }
    }

    fn fixture_lines() -> Vec<BlameLine> {
        vec![
            line(
                1,
                "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11",
                1_700_000_000,
                "fn main() {",
            ),
            line(
                2,
                "bbbbbbbb22222222bbbbbbbb22222222bbbbbbbb22",
                1_690_000_000,
                "    run();",
            ),
            line(
                3,
                "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11",
                1_700_000_000,
                "}",
            ),
        ]
    }

    #[test]
    fn sha_palette_index_is_stable_and_in_bounds() {
        for sha in ["aaaaaaaa11111111", "bbbbbbbb22222222", "short", ""] {
            let index = sha_palette_index(sha);
            assert!(index < SHA_PALETTE_LEN);
            assert_eq!(sha_palette_index(sha), index, "unstable for {sha}");
        }
    }

    #[test]
    fn sha_palette_spreads_across_the_palette() {
        // Deterministic input set must land on more than one bucket.
        let distinct: std::collections::HashSet<usize> = (0..16)
            .map(|i| sha_palette_index(&format!("{i:08x}")))
            .collect();
        assert!(distinct.len() > 1, "hash collapsed to one bucket");
    }

    #[test]
    fn sha_color_is_consistent_per_commit() {
        let tokens = Tokens::DARK;
        // Same sha → same colour, every call (stability is the contract).
        assert_eq!(
            sha_color(tokens, "aaaaaaaa11111111"),
            sha_color(tokens, "aaaaaaaa11111111")
        );
        // Chosen shas keep their slot's colour, whatever the slot.
        assert_eq!(
            sha_color(tokens, "bbbbbbbb22222222"),
            sha_color(tokens, "bbbbbbbb22222222")
        );
        // The colours come from the tokens' palette (light and dark differ
        // somewhere across a spread of shas).
        let differs = (0..16).any(|i| {
            let sha = format!("{i:08x}");
            sha_color(Tokens::LIGHT, &sha) != sha_color(Tokens::DARK, &sha)
        });
        assert!(differs, "sha_color ignores the tokens");
    }

    #[test]
    fn heat_color_is_bucketed_and_monotonic() {
        let tokens = Tokens::DARK;
        let hot = heat_color(tokens, 0);
        assert_eq!(hot, tokens.primary, "day 0 is the pure hot colour");
        // Within one bucket the colour is constant.
        assert_eq!(heat_color(tokens, 2), heat_color(tokens, 6));
        // Each bucket boundary moves a step colder.
        assert_ne!(heat_color(tokens, 0), heat_color(tokens, 1));
        assert_ne!(heat_color(tokens, 6), heat_color(tokens, 7));
        assert_ne!(heat_color(tokens, 364), heat_color(tokens, 365));
        // Fully cold past the last step.
        assert_eq!(heat_color(tokens, 10_000), tokens.muted_foreground);
    }

    #[test]
    fn blame_row_maps_the_fixture() {
        let now = 1_700_086_400; // exactly one day after the newest commit
        let model = blame_row(0, &fixture_lines()[0], now);
        assert_eq!(model.index, 0);
        assert_eq!(model.line_no, 1);
        assert_eq!(model.sha, "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11");
        assert_eq!(model.author, "Raul");
        assert_eq!(model.age, "1d ago");
        assert_eq!(model.age_days, 1);
        assert_eq!(model.text, "fn main() {");
    }

    #[test]
    fn blame_row_clamps_future_timestamps() {
        let model = blame_row(0, &line(1, "cccccccc", 2_000_000_000, "x"), 1_700_000_000);
        assert_eq!(model.age_days, 0);
        assert_eq!(model.age, "just now");
    }

    #[test]
    fn commit_digest_counts_lines_and_takes_first_signature() {
        let now = 1_700_006_400;
        let digest = commit_digest(
            &fixture_lines(),
            "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11",
            now,
        )
        .expect("commit present");
        assert_eq!(digest.sha, "aaaaaaa");
        assert_eq!(digest.author, "Raul");
        assert_eq!(digest.line_count, 2);
        assert!(commit_digest(&fixture_lines(), "missing", now).is_none());
    }

    #[test]
    fn opened_resets_state_and_emits_load() {
        let mut view = BlameView {
            open: false,
            path: Some("old.rs".to_string()),
            loading: false,
            lines: Some(fixture_lines()),
            error: Some("stale".to_string()),
            selected: Some("bbbbbbbb".to_string()),
        };
        let action = view.update(Msg::Opened {
            path: "src/main.rs".to_string(),
        });
        assert_eq!(
            action,
            Some(Action::Load {
                path: "src/main.rs".to_string(),
            })
        );
        assert!(view.open);
        assert_eq!(view.path.as_deref(), Some("src/main.rs"));
        assert!(view.loading);
        assert!(view.lines.is_none());
        assert!(view.error.is_none());
        assert_eq!(view.selected, None);
    }

    #[test]
    fn lines_loaded_keeps_selection_only_when_sha_survives() {
        let mut view = BlameView::default();
        let _ = view.update(Msg::Opened {
            path: "src/main.rs".to_string(),
        });
        let _ = view.update(Msg::LinesLoaded(Ok(fixture_lines())));
        let _ = view.update(Msg::SelectLine(1));
        assert_eq!(
            view.selected.as_deref(),
            Some("bbbbbbbb22222222bbbbbbbb22222222bbbbbbbb22")
        );

        // Reload without that commit: selection is dropped.
        let _ = view.update(Msg::LinesLoaded(Ok(vec![line(
            1,
            "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11",
            1,
            "fn main() {}",
        )])));
        assert_eq!(view.selected, None);

        // Reload that still contains the selected sha: selection is kept.
        let _ = view.update(Msg::SelectLine(0));
        let _ = view.update(Msg::LinesLoaded(Ok(fixture_lines())));
        assert_eq!(
            view.selected.as_deref(),
            Some("aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11")
        );
    }

    #[test]
    fn select_line_is_clamped_to_loaded_lines() {
        let mut view = BlameView::default();
        let _ = view.update(Msg::Opened {
            path: "f".to_string(),
        });
        let _ = view.update(Msg::LinesLoaded(Ok(fixture_lines())));

        assert_eq!(view.update(Msg::SelectLine(99)), None);
        assert_eq!(view.selected, None);
        assert_eq!(view.update(Msg::SelectLine(2)), None);
        assert_eq!(
            view.selected.as_deref(),
            Some("aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11")
        );
        // Without loaded lines a selection click is a no-op: the previous
        // selection is kept (no panic, no change).
        view.lines = None;
        assert_eq!(view.update(Msg::SelectLine(0)), None);
        assert_eq!(
            view.selected.as_deref(),
            Some("aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa11")
        );
    }

    #[test]
    fn close_and_error_paths() {
        let mut view = BlameView {
            open: true,
            ..BlameView::default()
        };
        assert_eq!(view.update(Msg::Close), None);
        assert!(!view.open);

        let mut view = BlameView {
            open: true,
            loading: true,
            ..BlameView::default()
        };
        assert_eq!(
            view.update(Msg::LinesLoaded(Err("blame failed".to_string()))),
            None
        );
        assert!(!view.loading);
        assert_eq!(view.error.as_deref(), Some("blame failed"));
        assert!(view.lines.is_none());
    }

    #[test]
    fn passthrough_intents() {
        let mut view = BlameView::default();
        assert_eq!(
            view.update(Msg::CopySha("abc".to_string())),
            Some(Action::CopySha("abc".to_string()))
        );
        assert_eq!(
            view.update(Msg::SelectCommit("abc".to_string())),
            Some(Action::SelectCommit("abc".to_string()))
        );
    }

    #[test]
    fn clip_keeps_short_text_and_ellipsizes_long_text() {
        assert_eq!(clip("short", 10), "short");
        let clipped = clip(&"m".repeat(20), 5);
        assert_eq!(clipped.chars().count(), 5);
        assert!(clipped.ends_with('…'));
    }
}
