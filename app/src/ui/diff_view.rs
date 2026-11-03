//! Virtualized diff viewer. Rows come from the pure [`diff_model`]; the
//! scrollable area renders only the visible slices (spacers preserve the
//! total height) unless word wrap is on, in which case rows flow naturally.

use std::collections::{HashMap, HashSet};

use iced::widget::{
    button, column, container, image, mouse_area, row, scrollable, text, Id, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::engine::types::{DiffLine, FileDiff};

use super::app::Message;
use super::diff_model::{
    build_layout, build_row_model, file_change_kind, file_kind_letter, is_context, visible_slices,
    DiffRow, DiffRowModel, LineSelection, RowLayout,
};
use super::status_model::kind_label;
use super::theme::{self, Tokens};

/// Which pane owns a diff (working copy vs selected commit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSlot {
    Working,
    Commit,
}

#[derive(Debug, Clone, Default)]
pub enum DiffContent {
    #[default]
    Empty,
    Loading,
    Ready(Vec<FileDiff>),
    Failed(String),
}

impl DiffContent {
    pub fn files(&self) -> &[FileDiff] {
        match self {
            Self::Ready(files) => files,
            _ => &[],
        }
    }
}

/// Decoded image previews for one binary-image file, keyed by path on the
/// pane. A side is `None` when that side has no content (added/deleted) or
/// its bytes failed to load — the row renders a placeholder there.
#[derive(Debug, Clone, Default)]
pub struct DiffImages {
    pub old: Option<image::Handle>,
    pub new: Option<image::Handle>,
}

/// Per-pane diff state: payload, derived rows and view-only options.
#[derive(Debug, Default)]
pub struct DiffPane {
    pub content: DiffContent,
    pub model: DiffRowModel,
    pub layout: RowLayout,
    pub collapsed: HashSet<usize>,
    pub lines: LineSelection,
    pub scroll: f32,
    pub viewport: f32,
    /// Identity of the shown diff; a new key resets the scroll state.
    pub key: Option<String>,
    /// Guards asynchronous loads against stale results.
    pub seq: u64,
    /// Decoded image previews keyed by file path (cleared when the diff
    /// identity changes; the shell populates via async blob reads).
    pub images: HashMap<String, DiffImages>,
}

impl DiffPane {
    pub fn files(&self) -> &[FileDiff] {
        self.content.files()
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.content, DiffContent::Ready(_))
    }

    pub fn set_loading(&mut self, key: String) {
        // Keep the line selection while the same diff reloads (watcher
        // refreshes); a new target starts from scratch.
        if self.key.as_deref() != Some(key.as_str()) {
            self.model = DiffRowModel::default();
            self.layout = RowLayout::default();
            self.lines.clear();
            self.scroll = 0.0;
        }
        self.content = DiffContent::Loading;
        self.key = Some(key);
    }

    pub fn set_failed(&mut self, error: String) {
        self.content = DiffContent::Failed(error);
        self.model = DiffRowModel::default();
        self.layout = RowLayout::default();
    }

    pub fn set_ready(&mut self, files: Vec<FileDiff>, split: bool) {
        self.content = DiffContent::Ready(files);
        self.images.clear();
        self.rebuild(split);
        self.lines.clear();
    }

    pub fn rebuild(&mut self, split: bool) {
        self.model = build_row_model(self.files(), split, &self.collapsed);
        self.layout = build_layout(&self.model.rows);
    }

    pub fn toggle_file(&mut self, file: usize, split: bool) {
        if !self.collapsed.remove(&file) {
            self.collapsed.insert(file);
        }
        self.rebuild(split);
    }
}

/// What the toolbar and row controls may do.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneMode {
    /// Show the per-file stage/unstage control.
    pub file_actions: bool,
    /// Show hunk and line staging controls (only safe when the file has no
    /// changes on the opposite side).
    pub line_actions: bool,
    /// Controls remove from the index instead of adding to it.
    pub unstage: bool,
}

