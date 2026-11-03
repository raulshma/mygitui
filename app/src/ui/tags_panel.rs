//! Tags panel: create/delete local tags and browse the tag list.
//!
//! Self-contained lane module: owns its state, its own [`Msg`] enum and
//! returns [`Action`]s from [`update`] for the shell to translate into
//! `crate::service::tags::{tag_list, tag_create, tag_delete}` calls. Pushing
//! tags is out of scope here (network lane). The create form follows the
//! engine's rules (see `service/tags.rs`): a non-empty message makes an
//! annotated tag, so the annotated toggle requires one; the target defaults
//! to HEAD when left empty. Create/delete confirm completion through
//! [`Msg::CreateDone`] / [`Msg::DeleteDone`] which clear [`TagsPanel::busy`].

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::TagInfo;

use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// State and form buffers for the tags panel.
#[derive(Debug, Default)]
pub struct TagsPanel {
    pub open: bool,
    pub loading: bool,
    pub list: Option<Vec<TagInfo>>,
    pub error: Option<String>,
    pub create_name: String,
    pub create_target: String,
    pub create_message: String,
    pub create_annotated: bool,
    /// The tag name awaiting delete confirmation (inline, per-row).
    pub pending_delete: Option<String>,
    pub busy: bool,
}

/// Panel-internal messages; the shell forwards each into [`update`].
#[derive(Debug, Clone)]
pub enum Msg {
    ToggleOpen,
    RefreshPressed,
    ListLoaded(Result<Vec<TagInfo>, String>),
    CreateNameChanged(String),
    CreateTargetChanged(String),
    CreateMessageChanged(String),
    CreateAnnotatedToggled(bool),
    CreatePressed,
    /// Shell reports the finished create (form cleared on success).
    CreateDone(Result<(), String>),
    /// Start the inline delete confirm for this tag name.
    DeleteRequested(String),
    DeleteConfirmed,
    DeleteCanceled,
    /// Shell reports the finished delete (busy cleared, error surfaced).
    DeleteDone(Result<(), String>),
}

/// What the shell should execute for the repo the panel is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    /// Create a tag; `target`/`message` are already normalized (trimmed,
    /// `None` = HEAD / lightweight).
    Create {
        name: String,
        target: Option<String>,
        message: Option<String>,
        annotated: bool,
    },
    Delete {
        name: String,
    },
}

