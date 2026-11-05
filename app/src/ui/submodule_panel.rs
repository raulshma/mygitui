//! Submodules panel: per-submodule state with update/sync controls.
//!
//! Self-contained lane module: owns its state, its own [`Msg`] enum and
//! returns [`Action`]s from [`update`] for the shell to translate into
//! `crate::service::files::{submodules, submodule_update, submodule_sync}`
//! calls. Busy bookkeeping is panel-internal: every accepted press sets
//! [`SubmodulePanel::busy`], presses while busy are ignored, and the shell
//! reports completion with [`Msg::OpDone`] (fields are `pub` for direct
//! resets if the shell prefers).

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::SubmoduleInfo;

use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// Load state for the submodules panel. `busy` carries a short label of the
/// running op (`Some` disables every Update/Sync button).
#[derive(Debug, Default)]
pub struct SubmodulePanel {
    pub open: bool,
    pub loading: bool,
    pub list: Option<Vec<SubmoduleInfo>>,
    pub error: Option<String>,
    pub busy: Option<String>,
}

/// Panel-internal messages; the shell forwards each into [`update`].
#[derive(Debug, Clone)]
pub enum Msg {
    ToggleOpen,
    RefreshPressed,
    ListLoaded(Result<Vec<SubmoduleInfo>, String>),
    /// `git submodule update` for one submodule path.
    UpdatePressed(String),
    /// `git submodule update` for every submodule.
    UpdateAllPressed,
    /// `git submodule sync` — `Some(path)` for one, `None` for all.
    SyncPressed(Option<String>),
    /// Shell reports the finished mutation (busy cleared, error surfaced).
    OpDone(Result<(), String>),
}

/// What the shell should execute for the repo the panel is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    /// `Update { path }` maps to `submodule_update`; initialize uninitialized
    /// submodules (`init: true`) at the shell's discretion.
    Update {
        path: String,
    },
    /// Update every submodule (`init`/`recursive` at the shell's discretion;
    /// the remote engine updates all regardless of path).
    UpdateAll,
    /// `None` syncs all submodules.
    Sync {
        path: Option<String>,
    },
}

impl SubmodulePanel {
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
            Msg::UpdatePressed(path) => {
                if self.busy.is_some() {
                    return None; // one mutation at a time
                }
                self.busy = Some(format!("update {path}"));
                Some(Action::Update { path })
            }
            Msg::UpdateAllPressed => {
                if self.busy.is_some() {
                    return None;
                }
                self.busy = Some("update all".into());
                Some(Action::UpdateAll)
            }
            Msg::SyncPressed(path) => {
                if self.busy.is_some() {
                    return None;
                }
                self.busy = Some(match &path {
                    Some(path) => format!("sync {path}"),
                    None => "sync all".into(),
                });
                Some(Action::Sync { path })
            }
            Msg::OpDone(result) => {
                self.busy = None;
                if let Err(error) = result {
                    self.error = Some(error);
                }
                None
            }
        }
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        let mut header = row![text("Submodules")
            .size(theme::TEXT_BODY)
            .font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
        if let Some(list) = &self.list {
            header = header.push(
                text(format!("{}", list.len()))
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            );
        }
        if self.loading {
            header = header.push(
                text("· loading…")
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            );
        }
        if let Some(busy) = &self.busy {
            header = header.push(
                text(format!("· {busy}…"))
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            );
        }
        header = header
            .push(Space::new().width(Length::Fill))
            .push(
                button(text("Update all").size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 8])
                    .on_press_maybe(self.actionable().then_some(Msg::UpdateAllPressed)),
            )
            .push(
                button(text("Sync all").size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 8])
                    .on_press_maybe(self.actionable().then_some(Msg::SyncPressed(None))),
            )
            .push(theme::tip(
                tokens,
                button(text(theme::icon::REFRESH).size(theme::TEXT_SMALL))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::RefreshPressed),
                "Reload submodules",
            ))
            .push(theme::tip(
                tokens,
                button(text(theme::icon::CLOSE).size(theme::TEXT_BODY))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::ToggleOpen),
                "Close submodules panel",
            ));

        let body: Element<'_, Msg> = match &self.list {
            Some(list) if !list.is_empty() => {
                let mut rows = column![].spacing(3).width(Length::Fill);
                for info in list {
                    rows = rows.push(submodule_row(info, self, tokens));
                }
                let height = (list.len() as f32 * 44.0).clamp(44.0, 220.0);
                scrollable(rows)
                    .height(Length::Fixed(height))
                    .width(Length::Fill)
                    .into()
            }
            Some(_) => theme::note(tokens, "No submodules.", false, theme::NoteKind::Empty),
            None => theme::note(
                tokens,
                "Loading submodules…",
                false,
                theme::NoteKind::Inline,
            ),
        };

        let error_note: Element<'_, Msg> = match &self.error {
            Some(error) => {
                // Refresh re-lists submodules, so it doubles as the retry.
                row![
                    theme::note(tokens, error.clone(), true, theme::NoteKind::Inline),
                    theme::mini_button(
                        tokens,
                        "Retry",
                        "Reload submodules",
                        Some(Msg::RefreshPressed),
                    ),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .into()
            }
            None => column![].into(),
        };

        container(
            column![header, error_note, body]
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

    /// Mutations are only offered while nothing runs.
    fn actionable(&self) -> bool {
        self.busy.is_none()
    }
}