/// Render one diff pane. `title` heads the toolbar (path or commit summary).
pub fn view<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    split: bool,
    wrap: bool,
    mode: PaneMode,
) -> Element<'a, Message> {
    match &pane.content {
        DiffContent::Empty => note(tokens, "Select a file or commit to see its diff."),
        DiffContent::Loading => note(tokens, "Loading diff…"),
        DiffContent::Failed(error) => note_error(tokens, error),
        DiffContent::Ready(files) if files.is_empty() => note(tokens, "No changes."),
        DiffContent::Ready(files) => {
            let toolbar = toolbar(pane, slot, tokens, split, wrap, mode, files);
            let body = if wrap {
                wrapped_rows(pane, slot, tokens, split, mode, files)
            } else {
                virtual_rows(pane, slot, tokens, split, mode, files)
            };
            column![toolbar, body]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }
}

fn toolbar<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    split: bool,
    wrap: bool,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    let mut stats = row![].spacing(8).align_y(Alignment::Center);
    stats = stats.push(
        text(format!("+{}", pane.model.total_additions))
            .size(11)
            .color(tokens.added),
    );
    stats = stats.push(
        text(format!("−{}", pane.model.total_deletions))
            .size(11)
            .color(tokens.deleted),
    );
    stats = stats.push(
        text(if files.len() == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", files.len())
        })
        .size(11)
        .color(tokens.muted_foreground),
    );

    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    if mode.line_actions && !pane.lines.is_empty() {
        actions = actions.push(
            button(
                text(format!(
                    "{} {} line{}",
                    if mode.unstage { "Unstage" } else { "Stage" },
                    pane.lines.len(),
                    if pane.lines.len() == 1 { "" } else { "s" }
                ))
                .size(11),
            )
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([3, 8])
            .on_press(Message::StageSelectedLines { slot }),
        );
        actions = actions.push(
            button(text("Clear").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([3, 8])
                .on_press(Message::ClearLineSelection { slot }),
        );
    }
    actions = actions.push(
        button(text(if split { "Split" } else { "Unified" }).size(11))
            .style(theme::button_style(
                tokens,
                if split {
                    theme::ButtonKind::Secondary
                } else {
                    theme::ButtonKind::Ghost
                },
            ))
            .padding([3, 8])
            .on_press(Message::DiffToggleSplit),
    );
    actions = actions.push(
        button(text("Wrap").size(11))
            .style(theme::button_style(
                tokens,
                if wrap {
                    theme::ButtonKind::Secondary
                } else {
                    theme::ButtonKind::Ghost
                },
            ))
            .padding([3, 8])
            .on_press(Message::DiffToggleWrap),
    );

    container(
        row![stats, Space::new().width(Length::Fill), actions]
            .align_y(Alignment::Center)
            .padding([4, 8]),
    )
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(tokens.card)),
        border: Border {
            color: tokens.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn virtual_rows<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    split: bool,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    let viewport = if pane.viewport > 0.0 {
        pane.viewport
    } else {
        600.0
    };
    let slices = visible_slices(&pane.layout, pane.scroll, viewport, 480.0);
    let mut content = column![].width(Length::Fill);
    let mut cursor = 0.0f32;

    for slice in slices {
        if slice.top > cursor {
            content = content.push(Space::new().height(Length::Fixed(slice.top - cursor)));
        }
        let end = (slice.first_row + slice.count).min(pane.model.rows.len());
        for row in &pane.model.rows[slice.first_row..end] {
            content = content.push(fixed_row(
                *row,
                render_row(*row, pane, slot, tokens, split, mode, files),
            ));
        }
        cursor = slice.top + slice.height;
    }
    if cursor < pane.layout.total_height {
        content =
            content.push(Space::new().height(Length::Fixed(pane.layout.total_height - cursor)));
    }

    scrollable(content)
        .id(Id::from(scroll_key(pane, slot, "v")))
        .on_scroll(move |viewport| Message::DiffScrolled {
            slot,
            y: viewport.absolute_offset().y,
            viewport: viewport.bounds().height,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn wrapped_rows<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    split: bool,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    const WRAP_CAP: usize = 4_000;
    let mut content = column![].width(Length::Fill);
    for row in pane.model.rows.iter().take(WRAP_CAP) {
        content = content.push(render_row(*row, pane, slot, tokens, split, mode, files));
    }
    if pane.model.rows.len() > WRAP_CAP {
        content = content.push(note(
            tokens,
            "Wrap mode renders at most 4000 rows; switch to the virtualized view for the rest.",
        ));
    }
    scrollable(content)
        .id(Id::from(scroll_key(pane, slot, "w")))
        .on_scroll(move |viewport| Message::DiffScrolled {
            slot,
            y: viewport.absolute_offset().y,
            viewport: viewport.bounds().height,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn scroll_key(pane: &DiffPane, slot: DiffSlot, mode: &str) -> String {
    format!(
        "diff:{:?}:{mode}:{}",
        slot,
        pane.key.as_deref().unwrap_or("-")
    )
}

fn fixed_row<'a>(row: DiffRow, content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(row.height()))
        .into()
}

fn render_row<'a>(
    row: DiffRow,
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    split: bool,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    match row {
        DiffRow::FileHeader { file, collapsed } => {
            file_header(tokens, slot, file, collapsed, mode, files)
        }
        DiffRow::HunkHeader {
            file,
            hunk,
            old_start,
            old_count,
            new_start,
            new_count,
        } => hunk_header(
            tokens, slot, file, hunk, old_start, old_count, new_start, new_count, mode,
        ),
        DiffRow::Line { file, hunk, line } => {
            unified_line(pane, slot, tokens, file, hunk, line, mode, files)
        }
        DiffRow::Context { file, hunk, line } => {
            if split {
                pair_side(
                    pane,
                    slot,
                    tokens,
                    file,
                    hunk,
                    Some(line),
                    None,
                    mode,
                    files,
                )
            } else {
                unified_line(pane, slot, tokens, file, hunk, line, mode, files)
            }
        }
        DiffRow::Pair {
            file,
            hunk,
            left,
            right,
        } => pair_side(pane, slot, tokens, file, hunk, left, right, mode, files),
        DiffRow::Binary { file } => binary_row(tokens, files.get(file)),
        DiffRow::Image { file } => {
            let preview = files.get(file).and_then(|f| pane.images.get(&f.path));
            image_row(tokens, files.get(file), preview)
        }
    }
}

fn file_header<'a>(
    tokens: Tokens,
    slot: DiffSlot,
    file_index: usize,
    collapsed: bool,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    let Some(file) = files.get(file_index) else {
        return Space::new().into();
    };
    let kind = file_change_kind(file);
    let color = match kind {
        crate::engine::types::ChangeKind::Added => tokens.added,
        crate::engine::types::ChangeKind::Deleted => tokens.deleted,
        crate::engine::types::ChangeKind::Renamed => tokens.renamed,
        _ => tokens.modified,
    };
    let path = match &file.old_path {
        Some(old) if old != &file.path => format!("{old} → {}", file.path),
        _ => file.path.clone(),
    };

    let summary = container(
        row![
            text(if collapsed { "▸" } else { "▾" })
                .size(10)
                .color(tokens.muted_foreground),
            text(file_kind_letter(kind))
                .size(11)
                .font(theme::mono())
                .color(color),
            text(path)
                .size(12)
                .font(theme::mono())
                .color(tokens.foreground)
                .width(Length::Fill),
            text(kind_label(kind))
                .size(10)
                .color(tokens.muted_foreground),
            text(format!("+{}", file.additions))
                .size(10)
                .color(tokens.added),
            text(format!("−{}", file.deletions))
                .size(10)
                .color(tokens.deleted),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .padding([0, 8]),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Alignment::Center);

    let toggle = button(summary)
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .on_press(Message::DiffToggleFile {
            slot,
            file: file_index,
        });

    let mut content = row![toggle].align_y(Alignment::Center);
    if mode.file_actions {
        content = content.push(
            button(
                text(if mode.unstage {
                    "Unstage file"
                } else {
                    "Stage file"
                })
                .size(10),
            )
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(Message::StageFile {
                path: file.path.clone(),
                unstage: mode.unstage,
            }),
        );
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::mix(
                tokens.card,
                tokens.muted,
                if tokens.dark { 0.35 } else { 0.5 },
            ))),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}
#[allow(clippy::too_many_arguments)]
fn hunk_header<'a>(
    tokens: Tokens,
    slot: DiffSlot,
    file: usize,
    hunk: usize,
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
    mode: PaneMode,
) -> Element<'a, Message> {
    let mut content = row![text(format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    ))
    .size(11)
    .font(theme::mono())
    .color(tokens.muted_foreground)
    .width(Length::Fill),]
    .align_y(Alignment::Center)
    .padding([0, 8]);

    if mode.line_actions {
        content = content.push(
            button(
                text(if mode.unstage {
                    "Unstage hunk"
                } else {
                    "Stage hunk"
                })
                .size(10),
            )
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .on_press(Message::StageHunk { slot, file, hunk }),
        );
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.muted)),
            ..container::Style::default()
        })
        .into()
}

