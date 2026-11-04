//! Modal confirmation overlay: a centered card over a dimmed scrim.
//!
//! The scrim is a [`mouse_area`], so presses outside the card dismiss the
//! dialog and never reach the workspace underneath.

use iced::widget::{
    button, checkbox, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{MergeFavor, MergeOptions, PreviewInfo, ResetKind};
use crate::service::files::GitignoreTemplate;

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

/// Pending pull from the remotes panel: remote plus its options.
#[derive(Debug, Clone, Default)]
pub struct PullRequest {
    pub remote: String,
    /// Branch to pull; empty pulls the upstream of the current branch.
    pub branch: String,
    /// Rebase instead of merge (`git pull --rebase`).
    pub rebase: bool,
    /// Fail instead of creating a merge when not fast-forward.
    pub ff_only: bool,
}

/// Pending push from the remotes panel: remote plus its options.
#[derive(Debug, Clone, Default)]
pub struct PushRequest {
    pub remote: String,
    /// Plain force (`+` refspec) — mutually exclusive with `force_with_lease`.
    pub force: bool,
    /// `--force-with-lease`: fails when the remote moved since the last
    /// fetch (mutually exclusive with `force`).
    pub force_with_lease: bool,
    /// Push every local tag (`--tags`).
    pub tags: bool,
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

/// Centered modal with per-remote pull options: optional branch (empty =
/// the current branch's upstream), rebase and ff-only.
pub fn pull<'a>(tokens: Tokens, request: &PullRequest) -> Element<'a, Message> {
    let body = column![
        text(format!("Pull from {}", request.remote))
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text("Leave the branch empty to pull the upstream of the current branch.")
            .size(12)
            .color(tokens.muted_foreground),
        text_input("branch (optional)", request.branch.as_str())
            .on_input(Message::PullBranchChanged)
            .on_submit(Message::PullConfirmed)
            .size(11)
            .padding([4, 6]),
        checkbox(request.rebase)
            .label("Rebase instead of merge")
            .size(12)
            .text_size(11)
            .spacing(4)
            .on_toggle(Message::PullRebaseToggled),
        checkbox(request.ff_only)
            .label("Fast-forward only")
            .size(12)
            .text_size(11)
            .spacing(4)
            .on_toggle(Message::PullFfOnlyToggled),
    ]
    .spacing(6);

    modal(
        tokens,
        400.0,
        body.into(),
        action_row(tokens, "Pull", Some(Message::PullConfirmed)),
    )
}

/// Centered modal with per-remote push options: force / force-with-lease
/// (mutually exclusive — the update arms clear the counterpart) and tags.
pub fn push<'a>(tokens: Tokens, request: &PushRequest) -> Element<'a, Message> {
    let body = column![
        text(format!("Push to {}", request.remote))
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text("Pushes the current branch and sets its upstream when missing.")
            .size(12)
            .color(tokens.muted_foreground),
        checkbox(request.force)
            .label("Force")
            .size(12)
            .text_size(11)
            .spacing(4)
            .on_toggle(Message::PushForceToggled),
        checkbox(request.force_with_lease)
            .label("Force with lease (fails if the remote moved since the last fetch)")
            .size(12)
            .text_size(11)
            .spacing(4)
            .on_toggle(Message::PushForceLeaseToggled),
        checkbox(request.tags)
            .label("Push all tags")
            .size(12)
            .text_size(11)
            .spacing(4)
            .on_toggle(Message::PushTagsToggled),
    ]
    .spacing(6);

    modal(
        tokens,
        440.0,
        body.into(),
        action_row(tokens, "Push", Some(Message::PushConfirmed)),
    )
}

// ---------------------------------------------------------------------------
// Gitignore template gallery
// ---------------------------------------------------------------------------

/// Pending gitignore template gallery: the search filter over the builtin
/// list. Applying stays open so several templates can land in one pass.
#[derive(Debug, Clone, Default)]
pub struct IgnoreTemplates {
    pub filter: String,
}

/// Case-insensitive substring filter over template name + description.
pub fn filter_templates<'a>(
    list: &'a [GitignoreTemplate],
    needle: &str,
) -> Vec<&'a GitignoreTemplate> {
    let needle = needle.trim().to_lowercase();
    list.iter()
        .filter(|template| {
            needle.is_empty()
                || template.name.to_lowercase().contains(&needle)
                || template.description.to_lowercase().contains(&needle)
        })
        .collect()
}

