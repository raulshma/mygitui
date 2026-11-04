//! Modal confirmation overlay: a centered card over a dimmed scrim.
//!
//! The scrim is a [`mouse_area`], so presses outside the card dismiss the
//! dialog and never reach the workspace underneath.

use iced::widget::{button, checkbox, column, container, mouse_area, row, text, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{MergeFavor, MergeOptions, PreviewInfo, ResetKind};

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
    /// `ops_preview` result for worktree-discarding kinds (loaded async;
    /// `None` while loading or when the mode keeps the files).
    pub preview: Option<PreviewInfo>,
}

/// Pending merge awaiting its confirm dialog: the branch being merged into
/// the current one plus the options the backend `merge_branch` supports.
#[derive(Debug, Clone)]
pub struct MergeRequest {
    /// Branch being merged (not the branch merged into).
    pub name: String,
    /// Force a merge commit even when a fast-forward is possible (`--no-ff`).
    pub no_ff: bool,
    /// Stage the merge result without committing (`--squash`).
    pub squash: bool,
    /// Stop before creating the merge commit (`--no-commit`).
    pub no_commit: bool,
    /// Auto-resolve conflicting hunks favoring one side (`-X ours|theirs`).
    pub favor: MergeFavor,
}

impl MergeRequest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            no_ff: false,
            squash: false,
            no_commit: false,
            favor: MergeFavor::None,
        }
    }

    /// The backend options these dialog fields map to.
    pub fn options(&self) -> MergeOptions {
        MergeOptions {
            no_ff: self.no_ff,
            squash: self.squash,
            no_commit: self.no_commit,
            favor: self.favor,
        }
    }
}

/// Reset modes that discard worktree changes (and therefore get a preview).
pub fn reset_kind_discards_worktree(kind: ResetKind) -> bool {
    matches!(kind, ResetKind::Hard | ResetKind::Keep | ResetKind::Merge)
}