fn line_background(tokens: Tokens, origin: char) -> Option<Color> {
    match origin {
        '+' => Some(tokens.added_bg()),
        '-' => Some(tokens.deleted_bg()),
        _ => None,
    }
}

fn origin_color(tokens: Tokens, origin: char) -> Color {
    match origin {
        '+' => tokens.added,
        '-' => tokens.deleted,
        _ => tokens.muted_foreground,
    }
}

#[allow(clippy::too_many_arguments)]
fn unified_line<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    file: usize,
    hunk: usize,
    line: usize,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    let Some(diff_line) = line_at(files, file, hunk, line) else {
        return Space::new().into();
    };
    let background = line_background(tokens, diff_line.origin);
    let gutter = line_gutter(pane, slot, tokens, file, hunk, diff_line, mode);

    let content = row![
        gutter,
        text(diff_line.old_no.map(number).unwrap_or_default())
            .size(11)
            .font(theme::mono())
            .color(tokens.muted_foreground)
            .width(Length::Fixed(40.0))
            .align_x(Alignment::End),
        text(diff_line.new_no.map(number).unwrap_or_default())
            .size(11)
            .font(theme::mono())
            .color(tokens.muted_foreground)
            .width(Length::Fixed(40.0))
            .align_x(Alignment::End),
        Space::new().width(6),
        text(diff_line.origin.to_string())
            .size(12)
            .font(theme::mono())
            .color(origin_color(tokens, diff_line.origin))
            .width(Length::Fixed(14.0)),
        text(diff_line.text.as_str())
            .size(12)
            .font(theme::mono())
            .color(tokens.foreground)
            .width(Length::Fill),
    ]
    .spacing(0)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: background.map(Background::Color),
            ..container::Style::default()
        })
        .into()
}

