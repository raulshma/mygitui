//! Branch picker panel: local branches with checkout, creation and deletion.
//!
//! Rendered as an expanding strip under the repo header (no overlay); the
//! header branch chip toggles it.

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{BranchInfo, RemoteBranchInfo};

use super::app::Message;
use super::repo_view::{chip, Tab};
use super::theme::{self, Tokens};

/// Load state and form buffers for the branch panel.
#[derive(Debug, Default)]
pub struct BranchesState {
    pub open: bool,
    pub loading: bool,
    pub list: Option<Vec<BranchInfo>>,
    pub error: Option<String>,
    pub create_name: String,
    /// Start point for the pending create (set by "branch from here"
    /// intents); `None` = HEAD.
    pub create_from: Option<String>,
    /// Remote tracking branches (`refs/remotes/*`), loaded with the panel.
    pub remote_list: Option<Vec<RemoteBranchInfo>>,
    pub remote_loading: bool,
    pub remote_error: Option<String>,
}

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let state = &tab.branches;

    let mut header = row![
        text("Branches").size(12).font(theme::semibold()),
        Space::new().width(6),
    ]
    .align_y(Alignment::Center);

    if let Some(list) = &state.list {
        header = header.push(
            text(format!("{}", list.len()))
                .size(10)
                .color(tokens.muted_foreground),
        );
    }
    if state.loading {
        header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
    }
    header = header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text("⟳").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::RefreshBranches),
        )
        .push(
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::ToggleBranchPanel),
        );

    let body: Element<'a, Message> = match &state.list {
        Some(list) if !list.is_empty() => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            for branch in list {
                rows = rows.push(branch_row(branch, tokens));
            }
            // Shrink to the row count, capped so long branch lists scroll.
            let height = (list.len() as f32 * 26.0).clamp(26.0, 200.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => container(text("No branches.").size(11).color(tokens.muted_foreground))
            .padding([6, 2])
            .into(),
        None if state.error.is_some() => container(
            text(state.error.clone().unwrap_or_default())
                .size(11)
                .color(tokens.deleted),
        )
        .padding([6, 2])
        .into(),
        None => container(
            text("Loading branches…")
                .size(11)
                .color(tokens.muted_foreground),
        )
        .padding([6, 2])
        .into(),
    };

    let create = row![
        text_input("new-branch", state.create_name.as_str())
            .on_input(Message::BranchCreateNameChanged)
            .on_submit(Message::BranchCreate)
            .size(11)
            .padding([4, 6])
            .width(Length::Fill),
        button(text("Create & checkout").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 10])
            .on_press_maybe(
                (!state.create_name.trim().is_empty()).then_some(Message::BranchCreate)
            ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let remote_section = remote_section(state, tokens);

    container(
        column![header, body, remote_section, create]
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

fn branch_row<'a>(branch: &'a BranchInfo, tokens: Tokens) -> Element<'a, Message> {
    let name_color = if branch.is_head {
        tokens.primary
    } else {
        tokens.foreground
    };

    let mut info = row![
        text(if branch.is_head { "●" } else { "○" })
            .size(9)
            .color(name_color),
        text(branch.name.as_str())
            .size(11)
            .font(theme::mono())
            .color(name_color),
    ]
    .spacing(5)
    .align_y(Alignment::Center);

    if let Some(upstream) = branch.upstream.as_deref() {
        info = info.push(
            text(if branch.gone {
                format!("{upstream} · gone")
            } else {
                upstream.to_string()
            })
            .size(9)
            .color(tokens.muted_foreground),
        );
    }
    if branch.ahead > 0 {
        info = info.push(chip(format!("↑{}", branch.ahead), tokens.modified));
    }
    if branch.behind > 0 {
        info = info.push(chip(format!("↓{}", branch.behind), tokens.renamed));
    }

    let mut line = row![
        button(
            container(info)
                .width(Length::Fill)
                .align_y(Alignment::Center)
        )
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([3, 6])
        .width(Length::Fill)
        .on_press_maybe((!branch.is_head).then(|| Message::BranchSwitch(branch.name.clone()))),
        Space::new().width(4),
    ]
    .align_y(Alignment::Center);

    if !branch.is_head {
        // Explicit width: a Shrink button beside the Fill row button resolves
        // to a zero-width hit area inside the scrollable row.
        line = line.push(
            button(text("Rebase").size(9))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 6])
                .width(Length::Fixed(48.0))
                .on_press(Message::RebaseOntoRequested(branch.name.clone())),
        );
        line = line.push(
            button(text("Merge").size(9))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 6])
                .width(Length::Fixed(46.0))
                .on_press(Message::MergeBranch(branch.name.clone())),
        );
        line = line.push(
            button(text("Delete").size(9))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 6])
                .width(Length::Fixed(48.0))
                .on_press(Message::BranchDeleteRequested(branch.name.clone())),
        );
    }

    mouse_area(
        container(line)
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(Background::Color(if branch.is_head {
                    theme::with_alpha(tokens.primary, 0.08)
                } else {
                    theme::with_alpha(tokens.muted, 0.0)
                })),
                ..container::Style::default()
            }),
    )
    .on_right_press(Message::BranchRowMenu {
        name: branch.name.clone(),
    })
    .into()
}

