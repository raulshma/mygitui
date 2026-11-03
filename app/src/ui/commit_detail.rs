//! Commit detail: metadata header plus the read-only diff of the selected
//! commit against its first parent.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Border, Element, Length};

use super::app::Message;
use super::diff_view::{self, DiffSlot, PaneMode};
use super::history_model::relative_age;
use super::repo_view::{chip, short_sha, Tab};
use super::theme::{self, Tokens};

fn clip_ref(reference: &str) -> String {
    const MAX: usize = 18;
    if reference.chars().count() <= MAX {
        return reference.to_string();
    }
    let head: String = reference.chars().take(MAX - 1).collect();
    format!("{head}…")
}

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let Some(commit) = tab.history.selected_commit() else {
        return container(
            text("Select a commit in the history list.")
                .size(12)
                .color(tokens.muted_foreground),
        )
        .padding(20)
        .into();
    };

    let age = relative_age(
        commit.author.time,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0),
    );

    let mut meta = row![
        text(short_sha(&commit.sha))
            .size(10)
            .font(theme::mono())
            .color(tokens.muted_foreground),
        Space::new().width(8),
        text(commit.author.name.as_str())
            .size(11)
            .color(tokens.foreground),
        text(format!("<{}>", commit.author.email))
            .size(10)
            .color(tokens.muted_foreground),
        Space::new().width(8),
        text(age).size(10).color(tokens.muted_foreground),
        Space::new().width(Length::Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    for reference in commit.refs.iter().take(3) {
        meta = meta.push(chip(clip_ref(reference), tokens.primary));
        meta = meta.push(Space::new().width(4));
    }

    let summary = if commit.summary.is_empty() {
        "(no message)".to_string()
    } else {
        commit.summary.clone()
    };

    // Close lives in the title row: refs on the right of `meta` can consume
    // the whole width.
    let title = row![
        text(summary)
            .size(14)
            .font(theme::semibold())
            .width(Length::Fill),
        button(text("Reset…").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 10])
            .on_press(Message::ResetRequested(commit.sha.clone())),
        button(text("Close").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 10])
            .on_press(Message::CloseDetail),
    ]
    .align_y(Alignment::Center);

    let message = container(
        scrollable(
            text(commit.message.trim_end())
                .size(11)
                .font(theme::mono())
                .color(tokens.foreground),
        )
        .width(Length::Fill),
    )
    .max_height(120.0)
    .width(Length::Fill);

    let is_root = commit.parents.is_empty();
    let body: Element<'a, Message> = if is_root {
        container(
            text("Root commit — there is no parent to diff against.")
                .size(11)
                .color(tokens.muted_foreground),
        )
        .padding(16)
        .into()
    } else {
        diff_view::view(
            &tab.commit_diff,
            DiffSlot::Commit,
            tokens,
            tab.diff_split,
            tab.diff_wrap,
            PaneMode::default(),
        )
    };

    let header = container(
        column![
            title,
            Space::new().height(3),
            meta,
            Space::new().height(6),
            message,
        ]
        .width(Length::Fill),
    )
    .padding([10, 12])
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(tokens.card)),
        border: Border {
            color: tokens.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    column![header, body]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
