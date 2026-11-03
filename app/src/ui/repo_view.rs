//! Repository workspace: tab strip, repo header and the three-column body
//! (status + commit form, diff/commit detail, history graph).
//!
//! All state that outlives a single frame lives on [`Tab`]; the panel
//! modules render it and emit [`Message`]s.

use std::collections::HashSet;

use iced::widget::{button, column, container, row, scrollable, text, text_editor, Space};
use iced::{Alignment, Color, Element, Length};

use crate::engine::types::{ChangeKind, RebaseStep, RepoId, RepoInfo, RepoStatus};

use super::app::{InProgressAction, Message};
use super::bisect_panel;
use super::branch_panel::{self, BranchesState};
use super::commit_bar;
use super::commit_detail;
use super::dialog::{self, ResetRequest};
use super::diff_view::{self, DiffPane, DiffSlot, PaneMode};
use super::forge_panel::{self, ForgeCtx};
use super::health_panel;
use super::history_model::{FilterFields, HistoryState};
use super::history_panel;
use super::reflog_panel;
use super::remote_panel::{self, RemotesState};
use super::stash_panel::{self, StashesState};
use super::status_model::{
    build_status_index, has_staged_changes, remap_selection, SectionId, StatusIndex, StatusRow,
    StatusSelection,
};
use super::status_panel;
use super::submodule_panel;
use super::tags_panel;
use super::terminal;
use super::theme::{self, Tokens};
use super::worktree_panel;

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

/// A rebase request waiting for confirmation: the current branch's commits
/// (the loaded pick plan) rewritten on top of `onto`.
pub struct PendingRebase {
    pub onto: String,
    pub plan: Vec<RebaseStep>,
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
    pub stashes: StashesState,
    pub remotes: RemotesState,
    /// Bisect stepper state (banner above the workspace) and the reflog
    /// browser (dropdown, like the remotes panel).
    pub bisect: bisect_panel::BisectPanel,
    pub reflog: reflog_panel::ReflogPanel,
    /// Repo health / maintenance / archive / sparse / LFS panel.
    pub health: health_panel::HealthPanel,
    /// Worktree, submodule and tag management dropdowns (behind the ⋯
    /// header button to keep the header from overflowing).
    pub worktree: worktree_panel::WorktreePanel,
    pub submodule: submodule_panel::SubmodulePanel,
    pub tags: tags_panel::TagsPanel,
    /// GitHub PR/CI panel state (dropdown, like the remotes panel). The
    /// context lives beside it so the panel's `'a` borrows come from the
    /// tab, not per-frame locals; the shell refreshes it on status/branch
    /// loads and on every forge message.
    pub forge: forge_panel::ForgeState,
    pub forge_ctx: ForgeCtx,
    /// Embedded PTY terminal pane and its visibility toggle.
    pub terminal: terminal::State,
    pub terminal_open: bool,
    /// Paths awaiting the discard confirmation dialog.
    pub pending_discard: Option<Vec<String>>,
    /// Branch awaiting the delete confirmation dialog.
    pub pending_branch_delete: Option<String>,
    /// Stash (index, message) awaiting the drop confirmation dialog.
    pub pending_stash_drop: Option<(u32, String)>,
    /// Checkpoint (id, reason) awaiting the restore confirmation dialog.
    pub pending_checkpoint_restore: Option<(String, String)>,
    /// Commit and mode awaiting the reset confirmation dialog.
    pub pending_reset: Option<ResetRequest>,
    /// Remote awaiting the remove confirmation dialog.
    pub pending_remote_remove: Option<String>,
    /// Rebase (target branch + pick plan) awaiting the confirmation dialog.
    pub pending_rebase: Option<PendingRebase>,

    pub diff_seq: u64,
    pub commit_diff_seq: u64,
}

