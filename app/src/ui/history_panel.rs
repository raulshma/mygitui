//! History sidebar: streamed commit list with per-row graph lanes.
//!
//! The list is virtualized by row height so long histories stay cheap; the
//! lane drawing comes from [`graph_canvas`].

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{
    button, canvas, checkbox, column, container, mouse_area, row, scrollable, text, text::Wrapping,
    text_input, Id, Space,
};
use iced::{Alignment, Background, Border, Element, Length};

use super::app::Message;
use super::graph_canvas::{graph_width, GraphCell, ROW_HEIGHT};
use super::history_model::{relative_age, short_summary, HistoryFilterField};
use super::repo_view::{short_sha, Tab, HISTORY_WIDTH};
use super::theme::{self, Tokens};

/// Rows rendered before and after the visible range.
const OVERSCAN: usize = 20;

// Fixed column widths of a commit row. `commit_row` lays them out from
// these and the summary clip below divides what is left by the rough
// char width of TEXT_SMALL text — keep the two in sync.
/// Short SHA column.
const SHA_COL_W: f32 = 52.0;
/// Relative-age column.
const AGE_COL_W: f32 = 44.0;
/// Refs badge allowance (sized for the 14-char clip on TEXT_META text).
const REFS_COL_W: f32 = 76.0;
/// Row chrome: inter-column spacing plus the hover container's padding.
const ROW_CHROME_W: f32 = 24.0;
/// Approximate rendered width of one TEXT_SMALL character.
const SUMMARY_CHAR_W: f32 = 5.6;

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let history = &tab.history;
    let filter_active = tab.history_filter.is_active();

    let status = if history.loading {
        format!("{} · loading…", history.len())
    } else if history.complete {
        format!("{} commits", history.len())
    } else {
        history.len().to_string()
    };

    let header = container(
        row![
            text("History")
                .size(theme::TEXT_BODY)
                .font(theme::semibold()),
            Space::new().width(Length::Fill),
            text(status)
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
            Space::new().width(4),
            button(text("Filter").size(theme::TEXT_META))
                .style(theme::button_style(
                    tokens,
                    if tab.history_filter_open {
                        theme::ButtonKind::Secondary
                    } else if filter_active {
                        theme::ButtonKind::Primary
                    } else {
                        theme::ButtonKind::Ghost
                    },
                ))
                .padding([2, 6])
                .on_press(Message::HistoryFilterToggle),
            theme::tip(
                tokens,
                button(text(theme::icon::REFRESH).size(theme::TEXT_SMALL))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 6])
                    .on_press(Message::RefreshLog(tab.repo.repo_id.clone())),
                "Refresh history",
            ),
        ]
        .align_y(Alignment::Center)
        .padding([8, 10]),
    );

    let body: Element<'a, Message> = if history.commits.is_empty() {
        let note = if history.loading {
            "Loading history…"
        } else if filter_active {
            "No commits match the filter."
        } else if !matches!(tab.status, super::repo_view::Load::Loading) {
            "No commits."
        } else {
            "Loading…"
        };
        theme::note(tokens, note, false, theme::NoteKind::Empty)
    } else {
        list(tab, tokens)
    };

    let mut panel = column![header];
    if tab.history_filter_open {
        panel = panel.push(filter_panel(tab, tokens));
    }
    panel
        .push(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn filter_panel<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let fields = &tab.history_filter;

    let input = |placeholder: &'a str, value: &'a str, field: HistoryFilterField| {
        text_input(placeholder, value)
            .on_input(move |value| Message::HistoryFilterChanged { field, value })
            .size(theme::TEXT_META)
            .padding([4, 6])
            .width(Length::Fill)
    };
    let toggle = |label: &'a str, value: bool, message: fn(bool) -> Message| {
        checkbox(value)
            .label(label)
            .size(theme::TEXT_BODY)
            .text_size(theme::TEXT_META)
            .spacing(4)
            .on_toggle(message)
    };

    let date_error = fields.date_error();
    let mut content = column![
        row![
            input("Message text", &fields.text, HistoryFilterField::Text),
            toggle("Regex", fields.regex, Message::HistoryFilterRegex),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        input("Author", &fields.author, HistoryFilterField::Author),
        row![
            input("Path", &fields.path, HistoryFilterField::Path),
            toggle("Follow", fields.follow, Message::HistoryFilterFollow),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        row![
            input("After YYYY-MM-DD", &fields.after, HistoryFilterField::After),
            input(
                "Before YYYY-MM-DD",
                &fields.before,
                HistoryFilterField::Before
            ),
        ]
        .spacing(6),
        row![
            input(
                "Pickaxe -S text",
                fields.pickaxe_value(),
                HistoryFilterField::Pickaxe
            ),
            toggle(
                "Regex",
                fields.pickaxe_regex_enabled(),
                Message::HistoryFilterPickaxeRegex
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        row![
            button(text("Apply").size(theme::TEXT_META))
                .style(theme::button_style(
                    tokens,
                    if date_error.is_none() {
                        theme::ButtonKind::Primary
                    } else {
                        theme::ButtonKind::Secondary
                    }
                ))
                .padding([3, 10])
                .on_press_maybe((date_error.is_none()).then_some(Message::HistoryFilterApply)),
            button(text("Clear").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([3, 10])
                .on_press(Message::HistoryFilterClear),
            Space::new().width(Length::Fill),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    ]
    .spacing(5);
    if let Some(error) = date_error {
        content = content.push(text(error).size(theme::TEXT_META).color(tokens.destructive));
    }

    container(content)
        .padding([6, 10])
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

fn list<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let history = &tab.history;
    let total = history.len();
    let viewport = if history.viewport > 0.0 {
        history.viewport
    } else {
        600.0
    };
    let scroll = history.scroll.max(0.0);

    let first = ((scroll / ROW_HEIGHT).floor() as usize).saturating_sub(OVERSCAN);
    let visible = (viewport / ROW_HEIGHT).ceil() as usize + OVERSCAN * 2;
    let end = (first + visible).min(total);

    let now = now_seconds();
    let lanes = graph_width(history.max_lanes);

    let mut content = column![].spacing(0).width(Length::Fill);
    if first > 0 {
        content = content.push(Space::new().height(Length::Fixed(first as f32 * ROW_HEIGHT)));
    }

    for index in first..end {
        let Some(commit) = history.commits.get(index) else {
            break;
        };
        let Some(graph) = history.rows.get(index) else {
            break;
        };
        let previous = if index > 0 {
            history.rows.get(index - 1).map(|row| row.edges.as_slice())
        } else {
            None
        }
        .unwrap_or(&[]);

        let selected = history.selected.as_deref() == Some(commit.sha.as_str());
        content = content.push(commit_row(
            tokens, commit, graph, previous, selected, lanes, now,
        ));
    }

    if end < total {
        content =
            content.push(Space::new().height(Length::Fixed((total - end) as f32 * ROW_HEIGHT)));
    }

    scrollable(content)
        .id(Id::from("history:commits"))
        .on_scroll(|viewport| Message::HistoryScrolled {
            y: viewport.absolute_offset().y,
            viewport: viewport.bounds().height,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn commit_row<'a>(
    tokens: Tokens,
    commit: &'a crate::engine::types::CommitInfo,
    graph: &'a crate::graph::types::GraphRow,
    previous: &'a [crate::graph::types::GraphEdge],
    selected: bool,
    lanes: f32,
    now: i64,
) -> Element<'a, Message> {
    let cell = GraphCell {
        row: graph,
        previous_edges: previous,
        tokens,
    };

    // Fixed-height rows: budget the summary length per available width so it
    // never wraps (author lives in the commit detail header instead).
    let badge_width = if commit.refs.is_empty() {
        0.0
    } else {
        REFS_COL_W
    };
    let budget = (((HISTORY_WIDTH - lanes - SHA_COL_W - AGE_COL_W - badge_width - ROW_CHROME_W)
        / SUMMARY_CHAR_W) as usize)
        .clamp(8, 48);
    let summary = clip(short_summary(commit), budget);
    let age = relative_age(commit.author.time, now);

    let mut line = row![
        canvas(cell)
            .width(Length::Fixed(lanes))
            .height(Length::Fixed(ROW_HEIGHT)),
        text(short_sha(&commit.sha))
            .size(theme::TEXT_META)
            .font(theme::mono())
            .color(tokens.muted_foreground)
            .width(Length::Fixed(SHA_COL_W)),
        text(summary)
            .size(theme::TEXT_SMALL)
            .wrapping(Wrapping::None)
            .color(tokens.foreground)
            .width(Length::Fill),
        text(age)
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(AGE_COL_W))
            .align_x(Alignment::End),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .height(Length::Fixed(ROW_HEIGHT));

    if !commit.refs.is_empty() {
        // Refs sit on TEXT_META now; REFS_COL_W above is the matching
        // budget so the badge never pushes the summary to wrap.
        let badge = container(
            text(clip(&commit.refs.join(" "), 14))
                .size(theme::TEXT_META)
                .color(tokens.primary),
        )
        .padding([0, 3]);
        line = line.push(badge);
    }

    let background = if selected {
        theme::with_alpha(tokens.primary, 0.14)
    } else {
        tokens.sidebar
    };

    mouse_area(
        button(container(line).width(Length::Fill).padding([0, 4]))
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
            .on_press(Message::SelectCommit(commit.sha.clone())),
    )
    .on_right_press(Message::CommitRowMenu {
        sha: commit.sha.clone(),
    })
    .into()
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}
