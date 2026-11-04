//! Home screen: open a repository or resume a recent one.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};

use super::app::Message;
use super::clone_dialog;
use super::config::RecentRepo;
use super::theme::{self, Tokens};

pub fn view<'a>(
    tokens: Tokens,
    recent: &'a [RecentRepo],
    busy: bool,
    now_ms: u64,
) -> Element<'a, Message> {
    let mut list = column![].spacing(2).width(Length::Fill);

    if recent.is_empty() {
        list = list.push(
            container(
                text("No recent repositories yet.")
                    .size(13)
                    .color(tokens.muted_foreground),
            )
            .padding([16, 0]),
        );
    } else {
        for entry in recent {
            list = list.push(recent_row(tokens, entry, now_ms));
        }
    }

    let open_button = if busy {
        button(text("Opening…").size(13))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([8, 16])
    } else {
        button(text("Open repository…").size(13))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([8, 16])
            .on_press(Message::OpenDialog)
    };

    let clone_button = button(text("Clone repository…").size(13))
        .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
        .padding([8, 16])
        .on_press(Message::Clone(clone_dialog::Msg::Open));

    let card = container(
        column![
            text("mygitui").size(24).font(theme::semibold()),
            text("A native Git client. Pick a repository to get started.")
                .size(13)
                .color(tokens.muted_foreground),
            Space::new().height(16),
            row![open_button, clone_button].spacing(8),
            Space::new().height(24),
            text("Recent repositories").size(12).font(theme::semibold()),
            Space::new().height(4),
            list,
        ]
        .width(Length::Fill),
    )
    .style(theme::panel_style(tokens))
    .padding(24)
    .width(Length::Fill)
    .max_width(760.0);

    container(
        scrollable(container(card).width(Length::Fill).center_x(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(24)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn recent_row<'a>(tokens: Tokens, entry: &'a RecentRepo, now_ms: u64) -> Element<'a, Message> {
    let meta = format!(
        "{}{}",
        ago(entry.last_opened, now_ms),
        if entry.pinned { " · pinned" } else { "" }
    );
    let label = container(
        column![
            text(entry.path.as_str()).size(13).color(tokens.foreground),
            text(meta).size(11).color(tokens.muted_foreground),
        ]
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .align_x(Alignment::Start);

    let open = button(label)
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([8, 10])
        .width(Length::Fill)
        .on_press(Message::OpenPath(entry.path.clone()));

    let pin_label = if entry.pinned { "Unpin" } else { "Pin" };
    let pin = button(text(pin_label).size(11))
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([6, 8])
        .on_press(Message::TogglePin(entry.path.clone()));

    let remove = button(text("Remove").size(11))
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([6, 8])
        .on_press(Message::RemoveRecent(entry.path.clone()));

    row![open, pin, remove]
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// Coarse relative time for the recents list (persisted milliseconds).
pub fn ago(last_ms: u64, now_ms: u64) -> String {
    if last_ms == 0 || now_ms <= last_ms {
        return "just now".to_string();
    }
    let secs = (now_ms - last_ms) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=2_591_999 => format!("{}d ago", secs / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", secs / 2_592_000),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn relative_time_buckets() {
        let now = 10_000_000_000;
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now - 5 * 60_000, now), "5m ago");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3h ago");
        assert_eq!(ago(now - 2 * 86_400_000, now), "2d ago");
        assert_eq!(ago(0, now), "just now");
    }
}