/// New-side gutter cell: clickable for line staging when allowed.
#[allow(clippy::too_many_arguments)]
fn line_gutter<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    file: usize,
    hunk: usize,
    line: &DiffLine,
    mode: PaneMode,
) -> Element<'a, Message> {
    let Some(new_no) = line.new_no else {
        return Space::new().width(Length::Fixed(0.0)).into();
    };
    let selected = pane.lines.contains(file, hunk, new_no);
    let cell = container(text("▍").size(12).color(if selected {
        tokens.primary
    } else {
        theme::with_alpha(tokens.primary, 0.0)
    }))
    .width(Length::Fixed(10.0))
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center);

    if mode.line_actions {
        mouse_area(cell)
            .on_press(Message::DiffToggleLine {
                slot,
                file,
                hunk,
                new_no,
            })
            .into()
    } else {
        cell.into()
    }
}

#[allow(clippy::too_many_arguments)]
fn pair_side<'a>(
    pane: &'a DiffPane,
    slot: DiffSlot,
    tokens: Tokens,
    file: usize,
    hunk: usize,
    left: Option<usize>,
    right: Option<usize>,
    mode: PaneMode,
    files: &'a [FileDiff],
) -> Element<'a, Message> {
    let left_line = left.and_then(|index| line_at(files, file, hunk, index));
    let right_line = right.and_then(|index| line_at(files, file, hunk, index));

    let left_background = left_line.and_then(|line| line_background(tokens, line.origin));
    let right_background = right_line.and_then(|line| line_background(tokens, line.origin));

    let left_cell = split_cell(
        tokens,
        left_line,
        left_background,
        false,
        false,
        slot,
        file,
        hunk,
        mode,
    );
    let right_selected = right_line
        .and_then(|line| line.new_no)
        .is_some_and(|new_no| pane.lines.contains(file, hunk, new_no));
    let right_cell = split_cell(
        tokens,
        right_line,
        right_background,
        true,
        right_selected,
        slot,
        file,
        hunk,
        mode,
    );

    let left_cell = container(left_cell)
        .width(Length::FillPortion(1))
        .height(Length::Fill);
    let right_cell = container(right_cell)
        .width(Length::FillPortion(1))
        .height(Length::Fill);

    row![left_cell, right_cell]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[allow(clippy::too_many_arguments)]
