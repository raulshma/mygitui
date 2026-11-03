//! Remotes & network panel: remote list, add/remove, fetch/pull/push.
//!
//! Progress for running net ops arrives through `BackendEvent::OpProgress`
//! (footer status line); credential prompts are global modals handled by
//! [`super::prompts`].

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{NetStats, RemoteInfo};

use super::app::Message;
use super::repo_view::Tab;
use super::theme::{self, Tokens};

/// Load state and form buffers for the remote panel.
#[derive(Debug, Default)]
pub struct RemotesState {
    pub open: bool,
    pub loading: bool,
    pub list: Option<Vec<RemoteInfo>>,
    pub error: Option<String>,
    pub add_name: String,
    pub add_url: String,
}

pub fn view<'a>(tab: &'a Tab, tokens: Tokens) -> Element<'a, Message> {
    let state = &tab.remotes;

    let mut header = row![text("Remotes").size(12).font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
    if let Some(list) = &state.list {
        header = header.push(
            text(format!("{}", list.len()))
                .size(10)
                .color(tokens.muted_foreground),
        );
    }
    if state.loading {
        header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
    }
    header = header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text("⟳").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::RefreshRemotes),
        )
        .push(
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Message::ToggleRemotePanel),
        );

    let body: Element<'a, Message> = match &state.list {
        Some(list) if !list.is_empty() => {
            let mut rows = column![].spacing(3).width(Length::Fill);
            for remote in list {
                rows = rows.push(remote_row(remote, tokens));
            }
            let height = (list.len() as f32 * 44.0).clamp(44.0, 220.0);
            scrollable(rows)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into()
        }
        Some(_) => container(
            text("No remotes configured.")
                .size(11)
                .color(tokens.muted_foreground),
        )
        .padding([6, 2])
        .into(),
        None if state.error.is_some() => container(
            text(state.error.clone().unwrap_or_default())
                .size(11)
                .color(tokens.deleted),
        )
        .padding([6, 2])
        .into(),
        None => container(
            text("Loading remotes…")
                .size(11)
                .color(tokens.muted_foreground),
        )
        .padding([6, 2])
        .into(),
    };

    let add = row![
        text_input("name", state.add_name.as_str())
            .on_input(Message::RemoteNameChanged)
            .on_submit(Message::RemoteAdd)
            .size(11)
            .padding([4, 6])
            .width(Length::Fixed(96.0)),
        text_input("https://host/repo.git", state.add_url.as_str())
            .on_input(Message::RemoteUrlChanged)
            .on_submit(Message::RemoteAdd)
            .size(11)
            .padding([4, 6])
            .width(Length::Fill),
        button(text("Add").size(10))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 10])
            .on_press_maybe(
                (!state.add_name.trim().is_empty() && !state.add_url.trim().is_empty())
                    .then_some(Message::RemoteAdd),
            ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(column![header, body, add].spacing(6).padding([8, 10]))
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

fn remote_row<'a>(remote: &'a RemoteInfo, tokens: Tokens) -> Element<'a, Message> {
    let mut info = column![
        text(remote.name.as_str())
            .size(11)
            .font(theme::mono())
            .color(tokens.foreground),
        text(clip(&remote.url, 56))
            .size(9)
            .color(tokens.muted_foreground),
    ]
    .spacing(0)
    .width(Length::Fill);

    if let Some(push_url) = remote.push_url.as_deref() {
        if push_url != remote.url {
            info = column![
                text(remote.name.as_str())
                    .size(11)
                    .font(theme::mono())
                    .color(tokens.foreground),
                text(clip(&remote.url, 56))
                    .size(9)
                    .color(tokens.muted_foreground),
                text(format!("push → {}", clip(push_url, 48)))
                    .size(9)
                    .color(tokens.muted_foreground),
            ]
            .spacing(0)
            .width(Length::Fill);
        }
    }

    let line = row![
        info,
        button(text("Fetch").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([2, 6])
            .width(Length::Fixed(46.0))
            .on_press(Message::FetchRemote(remote.name.clone())),
        button(text("Pull").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(40.0))
            .on_press(Message::PullRemote(remote.name.clone())),
        button(text("Push").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(40.0))
            .on_press(Message::PushRemote(remote.name.clone())),
        button(text("✕").size(9))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .width(Length::Fixed(24.0))
            .on_press(Message::RemoteRemoveRequested(remote.name.clone())),
    ]
    .spacing(2)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(line)
        .padding([3, 4])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.06))),
            border: Border {
                color: theme::with_alpha(tokens.border, 0.6),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// "fetch origin: 2 refs, 14 objects, 3.4 KiB" footer summary.
pub fn net_summary(label: &str, stats: &NetStats) -> String {
    format!(
        "{label}: {} refs, {} objects, {}",
        stats.updated_refs.len(),
        stats.objects,
        human_bytes(stats.received_bytes)
    )
}

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * 1024.0 {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{:.1} MiB", value / (KIB * 1024.0))
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_summary_counts_refs_and_formats_bytes() {
        let stats = NetStats {
            received_bytes: 3_481,
            objects: 14,
            updated_refs: vec![("refs/remotes/origin/main".into(), "a".into())],
        };
        assert_eq!(
            net_summary("fetch origin", &stats),
            "fetch origin: 1 refs, 14 objects, 3.4 KiB"
        );
        let empty = NetStats::default();
        assert_eq!(net_summary("push", &empty), "push: 0 refs, 0 objects, 0 B");
    }

    #[test]
    fn clip_truncates_long_urls() {
        assert_eq!(clip("https://example.com/x.git", 12), "https://exa…");
        assert_eq!(clip("short", 12), "short");
    }
}