/// Detail box content for a destructive-op preview: the op summary plus up
/// to `max` affected paths (with an "and N more" tail).
pub fn preview_detail(preview: &PreviewInfo, max: usize) -> String {
    let mut lines = vec![preview.summary.clone()];
    for file in preview.files.iter().take(max) {
        lines.push(format!("{}  {}", file.change, file.path));
    }
    if preview.files.len() > max {
        lines.push(format!("… and {} more", preview.files.len() - max));
    }
    lines.join("\n")
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

    let mut body = column![
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

    if reset_kind_discards_worktree(request.kind) {
        match &request.preview {
            Some(preview) => {
                body = body.push(
                    container(
                        text(preview_detail(preview, 5))
                            .size(11)
                            .font(theme::mono())
                            .color(if preview.files.is_empty() {
                                tokens.muted_foreground
                            } else {
                                tokens.deleted
                            }),
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
            None => {
                body = body.push(
                    text("Checking what would be discarded…")
                        .size(11)
                        .color(tokens.muted_foreground),
                );
            }
        }
    }

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

/// Shared modal scaffold for the option cards below: a centered card over a
/// dimmed scrim; pressing outside the card dispatches [`Message::CancelDialog`].
fn modal<'a>(
    tokens: Tokens,
    width: f32,
    body: Element<'a, Message>,
    actions: Element<'a, Message>,
) -> Element<'a, Message> {
    let card = container(column![body, Space::new().height(8), actions].spacing(6))
        .width(Length::Fixed(width))
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

/// One select line of an option card: radio dot + label + description.
fn option_line<'a>(
    tokens: Tokens,
    selected: bool,
    label: &'a str,
    description: &'a str,
    on_press: Message,
) -> Element<'a, Message> {
    button(
        row![
            text(if selected { "●" } else { "○" })
                .size(10)
                .color(if selected {
                    tokens.primary
                } else {
                    tokens.muted_foreground
                }),
            text(label).size(11).color(tokens.foreground),
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
    .on_press(on_press)
    .into()
}

/// Card action row: spacer + Cancel + the primary confirm button. The
/// confirm press is optional (the merge card drops it while dirty).
fn action_row<'a>(
    tokens: Tokens,
    confirm_label: &str,
    on_confirm: Option<Message>,
) -> Element<'a, Message> {
    row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(Message::CancelDialog),
        button(text(confirm_label.to_string()).size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 12])
            .on_press_maybe(on_confirm),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Centered modal confirming a merge into the current branch: the options
/// the backend `merge_branch` supports (ff policy, squash, `--no-commit`,
/// `-X ours|theirs`) and a dirty guard — uncommitted staged+unstaged
/// changes disable the confirm button.
pub fn merge<'a>(
    tokens: Tokens,
    request: &MergeRequest,
    head: Option<&str>,
    dirty: bool,
) -> Element<'a, Message> {
    let mut options = column![].spacing(4);
    for (no_ff, label, description) in [
        (
            false,
            "Fast-forward when possible",
            "just move the branch when it can",
        ),
        (true, "Always create a merge commit", "--no-ff"),
    ] {
        options = options.push(option_line(
            tokens,
            request.no_ff == no_ff,
            label,
            description,
            Message::MergeNoFfSelected(no_ff),
        ));
    }

    let mut favor = row![text("Conflicting hunks:")
        .size(10)
        .color(tokens.muted_foreground)]
    .spacing(4)
    .align_y(Alignment::Center);
    for (kind, label) in [
        (MergeFavor::None, "leave conflicts"),
        (MergeFavor::Ours, "-X ours"),
        (MergeFavor::Theirs, "-X theirs"),
    ] {
        favor = favor.push(
            button(text(label).size(10))
                .style(theme::button_style(
                    tokens,
                    if request.favor == kind {
                        theme::ButtonKind::Secondary
                    } else {
                        theme::ButtonKind::Ghost
                    },
                ))
                .padding([2, 6])
                .on_press(Message::MergeFavorSelected(kind)),
        );
    }

    let mut body = column![
        text(format!("Merge {}", request.name))
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text(match head {
            Some(head) => format!("Merge “{}” into “{head}”?", request.name),
            None => format!("Merge “{}” into the current branch?", request.name),
        })
        .size(12)
        .color(tokens.muted_foreground),
        Space::new().height(6),
        options,
        row![
            checkbox(request.squash)
                .label("Squash (--squash)")
                .size(12)
                .text_size(10)
                .spacing(4)
                .on_toggle(Message::MergeSquashToggled),
            checkbox(request.no_commit)
                .label("Stop before committing (--no-commit)")
                .size(12)
                .text_size(10)
                .spacing(4)
                .on_toggle(Message::MergeNoCommitToggled),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        favor,
    ]
    .spacing(6);

    if dirty {
        body = body.push(
            text("Stash your changes first")
                .size(10)
                .color(tokens.deleted),
        );
    }

    modal(
        tokens,
        440.0,
        body.into(),
        action_row(tokens, "Merge", (!dirty).then_some(Message::MergeConfirmed)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(files: &[&str]) -> PreviewInfo {
        PreviewInfo {
            kind: "reset_hard".to_string(),
            summary: "Hard reset to abc1234".to_string(),
            files: files
                .iter()
                .map(|path| crate::engine::types::PreviewFile {
                    path: (*path).to_string(),
                    change: "modified".to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn preview_detail_lists_files_and_caps() {
        let info = preview(&["a.txt", "b.txt"]);
        assert_eq!(
            preview_detail(&info, 5),
            "Hard reset to abc1234\nmodified  a.txt\nmodified  b.txt"
        );
        let info = preview(&["a.txt", "b.txt", "c.txt"]);
        assert!(preview_detail(&info, 2).ends_with("… and 1 more"));
    }

    #[test]
    fn only_worktree_discarding_reset_kinds_get_a_preview() {
        assert!(reset_kind_discards_worktree(ResetKind::Hard));
        assert!(reset_kind_discards_worktree(ResetKind::Keep));
        assert!(reset_kind_discards_worktree(ResetKind::Merge));
        assert!(!reset_kind_discards_worktree(ResetKind::Soft));
        assert!(!reset_kind_discards_worktree(ResetKind::Mixed));
    }
}