fn split_cell<'a>(
    tokens: Tokens,
    line: Option<&DiffLine>,
    background: Option<Color>,
    new_side: bool,
    selected: bool,
    slot: DiffSlot,
    file: usize,
    hunk: usize,
    mode: PaneMode,
) -> Element<'a, Message> {
    let (line_number, origin, body) = match line {
        Some(line) => (
            if new_side { line.new_no } else { line.old_no },
            line.origin,
            line.text.as_str(),
        ),
        None => (None, ' ', ""),
    };
    let marker = if is_context(origin) {
        " ".to_string()
    } else {
        origin.to_string()
    };

    let mut content = row![].align_y(Alignment::Center).height(Length::Fill);
    if let Some(new_no) = line.and_then(|line| line.new_no) {
        if new_side && mode.line_actions {
            content = content.push(
                mouse_area(
                    container(text("▍").size(12).color(if selected {
                        tokens.primary
                    } else {
                        theme::with_alpha(tokens.primary, 0.0)
                    }))
                    .width(Length::Fixed(10.0))
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::DiffToggleLine {
                    slot,
                    file,
                    hunk,
                    new_no,
                }),
            );
        }
    }
    content = content.push(
        text(
            line_number
                .map(|value| value.to_string())
                .unwrap_or_default(),
        )
        .size(11)
        .font(theme::mono())
        .color(tokens.muted_foreground)
        .width(Length::Fixed(40.0))
        .align_x(Alignment::End),
    );
    content = content.push(Space::new().width(4));
    content = content.push(
        text(marker)
            .size(12)
            .font(theme::mono())
            .color(origin_color(tokens, origin))
            .width(Length::Fixed(12.0)),
    );
    content = content.push(
        text(body.to_string())
            .size(12)
            .font(theme::mono())
            .color(tokens.foreground)
            .width(Length::Fill),
    );

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: background.map(Background::Color),
            ..container::Style::default()
        })
        .into()
}

fn binary_row<'a>(tokens: Tokens, file: Option<&FileDiff>) -> Element<'a, Message> {
    let label = file
        .map(|file| format!("Binary file — {} bytes of content are not shown", file.path))
        .unwrap_or_else(|| "Binary file".to_string());
    note(tokens, label.as_str())
}

/// Side-by-side image preview. Handles arrive asynchronously from the shell
/// (workdir reads + `git show <rev>:<path>` blob reads); a missing side
/// renders a placeholder, so added/deleted/failed sides degrade cleanly.
fn image_row<'a>(
    tokens: Tokens,
    file: Option<&FileDiff>,
    preview: Option<&DiffImages>,
) -> Element<'a, Message> {
    let label = file
        .map(|file| format!("Image diff — {}", file.path))
        .unwrap_or_else(|| "Image diff".to_string());
    let (old, new) = preview
        .map(|p| (p.old.clone(), p.new.clone()))
        .unwrap_or((None, None));

    let side = |title: &'a str, handle: Option<image::Handle>| -> Element<'a, Message> {
        let body: Element<'a, Message> = match handle {
            Some(handle) => image(handle)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => text("no preview")
                .size(11)
                .color(tokens.muted_foreground)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .into(),
        };
        column![
            text(title).size(10).color(tokens.muted_foreground),
            container(body)
                .width(Length::Fill)
                .height(Length::Fixed(220.0))
                .style(theme::surface_style(tokens)),
        ]
        .spacing(4)
        .width(Length::Fill)
        .into()
    };

    container(
        column![
            text(label).size(12).color(tokens.muted_foreground),
            row![side("Before", old), side("After", new)].spacing(8),
        ]
        .spacing(8)
        .width(Length::Fill)
        .padding(12),
    )
    .width(Length::Fill)
    .height(Length::Shrink)
    .style(theme::surface_style(tokens))
    .into()
}

fn line_at(files: &[FileDiff], file: usize, hunk: usize, line: usize) -> Option<&DiffLine> {
    files.get(file)?.hunks.get(hunk)?.lines.get(line)
}

fn number(value: u32) -> String {
    value.to_string()
}

fn note<'a>(tokens: Tokens, message: impl Into<String>) -> Element<'a, Message> {
    container(text(message.into()).size(12).color(tokens.muted_foreground))
        .padding([16, 12])
        .width(Length::Fill)
        .into()
}

fn note_error<'a>(tokens: Tokens, message: impl Into<String>) -> Element<'a, Message> {
    container(text(message.into()).size(12).color(tokens.deleted))
        .padding([16, 12])
        .width(Length::Fill)
        .into()
}
