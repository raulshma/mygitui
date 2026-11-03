//! Stash and checkpoint panel: WIP stack on top, undo snapshots below.
//!
//! Opened from the repo header and rendered as an overlay (like the branch
//! picker) so it never reflows the workspace.

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{CheckpointInfo, StashInfo};

use super::app::Message;
use super::history_model::relative_age;
use super::repo_view::{short_sha, Tab};
use super::theme::{self, Tokens};

/// Load state and form buffers for the stash/checkpoint panel.
#[derive(Debug, Default)]
pub struct StashesState {
    pub open: bool,
    pub loading: bool,
    pub list: Option<Vec<StashInfo>>,
    pub error: Option<String>,
    pub message: String,
    pub include_untracked: bool,
    pub checkpoints: Option<Vec<CheckpointInfo>>,
    pub checkpoints_loading: bool,
    pub checkpoints_error: Option<String>,
}

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let state = &tab.stashes;

    let stash_count = state.list.as_ref().map(|list| list.len());
    let checkpoint_count = state.checkpoints.as_ref().map(|list| list.len());

    let mut header = row![text("Stash & checkpoints").size(12).font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
    if let Some(count) = stash_count {
        header = header.push(
            text(format!("{count} stashed"))
                .size(10)
                .color(tokens.muted_foreground),
        );
    }
    if state.loading || state.checkpoints_loading {
        header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
    }
    header = header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text("⟳").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::RefreshStashes),
        )
        .push(
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::ToggleStashPanel),
        );

    let push_form = row![
        text_input("Stash message (optional)", state.message.as_str())
            .on_input(Message::StashMessageChanged)
            .on_submit(Message::StashPush)
            .size(11)
            .padding([4, 6])
            .width(Length::Fill),
        button(text("✨").size(10))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .on_press(Message::AiGenerate(super::app::AiAction::StashMessage)),
        checkbox(state.include_untracked)
            .label("untracked")
            .size(12)
            .text_size(10)
            .spacing(4)
            .on_toggle(Message::StashIncludeUntracked),
        button(text("Stash").size(10))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 10])
            .on_press(Message::StashPush),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let stash_rows: Element<'a, Message> = match &state.list {
        Some(list) if !list.is_empty() => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            for stash in list {
                rows = rows.push(stash_row(stash, tokens));
            }
            let height = (list.len() as f32 * 26.0).clamp(26.0, 180.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => note(tokens, "No stashes."),
        None if state.error.is_some() => note(tokens, state.error.clone().unwrap_or_default()),
        None => note(tokens, "Loading stashes…"),
    };

    let mut checkpoint_header = row![text(format!(
        "Checkpoints ({})",
        checkpoint_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "…".into())
    ))
    .size(10)
    .color(tokens.muted_foreground),]
    .align_y(Alignment::Center);

    if state.checkpoints_loading {
        checkpoint_header = checkpoint_header
            .push(Space::new().width(6))
            .push(text("· loading…").size(10).color(tokens.muted_foreground));
    }
    checkpoint_header = checkpoint_header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text("Undo point").size(9))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([2, 8])
                .on_press(Message::CheckpointCreate),
        );

    let checkpoint_rows: Element<'a, Message> = match &state.checkpoints {
        Some(list) if !list.is_empty() => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            let now = now_seconds();
            for checkpoint in list {
                rows = rows.push(checkpoint_row(checkpoint, now, tokens));
            }
            let height = (list.len() as f32 * 26.0).clamp(26.0, 180.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => note(
            tokens,
            "No checkpoints yet. Restoring one snapshots the current state first.",
        ),
        None if state.checkpoints_error.is_some() => {
            note(tokens, state.checkpoints_error.clone().unwrap_or_default())
        }
        None => note(tokens, "Loading checkpoints…"),
    };

    container(
        column![
            header,
            push_form,
            stash_rows,
            checkpoint_header,
            checkpoint_rows
        ]
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

fn stash_row<'a>(stash: &'a StashInfo, tokens: Tokens) -> Element<'a, Message> {
    let mut info = column![
        text(clip(&stash.message, 46))
            .size(11)
            .color(tokens.foreground),
        text(format!(
            "stash@{{{}}} · {} · {}",
            stash.index,
            short_sha(&stash.sha),
            stash.author.name
        ))
        .size(9)
        .color(tokens.muted_foreground),
    ]
    .spacing(0)
    .width(Length::Fill);

    if stash.message.trim().is_empty() {
        info = column![
            text("(no message)").size(11).color(tokens.muted_foreground),
            text(format!(
                "stash@{{{}}} · {}",
                stash.index,
                short_sha(&stash.sha)
            ))
            .size(9)
            .color(tokens.muted_foreground),
        ]
        .spacing(0)
        .width(Length::Fill);
    }

    let row = row![
        info,
        button(text("Apply").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(46.0))
            .on_press(Message::StashApply {
                index: stash.index,
                pop: false,
            }),
        button(text("Pop").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([2, 6])
            .width(Length::Fixed(38.0))
            .on_press(Message::StashApply {
                index: stash.index,
                pop: true,
            }),
        button(text("✕").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(24.0))
            .on_press(Message::StashDropRequested(stash.index)),
    ]
    .spacing(2)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(row).padding([2, 4]).width(Length::Fill).into()
}

fn checkpoint_row<'a>(
    checkpoint: &'a CheckpointInfo,
    now: i64,
    tokens: Tokens,
) -> Element<'a, Message> {
    let mut label = row![
        text(clip(&checkpoint.reason, 30))
            .size(11)
            .color(tokens.foreground),
        Space::new().width(6),
        text(relative_age(checkpoint.created_at, now))
            .size(9)
            .color(tokens.muted_foreground),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    if let Some(branch) = checkpoint.branch.as_deref() {
        label = label.push(Space::new().width(6)).push(
            text(format!("on {branch}"))
                .size(9)
                .color(tokens.muted_foreground),
        );
    }

    let row = row![
        label,
        button(text("Restore").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(56.0))
            .on_press(Message::CheckpointRestoreRequested {
                id: checkpoint.id.clone(),
                reason: checkpoint.reason.clone(),
            }),
    ]
    .spacing(2)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(row).padding([2, 4]).width(Length::Fill).into()
}

fn note<'a>(tokens: Tokens, label: impl Into<String>) -> Element<'a, Message> {
    container(text(label.into()).size(10).color(tokens.muted_foreground))
        .padding([4, 4])
        .into()
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clip_keeps_short_and_truncates_long() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("0123456789", 5), "0123…");
    }
}