fn submodule_row<'a>(
    info: &'a SubmoduleInfo,
    panel: &SubmodulePanel,
    tokens: Tokens,
) -> Element<'a, Msg> {
    let mut chips = row![].spacing(4).align_y(Alignment::Center);
    let chip = status_chip(info);
    chips = chips.push(chip_view(chip, tokens));

    let line = row![
        text(info.path.as_str())
            .size(theme::TEXT_SMALL)
            .font(theme::mono())
            .color(tokens.foreground),
        chips,
        Space::new().width(Length::Fill),
        text(commit_label(info))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let buttons = row![
        button(text("Update").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(48.0))
            .on_press_maybe(
                panel
                    .actionable()
                    .then_some(Msg::UpdatePressed(info.path.clone()))
            ),
        button(text("Sync").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(42.0))
            .on_press_maybe(
                panel
                    .actionable()
                    .then_some(Msg::SyncPressed(Some(info.path.clone())))
            ),
    ]
    .spacing(2)
    .align_y(Alignment::Center);

    let card = column![
        row![line, buttons].spacing(4).align_y(Alignment::Center),
        text(clip(&info.url, 64))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground)
    ]
    .spacing(2)
    .width(Length::Fill);

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

/// Row status chip (unit-tested): uninitialized rows, then the engine's
/// `git status`-style status string, then clean.
pub(crate) fn status_chip(info: &SubmoduleInfo) -> Chip {
    if !info.initialized {
        return Chip {
            label: "not initialized".into(),
            danger: true,
        };
    }
    if info.status.is_empty() {
        Chip {
            label: "clean".into(),
            danger: false,
        }
    } else {
        Chip {
            label: info.status.clone(),
            danger: true,
        }
    }
}

/// "at <head>" when the checkout matches the recorded commit, otherwise
/// "at <head> · recorded <recorded>"; uninitialized rows show the recorded
/// commit only (`SubmoduleInfo` carries no branch).
pub(crate) fn commit_label(info: &SubmoduleInfo) -> String {
    match &info.head_sha {
        Some(head) if head == &info.recorded_sha => format!("at {}", short_sha(head)),
        Some(head) => format!(
            "at {} · recorded {}",
            short_sha(head),
            short_sha(&info.recorded_sha)
        ),
        None => format!("recorded {}", short_sha(&info.recorded_sha)),
    }
}

/// A small status chip: neutral (`clean`) or danger (everything else).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chip {
    pub label: String,
    pub danger: bool,
}

