//! Modal confirmation overlay: a centered card over a dimmed scrim.
//!
//! The scrim is a [`mouse_area`], so presses outside the card dismiss the
//! dialog and never reach the workspace underneath.

use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::ResetKind;

use super::app::Message;
use super::theme::{self, Tokens};

/// Everything a confirmation card shows.
pub struct Confirm {
    pub title: String,
    pub body: String,
    /// Extra monospace lines (affected files, branch name, ...).
    pub detail: Option<String>,
    pub confirm_label: String,
    pub danger: bool,
}

/// Pending `git reset` from the commit detail view.
#[derive(Debug, Clone)]
pub struct ResetRequest {
    pub sha: String,
    pub summary: String,
    pub kind: ResetKind,
}

const RESET_KINDS: [(ResetKind, &str, &str); 5] = [
    (
        ResetKind::Soft,
        "Soft",
        "Move HEAD only; index and files stay.",
    ),
    (
        ResetKind::Mixed,
        "Mixed",
        "Move HEAD and reset the index; files stay.",
    ),
    (
        ResetKind::Hard,
        "Hard",
        "Move HEAD and discard index + file changes.",
    ),
    (
        ResetKind::Keep,
        "Keep",
        "Move HEAD; unrelated local edits survive.",
    ),
    (
        ResetKind::Merge,
        "Merge",
        "Mixed reset that aborts on conflicting edits.",
    ),
];

/// Short label for a reset mode (used in notices and buttons).
pub fn reset_kind_label(kind: ResetKind) -> &'static str {
    match kind {
        ResetKind::Soft => "soft",
        ResetKind::Mixed => "mixed",
        ResetKind::Hard => "hard",
        ResetKind::Keep => "keep",
        ResetKind::Merge => "merge",
    }
}

/// Centered modal asking which reset mode to use for a commit.
pub fn reset<'a>(tokens: Tokens, request: &ResetRequest) -> Element<'a, Message> {
    let mut options = column![].spacing(4);
    for (kind, label, description) in RESET_KINDS {
        let selected = request.kind == kind;
        let kind_color = if kind == ResetKind::Hard {
            tokens.deleted
        } else {
            tokens.foreground
        };
        options = options.push(
            button(
                row![
                    text(if selected { "●" } else { "○" })
                        .size(10)
                        .color(if selected {
                            tokens.primary
                        } else {
                            tokens.muted_foreground
                        }),
                    text(label).size(11).color(kind_color),
                    text(description).size(10).color(tokens.muted_foreground),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .style(theme::button_style(
                tokens,
                if selected {
                    theme::ButtonKind::Secondary
                } else {
                    theme::ButtonKind::Ghost
                },
            ))
            .padding([4, 8])
            .width(Length::Fill)
            .on_press(Message::ResetKindSelected(kind)),
        );
    }

    let body = column![
        text(format!("Reset to {}", request.summary))
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text(format!(
            "Move the current branch to {}. A checkpoint of the current state is saved first.",
            &request.sha[..request.sha.len().min(7)]
        ))
        .size(12)
        .color(tokens.muted_foreground),
        Space::new().height(6),
        options,
    ]
    .spacing(2);

    let confirm_label = match request.kind {
        ResetKind::Soft => "Reset (soft)",
        ResetKind::Mixed => "Reset (mixed)",
        ResetKind::Hard => "Reset (hard)",
        ResetKind::Keep => "Reset (keep)",
        ResetKind::Merge => "Reset (merge)",
    };
    let danger = request.kind == ResetKind::Hard;

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(Message::CancelDialog),
        button(text(confirm_label).size(11))
            .style(theme::button_style(
                tokens,
                if danger {
                    theme::ButtonKind::Danger
                } else {
                    theme::ButtonKind::Primary
                },
            ))
            .padding([4, 12])
            .on_press(Message::ResetConfirmed),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let card = container(column![body, Space::new().height(8), actions].spacing(6))
        .width(Length::Fixed(440.0))
        .padding([16, 18])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.card)),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        });

    let scrim = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(
                iced::Color::BLACK,
                0.4,
            ))),
            ..container::Style::default()
        });

    mouse_area(scrim).on_press(Message::CancelDialog).into()
}

/// Full-window overlay with a centered confirmation card.
pub fn confirm<'a>(
    tokens: Tokens,
    spec: &Confirm,
    cancel: Message,
    confirm: Message,
) -> Element<'a, Message> {
    let mut body = column![
        text(spec.title.clone()).size(14).font(theme::semibold()),
        Space::new().height(4),
        text(spec.body.clone())
            .size(12)
            .color(tokens.muted_foreground),
    ]
    .spacing(2);

    if let Some(detail) = &spec.detail {
        body = body.push(
            container(
                text(detail.clone())
                    .size(11)
                    .font(theme::mono())
                    .color(tokens.foreground),
            )
            .padding([6, 8])
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(Background::Color(tokens.muted)),
                border: Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            }),
        );
    }

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(cancel.clone()),
        button(text(spec.confirm_label.clone()).size(11))
            .style(theme::button_style(
                tokens,
                if spec.danger {
                    theme::ButtonKind::Danger
                } else {
                    theme::ButtonKind::Primary
                },
            ))
            .padding([4, 12])
            .on_press(confirm),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let card = container(column![body, Space::new().height(8), actions].spacing(6))
        .width(Length::Fixed(400.0))
        .padding([16, 18])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.card)),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        });

    let scrim = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(
                iced::Color::BLACK,
                0.4,
            ))),
            ..container::Style::default()
        });

    mouse_area(scrim).on_press(cancel).into()
}
