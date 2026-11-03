//! Modal confirmation overlay: a centered card over a dimmed scrim.
//!
//! The scrim is a [`mouse_area`], so presses outside the card dismiss the
//! dialog and never reach the workspace underneath.

use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{Alignment, Background, Border, Element, Length};

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
