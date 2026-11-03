//! Worktrees panel: linked worktrees with add/remove/prune.
//!
//! Self-contained lane module (like the former Svelte panel): it owns its
//! state, its own [`Msg`] enum, and returns [`Action`]s from [`update`] for
//! the shell to translate into service calls (`crate::service::worktree`).
//! Nothing here touches the shell's `Message` — wiring maps both enums.
//!
//! Add form is one branch name plus an optional folder: an existing local
//! branch is checked out, anything else becomes a fresh branch (the mapping
//! lives in `crate::service::worktree::worktree_add_named`). Remove confirms
//! in-panel (no shell dialog); locked/prunable rows remove with force —
//! they cannot be removed without it. Prune surfaces the returned count
//! through [`Msg::PruneDone`] as a status note.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::WorktreeInfo;

use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// State and form buffers for the worktrees panel.
#[derive(Debug, Default)]
pub struct WorktreePanel {
    pub open: bool,
    pub loading: bool,
    pub list: Option<Vec<WorktreeInfo>>,
    pub error: Option<String>,
    pub add_name: String,
    pub add_path: String,
    /// The worktree name awaiting removal confirmation (inline, per-row).
    pub pending_remove: Option<String>,
    /// One-line result of the last prune (`PruneDone`), until the next load.
    pub prune_note: Option<String>,
}

/// Panel-internal messages; the shell forwards each into [`update`].
#[derive(Debug, Clone)]
pub enum Msg {
    ToggleOpen,
    RefreshPressed,
    ListLoaded(Result<Vec<WorktreeInfo>, String>),
    AddNameChanged(String),
    AddPathChanged(String),
    AddPressed,
    /// Start the inline remove confirm for this worktree name.
    RemoveRequested(String),
    RemoveConfirmed,
    RemoveCanceled,
    PrunePressed,
    /// Shell reports the finished prune (`git worktree prune` count).
    PruneDone(u32),
}

/// What the shell should execute for the repo the panel is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    /// `name` resolves to an existing branch or a fresh one; empty `path`
    /// means "derive a sibling folder" (service-side default).
    Add {
        name: String,
        path: String,
    },
    Remove {
        name: String,
        force: bool,
    },
    Prune,
}

