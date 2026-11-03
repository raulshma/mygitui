//! Repository screen: tab strip, repo header and the working-copy status list.
//!
//! The diff viewer, history graph and the remaining panels land in the next
//! wave; this screen already exercises the full service/bus path (open →
//! status → watcher refresh).

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Color, Element, Length};

use crate::engine::types::{ChangeKind, RepoId, RepoInfo, RepoStatus, StatusEntry};

use super::app::Message;
use super::theme::{self, Tokens};

/// Load state for data fetched through the service layer.
#[derive(Debug, Clone)]
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

/// One open repository (a tab).
#[derive(Debug, Clone)]
pub struct Tab {
    pub repo: RepoInfo,
    pub status: Load<RepoStatus>,
    pub selected: Option<String>,
}

impl Tab {
    pub fn new(repo: RepoInfo) -> Self {
        Self {
            repo,
            status: Load::Loading,
            selected: None,
        }
    }
}

pub fn view<'a>(tabs: &'a [Tab], active: usize, tokens: Tokens) -> Element<'a, Message> {
    let Some(tab) = tabs.get(active) else {
        return Space::new().into();
    };

    column![
        tab_strip(tabs, active, tokens),
        repo_header(tab, tokens),
        repo_body(tab, tokens),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
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
        .and_then(|status| status.branch.as_deref())
        .unwrap_or(if tab.repo.bare { "bare" } else { "detached" });

    let mut sync = row![].spacing(6).align_y(Alignment::Center);
    if let Some(status) = status {
        if status.ahead > 0 {
            sync = sync.push(chip(format!("↑{}", status.ahead), tokens.modified));
        }
        if status.behind > 0 {
            sync = sync.push(chip(format!("↓{}", status.behind), tokens.renamed));
        }
    }

    let refresh = button(text("Refresh").size(11))
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([6, 10])
        .on_press(Message::Refresh(tab.repo.repo_id.clone()));

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
            chip(branch, tokens.muted_foreground),
            sync,
            Space::new().width(Length::Fill),
            refresh,
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

fn repo_body<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    row![status_panel(tab, tokens), detail_panel(tab, tokens),]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn status_panel<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let body: Element<'a, Message> = match &tab.status {
        Load::Loading => padded_note(tokens, "Loading status…"),
        Load::Failed(err) => padded_note(tokens, err),
        Load::Ready(status) if status.entries.is_empty() => {
            padded_note(tokens, "Working tree clean.")
        }
        Load::Ready(status) => {
            let mut list = column![].spacing(1).width(Length::Fill);
            for entry in &status.entries {
                list = list.push(status_row(tokens, entry, tab.selected.as_deref()));
            }
            scrollable(list)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    };

    let header = container(
        row![
            text("Changes").size(12).font(theme::semibold()),
            Space::new().width(Length::Fill),
            text(counts_label(tab.status.ready()))
                .size(11)
                .color(tokens.muted_foreground),
        ]
        .align_y(Alignment::Center)
        .padding([8, 10]),
    );

    container(column![header, body].height(Length::Fill))
        .width(380.0)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(tokens.sidebar)),
            border: iced::Border {
                color: tokens.sidebar_border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn status_row<'a>(
    tokens: Tokens,
    entry: &'a StatusEntry,
    selected: Option<&str>,
) -> Element<'a, Message> {
    let is_selected = selected == Some(entry.path.as_str());
    let path = match &entry.old_path {
        Some(old) => format!("{old} → {}", entry.path),
        None => entry.path.clone(),
    };

    let content = row![
        badge(tokens, entry.index, true),
        badge(tokens, entry.worktree, false),
        text(path)
            .size(12)
            .font(theme::mono())
            .color(tokens.foreground)
            .width(Length::Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    button(
        container(content)
            .width(Length::Fill)
            .align_x(Alignment::Start),
    )
    .style(theme::button_style(
        tokens,
        if is_selected {
            theme::ButtonKind::Secondary
        } else {
            theme::ButtonKind::Ghost
        },
    ))
    .padding([4, 8])
    .width(Length::Fill)
    .on_press(Message::SelectFile(entry.path.clone()))
    .into()
}

fn detail_panel<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let entry = match (&tab.status, &tab.selected) {
        (Load::Ready(status), Some(path)) => {
            status.entries.iter().find(|entry| &entry.path == path)
        }
        _ => None,
    };

    let content: Element<'a, Message> = match (tab.selected.as_deref(), entry) {
        (Some(path), Some(entry)) => column![
            text(path).size(13).font(theme::mono()),
            Space::new().height(6),
            text(format!(
                "index: {}   ·   worktree: {}",
                kind_label(entry.index),
                kind_label(entry.worktree)
            ))
            .size(11)
            .color(tokens.muted_foreground),
            Space::new().height(24),
            text("The line-granular diff viewer arrives with the next wave.")
                .size(12)
                .color(tokens.muted_foreground),
        ]
        .into(),
        (Some(path), None) => column![
            text(path).size(13).font(theme::mono()),
            Space::new().height(8),
            text("This path is no longer present in the working tree.")
                .size(12)
                .color(tokens.muted_foreground),
        ]
        .into(),
        _ => column![
            text("Select a file").size(13).font(theme::semibold()),
            Space::new().height(6),
            text("Changed files appear on the left. The diff, staging controls, history graph and remote panels land in the next waves.")
                .size(12)
                .color(tokens.muted_foreground),
        ]
        .into(),
    };

    container(content)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(tokens.background)),
            ..container::Style::default()
        })
        .into()
}