impl TagsPanel {
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ToggleOpen => {
                self.open = !self.open;
                // First open has nothing to show yet.
                if self.open && self.list.is_none() {
                    self.loading = true;
                    return Some(Action::Refresh);
                }
                None
            }
            Msg::RefreshPressed => {
                self.loading = true;
                Some(Action::Refresh)
            }
            Msg::ListLoaded(result) => {
                self.loading = false;
                match result {
                    Ok(list) => {
                        self.error = None;
                        self.list = Some(list);
                    }
                    Err(error) => self.error = Some(error),
                }
                None
            }
            Msg::CreateNameChanged(name) => {
                self.create_name = name;
                None
            }
            Msg::CreateTargetChanged(target) => {
                self.create_target = target;
                None
            }
            Msg::CreateMessageChanged(message) => {
                self.create_message = message;
                None
            }
            Msg::CreateAnnotatedToggled(annotated) => {
                self.create_annotated = annotated;
                None
            }
            Msg::CreatePressed => {
                if self.busy || create_error(self).is_some() {
                    return None; // busy, or the form violates the engine rules
                }
                let annotated = self.create_annotated;
                let name = self.create_name.trim().to_owned();
                let target = non_empty(self.create_target.trim());
                let message = if annotated {
                    Some(self.create_message.trim().to_owned())
                } else {
                    None
                };
                self.busy = true;
                Some(Action::Create {
                    name,
                    target,
                    message,
                    annotated,
                })
            }
            Msg::CreateDone(result) => {
                self.busy = false;
                match result {
                    Ok(()) => {
                        self.create_name.clear();
                        self.create_target.clear();
                        self.create_message.clear();
                        self.create_annotated = false;
                    }
                    Err(error) => self.error = Some(error),
                }
                None
            }
            Msg::DeleteRequested(name) => {
                self.pending_delete = Some(name);
                None
            }
            Msg::DeleteConfirmed => {
                if self.busy {
                    return None;
                }
                let name = self.pending_delete.take()?;
                self.busy = true;
                Some(Action::Delete { name })
            }
            Msg::DeleteCanceled => {
                self.pending_delete = None;
                None
            }
            Msg::DeleteDone(result) => {
                self.busy = false;
                if let Err(error) = result {
                    self.error = Some(error);
                }
                None
            }
        }
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        let mut header = row![text("Tags").size(12).font(theme::semibold())]
            .spacing(6)
            .align_y(Alignment::Center);
        if let Some(list) = &self.list {
            header = header.push(
                text(format!("{}", list.len()))
                    .size(10)
                    .color(tokens.muted_foreground),
            );
        }
        if self.loading {
            header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
        }
        if self.busy {
            header = header.push(text("· working…").size(10).color(tokens.muted_foreground));
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

        let create_enabled = create_error(self).is_none() && !self.busy;
        let mut create_form = row![
            text_input("Tag name", self.create_name.as_str())
                .on_input(Msg::CreateNameChanged)
                .on_submit(Msg::CreatePressed)
                .size(11)
                .padding([4, 6])
                .width(Length::Fixed(120.0)),
            text_input("HEAD (default)", self.create_target.as_str())
                .on_input(Msg::CreateTargetChanged)
                .on_submit(Msg::CreatePressed)
                .size(11)
                .padding([4, 6])
                .width(Length::Fixed(140.0)),
            checkbox(self.create_annotated)
                .label("annotated")
                .size(12)
                .text_size(10)
                .spacing(4)
                .on_toggle(Msg::CreateAnnotatedToggled),
            button(text("Create").size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                .padding([4, 10])
                .on_press_maybe(create_enabled.then_some(Msg::CreatePressed)),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        if self.create_annotated {
            create_form = create_form.push(
                text_input("Tag message", self.create_message.as_str())
                    .on_input(Msg::CreateMessageChanged)
                    .on_submit(Msg::CreatePressed)
                    .size(11)
                    .padding([4, 6])
                    .width(Length::Fill),
            );
        } else {
            create_form = create_form.push(Space::new().width(Length::Fill));
        }

        let body: Element<'_, Msg> = match &self.list {
            Some(list) if !list.is_empty() => {
                let mut rows = column![].spacing(3).width(Length::Fill);
                for tag in list {
                    rows = rows.push(tag_row(tag, self, tokens));
                }
                let height = (list.len() as f32 * 30.0).clamp(30.0, 220.0);
                scrollable(rows)
                    .height(Length::Fixed(height))
                    .width(Length::Fill)
                    .into()
            }
            Some(_) => note(tokens, "No tags.", false),
            None => note(tokens, "Loading tags…", false),
        };

        let error_note: Element<'_, Msg> = match &self.error {
            Some(error) => note(tokens, error.clone(), true),
            None => column![].into(),
        };

        container(
            column![header, create_form, error_note, body]
                .spacing(6)
                .padding([8, 10]),
        )
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
        .into()
    }
}

/// Form validation against the engine's rules (unit-tested): a name is
/// required, and `annotated` requires a message (the engine maps
/// message → annotated tag; a blank message would silently stay lightweight).
pub(crate) fn create_error(panel: &TagsPanel) -> Option<&'static str> {
    if panel.create_name.trim().is_empty() {
        return Some("a tag name is required");
    }
    if panel.create_annotated && panel.create_message.trim().is_empty() {
        return Some("an annotated tag needs a message");
    }
    None
}