/// Centered modal gallery of the builtin gitignore templates: a search box
/// over name + description and one Apply row per template.
pub fn templates<'a>(tokens: Tokens, state: &IgnoreTemplates) -> Element<'a, Message> {
    let list = crate::service::files::gitignore_templates_list();
    let matches = filter_templates(&list, &state.filter);

    let mut rows = column![].spacing(2).width(Length::Fill);
    for template in &matches {
        // The gallery is rebuilt per frame, so row labels clone out of the
        // local list (matching the other dialogs' owned-string discipline).
        rows = rows.push(
            row![
                column![
                    text(template.name.clone()).size(11).font(theme::semibold()),
                    text(template.description.clone())
                        .size(10)
                        .color(tokens.muted_foreground),
                ]
                .spacing(1)
                .width(Length::Fill),
                button(text("Apply").size(10))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([3, 8])
                    .on_press(Message::IgnoreTemplateApplied(template.name.clone())),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        );
    }

    let body = column![
        text("Ignore templates").size(14).font(theme::semibold()),
        Space::new().height(4),
        text(
            "Applying appends a template's patterns under a `# <name>` header \
             to the repo-root .gitignore."
        )
        .size(12)
        .color(tokens.muted_foreground),
        text_input("Search templates", state.filter.as_str())
            .on_input(Message::IgnoreTemplatesFilterChanged)
            .size(11)
            .padding([4, 6]),
        if matches.is_empty() {
            Element::<Message>::from(
                text("No templates match.")
                    .size(11)
                    .color(tokens.muted_foreground),
            )
        } else {
            scrollable(rows)
                .height(Length::Fixed(320.0))
                .width(Length::Fill)
                .into()
        },
    ]
    .spacing(6);

    let actions: Element<'a, Message> = row![
        Space::new().width(Length::Fill),
        button(text("Close").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(Message::CancelDialog),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into();

    modal(tokens, 460.0, body.into(), actions)
}

// ---------------------------------------------------------------------------
// Tag at commit
// ---------------------------------------------------------------------------

/// Pending "tag this commit" dialog: the target sha plus the create form.
#[derive(Debug, Clone, Default)]
pub struct TagAtRequest {
    pub sha: String,
    pub name: String,
    pub annotated: bool,
    pub message: String,
}

impl TagAtRequest {
    /// Same rules as the tags panel: a name is required and an annotated
    /// tag needs a message.
    pub fn confirm_ready(&self) -> bool {
        if self.name.trim().is_empty() {
            return false;
        }
        !self.annotated || !self.message.trim().is_empty()
    }

    /// `(target, message)` as the service call wants them.
    pub fn service_args(&self) -> (String, Option<String>) {
        let message = if self.annotated {
            Some(self.message.trim().to_string())
        } else {
            None
        };
        (self.name.trim().to_string(), message)
    }
}

/// Centered modal creating a tag at a commit (name, annotated toggle +
/// message; the target is fixed to the sha the menu was opened on).
pub fn tag_at<'a>(tokens: Tokens, request: &TagAtRequest) -> Element<'a, Message> {
    let mut body = column![
        text(format!("Tag {}", super::repo_view::short_sha(&request.sha)))
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text("Create a tag pointing at this commit.")
            .size(12)
            .color(tokens.muted_foreground),
        text_input("Tag name", request.name.as_str())
            .on_input(Message::TagAtNameChanged)
            .size(11)
            .padding([4, 6]),
        checkbox(request.annotated)
            .label("Annotated")
            .size(12)
            .text_size(11)
            .spacing(4)
            .on_toggle(Message::TagAtAnnotatedToggled),
    ]
    .spacing(6);

    if request.annotated {
        body = body.push(
            text_input("Tag message", request.message.as_str())
                .on_input(Message::TagAtMessageChanged)
                .size(11)
                .padding([4, 6]),
        );
    }

    modal(
        tokens,
        400.0,
        body.into(),
        action_row(
            tokens,
            "Create tag",
            request.confirm_ready().then_some(Message::TagAtConfirmed),
        ),
    )
}

// ---------------------------------------------------------------------------
// Branch rename
// ---------------------------------------------------------------------------

/// Pending branch rename: the current name plus the edited target (the
/// input starts prefilled with the current name).
#[derive(Debug, Clone, Default)]
pub struct RenameRequest {
    pub old: String,
    pub new: String,
}

impl RenameRequest {
    /// The confirm unlocks only for a real change (non-empty and different
    /// from the current name).
    pub fn confirm_ready(&self) -> bool {
        let new = self.new.trim();
        !new.is_empty() && new != self.old.trim()
    }
}

/// Centered modal renaming a branch: one text input prefilled with the
/// current name.
pub fn rename<'a>(tokens: Tokens, request: &RenameRequest) -> Element<'a, Message> {
    let submit = if request.confirm_ready() {
        Message::BranchRenameConfirmed
    } else {
        Message::Noop
    };
    let body = column![
        text("Rename branch").size(14).font(theme::semibold()),
        Space::new().height(4),
        text(format!("Rename “{}” to:", request.old))
            .size(12)
            .color(tokens.muted_foreground),
        text_input("new-branch-name", request.new.as_str())
            .on_input(Message::BranchRenameChanged)
            .on_submit(submit)
            .size(11)
            .padding([4, 6]),
    ]
    .spacing(6);

    modal(
        tokens,
        400.0,
        body.into(),
        action_row(
            tokens,
            "Rename",
            request
                .confirm_ready()
                .then_some(Message::BranchRenameConfirmed),
        ),
    )
}

// ---------------------------------------------------------------------------
// Clean untracked files
// ---------------------------------------------------------------------------

/// Selections larger than this require typing `clean` before the confirm
/// button unlocks.
pub const CLEAN_TYPE_TO_CONFIRM_THRESHOLD: usize = 10;

/// Pending `git clean` of untracked paths: the paths from the status state,
/// per-file checkboxes (all on by default) and the typed confirmation for
/// large selections.
#[derive(Debug, Clone, Default)]
pub struct CleanRequest {
    pub paths: Vec<String>,
    /// Parallel to `paths`; every entry starts checked.
    pub checked: Vec<bool>,
    pub confirm_text: String,
}

impl CleanRequest {
    pub fn new(paths: Vec<String>) -> Self {
        let checked = vec![true; paths.len()];
        Self {
            paths,
            checked,
            confirm_text: String::new(),
        }
    }

    pub fn is_checked(&self, path: &str) -> bool {
        self.paths
            .iter()
            .position(|candidate| candidate == path)
            .and_then(|index| self.checked.get(index))
            .copied()
            .unwrap_or(false)
    }

    pub fn toggle(&mut self, path: &str) {
        if let Some(index) = self.paths.iter().position(|candidate| candidate == path) {
            if let Some(flag) = self.checked.get_mut(index) {
                *flag = !*flag;
            }
        }
    }

    /// The selected paths, in the original (status) order.
    pub fn selected(&self) -> Vec<String> {
        self.paths
            .iter()
            .zip(&self.checked)
            .filter(|(_, checked)| **checked)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn needs_typed_confirmation(&self) -> bool {
        self.selected().len() > CLEAN_TYPE_TO_CONFIRM_THRESHOLD
    }

    /// Confirm unlocks when anything is selected and — for large
    /// selections — the typed text matches `clean` (case-insensitive).
    pub fn confirm_ready(&self) -> bool {
        if self.selected().is_empty() {
            return false;
        }
        !self.needs_typed_confirmation() || self.confirm_text.trim().eq_ignore_ascii_case("clean")
    }
}

/// Paths grouped by directory (first-appearance order; the root group is
/// `""`). Backs the dialog's per-directory sections.
pub fn group_by_dir(paths: &[String]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for path in paths {
        let (dir, _) = super::status_model::split_path(path);
        let dir = dir.to_string();
        match groups.iter_mut().find(|(group, _)| *group == dir) {
            Some((_, members)) => members.push(path.clone()),
            None => groups.push((dir, vec![path.clone()])),
        }
    }
    groups
}

/// Centered modal deleting checked untracked paths: per-directory groups
/// with checkboxes (all on), and a type-`clean` guard for large selections.
/// The backend snapshots a checkpoint before deleting, so the dialog says
/// so instead of demanding the discard-style "cannot be undone" wording.
pub fn clean<'a>(tokens: Tokens, request: &CleanRequest) -> Element<'a, Message> {
    let selected = request.selected();
    let mut intro = column![
        text("Clean untracked files").size(14).font(theme::semibold()),
        Space::new().height(4),
        text(format!(
            "Delete {} of {} untracked path{}? A checkpoint of the current state is saved first, so this can be undone from the checkpoints panel.",
            selected.len(),
            request.paths.len(),
            if request.paths.len() == 1 { "" } else { "s" }
        ))
        .size(12)
        .color(tokens.muted_foreground),
    ]
    .spacing(6);

    let mut list = column![].spacing(1).width(Length::Fill);
    for (dir, members) in group_by_dir(&request.paths) {
        list = list.push(
            text(if dir.is_empty() {
                "(root)".to_string()
            } else {
                dir
            })
            .size(10)
            .font(theme::semibold())
            .color(tokens.muted_foreground),
        );
        for path in members {
            list = list.push(
                checkbox(request.is_checked(&path))
                    .label(path.clone())
                    .size(12)
                    .text_size(11)
                    .spacing(6)
                    .on_toggle(move |_| Message::CleanToggled(path.clone())),
            );
        }
    }
    intro = intro.push(
        scrollable(list)
            .height(Length::Fixed(280.0))
            .width(Length::Fill),
    );

    if request.needs_typed_confirmation() {
        intro = intro.push(
            column![
                text(format!(
                    "Type “clean” to confirm deleting {} paths.",
                    selected.len()
                ))
                .size(11)
                .color(tokens.deleted),
                text_input("clean", request.confirm_text.as_str())
                    .on_input(Message::CleanConfirmChanged)
                    .size(11)
                    .padding([4, 6]),
            ]
            .spacing(4),
        );
    }

    modal(
        tokens,
        460.0,
        intro.into(),
        action_row(
            tokens,
            "Clean",
            request.confirm_ready().then_some(Message::CleanConfirmed),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_filter_matches_name_or_description_case_insensitively() {
        let list = crate::service::files::gitignore_templates_list();
        assert_eq!(filter_templates(&list, "").len(), list.len());
        assert_eq!(filter_templates(&list, "  ").len(), list.len());
        let rust = filter_templates(&list, "rust");
        assert_eq!(rust.len(), 1);
        assert_eq!(rust[0].name, "Rust");
        assert_eq!(filter_templates(&list, "NODE").len(), 1);
        // Descriptions match too ("Finder metadata" → macOS).
        let macos = filter_templates(&list, "finder metadata");
        assert_eq!(macos.len(), 1);
        assert_eq!(macos[0].name, "macOS");
        assert!(filter_templates(&list, "zzz-no-match").is_empty());
    }

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

    #[test]
    fn clean_selection_starts_all_checked_and_toggles() {
        let mut request = CleanRequest::new(vec!["a.txt".into(), "b.log".into(), "c/".into()]);
        assert_eq!(request.selected(), vec!["a.txt", "b.log", "c/"]);
        request.toggle("b.log");
        assert_eq!(request.selected(), vec!["a.txt", "c/"]);
        assert!(!request.is_checked("b.log"));
        request.toggle("b.log");
        assert!(request.is_checked("b.log"));
        // Unknown paths toggle nothing.
        request.toggle("missing");
        assert_eq!(request.selected().len(), 3);
    }

    #[test]
    fn clean_confirm_gate_needs_typing_only_for_big_selections() {
        let paths: Vec<String> = (0..10).map(|index| format!("f{index}.txt")).collect();
        let mut request = CleanRequest::new(paths);
        assert!(!request.needs_typed_confirmation());
        assert!(request.confirm_ready(), "small selections confirm freely");

        // Empty selection never confirms.
        for path in request.paths.clone() {
            request.toggle(&path);
        }
        assert!(request.selected().is_empty());
        assert!(!request.confirm_ready());
        for index in 0..10 {
            request.toggle(&format!("f{index}.txt"));
        }

        // 11 selected (threshold 10): typing is required.
        request.paths.push("extra.txt".into());
        request.checked.push(true);
        assert!(request.needs_typed_confirmation());
        assert!(!request.confirm_ready());
        request.confirm_text = "nope".into();
        assert!(!request.confirm_ready());
        request.confirm_text = "  Clean ".into();
        assert!(
            request.confirm_ready(),
            "match is trimmed + case-insensitive"
        );
    }

    #[test]
    fn group_by_dir_keeps_first_appearance_order() {
        let paths = vec![
            "src/b.rs".to_string(),
            "a.txt".to_string(),
            "src/c.rs".to_string(),
            "build/".to_string(),
            "d.log".to_string(),
        ];
        let groups = group_by_dir(&paths);
        assert_eq!(
            groups,
            vec![
                (
                    "src/".to_string(),
                    vec!["src/b.rs".to_string(), "src/c.rs".to_string()]
                ),
                (
                    "".to_string(),
                    vec!["a.txt".to_string(), "d.log".to_string()]
                ),
                ("build/".to_string(), vec!["build/".to_string()]),
            ]
        );
        assert!(group_by_dir(&[]).is_empty());
    }
}
