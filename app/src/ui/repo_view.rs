//! Repository workspace: tab strip, repo header and the three-column body
//! (status + commit form, diff/commit detail, history graph).
//!
//! All state that outlives a single frame lives on [`Tab`]; the panel
//! modules render it and emit [`Message`]s.

use std::collections::HashSet;

use iced::widget::{button, column, container, row, scrollable, text, text_editor, Space};
use iced::{Alignment, Color, Element, Length};

use crate::engine::types::{ChangeKind, RepoId, RepoInfo, RepoStatus};

use super::app::Message;
use super::branch_panel::{self, BranchesState};
use super::commit_bar;
use super::commit_detail;
use super::dialog;
use super::diff_view::{self, DiffPane, DiffSlot, PaneMode};
use super::history_model::{FilterFields, HistoryState};
use super::history_panel;
use super::status_model::{
    build_status_index, has_staged_changes, remap_selection, SectionId, StatusIndex, StatusRow,
    StatusSelection,
};
use super::status_panel;
use super::theme::{self, Tokens};

pub const SIDEBAR_WIDTH: f32 = 320.0;
pub const HISTORY_WIDTH: f32 = 340.0;

/// Load state for data fetched through the service layer.
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// Selection movement requested by the last mutation, applied when the
/// following status load lands: keys that now live in `section` move there.
pub struct PendingRemap {
    pub paths: HashSet<String>,
    pub unstage: bool,
}

/// One open repository (a tab).
pub struct Tab {
    pub repo: RepoInfo,
    pub status: Load<RepoStatus>,
    pub status_index: StatusIndex,
    pub selection: StatusSelection,
    /// Key of the row shown in the diff pane.
    pub selected_row: Option<String>,
    pub filter: String,
    pub collapsed_sections: [bool; 2],
    pub pending_remap: Option<PendingRemap>,
    /// Force a diff reload on the next status refresh (hunk/line staging).
    pub reload_diff: bool,

    pub diff: DiffPane,
    pub commit_diff: DiffPane,
    pub diff_split: bool,
    pub diff_wrap: bool,
    pub detail_open: bool,

    pub message: text_editor::Content,
    pub amend: bool,
    pub no_verify: bool,
    pub committing: bool,
    pub commit_error: Option<String>,

    pub history: HistoryState,
    pub history_open: bool,
    pub history_filter: FilterFields,
    pub history_filter_open: bool,

    pub branches: BranchesState,
    /// Paths awaiting the discard confirmation dialog.
    pub pending_discard: Option<Vec<String>>,
    /// Branch awaiting the delete confirmation dialog.
    pub pending_branch_delete: Option<String>,

    pub diff_seq: u64,
    pub commit_diff_seq: u64,
}

impl Tab {
    pub fn new(repo: RepoInfo) -> Self {
        Self {
            repo,
            status: Load::Loading,
            status_index: StatusIndex::default(),
            selection: StatusSelection::default(),
            selected_row: None,
            filter: String::new(),
            collapsed_sections: [false, false],
            pending_remap: None,
            reload_diff: true,
            diff: DiffPane::default(),
            commit_diff: DiffPane::default(),
            diff_split: false,
            diff_wrap: false,
            detail_open: false,
            message: text_editor::Content::with_text(""),
            amend: false,
            no_verify: false,
            committing: false,
            commit_error: None,
            history: HistoryState::default(),
            history_open: true,
            history_filter: FilterFields::default(),
            history_filter_open: false,
            branches: BranchesState::default(),
            pending_discard: None,
            pending_branch_delete: None,
            diff_seq: 0,
            commit_diff_seq: 0,
        }
    }

    pub fn selected_status_row(&self) -> Option<&StatusRow> {
        self.selected_row
            .as_deref()
            .and_then(|key| self.status_index.row(key))
    }

    /// Which staging controls are safe for the selected row.
    pub fn working_mode(&self) -> PaneMode {
        let Some(row) = self.selected_status_row() else {
            return PaneMode::default();
        };
        if matches!(row.index, ChangeKind::Conflicted)
            || matches!(row.worktree, ChangeKind::Conflicted)
        {
            return PaneMode::default();
        }
        match row.section {
            SectionId::Staged => {
                let clean = matches!(row.worktree, ChangeKind::Unmodified | ChangeKind::Ignored);
                PaneMode {
                    file_actions: true,
                    line_actions: clean,
                    unstage: true,
                }
            }
            SectionId::Changes => {
                let clean = matches!(row.index, ChangeKind::Unmodified | ChangeKind::Ignored);
                PaneMode {
                    file_actions: true,
                    line_actions: clean,
                    unstage: false,
                }
            }
        }
    }

    /// Paths to include in the diff request (old path first for renames).
    pub fn diff_paths(&self) -> Option<Vec<String>> {
        let row = self.selected_status_row()?;
        let mut paths = Vec::new();
        if let Some(old_path) = &row.old_path {
            if old_path != &row.path {
                paths.push(old_path.clone());
            }
        }
        paths.push(row.path.clone());
        Some(paths)
    }