fn chip_view<'a>(chip: Chip, tokens: Tokens) -> Element<'a, Msg> {
    let color = if chip.danger {
        tokens.destructive
    } else {
        tokens.muted_foreground
    };
    theme::chip(chip.label, color, theme::ChipShape::Pill)
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
    use super::{clip, commit_label, status_chip, Action, Chip, Msg, SubmodulePanel};
    use crate::engine::types::SubmoduleInfo;

    fn fixture() -> SubmoduleInfo {
        SubmoduleInfo {
            path: "libs/vendored".into(),
            name: "vendored".into(),
            url: "https://example.com/vendored.git".into(),
            head_sha: Some("0123456789abcdef0123456789abcdef01234567".into()),
            recorded_sha: "0123456789abcdef0123456789abcdef01234567".into(),
            initialized: true,
            status: String::new(),
        }
    }

    fn panel_with(list: Vec<SubmoduleInfo>) -> SubmodulePanel {
        let mut panel = SubmodulePanel::default();
        let _ = panel.update(Msg::ListLoaded(Ok(list)));
        panel
    }

    #[test]
    fn row_mapping_clean_modified_and_uninitialized() {
        let clean = fixture();
        assert_eq!(
            status_chip(&clean),
            Chip {
                label: "clean".into(),
                danger: false
            }
        );
        assert_eq!(commit_label(&clean), "at 0123456");

        let mut modified = fixture();
        modified.status = "new commits".into();
        assert_eq!(
            status_chip(&modified),
            Chip {
                label: "new commits".into(),
                danger: true
            }
        );
        modified.head_sha = Some("9999999999999999999999999999999999999999".into());
        assert_eq!(commit_label(&modified), "at 9999999 · recorded 0123456");

        let mut uninitialized = fixture();
        uninitialized.initialized = false;
        uninitialized.head_sha = None;
        assert_eq!(
            status_chip(&uninitialized),
            Chip {
                label: "not initialized".into(),
                danger: true
            }
        );
        assert_eq!(commit_label(&uninitialized), "recorded 0123456");
    }

    #[test]
    fn busy_guards_block_concurrent_mutations() {
        let mut panel = panel_with(vec![fixture()]);
        assert_eq!(
            panel.update(Msg::UpdatePressed("libs/vendored".into())),
            Some(Action::Update {
                path: "libs/vendored".into()
            })
        );
        assert_eq!(panel.busy.as_deref(), Some("update libs/vendored"));

        // Every mutation press while busy is ignored (no queued actions).
        assert_eq!(
            panel.update(Msg::UpdatePressed("libs/vendored".into())),
            None
        );
        assert_eq!(panel.update(Msg::UpdateAllPressed), None);
        assert_eq!(panel.update(Msg::SyncPressed(None)), None);
        assert_eq!(
            panel.update(Msg::SyncPressed(Some("libs/vendored".into()))),
            None
        );

        // Completion clears busy; a fresh press is accepted again.
        panel.update(Msg::OpDone(Ok(())));
        assert!(panel.busy.is_none());
        assert_eq!(panel.update(Msg::UpdateAllPressed), Some(Action::UpdateAll));
        panel.update(Msg::OpDone(Ok(())));
        assert_eq!(
            panel.update(Msg::SyncPressed(None)),
            Some(Action::Sync { path: None })
        );
    }

    #[test]
    fn op_done_surfaces_errors_without_sticking_busy() {
        let mut panel = panel_with(vec![fixture()]);
        panel.update(Msg::UpdateAllPressed);
        panel.update(Msg::OpDone(Err("fetch failed".into())));
        assert!(panel.busy.is_none());
        assert_eq!(panel.error.as_deref(), Some("fetch failed"));
        // A successful op clears a stale error only via ListLoaded.
        assert_eq!(panel.update(Msg::OpDone(Ok(()))), None);
        assert_eq!(panel.error.as_deref(), Some("fetch failed"));
    }

    #[test]
    fn list_loading_and_toggle_guards() {
        let mut panel = SubmodulePanel::default();
        assert_eq!(panel.update(Msg::ToggleOpen), Some(Action::Refresh));
        assert!(panel.open);
        assert_eq!(panel.update(Msg::ListLoaded(Err("boom".into()))), None);
        assert_eq!(panel.error.as_deref(), Some("boom"));
        assert!(!panel.loading);
        assert_eq!(panel.update(Msg::ListLoaded(Ok(vec![fixture()]))), None);
        assert!(panel.error.is_none());
        assert_eq!(panel.list.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn clip_truncates_long_urls() {
        assert_eq!(clip("https://example.com/x.git", 12), "https://exa…");
        assert_eq!(clip("short", 12), "short");
    }
}