fn padded_note<'a>(tokens: Tokens, note: &'a str) -> Element<'a, Message> {
    container(text(note).size(12).color(tokens.muted_foreground))
        .padding([16, 12])
        .into()
}

fn chip<'a>(label: impl Into<String>, color: Color) -> Element<'a, Message> {
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

fn badge<'a>(tokens: Tokens, kind: ChangeKind, staged: bool) -> Element<'a, Message> {
    let letter = kind_letter(kind);
    let color = if letter.is_empty() {
        tokens.muted_foreground
    } else {
        kind_color(tokens, kind, staged)
    };
    container(text(letter).size(10).font(theme::mono()).color(color))
        .width(14)
        .align_x(Alignment::Center)
        .into()
}

fn kind_letter(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Unmodified | ChangeKind::Ignored => "",
        ChangeKind::Added => "A",
        ChangeKind::Deleted => "D",
        ChangeKind::Modified => "M",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::Untracked => "U",
        ChangeKind::Conflicted => "!",
    }
}

fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Unmodified => "unmodified",
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Modified => "modified",
        ChangeKind::Renamed => "renamed",
        ChangeKind::Copied => "copied",
        ChangeKind::Untracked => "untracked",
        ChangeKind::Conflicted => "conflicted",
        ChangeKind::Ignored => "ignored",
    }
}

fn kind_color(tokens: Tokens, kind: ChangeKind, staged: bool) -> Color {
    match kind {
        ChangeKind::Added if staged => tokens.added,
        ChangeKind::Added => tokens.untracked,
        ChangeKind::Untracked => tokens.untracked,
        ChangeKind::Deleted => tokens.deleted,
        ChangeKind::Modified => tokens.modified,
        ChangeKind::Renamed | ChangeKind::Copied => tokens.renamed,
        ChangeKind::Conflicted => tokens.conflicted,
        ChangeKind::Unmodified | ChangeKind::Ignored => tokens.muted_foreground,
    }
}

fn counts_label(status: Option<&RepoStatus>) -> String {
    let Some(status) = status else {
        return String::new();
    };
    let staged = status
        .entries
        .iter()
        .filter(|entry| changed(entry.index))
        .count();
    let unstaged = status
        .entries
        .iter()
        .filter(|entry| changed(entry.worktree))
        .count();
    format!("{staged} staged · {unstaged} unstaged")
}

fn changed(kind: ChangeKind) -> bool {
    !matches!(kind, ChangeKind::Unmodified | ChangeKind::Ignored)
}

/// Exposed for the tab strip: finds a tab by repo id.
pub fn tab_index(tabs: &[Tab], repo_id: &RepoId) -> Option<usize> {
    tabs.iter().position(|tab| &tab.repo.repo_id == repo_id)
}
