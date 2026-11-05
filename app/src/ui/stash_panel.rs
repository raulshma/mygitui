//! Stash and checkpoint panel: WIP stack on top, undo snapshots below.
//!
//! Opened from the repo header and rendered as an overlay (like the branch
//! picker) so it never reflows the workspace.

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{BranchTrashEntry, CheckpointInfo, StashInfo};

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
    /// `git stash push --keep-index`: leave the staged changes in the index.
    pub keep_index: bool,
    pub checkpoints: Option<Vec<CheckpointInfo>>,
    pub checkpoints_loading: bool,
    pub checkpoints_error: Option<String>,
    /// Branches preserved under `refs/mygitui/trash/*` (deleted-branch
    /// trash), loaded alongside the checkpoints.
    pub trash: Option<Vec<BranchTrashEntry>>,
    pub trash_loading: bool,
    pub trash_error: Option<String>,
}

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let state = &tab.stashes;

    let stash_count = state.list.as_ref().map(|list| list.len());
    let checkpoint_count = state.checkpoints.as_ref().map(|list| list.len());

    let mut header = row![text("Stash & checkpoints")
        .size(theme::TEXT_BODY)
        .font(theme::semibold())]
    .spacing(6)
    .align_y(Alignment::Center);
    if let Some(count) = stash_count {
        header = header.push(
            text(format!("{count} stashed"))
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }
    if state.loading || state.checkpoints_loading {
        header = header.push(
            text("· loading…")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }
    header = header
        .push(Space::new().width(Length::Fill))
        .push(theme::mini_button(
            tokens,
            theme::icon::REFRESH,
            "Refresh stashes",
            Some(Message::RefreshStashes),
        ))
        .push(theme::mini_button(
            tokens,
            theme::icon::CLOSE,
            "Close panel",
            Some(Message::ToggleStashPanel),
        ));

    // Stash push has nothing to do on a clean tree — guard the button and
    // say why instead of letting the op fail confusingly.
    let dirty = tab.working_tree_dirty();

    let push_form = row![
        text_input("Stash message (optional)", state.message.as_str())
            .on_input(Message::StashMessageChanged)
            .on_submit(if dirty {
                Message::StashPush
            } else {
                Message::Noop
            })
            .size(theme::TEXT_SMALL)
            .padding([4, 6])
            .width(Length::Fill),
        theme::mini_button(
            tokens,
            theme::icon::AI,
            "Generate stash message with AI",
            Some(Message::AiGenerate(super::app::AiAction::StashMessage)),
        ),
        checkbox(state.include_untracked)
            .label("untracked")
            .size(theme::TEXT_BODY)
            .text_size(theme::TEXT_META)
            .spacing(4)
            .on_toggle(Message::StashIncludeUntracked),
        checkbox(state.keep_index)
            .label("Keep staged changes")
            .size(theme::TEXT_BODY)
            .text_size(theme::TEXT_META)
            .spacing(4)
            .on_toggle(Message::StashKeepIndex),
        button(text("Stash").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 10])
            .on_press_maybe(dirty.then_some(Message::StashPush)),
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
        Some(_) => theme::note(tokens, "No stashes.", false, theme::NoteKind::Empty),
        None if state.error.is_some() => theme::note(
            tokens,
            state.error.clone().unwrap_or_default(),
            true,
            theme::NoteKind::Empty,
        ),
        None => theme::note(tokens, "Loading stashes…", false, theme::NoteKind::Empty),
    };

    let mut checkpoint_header = row![text(format!(
        "Checkpoints ({})",
        checkpoint_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "…".into())
    ))
    .size(theme::TEXT_META)
    .color(tokens.muted_foreground),]
    .align_y(Alignment::Center);

    if state.checkpoints_loading {
        checkpoint_header = checkpoint_header.push(Space::new().width(6)).push(
            text("· loading…")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }
    checkpoint_header = checkpoint_header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text("Undo point").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([2, 8])
                .on_press(Message::CheckpointCreate),
        )
        .push(
            button(text("GC").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::CheckpointGc),
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
        Some(_) => theme::note(
            tokens,
            "No checkpoints yet. Restoring one snapshots the current state first.",
            false,
            theme::NoteKind::Empty,
        ),
        None if state.checkpoints_error.is_some() => theme::note(
            tokens,
            state.checkpoints_error.clone().unwrap_or_default(),
            true,
            theme::NoteKind::Empty,
        ),
        None => theme::note(
            tokens,
            "Loading checkpoints…",
            false,
            theme::NoteKind::Empty,
        ),
    };

    let trash_count = state.trash.as_ref().map(|list| list.len());
    let mut trash_header = row![text(format!(
        "Deleted branches ({})",
        trash_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "…".into())
    ))
    .size(theme::TEXT_META)
    .color(tokens.muted_foreground)]
    .align_y(Alignment::Center);
    if state.trash_loading {
        trash_header = trash_header.push(Space::new().width(6)).push(
            text("· loading…")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }
    trash_header = trash_header
        .push(Space::new().width(Length::Fill))
        .push(theme::mini_button(
            tokens,
            theme::icon::REFRESH,
            "Refresh deleted branches",
            Some(Message::RefreshTrash),
        ));

    let trash_rows: Element<'a, Message> = match &state.trash {
        Some(list) if !list.is_empty() => {
            let mut rows = column![].spacing(1).width(Length::Fill);
            let now = now_seconds();
            for entry in list {
                rows = rows.push(trash_row(entry, now, tokens));
            }
            let height = (list.len() as f32 * 26.0).clamp(26.0, 120.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => theme::note(
            tokens,
            "No deleted branches. Deleting a branch keeps a restore point here.",
            false,
            theme::NoteKind::Empty,
        ),
        None if state.trash_error.is_some() => theme::note(
            tokens,
            state.trash_error.clone().unwrap_or_default(),
            true,
            theme::NoteKind::Empty,
        ),
        None => theme::note(
            tokens,
            "Loading deleted branches…",
            false,
            theme::NoteKind::Empty,
        ),
    };

    // One guard line at a time: why Stash is disabled (clean tree), or what
    // Apply/Pop will do to uncommitted work (dirty tree).
    let mut content = column![header, push_form].spacing(6);
    if dirty {
        content = content.push(
            text("Applying will mix with uncommitted changes")
                .size(theme::TEXT_META)
                .color(tokens.modified),
        );
    } else {
        content = content.push(
            text("Nothing to stash — the working tree is clean.")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }
    let content = content
        .push(stash_rows)
        .push(checkpoint_header)
        .push(checkpoint_rows)
        .push(trash_header)
        .push(trash_rows)
        .padding([8, 10]);

    container(content)
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
            .size(theme::TEXT_SMALL)
            .color(tokens.foreground),
        text(format!(
            "stash@{{{}}} · {} · {}",
            stash.index,
            short_sha(&stash.sha),
            stash.author.name
        ))
        .size(theme::TEXT_META)
        .color(tokens.muted_foreground),
    ]
    .spacing(0)
    .width(Length::Fill);

    if stash.message.trim().is_empty() {
        info = column![
            text("(no message)")
                .size(theme::TEXT_SMALL)
                .color(tokens.muted_foreground),
            text(format!(
                "stash@{{{}}} · {}",
                stash.index,
                short_sha(&stash.sha)
            ))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
        ]
        .spacing(0)
        .width(Length::Fill);
    }

    let row = row![
        info,
        button(text("Apply").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(46.0))
            .on_press(Message::StashApply {
                index: stash.index,
                pop: false,
            }),
        button(text("Pop").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([2, 6])
            .width(Length::Fixed(38.0))
            .on_press(Message::StashApply {
                index: stash.index,
                pop: true,
            }),
        theme::mini_button(
            tokens,
            theme::icon::CLOSE,
            "Drop stash",
            Some(Message::StashDropRequested(stash.index)),
        ),
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
            .size(theme::TEXT_SMALL)
            .color(tokens.foreground),
        Space::new().width(6),
        text(relative_age(checkpoint.created_at, now))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    if let Some(branch) = checkpoint.branch.as_deref() {
        label = label.push(Space::new().width(6)).push(
            text(format!("on {branch}"))
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }

    let row = row![
        label,
        button(text("Restore").size(theme::TEXT_META))
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

fn trash_row<'a>(entry: &'a BranchTrashEntry, now: i64, tokens: Tokens) -> Element<'a, Message> {
    let row = row![
        column![
            text(clip(&entry.name, 30))
                .size(theme::TEXT_SMALL)
                .color(tokens.foreground),
            text(format!(
                "{} · {}",
                relative_age(entry.deleted_at, now),
                short_sha(&entry.sha)
            ))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
        ]
        .spacing(0)
        .width(Length::Fill),
        button(text("Restore").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(56.0))
            .on_press(Message::TrashRestore(entry.id.clone())),
    ]
    .spacing(2)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(row).padding([2, 4]).width(Length::Fill).into()
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
