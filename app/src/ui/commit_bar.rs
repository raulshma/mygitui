//! Commit form pinned under the status list: message editor, amend /
//! skip-hooks toggles and the commit button.

use iced::widget::{button, checkbox, column, container, row, text, text_editor, Space};
use iced::{Alignment, Border, Element, Length};

use super::app::Message;
use super::repo_view::Tab;
use super::status_model::has_staged_changes;
use super::theme::{self, Tokens};

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let staged = tab
        .status
        .ready()
        .map(|status| {
            status
                .entries
                .iter()
                .filter(|entry| {
                    !matches!(
                        entry.index,
                        crate::engine::types::ChangeKind::Unmodified
                            | crate::engine::types::ChangeKind::Ignored
                    )
                })
                .count()
        })
        .unwrap_or(0);
    let staged_label = if has_staged_changes(
        tab.status
            .ready()
            .map(|status| status.entries.as_slice())
            .unwrap_or(&[]),
    ) {
        format!("{} staged", staged)
    } else {
        "nothing staged".to_string()
    };

    let editor = text_editor(&tab.message)
        .placeholder("Commit message")
        .on_action(Message::CommitMessageChanged)
        .size(12)
        .padding(8)
        .height(Length::Fixed(104.0));

    let mut toggles = row![
        checkbox(tab.amend)
            .label("Amend")
            .text_size(11)
            .size(14)
            .spacing(5)
            .on_toggle(Message::CommitAmend),
        Space::new().width(10),
        checkbox(tab.no_verify)
            .label("Skip hooks")
            .text_size(11)
            .size(14)
            .spacing(5)
            .on_toggle(Message::CommitNoVerify),
        Space::new().width(Length::Fill),
    ]
    .align_y(Alignment::Center);

    let label = if tab.committing {
        "Committing…".to_string()
    } else if tab.amend {
        "Amend".to_string()
    } else {
        "Commit".to_string()
    };
    let enabled = tab.can_commit();
    let commit = button(text(label).size(12).color(tokens.primary_foreground))
        .style(theme::button_style(
            tokens,
            if enabled {
                theme::ButtonKind::Primary
            } else {
                theme::ButtonKind::Secondary
            },
        ))
        .padding([6, 16])
        .on_press_maybe(enabled.then_some(Message::CommitPressed));
    toggles = toggles.push(commit);

    let mut form = column![
        row![
            text("Commit").size(12).font(theme::semibold()),
            Space::new().width(Length::Fill),
            text(staged_label).size(10).color(tokens.muted_foreground),
        ]
        .align_y(Alignment::Center),
        Space::new().height(4),
        editor,
        Space::new().height(4),
        toggles,
    ]
    .spacing(2)
    .width(Length::Fill);

    if let Some(error) = &tab.commit_error {
        form = form.push(Space::new().height(4));
        form = form.push(text(error).size(10).color(tokens.deleted));
    }

    container(form)
        .padding([8, 10])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(tokens.sidebar)),
            border: Border {
                color: tokens.sidebar_border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}