    pub fn section_collapsed(&self, section: SectionId) -> bool {
        self.collapsed_sections[section.index()]
    }

    pub fn toggle_section(&mut self, section: SectionId) {
        let slot = &mut self.collapsed_sections[section.index()];
        *slot = !*slot;
    }

    pub fn can_commit(&self) -> bool {
        if self.committing {
            return false;
        }
        if self.amend {
            return true;
        }
        let message = self.message.text();
        !message.trim().is_empty()
            && self
                .status
                .ready()
                .map(|status| has_staged_changes(&status.entries))
                .unwrap_or(false)
    }

    /// Rebuild the row index from a fresh status and reconcile selection.
    pub fn apply_status(&mut self, status: RepoStatus) {
        let previous = self.selected_status_row().cloned();
        self.status_index = build_status_index(&status.entries, None);
        let live = self.status_index.live_keys();

        let pending = self.pending_remap.take();
        if let Some(pending) = &pending {
            self.selection.selected = remap_selection(
                &self.selection.selected,
                &pending.paths,
                pending.unstage,
                &self.status_index.rows,
                None,
            );
        }
        self.selection.prune(&live);

        if let Some(key) = self.selected_row.clone() {
            if !live.contains(&key) {
                let moved = pending.as_ref().and_then(|pending| {
                    let row = previous.as_ref()?;
                    if !pending.paths.contains(&row.path) {
                        return None;
                    }
                    let target = if pending.unstage {
                        SectionId::Changes
                    } else {
                        SectionId::Staged
                    };
                    let candidate = format!("{}:{}", target.as_str(), row.path);
                    live.contains(&candidate).then_some(candidate)
                });
                self.selected_row = moved;
                if self.selected_row.is_none() {
                    self.detail_open = false;
                }
            }
        }

        self.status = Load::Ready(status);
    }

    /// Whether the selected working diff should be re-fetched.
    pub fn diff_is_stale(&self) -> bool {
        self.reload_diff
            || self
                .working_diff_key()
                .map(|key| self.diff.key.as_deref() != Some(key.as_str()))
                .unwrap_or(false)
    }

    /// Identity key of the working diff for the selected row.
    pub fn working_diff_key(&self) -> Option<String> {
        let row = self.selected_status_row()?;
        Some(format!("{:?}:{}", row.section, row.key))
    }
}

pub fn view<'a>(tabs: &'a [Tab], active: usize, tokens: Tokens) -> Element<'a, Message> {
    let Some(tab) = tabs.get(active) else {
        return Space::new().into();
    };

    let page = column![
        tab_strip(tabs, active, tokens),
        repo_header(tab, tokens),
        workspace(tab, tokens),
    ];

    let base: Element<'a, Message> = page.width(Length::Fill).height(Length::Fill).into();
    match dialog_overlay(tab, tokens) {
        Some(overlay) => iced::widget::stack![base, overlay]
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        None => base,
    }
}

/// The modal dialog a tab currently asks for, if any.
fn dialog_overlay<'a>(tab: &'a Tab, tokens: Tokens) -> Option<Element<'a, Message>> {
    if let Some(paths) = &tab.pending_discard {
        let mut detail = paths
            .iter()
            .take(6)
            .map(|path| path.clone())
            .collect::<Vec<_>>()
            .join("\n");
        if paths.len() > 6 {
            detail.push_str(&format!("\n… and {} more", paths.len() - 6));
        }
        let spec = dialog::Confirm {
            title: "Discard changes".to_string(),
            body: format!(
                "Discard uncommitted changes in {} file{}? This cannot be undone.",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            ),
            detail: Some(detail),
            confirm_label: "Discard".to_string(),
            danger: true,
        };
        return Some(dialog::confirm(
            tokens,
            &spec,
            Message::CancelDialog,
            Message::ConfirmDiscard,
        ));
    }

    if let Some(name) = &tab.pending_branch_delete {
        let spec = dialog::Confirm {
            title: "Delete branch".to_string(),
            body: format!("Delete the branch “{name}”? This cannot be undone."),
            detail: None,
            confirm_label: "Delete".to_string(),
            danger: true,
        };
        return Some(dialog::confirm(
            tokens,
            &spec,
            Message::CancelDialog,
            Message::BranchDeleteConfirmed,
        ));
    }

    None
}

