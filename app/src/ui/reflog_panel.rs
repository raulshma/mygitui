//! Reflog panel: read-only reflog browser with per-row intents.
//!
//! Self-contained surface: owns its messages ([`Msg`]) and intents
//! ([`Action`]). The shell routes `Msg` into [`ReflogPanel::update`] (pure —
//! no service calls, no mutations) and maps the returned [`Action`] onto
//! the service layer: `Refresh` → `service::repo_tools::reflog` (feedback
//! via `Msg::EntriesLoaded`), `CopySha` → clipboard, `ResetTo` → the
//! shell's reset-confirm flow, `BranchFrom` → its branch-create dialog.
//! The panel itself never mutates repo state.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::ReflogEntry;

use super::history_model::relative_age;
use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// Panel state: load state, entry list and selection.
#[derive(Debug, Default)]
pub struct ReflogPanel {
    pub open: bool,
    pub loading: bool,
    pub entries: Option<Vec<ReflogEntry>>,
    pub error: Option<String>,
    /// Index into `entries` of the highlighted row.
    pub selected: Option<usize>,
    /// Ref filter (blank = HEAD).
    pub ref_name: String,
}

/// Panel messages (shell → [`ReflogPanel::update`]).
#[derive(Debug, Clone)]
pub enum Msg {
    ToggleOpen,
    RefreshPressed,
    RefChanged(String),
    /// Result of `service::repo_tools::reflog`.
    EntriesLoaded(Result<Vec<ReflogEntry>, String>),
    SelectEntry(usize),
    CopySha(String),
    ResetTo(String),
    BranchFrom(String),
}

/// Intents returned by [`ReflogPanel::update`] for the shell to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Fetch the reflog for `ref_name` (`None` = HEAD); feed the result
    /// back as `Msg::EntriesLoaded`.
    Refresh { ref_name: Option<String> },
    /// "Reset branch here": the shell routes this into its existing
    /// reset-confirm flow (`Message::ResetRequested`-style).
    ResetTo { sha: String },
    /// "Create branch here": the shell opens its branch-create dialog with
    /// this start point and decides.
    BranchFrom { sha: String },
    /// Copy the full sha to the clipboard.
    CopySha(String),
}

/// Display model for one reflog row (pure mapping, unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogRow {
    /// Position in the list; git's `ref@{n}` counts from the most recent.
    pub index: usize,
    /// `HEAD@{0}` / `main@{3}` selector label.
    pub selector: String,
    /// Short new sha of the entry.
    pub sha: String,
    pub message: String,
    pub author: String,
    /// Relative age, or empty when the entry has no usable timestamp.
    pub age: String,
}

impl ReflogPanel {
    /// The ref the shell should fetch: the trimmed filter, or `None` for
    /// HEAD when blank.
    pub fn requested_ref(&self) -> Option<String> {
        let name = self.ref_name.trim();
        (!name.is_empty()).then(|| name.to_string())
    }