/// The remote-tracking section under the local list: rows carry checkout
/// (with the shell's dirty-tree confirm), copy-name and delete (a push
/// delete confirmed by the shell) actions.
fn remote_section<'a>(state: &'a BranchesState, tokens: Tokens) -> Element<'a, Message> {
    let mut header = row![text("Remote")
        .size(10)
        .font(theme::semibold())
        .color(tokens.muted_foreground)]
    .align_y(Alignment::Center);
    if state.remote_loading {
        header = header
            .push(Space::new().width(6))
            .push(text("· loading…").size(10).color(tokens.muted_foreground));
    }

    let rows: Element<'a, Message> = match &state.remote_list {
        Some(list) if !list.is_empty() => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            for branch in list {
                rows = rows.push(remote_branch_row(branch, tokens));
            }
            let height = (list.len() as f32 * 24.0).clamp(24.0, 140.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => container(
            text("No remote branches.")
                .size(10)
                .color(tokens.muted_foreground),
        )
        .padding([2, 2])
        .into(),
        None if state.remote_error.is_some() => container(
            text(state.remote_error.clone().unwrap_or_default())
                .size(10)
                .color(tokens.deleted),
        )
        .padding([2, 2])
        .into(),
        None => container(
            text("Loading remote branches…")
                .size(10)
                .color(tokens.muted_foreground),
        )
        .padding([2, 2])
        .into(),
    };

    column![header, rows].spacing(2).width(Length::Fill).into()
}

fn remote_branch_row<'a>(branch: &'a RemoteBranchInfo, tokens: Tokens) -> Element<'a, Message> {
    let full = format!("{}/{}", branch.remote, branch.name);
    let mut line = row![
        text(full.clone())
            .size(11)
            .font(theme::mono())
            .color(tokens.foreground),
        text(super::repo_view::short_sha(&branch.sha))
            .size(9)
            .font(theme::mono())
            .color(tokens.muted_foreground),
        Space::new().width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    line = line.push(
        button(text("Checkout").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(58.0))
            .on_press(Message::RemoteBranchCheckout {
                remote: branch.remote.clone(),
                name: branch.name.clone(),
            }),
    );
    line = line.push(
        button(text("Copy").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(40.0))
            .on_press(Message::CopyText(full)),
    );
    line = line.push(
        button(text("Delete").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(48.0))
            .on_press(Message::RemoteBranchDeleteRequested {
                remote: branch.remote.clone(),
                name: branch.name.clone(),
            }),
    );

    container(line).padding([2, 4]).width(Length::Fill).into()
}