fn tab_strip<'a>(tabs: &'a [Tab], active: usize, tokens: Tokens) -> Element<'a, Message> {
    let mut strip = row![].spacing(2).align_y(Alignment::Center);

    for (index, tab) in tabs.iter().enumerate() {
        let selected = index == active;
        strip = strip.push(
            button(text(tab.repo.name.as_str()).size(12))
                .style(theme::button_style(
                    tokens,
                    if selected {
                        theme::ButtonKind::Secondary
                    } else {
                        theme::ButtonKind::Ghost
                    },
                ))
                .padding([5, 12])
                .on_press(Message::SelectTab(index)),
        );
        if selected {
            strip = strip.push(
                button(text("×").size(12))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([5, 7])
                    .on_press(Message::CloseTab(tab.repo.repo_id.clone())),
            );
        }
    }

    container(
        scrollable(strip)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::default(),
            ))
            .width(Length::Fill),
    )
    .padding([4, 8])
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(tokens.sidebar)),
        ..container::Style::default()
    })
    .into()
}

fn repo_header<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let status = tab.status.ready();

    let branch = status
        .and_then(|status| status.branch.clone())
        .unwrap_or_else(|| {
            if tab.repo.bare {
                "bare".to_string()
            } else if let Some(head) = status.and_then(|status| status.head.as_deref()) {
                format!("detached @ {}", short_sha(head))
            } else {
                "no commits".to_string()
            }
        });

    let mut sync = row![].spacing(6).align_y(Alignment::Center);
    if let Some(status) = status {
        if status.ahead > 0 {
            sync = sync.push(chip(format!("↑{}", status.ahead), tokens.modified));
        }
        if status.behind > 0 {
            sync = sync.push(chip(format!("↓{}", status.behind), tokens.renamed));
        }
        if status.merging {
            sync = sync.push(chip("merging", tokens.modified));
        }
        if status.rebasing {
            sync = sync.push(chip("rebasing", tokens.modified));
        }
        if status.sequencer {
            sync = sync.push(chip("sequencer", tokens.deleted));
        }
    }

    let history_label = if tab.history_open {
        "Hide history"
    } else {
        "History"
    };

    container(
        row![
            column![
                text(tab.repo.name.as_str())
                    .size(16)
                    .font(theme::semibold()),
                text(tab.repo.root.as_str())
                    .size(11)
                    .color(tokens.muted_foreground),
            ]
            .spacing(1),
            Space::new().width(12),
            button(
                text(format!("{branch} ▾"))
                    .size(11)
                    .color(if tab.branches.open {
                        tokens.primary
                    } else {
                        tokens.muted_foreground
                    }),
            )
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(Message::ToggleBranchPanel),
            sync,
            Space::new().width(Length::Fill),
            button(text(history_label).size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([6, 10])
                .on_press(Message::ToggleHistory),
            button(text("Refresh").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([6, 10])
                .on_press(Message::Refresh(tab.repo.repo_id.clone())),
        ]
        .align_y(Alignment::Center)
        .padding([8, 12]),
    )
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(tokens.card)),
        border: iced::Border {
            color: tokens.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn workspace<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let sidebar = container(
        column![
            container(status_panel::view(tab, tokens))
                .width(Length::Fill)
                .height(Length::Fill),
            commit_bar::view(tab, tokens),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fixed(SIDEBAR_WIDTH))
    .height(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(tokens.sidebar)),
        border: iced::Border {
            color: tokens.sidebar_border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    let center: Element<'a, Message> = if tab.detail_open {
        commit_detail::view(tab, tokens)
    } else {
        diff_view::view(
            &tab.diff,
            DiffSlot::Working,
            tokens,
            tab.diff_split,
            tab.diff_wrap,
            tab.working_mode(),
        )
    };

    let mut body = row![
        sidebar,
        container(center).width(Length::Fill).height(Length::Fill),
    ]
    .spacing(0)
    .width(Length::Fill)
    .height(Length::Fill);

    if tab.history_open {
        body = body.push(
            container(history_panel::view(tab, tokens))
                .width(Length::Fixed(HISTORY_WIDTH))
                .height(Length::Fill)
                .style(move |_theme| container::Style {
                    background: Some(iced::Background::Color(tokens.sidebar)),
                    border: iced::Border {
                        color: tokens.sidebar_border,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Style::default()
                }),
        );
    }

    let surface = container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::surface_style(tokens));

    if tab.branches.open {
        // Dropdown: overlaid on the workspace top-left so opening it never
        // reflows the panes underneath.
        let overlay =
            container(container(branch_panel::view(tab, tokens)).width(Length::Fixed(320.0)))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Start)
                .align_y(Alignment::Start)
                .padding([6, 8]);

        iced::widget::stack![surface, overlay]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    } else {
        surface.into()
    }
}

pub fn chip<'a>(label: impl Into<String>, color: Color) -> Element<'a, Message> {
    container(text(label.into()).size(11).color(color))
        .padding([2, 7])
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(theme::with_alpha(color, 0.12))),
            border: iced::Border {
                color: theme::with_alpha(color, 0.25),
                width: 1.0,
                radius: 99.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

pub fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Exposed for the tab strip: finds a tab by repo id.
pub fn tab_index(tabs: &[Tab], repo_id: &RepoId) -> Option<usize> {
    tabs.iter().position(|tab| &tab.repo.repo_id == repo_id)
}