fn tag_row<'a>(tag: &'a TagInfo, panel: &TagsPanel, tokens: Tokens) -> Element<'a, Msg> {
    let line = row![
        text(tag.name.as_str())
            .size(11)
            .font(theme::mono())
            .color(tokens.foreground),
        if tag.annotated {
            chip_view(tokens)
        } else {
            Space::new().width(0.0).into()
        },
        Space::new().width(Length::Fill),
        text(short_sha(&tag.target))
            .size(9)
            .font(theme::mono())
            .color(tokens.muted_foreground),
        button(text("✕").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(24.0))
            .on_press_maybe((!panel.busy).then_some(Msg::DeleteRequested(tag.name.clone()))),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let mut card = column![line].spacing(2).width(Length::Fill);
    if let Some(subject) = subject(tag) {
        card = card.push(
            text(clip(subject, 56))
                .size(9)
                .color(tokens.muted_foreground),
        );
    }

    if panel.pending_delete.as_deref() == Some(tag.name.as_str()) {
        card = card.push(
            row![
                text(format!("Delete tag `{}`?", tag.name))
                    .size(9)
                    .color(tokens.muted_foreground),
                Space::new().width(Length::Fill),
                button(text("Cancel").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 6])
                    .on_press(Msg::DeleteCanceled),
                button(text("Confirm delete").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                    .padding([2, 6])
                    .on_press_maybe((!panel.busy).then_some(Msg::DeleteConfirmed)),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        );
    }

    container(card)
        .padding([4, 6])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.06))),
            border: Border {
                color: theme::with_alpha(tokens.border, 0.6),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The "annotated" chip shown on annotated rows.
fn chip_view<'a>(tokens: Tokens) -> Element<'a, Msg> {
    let color = tokens.muted_foreground;
    container(text("annotated").size(9).color(color))
        .padding([1, 6])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(color, 0.12))),
            border: Border {
                color: theme::with_alpha(color, 0.4),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// First line of an annotated tag's message (None for lightweight tags).
pub(crate) fn subject(tag: &TagInfo) -> Option<&str> {
    tag.message
        .as_deref()?
        .lines()
        .find(|line| !line.trim().is_empty())
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn note<'a>(tokens: Tokens, label: impl Into<String>, error: bool) -> Element<'a, Msg> {
    let color = if error {
        tokens.deleted
    } else {
        tokens.muted_foreground
    };
    container(text(label.into()).size(10).color(color))
        .padding([2, 4])
        .into()
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::{clip, create_error, subject, Action, Msg, TagsPanel};
    use crate::engine::types::TagInfo;

    fn fixture() -> TagInfo {
        TagInfo {
            name: "v1.0".into(),
            sha: "aaaa111111111111111111111111111111111111".into(),
            target: "0123456789abcdef0123456789abcdef01234567".into(),
            annotated: true,
            tagger: None,
            message: Some("First release\n\nnotes".into()),
        }
    }

    fn panel_with(list: Vec<TagInfo>) -> TagsPanel {
        let mut panel = TagsPanel::default();
        let _ = panel.update(Msg::ListLoaded(Ok(list)));
        panel
    }

    fn filled() -> TagsPanel {
        let mut panel = TagsPanel::default();
        panel.update(Msg::CreateNameChanged("v2.0".into()));
        panel.update(Msg::CreateAnnotatedToggled(true));
        panel.update(Msg::CreateMessageChanged("second release".into()));
        panel
    }

    #[test]
    fn create_requires_a_name() {
        let mut panel = TagsPanel::default();
        assert_eq!(panel.update(Msg::CreatePressed), None);
        panel.update(Msg::CreateNameChanged("   ".into()));
        assert_eq!(panel.update(Msg::CreatePressed), None);
        panel.update(Msg::CreateNameChanged("v1.0".into()));
        assert_eq!(
            panel.update(Msg::CreatePressed),
            Some(Action::Create {
                name: "v1.0".into(),
                target: None,
                message: None,
                annotated: false
            })
        );
        assert!(panel.busy);
    }

    #[test]
    fn annotated_requires_a_message() {
        let mut panel = TagsPanel::default();
        panel.update(Msg::CreateNameChanged("v1.0".into()));
        panel.update(Msg::CreateAnnotatedToggled(true));
        // Blank message would silently stay lightweight in the engine, so
        // the panel refuses to submit instead.
        assert_eq!(panel.update(Msg::CreatePressed), None);
        assert_eq!(
            create_error(&panel),
            Some("an annotated tag needs a message")
        );
        panel.update(Msg::CreateMessageChanged("  release  ".into()));
        assert_eq!(create_error(&panel), None);
        assert_eq!(
            panel.update(Msg::CreatePressed),
            Some(Action::Create {
                name: "v1.0".into(),
                target: None,
                message: Some("release".into()),
                annotated: true
            })
        );
    }

    #[test]
    fn create_form_normalizes_target_and_clears_on_success() {
        let mut panel = filled();
        panel.update(Msg::CreateTargetChanged("  HEAD~2  ".into()));
        assert_eq!(
            panel.update(Msg::CreatePressed),
            Some(Action::Create {
                name: "v2.0".into(),
                target: Some("HEAD~2".into()),
                message: Some("second release".into()),
                annotated: true
            })
        );
        // Unannotated tags never carry a message (the engine would turn any
        // message into an annotated tag).
        panel.update(Msg::CreateDone(Ok(())));
        assert!(!panel.busy);
        assert!(panel.create_name.is_empty() && panel.create_message.is_empty());
        panel.update(Msg::CreateNameChanged("lw".into()));
        panel.update(Msg::CreateAnnotatedToggled(false));
        panel.update(Msg::CreateMessageChanged("ignored".into()));
        assert_eq!(
            panel.update(Msg::CreatePressed),
            Some(Action::Create {
                name: "lw".into(),
                target: None,
                message: None,
                annotated: false
            })
        );
    }

    #[test]
    fn busy_blocks_create_and_delete_presses() {
        let mut panel = panel_with(vec![fixture()]);
        panel.update(Msg::CreateNameChanged("v9".into()));
        panel.busy = true;
        assert_eq!(panel.update(Msg::CreatePressed), None);
        // The inline delete confirm still opens, but confirming is blocked.
        panel.update(Msg::DeleteRequested("v1.0".into()));
        assert_eq!(panel.pending_delete.as_deref(), Some("v1.0"));
        assert_eq!(panel.update(Msg::DeleteConfirmed), None);
        panel.busy = false;
        assert_eq!(
            panel.update(Msg::DeleteConfirmed),
            Some(Action::Delete {
                name: "v1.0".into()
            })
        );
        assert!(panel.busy && panel.pending_delete.is_none());
    }

    #[test]
    fn delete_confirm_flow_cancels_and_completes() {
        let mut panel = panel_with(vec![fixture()]);
        assert_eq!(panel.update(Msg::DeleteConfirmed), None);
        panel.update(Msg::DeleteRequested("v1.0".into()));
        panel.update(Msg::DeleteCanceled);
        assert!(panel.pending_delete.is_none());
        assert_eq!(panel.update(Msg::DeleteConfirmed), None);
        panel.update(Msg::DeleteRequested("v1.0".into()));
        assert_eq!(
            panel.update(Msg::DeleteConfirmed),
            Some(Action::Delete {
                name: "v1.0".into()
            })
        );
        panel.update(Msg::DeleteDone(Err("protected ref".into())));
        assert!(!panel.busy);
        assert_eq!(panel.error.as_deref(), Some("protected ref"));
    }

    #[test]
    fn list_mapping_subject_and_lightweight_rows() {
        let mut annotated = fixture();
        assert_eq!(subject(&annotated), Some("First release"));
        assert!(annotated.annotated);

        annotated.message = Some("\n\nonly blank lines\n".into());
        assert_eq!(subject(&annotated), Some("only blank lines"));

        let lightweight = TagInfo {
            name: "lw".into(),
            sha: "bbbb222222222222222222222222222222222222".into(),
            target: "9999999999999999999999999999999999999999".into(),
            annotated: false,
            tagger: None,
            message: None,
        };
        assert_eq!(subject(&lightweight), None);

        let panel = panel_with(vec![annotated, lightweight]);
        let names: Vec<&str> = panel
            .list
            .as_ref()
            .unwrap()
            .iter()
            .map(|tag| tag.name.as_str())
            .collect();
        assert_eq!(names, ["v1.0", "lw"]);
        assert_eq!(short_target(&panel.list.as_ref().unwrap()[0]), "0123456");
    }

    fn short_target(tag: &TagInfo) -> &str {
        tag.target.get(..7).unwrap_or(&tag.target)
    }

    #[test]
    fn toggle_and_loading_guards() {
        let mut panel = TagsPanel::default();
        assert_eq!(panel.update(Msg::ToggleOpen), Some(Action::Refresh));
        assert!(panel.open && panel.loading);
        assert_eq!(panel.update(Msg::ListLoaded(Err("boom".into()))), None);
        assert_eq!(panel.error.as_deref(), Some("boom"));
        assert!(!panel.loading);
        assert_eq!(panel.update(Msg::CreateAnnotatedToggled(true)), None);
        assert!(panel.create_annotated);
    }

    #[test]
    fn clip_truncates_long_subjects() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("0123456789", 5), "0123…");
    }
}
