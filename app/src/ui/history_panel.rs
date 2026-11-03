//! History sidebar: streamed commit list with per-row graph lanes.
//!
//! The list is virtualized by row height so long histories stay cheap; the
//! lane drawing comes from [`graph_canvas`].

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{
    button, canvas, column, container, row, scrollable, text, text::Wrapping, Id, Space,
};
use iced::{Alignment, Background, Border, Element, Length};

use super::app::Message;
use super::graph_canvas::{graph_width, GraphCell, ROW_HEIGHT};
use super::history_model::{relative_age, short_summary};
use super::repo_view::{short_sha, Tab, HISTORY_WIDTH};
use super::theme::{self, Tokens};

/// Rows rendered before and after the visible range.
const OVERSCAN: usize = 20;

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let history = &tab.history;

    let status = if history.loading {
        format!("{} · loading…", history.len())
    } else if history.complete {
        format!("{} commits", history.len())
    } else {
        history.len().to_string()
    };

    let header = container(
        row![
            text("History").size(12).font(theme::semibold()),
            Space::new().width(Length::Fill),
            text(status).size(10).color(tokens.muted_foreground),
            Space::new().width(6),
            button(text("⟳").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 6])
                .on_press(Message::RefreshLog(tab.repo.repo_id.clone())),
        ]
        .align_y(Alignment::Center)
        .padding([8, 10]),
    );

    let body: Element<'a, Message> = if history.commits.is_empty() {
        let note = if history.loading {
            "Loading history…"
        } else if !matches!(tab.status, super::repo_view::Load::Loading) {
            "No commits."
        } else {
            "Loading…"
        };
        container(text(note).size(11).color(tokens.muted_foreground))
            .padding([16, 12])
            .into()
    } else {
        list(tab, tokens)
    };

    column![header, body]
        .width(Length::Fill)
        .height(Length::Fill)
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
    let badge_width = if commit.refs.is_empty() { 0.0 } else { 62.0 };
    let budget =
        (((HISTORY_WIDTH - lanes - 52.0 - 44.0 - badge_width - 24.0) / 5.6) as usize).clamp(8, 48);
    let summary = clip(short_summary(commit), budget);
    let age = relative_age(commit.author.time, now);

    let mut line = row![
        canvas(cell)
            .width(Length::Fixed(lanes))
            .height(Length::Fixed(ROW_HEIGHT)),
        text(short_sha(&commit.sha))
            .size(10)
            .font(theme::mono())
            .color(tokens.muted_foreground)
            .width(Length::Fixed(52.0)),
        text(summary)
            .size(11)
            .wrapping(Wrapping::None)
            .color(tokens.foreground)
            .width(Length::Fill),
        text(age)
            .size(9)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(44.0))
            .align_x(Alignment::End),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .height(Length::Fixed(ROW_HEIGHT));

    if !commit.refs.is_empty() {
        let badge = container(
            text(clip(&commit.refs.join(" "), 14))
                .size(8)
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
        .on_press(Message::SelectCommit(commit.sha.clone()))
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