impl Tab {
    pub fn new(repo: RepoInfo) -> Self {
        let terminal = terminal::State::new(Some(repo.root.clone().into()));
        Self {
            repo,
            terminal,
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
            stashes: StashesState::default(),
            remotes: RemotesState::default(),
            forge: forge_panel::ForgeState::default(),
            forge_ctx: ForgeCtx::default(),
            bisect: bisect_panel::BisectPanel::default(),
            reflog: reflog_panel::ReflogPanel::default(),
            health: health_panel::HealthPanel::default(),
            worktree: worktree_panel::WorktreePanel::default(),
            submodule: submodule_panel::SubmodulePanel::default(),
            tags: tags_panel::TagsPanel::default(),
            terminal_open: false,
            pending_discard: None,
            pending_branch_delete: None,
            pending_stash_drop: None,
            pending_checkpoint_restore: None,
            pending_reset: None,
            pending_remote_remove: None,
            pending_rebase: None,
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
        let mut detail = paths.iter().take(6).cloned().collect::<Vec<_>>().join("\n");
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

    if let Some(name) = &tab.pending_remote_remove {
        let spec = dialog::Confirm {
            title: "Remove remote".to_string(),
            body: format!(
                "Remove the remote “{name}”? Its tracking branches and configuration are deleted; the remote repository is untouched."
            ),
            detail: None,
            confirm_label: "Remove".to_string(),
            danger: true,
        };
        return Some(dialog::confirm(
            tokens,
            &spec,
            Message::CancelDialog,
            Message::RemoteRemoveConfirmed,
        ));
    }

    if let Some((index, message)) = &tab.pending_stash_drop {
        let spec = dialog::Confirm {
            title: "Drop stash".to_string(),
            body: format!("Drop stash@{{{index}}}? This cannot be undone."),
            detail: (!message.trim().is_empty()).then(|| message.clone()),
            confirm_label: "Drop".to_string(),
            danger: true,
        };
        return Some(dialog::confirm(
            tokens,
            &spec,
            Message::CancelDialog,
            Message::StashDropConfirmed,
        ));
    }

    if let Some((id, reason)) = &tab.pending_checkpoint_restore {
        let spec = dialog::Confirm {
            title: "Restore checkpoint".to_string(),
            body: format!(
                "Restore the worktree to “{reason}”? The current state is snapshotted first."
            ),
            detail: Some(id.clone()),
            confirm_label: "Restore".to_string(),
            danger: false,
        };
        return Some(dialog::confirm(
            tokens,
            &spec,
            Message::CancelDialog,
            Message::CheckpointRestoreConfirmed,
        ));
    }

    if let Some(request) = &tab.pending_reset {
        return Some(dialog::reset(tokens, request));
    }

    if let Some(rebase) = &tab.pending_rebase {
        let mut detail = rebase
            .plan
            .iter()
            .take(6)
            .map(|step| short_sha(&step.sha))
            .collect::<Vec<_>>()
            .join("\n");
        if rebase.plan.len() > 6 {
            detail.push_str(&format!("\n… and {} more", rebase.plan.len() - 6));
        }
        let spec = dialog::Confirm {
            title: format!("Rebase onto {}", rebase.onto),
            body: format!(
                "Rewrite {} commit{} of the current branch on top of “{}”? This changes their commit ids.",
                rebase.plan.len(),
                if rebase.plan.len() == 1 { "" } else { "s" },
                rebase.onto
            ),
            detail: Some(detail),
            confirm_label: "Rebase".to_string(),
            danger: true,
        };
        return Some(dialog::confirm(
            tokens,
            &spec,
            Message::CancelDialog,
            Message::RebaseConfirmed,
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
        // Finish or back out of an in-progress operation without leaving the
        // app for a terminal.
        if status.merging {
            sync = sync.push(control_button(
                tokens,
                "Abort merge",
                theme::ButtonKind::Danger,
                Message::InProgress(InProgressAction::AbortMerge),
            ));
        }
        if status.rebasing {
            if !status.sequencer {
                sync = sync.push(control_button(
                    tokens,
                    "Continue",
                    theme::ButtonKind::Secondary,
                    Message::InProgress(InProgressAction::ContinueRebase),
                ));
            }
            sync = sync.push(control_button(
                tokens,
                "Abort",
                theme::ButtonKind::Danger,
                Message::InProgress(InProgressAction::AbortRebase),
            ));
        } else if status.sequencer && !status.merging {
            sync = sync.push(control_button(
                tokens,
                "Abort",
                theme::ButtonKind::Danger,
                Message::InProgress(InProgressAction::AbortSequencer),
            ));
        }
    }

    let history_label = if tab.history_open {
        "Hide history"
    } else {
        "History"
    };
    let stash_label = if tab.stashes.open {
        "Hide stash"
    } else {
        "Stash"
    };
    let remote_label = if tab.remotes.open {
        "Hide remote"
    } else {
        "Remote"
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
            button(text(remote_label).size(11).color(if tab.remotes.open {
                tokens.primary
            } else {
                tokens.muted_foreground
            }),)
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::ToggleRemotePanel),
            button(text(stash_label).size(11).color(if tab.stashes.open {
                tokens.primary
            } else {
                tokens.muted_foreground
            }),)
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::ToggleStashPanel),
            button(text(history_label).size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([6, 10])
                .on_press(Message::ToggleHistory),
            button(text("Forge").size(11).color(if tab.forge.open {
                tokens.primary
            } else {
                tokens.foreground
            }))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::Forge(forge_panel::Msg::ToggleOpen)),
            button(
                text("Bisect")
                    .size(11)
                    .color(if tab.bisect.open || tab.bisect.is_active() {
                        tokens.primary
                    } else {
                        tokens.foreground
                    })
            )
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::Bisect(bisect_panel::Msg::ToggleOpen)),
            button(text("Health").size(11).color(if tab.health.open {
                tokens.primary
            } else {
                tokens.foreground
            }))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::Health(health_panel::Msg::ToggleOpen)),
            button(text("⋯").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([6, 10])
                .on_press(Message::MoreMenu),
            button(text("Reflog").size(11).color(if tab.reflog.open {
                tokens.primary
            } else {
                tokens.foreground
            }))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::Reflog(reflog_panel::Msg::ToggleOpen)),
            button(text("Terminal").size(11).color(if tab.terminal_open {
                tokens.primary
            } else {
                tokens.foreground
            }))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([6, 10])
            .on_press(Message::ToggleTerminal),
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

    let main: Element<'a, Message> = if tab.branches.open
        || tab.stashes.open
        || tab.remotes.open
        || tab.forge.open
        || tab.reflog.open
        || tab.health.open
        || tab.worktree.open
        || tab.submodule.open
        || tab.tags.open
    {
        // Dropdowns: overlaid on the workspace so opening them never
        // reflows the panes underneath. The branch picker anchors top-left,
        // the stash/checkpoint and remote panels top-right (opening one
        // closes the others).
        let mut layers: Vec<Element<'a, Message>> = vec![surface.into()];

        if tab.branches.open {
            layers.push(
                container(container(branch_panel::view(tab, tokens)).width(Length::Fixed(320.0)))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Start)
                    .align_y(Alignment::Start)
                    .padding([6, 8])
                    .into(),
            );
        }
        if tab.stashes.open {
            layers.push(
                container(container(stash_panel::view(tab, tokens)).width(Length::Fixed(420.0)))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding([6, 8])
                    .into(),
            );
        }
        if tab.remotes.open {
            layers.push(
                container(container(remote_panel::view(tab, tokens)).width(Length::Fixed(460.0)))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding([6, 8])
                    .into(),
            );
        }
        if tab.worktree.open {
            layers.push(
                container(
                    container(tab.worktree.view(tokens).map(Message::Worktree))
                        .width(Length::Fixed(520.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Start)
                .padding([6, 8])
                .into(),
            );
        }
        if tab.submodule.open {
            layers.push(
                container(
                    container(tab.submodule.view(tokens).map(Message::Submodule))
                        .width(Length::Fixed(520.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Start)
                .padding([6, 8])
                .into(),
            );
        }
        if tab.tags.open {
            layers.push(
                container(
                    container(tab.tags.view(tokens).map(Message::Tags)).width(Length::Fixed(520.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Start)
                .padding([6, 8])
                .into(),
            );
        }
        if tab.health.open {
            layers.push(
                container(
                    container(tab.health.view(tokens).map(Message::Health))
                        .width(Length::Fixed(520.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Start)
                .padding([6, 8])
                .into(),
            );
        }
        if tab.reflog.open {
            if let Some(panel) = tab.reflog.view(tokens) {
                layers.push(
                    container(container(panel.map(Message::Reflog)).width(Length::Fixed(520.0)))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(Alignment::End)
                        .align_y(Alignment::Start)
                        .padding([6, 8])
                        .into(),
                );
            }
        }
        if tab.forge.open {
            layers.push(
                container(
                    container(
                        forge_panel::view(&tab.forge, tokens, &tab.forge_ctx).map(Message::Forge),
                    )
                    .width(Length::Fixed(520.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Start)
                .padding([6, 8])
                .into(),
            );
        }

        iced::widget::stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    } else {
        surface.into()
    };

    let main: Element<'a, Message> = match tab.bisect.view(tokens) {
        Some(banner) => column![banner.map(Message::Bisect), main]
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        None => main,
    };

    if tab.terminal_open {
        // Bottom dock: overlays above still cover the whole workspace; the
        // terminal shrinks the panes above it.
        column![
            main,
            container(terminal::view(&tab.terminal, tokens).map(Message::Terminal))
                .width(Length::Fill)
                .height(Length::Fixed(TERMINAL_HEIGHT))
                .style(move |_theme| container::Style {
                    border: iced::Border {
                        color: tokens.border,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Style::default()
                }),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        main
    }
}

/// Height of the docked terminal pane.
const TERMINAL_HEIGHT: f32 = 280.0;

fn control_button<'a>(
    tokens: Tokens,
    label: &'a str,
    kind: theme::ButtonKind,
    message: Message,
) -> Element<'a, Message> {
    button(text(label).size(10))
        .style(theme::button_style(tokens, kind))
        .padding([2, 8])
        .on_press(message)
        .into()
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
