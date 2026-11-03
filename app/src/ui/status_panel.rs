//! Source-control sidebar: filter, staged/changes sections and row actions.
//!
//! Rows come from [`status_model`]; selection and section collapsing live on
//! [`Tab`], so this module only renders and emits messages.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::engine::types::{ChangeKind, ConflictResolution};

use super::app::Message;
use super::repo_view::{Load, Tab};
use super::status_model::{
    filter_sections, kind_letter, selection_actions, SectionId, SectionView, StatusRow,
};
use super::theme::{self, Tokens};

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let summary = match &tab.status {
        Load::Loading => "loading…".to_string(),
        Load::Failed(_) => "error".to_string(),
        Load::Ready(_) => {
            let staged = tab.status_index.section_len(SectionId::Staged);
            let changes = tab.status_index.section_len(SectionId::Changes);
            format!("{staged} staged · {changes} changes")
        }
    };

    let header = container(
        row![
            text("Source Control").size(12).font(theme::semibold()),
            Space::new().width(Length::Fill),
            text(summary).size(10).color(tokens.muted_foreground),
        ]
        .align_y(Alignment::Center)
        .padding([8, 10]),
    );

    let filter = container(
        row![
            text_input("Filter files", tab.filter.as_str())
                .on_input(Message::StatusFilterChanged)
                .size(11)
                .padding([5, 8]),
            mini(tokens, "All", "Select all visible files").on_press(Message::StatusSelectAll),
            mini(tokens, "None", "Clear selection").on_press(Message::StatusSelectNone),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([0, 8])
    .width(Length::Fill);

    let body: Element<'a, Message> = match &tab.status {
        Load::Loading => note(tokens, "Loading status…"),
        Load::Failed(error) => note(tokens, error.clone()),
        Load::Ready(_) => sections(tab, tokens),
    };

    let mut panel = column![header, filter, selection_bar(tab, tokens)]
        .width(Length::Fill)
        .height(Length::Fill);
    panel = panel.push(body);

    panel.into()
}

fn sections<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let views = filter_sections(&tab.status_index, &tab.filter);
    if tab.status_index.total() == 0 {
        return note(tokens, "Working tree clean.");
    }
    if views.is_empty() {
        return note(tokens, "No files match the filter.");
    }

    let mut list = column![].spacing(1).width(Length::Fill);
    for section in &views {
        list = list.push(section_header(tab, tokens, section));
        if !tab.section_collapsed(section.id) {
            for status_row in &section.rows {
                list = list.push(row_view(tab, tokens, status_row));
            }
        }
    }

    scrollable(list)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn section_header<'a>(
    tab: &'a Tab,
    tokens: Tokens,
    section: &SectionView<'a>,
) -> Element<'a, Message> {
    let collapsed = tab.section_collapsed(section.id);
    let chevron = if collapsed { "▸" } else { "▾" };
    let label = format!("{chevron} {} ({})", section.id.label(), section.rows.len());

    let mut header = row![
        button(text(label).size(11).font(theme::semibold()))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([5, 6])
            .on_press(Message::ToggleSection(section.id)),
        Space::new().width(Length::Fill),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let paths: Vec<String> = section.rows.iter().map(|row| row.path.clone()).collect();
    if !paths.is_empty() {
        let (label, unstage) = match section.id {
            SectionId::Staged => ("Unstage all", true),
            SectionId::Changes => ("Stage all", false),
        };
        header = header.push(
            button(text(label).size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([3, 6])
                .on_press(Message::StagePaths { paths, unstage }),
        );
    }

    container(header)
        .padding([2, 4])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.06))),
            ..container::Style::default()
        })
        .into()
}

fn row_view<'a>(tab: &'a Tab, tokens: Tokens, item: &'a StatusRow) -> Element<'a, Message> {
    let selected = tab.selection.is_selected(&item.key);
    let current = tab.selected_row.as_deref() == Some(item.key.as_str());
    let color = kind_color(tokens, item.kind);

    let mut label = column![text(clip(&item.base, 34)).size(12).font(theme::mono())]
        .spacing(0)
        .width(Length::Fill);
    if !item.dir.is_empty() {
        label = label.push(
            text(clip(&item.dir, 42))
                .size(10)
                .color(tokens.muted_foreground),
        );
    }

    let accent = container(Space::new().width(2).height(Length::Fill)).style(move |_theme| {
        container::Style {
            background: Some(Background::Color(if current {
                tokens.primary
            } else {
                theme::with_alpha(tokens.primary, 0.0)
            })),
            ..container::Style::default()
        }
    });

    let mut content = row![
        accent,
        badge(item.kind, color),
        label,
        row_actions(tokens, item),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    if item.index == ChangeKind::Conflicted || item.worktree == ChangeKind::Conflicted {
        content = content
            .push(container(text("resolve").size(9).color(tokens.conflicted)).padding([0, 4]));
    }

    let background = if current {
        theme::with_alpha(tokens.primary, 0.12)
    } else if selected {
        theme::with_alpha(tokens.primary, 0.07)
    } else {
        tokens.sidebar
    };

    button(container(content).padding([3, 6]).width(Length::Fill))
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
        .on_press(Message::StatusSelect {
            key: item.key.clone(),
        })
        .into()
}

fn row_actions<'a>(tokens: Tokens, item: &'a StatusRow) -> Element<'a, Message> {
    if item.index == ChangeKind::Conflicted || item.worktree == ChangeKind::Conflicted {
        return Space::new().width(0).into();
    }

    let mut actions = row![].spacing(2).align_y(Alignment::Center);
    match item.section {
        SectionId::Staged => {
            actions = actions.push(mini(tokens, "−", "Unstage file").on_press(
                Message::StageFile {
                    path: item.path.clone(),
                    unstage: true,
                },
            ));
        }
        SectionId::Changes => {
            actions = actions.push(
                mini(tokens, "＋", "Stage file").on_press(Message::StageFile {
                    path: item.path.clone(),
                    unstage: false,
                }),
            );
            actions = actions.push(
                mini(tokens, "↺", "Discard changes")
                    .on_press(Message::DiscardPath(item.path.clone())),
            );
        }
    }
    actions.into()
}