impl WorktreePanel {
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
                self.prune_note = None;
                match result {
                    Ok(list) => {
                        self.error = None;
                        self.list = Some(list);
                    }
                    Err(error) => self.error = Some(error),
                }
                None
            }
            Msg::AddNameChanged(name) => {
                self.add_name = name;
                None
            }
            Msg::AddPathChanged(path) => {
                self.add_path = path;
                None
            }
            Msg::AddPressed => {
                // Guard: the branch name is the only required field.
                let name = self.add_name.trim().to_owned();
                if name.is_empty() {
                    return None;
                }
                let path = self.add_path.trim().to_owned();
                self.add_name.clear();
                self.add_path.clear();
                Some(Action::Add { name, path })
            }
            Msg::RemoveRequested(name) => {
                self.pending_remove = Some(name);
                None
            }
            Msg::RemoveConfirmed => {
                let name = self.pending_remove.take()?;
                Some(Action::Remove {
                    force: needs_force(self.info(&name)),
                    name,
                })
            }
            Msg::RemoveCanceled => {
                self.pending_remove = None;
                None
            }
            Msg::PrunePressed => Some(Action::Prune),
            Msg::PruneDone(count) => {
                self.prune_note = Some(format!(
                    "Pruned {count} stale worktree{}",
                    if count == 1 { "" } else { "s" }
                ));
                None
            }
        }
    }

    /// The listed worktree with this name, for force-decision lookups.
    fn info(&self, name: &str) -> Option<&WorktreeInfo> {
        self.list.as_ref()?.iter().find(|info| info.name == name)
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        let count = self.list.as_ref().map(|list| list.len());

        let mut header = row![text("Worktrees").size(12).font(theme::semibold())]
            .spacing(6)
            .align_y(Alignment::Center);
        if let Some(count) = count {
            header = header.push(
                text(format!("{count}"))
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
                button(text("Prune").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 8])
                    .on_press(Msg::PrunePressed),
            )
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

        let add_form = row![
            text_input("Branch (existing or new)", self.add_name.as_str())
                .on_input(Msg::AddNameChanged)
                .on_submit(Msg::AddPressed)
                .size(11)
                .padding([4, 6])
                .width(Length::Fixed(150.0)),
            text_input("Folder (optional)", self.add_path.as_str())
                .on_input(Msg::AddPathChanged)
                .on_submit(Msg::AddPressed)
                .size(11)
                .padding([4, 6])
                .width(Length::Fill),
            button(text("Add").size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                .padding([4, 10])
                .on_press_maybe((!self.add_name.trim().is_empty()).then_some(Msg::AddPressed)),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let status_note: Element<'_, Msg> = match (&self.error, &self.prune_note) {
            (Some(error), _) => note(tokens, error.clone(), true),
            (None, Some(done)) => note(tokens, done.clone(), false),
            (None, None) => column![].into(),
        };

        let body: Element<'_, Msg> = match &self.list {
            Some(list) if !list.is_empty() => {
                let mut rows = column![].spacing(3).width(Length::Fill);
                for info in list {
                    rows = rows.push(worktree_row(info, self.pending_remove.as_deref(), tokens));
                }
                let height = (list.len() as f32 * 44.0).clamp(44.0, 220.0);
                scrollable(rows)
                    .height(Length::Fixed(height))
                    .width(Length::Fill)
                    .into()
            }
            Some(_) => note(tokens, "No linked worktrees.", false),
            None => note(tokens, "Loading worktrees…", false),
        };

        container(
            column![header, add_form, status_note, body]
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

fn worktree_row<'a>(
    info: &'a WorktreeInfo,
    pending_remove: Option<&str>,
    tokens: Tokens,
) -> Element<'a, Msg> {
    let mut chips = row![].spacing(4).align_y(Alignment::Center);
    for chip in worktree_chips(info) {
        chips = chips.push(chip_view(chip, tokens));
    }

    let line = row![
        text(info.name.as_str())
            .size(11)
            .font(theme::mono())
            .color(tokens.foreground),
        chips,
        Space::new().width(Length::Fill),
        text(ref_label(info)).size(9).color(tokens.muted_foreground),
        button(text("Remove").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(50.0))
            .on_press(Msg::RemoveRequested(info.name.clone())),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let mut card = column![
        line,
        text(clip(&info.path, 64))
            .size(9)
            .color(tokens.muted_foreground)
    ]
    .spacing(2)
    .width(Length::Fill);

    if pending_remove == Some(info.name.as_str()) {
        let force_note = if needs_force(Some(info)) {
            "locked or prunable — removed with force"
        } else {
            ""
        };
        card = card.push(
            row![
                text(force_note).size(9).color(tokens.muted_foreground),
                Space::new().width(Length::Fill),
                button(text("Cancel").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 6])
                    .on_press(Msg::RemoveCanceled),
                button(text("Confirm remove").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                    .padding([2, 6])
                    .on_press(Msg::RemoveConfirmed),
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

/// A small status chip: neutral (`main`, `detached`) or danger
/// (`locked`, `prunable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chip {
    pub label: String,
    pub danger: bool,
}

/// Pure row mapping (unit-tested): chips for the flags `WorktreeInfo`
/// carries. `main` marks the repository's own root, `detached` a HEAD not on
/// a branch, `locked`/`prunable` the removal-relevant states.
pub(crate) fn worktree_chips(info: &WorktreeInfo) -> Vec<Chip> {
    let mut chips = Vec::new();
    if info.is_main {
        chips.push(Chip {
            label: "main".into(),
            danger: false,
        });
    }
    if info.detached {
        chips.push(Chip {
            label: "detached".into(),
            danger: false,
        });
    }
    if info.locked {
        chips.push(Chip {
            label: "locked".into(),
            danger: true,
        });
    }
    if let Some(reason) = info.prunable.as_deref() {
        chips.push(Chip {
            label: format!("prunable: {reason}"),
            danger: true,
        });
    }
    chips
}

/// Right-aligned ref label: the checked-out branch, else `@ <short sha>`.
pub(crate) fn ref_label(info: &WorktreeInfo) -> String {
    match (&info.branch, &info.head) {
        (Some(branch), _) => branch.clone(),
        (None, Some(head)) => format!("@ {}", short_sha(head)),
        (None, None) => "?".into(),
    }
}

/// Locked or prunable worktrees only remove with force (the engine refuses
/// otherwise), so the confirm strip implies it instead of offering a toggle.
pub(crate) fn needs_force(info: Option<&WorktreeInfo>) -> bool {
    info.is_some_and(|info| info.locked || info.prunable.is_some())
}

fn chip_view<'a>(chip: Chip, tokens: Tokens) -> Element<'a, Msg> {
    let color = if chip.danger {
        tokens.destructive
    } else {
        tokens.muted_foreground
    };
    container(text(chip.label).size(9).color(color))
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
    use super::{clip, needs_force, ref_label, worktree_chips, Action, Msg, WorktreePanel};
    use crate::engine::types::WorktreeInfo;

    fn fixture() -> WorktreeInfo {
        WorktreeInfo {
            path: "/repos/wt-spike".into(),
            name: "wt-spike".into(),
            branch: Some("spike".into()),
            head: Some("0123456789abcdef0123456789abcdef01234567".into()),
            detached: false,
            locked: false,
            prunable: None,
            is_main: false,
        }
    }

    fn panel_with(list: Vec<WorktreeInfo>) -> WorktreePanel {
        let mut panel = WorktreePanel::default();
        let _ = panel.update(Msg::ListLoaded(Ok(list)));
        panel
    }

    #[test]
    fn add_requires_a_non_empty_name() {
        let mut panel = WorktreePanel::default();
        assert_eq!(panel.update(Msg::AddPressed), None);
        panel.update(Msg::AddNameChanged("   ".into()));
        assert_eq!(panel.update(Msg::AddPressed), None);
        panel.update(Msg::AddNameChanged("spike".into()));
        panel.update(Msg::AddPathChanged("/tmp/wt".into()));
        assert_eq!(
            panel.update(Msg::AddPressed),
            Some(Action::Add {
                name: "spike".into(),
                path: "/tmp/wt".into()
            })
        );
        // The form resets after a successful submit.
        assert!(panel.add_name.is_empty() && panel.add_path.is_empty());
    }

    #[test]
    fn remove_needs_the_inline_confirm() {
        let mut panel = panel_with(vec![fixture()]);
        // Confirm without a pending request is a no-op.
        assert_eq!(panel.update(Msg::RemoveConfirmed), None);
        panel.update(Msg::RemoveRequested("wt-spike".into()));
        // Cancel clears the pending request; nothing fires afterwards.
        assert_eq!(panel.update(Msg::RemoveCanceled), None);
        assert_eq!(panel.update(Msg::RemoveConfirmed), None);
        // Request then confirm: unlocked row removes without force.
        panel.update(Msg::RemoveRequested("wt-spike".into()));
        assert_eq!(
            panel.update(Msg::RemoveConfirmed),
            Some(Action::Remove {
                name: "wt-spike".into(),
                force: false
            })
        );
    }

    #[test]
    fn remove_implies_force_for_locked_or_prunable_rows() {
        let mut locked = fixture();
        locked.locked = true;
        let mut panel = panel_with(vec![locked]);
        panel.update(Msg::RemoveRequested("wt-spike".into()));
        assert_eq!(
            panel.update(Msg::RemoveConfirmed),
            Some(Action::Remove {
                name: "wt-spike".into(),
                force: true
            })
        );
    }

    #[test]
    fn list_loading_and_toggle_guards() {
        let mut panel = WorktreePanel::default();
        assert_eq!(panel.update(Msg::ToggleOpen), Some(Action::Refresh));
        assert!(panel.open);
        assert_eq!(panel.update(Msg::ToggleOpen), None);
        assert!(!panel.open);
        let err = panel.update(Msg::ListLoaded(Err("boom".into())));
        assert_eq!(err, None);
        assert_eq!(panel.error.as_deref(), Some("boom"));
        assert!(!panel.loading);
    }

    #[test]
    fn prune_note_surfaces_the_returned_count() {
        let mut panel = WorktreePanel::default();
        assert_eq!(panel.update(Msg::PrunePressed), Some(Action::Prune));
        panel.update(Msg::PruneDone(1));
        assert_eq!(panel.prune_note.as_deref(), Some("Pruned 1 stale worktree"));
        panel.update(Msg::PruneDone(3));
        assert_eq!(
            panel.prune_note.as_deref(),
            Some("Pruned 3 stale worktrees")
        );
        // A fresh list clears the note.
        panel.update(Msg::ListLoaded(Ok(vec![fixture()])));
        assert!(panel.prune_note.is_none());
    }

    #[test]
    fn row_mapping_main_detached_and_ref_labels() {
        let mut info = fixture();
        assert!(worktree_chips(&info).is_empty());
        assert_eq!(ref_label(&info), "spike");

        info.is_main = true;
        info.detached = true;
        info.locked = true;
        info.prunable = Some("missing".into());
        let chips = worktree_chips(&info);
        let labels: Vec<&str> = chips.iter().map(|chip| chip.label.as_str()).collect();
        assert_eq!(labels, ["main", "detached", "locked", "prunable: missing"]);
        assert!(chips.iter().take(2).all(|chip| !chip.danger));
        assert!(chips.iter().skip(2).all(|chip| chip.danger));
        info.branch = None;
        assert_eq!(ref_label(&info), "@ 0123456");
    }

    #[test]
    fn needs_force_matches_locked_or_prunable_only() {
        let mut info = fixture();
        assert!(!needs_force(Some(&info)));
        info.locked = true;
        assert!(needs_force(Some(&info)));
        info.locked = false;
        info.prunable = Some("gone".into());
        assert!(needs_force(Some(&info)));
        assert!(!needs_force(None));
    }

    #[test]
    fn clip_truncates_long_paths() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("0123456789", 5), "0123…");
    }
}