    /// Pure state transition; returns the intent for the shell to run.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ToggleOpen => {
                self.open = !self.open;
                if !self.open || self.loading {
                    return None;
                }
                if self.entries.is_none() {
                    self.loading = true;
                    return Some(Action::Refresh {
                        ref_name: self.requested_ref(),
                    });
                }
                None
            }
            Msg::RefreshPressed => {
                if self.loading {
                    return None;
                }
                self.loading = true;
                self.error = None;
                Some(Action::Refresh {
                    ref_name: self.requested_ref(),
                })
            }
            Msg::RefChanged(name) => {
                self.ref_name = name;
                None
            }
            Msg::EntriesLoaded(Ok(entries)) => {
                self.loading = false;
                self.error = None;
                // Drop a selection that no longer exists after a reload.
                self.selected = self.selected.filter(|&i| i < entries.len());
                self.entries = Some(entries);
                None
            }
            Msg::EntriesLoaded(Err(err)) => {
                self.loading = false;
                self.error = Some(err);
                None
            }
            Msg::SelectEntry(index) => {
                // Out-of-bounds clicks are ignored; selection is pure state.
                if self.entries.as_ref().is_some_and(|e| index < e.len()) {
                    self.selected = Some(index);
                }
                None
            }
            Msg::CopySha(sha) => Some(Action::CopySha(sha)),
            Msg::ResetTo(sha) => Some(Action::ResetTo { sha }),
            Msg::BranchFrom(sha) => Some(Action::BranchFrom { sha }),
        }
    }

    /// Overlay card, or `None` while closed (the shell decides placement).
    pub fn view(&self, tokens: Tokens) -> Option<Element<'_, Msg>> {
        if !self.open {
            return None;
        }
        let ref_label = self.requested_ref().unwrap_or_else(|| "HEAD".to_string());

        let mut header = row![text("Reflog").size(12).font(theme::semibold())]
            .spacing(6)
            .align_y(Alignment::Center);
        if let Some(list) = &self.entries {
            header = header.push(
                text(format!("{ref_label} · {} entries", list.len()))
                    .size(10)
                    .color(tokens.muted_foreground),
            );
        }
        if self.loading {
            header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
        }
        header = header
            .push(Space::new().width(Length::Fill))
            .push(
                button(text("⟳").size(11))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Msg::RefreshPressed),
            )
            .push(
                button(text("×").size(12))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Msg::ToggleOpen),
            );

        let filter = row![
            text("Ref").size(9).color(tokens.muted_foreground),
            text_input("ref (blank = HEAD)", self.ref_name.as_str())
                .on_input(Msg::RefChanged)
                .on_submit(Msg::RefreshPressed)
                .size(11)
                .padding([4, 6])
                .width(Length::Fixed(160.0)),
            text("Blank lists HEAD's reflog.")
                .size(9)
                .color(tokens.muted_foreground),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let body: Element<'_, Msg> = match &self.entries {
            Some(list) if !list.is_empty() => {
                let now = now_seconds();
                let mut rows = column![].spacing(1).width(Length::Fill);
                for (index, entry) in list.iter().enumerate() {
                    let selected = self.selected == Some(index);
                    rows = rows.push(reflog_row_view(
                        reflog_row_model(index, entry, &ref_label, now),
                        selected,
                        entry,
                        tokens,
                    ));
                }
                let height = (list.len() as f32 * 24.0).clamp(24.0, 260.0);
                scrollable(rows)
                    .height(Length::Fixed(height))
                    .width(Length::Fill)
                    .into()
            }
            Some(_) => note(tokens, "No reflog entries.", false),
            None if self.error.is_some() => {
                note(tokens, self.error.as_deref().unwrap_or_default(), true)
            }
            None => note(tokens, "Loading reflog…", false),
        };

        let mut card = column![header, filter].spacing(6);
        if let Some(err) = &self.error {
            if self.entries.is_some() {
                card = card.push(
                    text(err.as_str())
                        .size(10)
                        .color(tokens.deleted)
                        .width(Length::Fill),
                );
            }
        }
        card = card.push(body);

        Some(
            container(card.padding([8, 10]))
                .width(Length::Fill)
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

/// Map a [`ReflogEntry`] to its display row (pure; see [`ReflogRow`]).
pub fn reflog_row_model(index: usize, entry: &ReflogEntry, ref_label: &str, now: i64) -> ReflogRow {
    ReflogRow {
        index,
        selector: format!("{ref_label}@{{{index}}}"),
        sha: short_sha(&entry.new_sha).to_string(),
        message: clip(entry.message.trim(), 56),
        author: entry.signature.name.clone(),
        age: if entry.signature.time > 0 {
            relative_age(entry.signature.time, now)
        } else {
            String::new()
        },
    }
}

fn reflog_row_view<'a>(
    model: ReflogRow,
    selected: bool,
    entry: &'a ReflogEntry,
    tokens: Tokens,
) -> Element<'a, Msg> {
    // The whole info area selects the row; actions appear on the selection.
    // The row model is consumed (owned strings) so the element borrows
    // nothing from per-iteration locals.
    let info = button(
        row![
            text(model.selector)
                .size(10)
                .font(theme::mono())
                .color(tokens.muted_foreground)
                .width(Length::Fixed(86.0)),
            text(model.sha)
                .size(10)
                .font(theme::mono())
                .color(tokens.foreground)
                .width(Length::Fixed(58.0)),
            text(model.message)
                .size(11)
                .color(tokens.foreground)
                .width(Length::Fill),
            text(model.author)
                .size(9)
                .color(tokens.muted_foreground)
                .width(Length::Fixed(72.0)),
            text(model.age)
                .size(9)
                .color(tokens.muted_foreground)
                .width(Length::Fixed(54.0)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
    .padding([2, 4])
    .width(Length::Fill)
    .on_press(Msg::SelectEntry(model.index));

    let sha = entry.new_sha.clone();
    let mut line = row![info]
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if selected {
        line = line
            .push(
                button(text("Copy SHA").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 6])
                    .on_press(Msg::CopySha(sha.clone())),
            )
            .push(
                button(text("Reset here").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 6])
                    .on_press(Msg::ResetTo(sha.clone())),
            )
            .push(
                button(text("Branch here").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 6])
                    .on_press(Msg::BranchFrom(sha)),
            );
    }

    container(line)
        .padding([2, 4])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: selected.then(|| Background::Color(theme::with_alpha(tokens.accent, 0.15))),
            border: Border {
                color: if selected {
                    theme::with_alpha(tokens.accent, 0.5)
                } else {
                    theme::with_alpha(tokens.border, 0.4)
                },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn note<'a>(tokens: Tokens, label: impl Into<String>, is_error: bool) -> Element<'a, Msg> {
    let color = if is_error {
        tokens.deleted
    } else {
        tokens.muted_foreground
    };
    container(text(label.into()).size(10).color(color))
        .padding([4, 4])
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

    fn entry(new_sha: &str, message: &str, time: i64) -> ReflogEntry {
        ReflogEntry {
            old_sha: "0".repeat(40),
            new_sha: new_sha.to_string(),
            signature: GitSignature {
                name: "Raul".to_string(),
                email: "raul@example.com".to_string(),
                time,
                offset_minutes: 0,
            },
            message: message.to_string(),
        }
    }

    fn fixture_entries() -> Vec<ReflogEntry> {
        vec![
            entry("aaaaaaaa11111111", "commit: add panel", 1_700_000_100),
            entry(
                "bbbbbbbb22222222",
                "checkout: moving to main",
                1_700_000_000,
            ),
            entry("cccccccc33333333", "commit (initial): init", 1_699_999_000),
        ]
    }

    fn loaded_panel() -> ReflogPanel {
        let mut panel = ReflogPanel {
            open: true,
            ..ReflogPanel::default()
        };
        panel.update(Msg::EntriesLoaded(Ok(fixture_entries())));
        panel
    }

    #[test]
    fn toggle_open_first_time_emits_refresh_for_head() {
        let mut panel = ReflogPanel::default();
        let action = panel.update(Msg::ToggleOpen);
        assert!(panel.open);
        assert!(panel.loading);
        assert_eq!(action, Some(Action::Refresh { ref_name: None }));
    }

    #[test]
    fn toggle_open_with_cached_entries_needs_nothing() {
        let mut panel = loaded_panel();
        assert_eq!(panel.update(Msg::ToggleOpen), None); // close
        assert_eq!(panel.update(Msg::ToggleOpen), None); // reopen, cached
        assert!(panel.open);
    }

    #[test]
    fn refresh_uses_trimmed_ref_filter() {
        let mut panel = loaded_panel();
        panel.ref_name = " feature ".to_string();
        panel.loading = false;
        assert_eq!(
            panel.update(Msg::RefreshPressed),
            Some(Action::Refresh {
                ref_name: Some("feature".to_string()),
            })
        );
        panel.ref_name = "   ".to_string();
        panel.loading = false;
        assert_eq!(
            panel.update(Msg::RefreshPressed),
            Some(Action::Refresh { ref_name: None })
        );
        assert_eq!(panel.requested_ref(), None);
    }

    #[test]
    fn refresh_suppressed_while_loading() {
        let mut panel = loaded_panel();
        panel.loading = true;
        assert_eq!(panel.update(Msg::RefreshPressed), None);
    }

    #[test]
    fn entries_loaded_stores_and_clears_state() {
        let mut panel = ReflogPanel {
            loading: true,
            error: Some("old".to_string()),
            ..ReflogPanel::default()
        };
        panel.update(Msg::EntriesLoaded(Ok(fixture_entries())));
        assert!(!panel.loading);
        assert!(panel.error.is_none());
        let entries = panel.entries.expect("entries stored");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].new_sha, "aaaaaaaa11111111");
    }

    #[test]
    fn entries_loaded_error_keeps_previous_entries() {
        let mut panel = loaded_panel();
        panel.update(Msg::EntriesLoaded(Err("reflog read failed".to_string())));
        assert!(!panel.loading);
        assert_eq!(panel.error.as_deref(), Some("reflog read failed"));
        assert_eq!(panel.entries.expect("kept").len(), 3);
    }

    #[test]
    fn entries_loaded_clamps_stale_selection() {
        let mut panel = loaded_panel();
        panel.selected = Some(2);
        panel.update(Msg::EntriesLoaded(Ok(vec![entry(
            "dddddddd44444444",
            "x",
            1,
        )])));
        assert_eq!(panel.selected, None);
        panel.selected = Some(0);
        panel.update(Msg::EntriesLoaded(Ok(fixture_entries())));
        assert_eq!(panel.selected, Some(0));
    }

    #[test]
    fn select_entry_respects_bounds_and_is_pure() {
        let mut panel = loaded_panel();
        assert_eq!(panel.update(Msg::SelectEntry(9)), None);
        assert_eq!(panel.selected, None);
        assert_eq!(panel.update(Msg::SelectEntry(1)), None);
        assert_eq!(panel.selected, Some(1));
    }

    #[test]
    fn row_actions_pass_sha_through() {
        assert_eq!(
            ReflogPanel::default().update(Msg::CopySha("abc".to_string())),
            Some(Action::CopySha("abc".to_string()))
        );
        assert_eq!(
            ReflogPanel::default().update(Msg::ResetTo("abc".to_string())),
            Some(Action::ResetTo {
                sha: "abc".to_string()
            })
        );
        assert_eq!(
            ReflogPanel::default().update(Msg::BranchFrom("abc".to_string())),
            Some(Action::BranchFrom {
                sha: "abc".to_string()
            })
        );
    }

    #[test]
    fn row_model_maps_fixture_entries() {
        let entries = fixture_entries();
        let now = 1_700_000_160;

        let first = reflog_row_model(0, &entries[0], "HEAD", now);
        assert_eq!(first.index, 0);
        assert_eq!(first.selector, "HEAD@{0}");
        assert_eq!(first.sha, "aaaaaaa"); // short_sha: 7 chars
        assert_eq!(first.message, "commit: add panel");
        assert_eq!(first.author, "Raul");
        assert_eq!(first.age, "1m ago");

        let second = reflog_row_model(1, &entries[1], "HEAD", now);
        assert_eq!(second.selector, "HEAD@{1}");
        assert_eq!(second.age, "2m ago");

        let named_ref = reflog_row_model(2, &entries[2], "main", now);
        assert_eq!(named_ref.selector, "main@{2}");
    }

    #[test]
    fn row_model_without_timestamp_has_empty_age() {
        let stale = entry("eeeeeeee55555555", "commit: old", 0);
        let model = reflog_row_model(0, &stale, "HEAD", 1_700_000_000);
        assert!(model.age.is_empty());
    }

    #[test]
    fn row_model_clips_long_messages() {
        let long = entry("ffffffff66666666", &"m".repeat(80), 1_700_000_000);
        let model = reflog_row_model(0, &long, "HEAD", 1_700_000_000);
        assert_eq!(model.message.chars().count(), 56);
        assert!(model.message.ends_with('…'));
    }

    #[test]
    fn clip_keeps_short_text() {
        assert_eq!(clip("short", 10), "short");
    }
}