fn mini<'a>(tokens: Tokens, glyph: &'a str, _tip: &'a str) -> iced::widget::Button<'a, Message> {
    button(text(glyph).size(11).color(tokens.muted_foreground))
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([1, 5])
}

fn selection_bar<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let count = tab.selection.count();
    if count == 0 {
        return Space::new().height(0).into();
    }

    let rows: Vec<&StatusRow> = tab
        .status_index
        .rows
        .iter()
        .filter(|row| tab.selection.is_selected(&row.key))
        .collect();
    let actions = selection_actions(&rows);

    let mut bar = row![
        text(format!("{count} selected"))
            .size(10)
            .color(tokens.muted_foreground),
        Space::new().width(6),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    if !actions.stage.is_empty() {
        bar = bar.push(
            button(text(format!("Stage {}", actions.stage.len())).size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([2, 6])
                .on_press(Message::StagePaths {
                    paths: actions.stage,
                    unstage: false,
                }),
        );
    }
    if !actions.unstage.is_empty() {
        bar = bar.push(
            button(text(format!("Unstage {}", actions.unstage.len())).size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([2, 6])
                .on_press(Message::StagePaths {
                    paths: actions.unstage,
                    unstage: true,
                }),
        );
    }
    if !actions.discard.is_empty() {
        bar = bar.push(
            button(text(format!("Discard {}", actions.discard.len())).size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                .padding([2, 6])
                .on_press(Message::DiscardPaths(actions.discard)),
        );
    }

    let conflicts: Vec<String> = rows
        .iter()
        .filter(|row| {
            matches!(row.index, ChangeKind::Conflicted)
                || matches!(row.worktree, ChangeKind::Conflicted)
        })
        .map(|row| row.path.clone())
        .collect();
    if !conflicts.is_empty() {
        for (label, resolution) in [
            ("Ours", ConflictResolution::Ours),
            ("Theirs", ConflictResolution::Theirs),
            ("Both", ConflictResolution::Both),
        ] {
            bar = bar.push(
                button(text(label).size(10).color(tokens.conflicted))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([2, 6])
                    .on_press(Message::ConflictResolve {
                        paths: conflicts.clone(),
                        resolution,
                    }),
            );
        }
    }

    bar = bar.push(Space::new().width(Length::Fill));
    bar = bar.push(
        button(text("✕").size(10))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .on_press(Message::StatusSelectNone),
    );

    container(bar)
        .padding([3, 8])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.primary, 0.08))),
            border: Border {
                color: theme::with_alpha(tokens.primary, 0.2),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn badge<'a>(kind: ChangeKind, color: Color) -> Element<'a, Message> {
    container(
        text(kind_letter(kind))
            .size(9)
            .font(theme::mono())
            .color(color),
    )
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(16.0))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(theme::with_alpha(color, 0.14))),
        border: Border {
            color: theme::with_alpha(color, 0.3),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn note<'a>(tokens: Tokens, note: impl Into<String>) -> Element<'a, Message> {
    container(text(note.into()).size(11).color(tokens.muted_foreground))
        .padding([16, 12])
        .into()
}

pub fn kind_color(tokens: Tokens, kind: ChangeKind) -> Color {
    match kind {
        ChangeKind::Added | ChangeKind::Untracked => tokens.untracked,
        ChangeKind::Deleted => tokens.deleted,
        ChangeKind::Renamed | ChangeKind::Copied => tokens.renamed,
        ChangeKind::Conflicted => tokens.conflicted,
        ChangeKind::Unmodified | ChangeKind::Ignored => tokens.muted_foreground,
        _ => tokens.modified,
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .skip(count.saturating_sub(max_chars.saturating_sub(1)))
        .collect();
    format!("…{tail}")
}
