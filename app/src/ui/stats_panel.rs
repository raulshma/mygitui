#![allow(dead_code)] // unwired lane — the orchestrator registers and routes it
//! Contribution statistics panel: contributor leaderboard + activity
//! heatmap, over `crate::service::forge`'s stats reads.
//!
//! Self-contained lane module (like [`super::health_panel`]): it owns its
//! state, its [`Msg`] enum, and returns [`Action`] intents from [`update`]
//! for the shell to translate into service calls. Nothing here touches
//! services.
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod stats_panel;` in `ui/mod.rs` (drop the temporary
//!    `#![allow(dead_code)]` at the top of this file in the same commit).
//! 2. Add `Stats(stats_panel::Msg)` to `app::Message` and hold one
//!    `stats_panel::StatsPanel` per tab.
//! 3. Route messages:
//!    `Message::Stats(msg) => {
//!        if let Some(action) = tab.stats.update(msg) {
//!            Self::run_stats_action(repo_id, &mut tab.stats, action)
//!        }
//!    }`.
//! 4. Execute actions via `crate::service::forge`:
//!    - `Action::Refresh { window }`: `forge::commit_activity(services,
//!      repo_id, window, None)` → `Msg::ActivityLoaded` AND
//!      `forge::contributor_stats(services, repo_id, window)` →
//!      `Msg::ContributorsLoaded` (two queued reads; the panel counts
//!      down `pending` and unlocks when both land).
//! 5. View: in the repo overlay stack, like the health panel —
//!    `container(container(tab.stats.view(tokens).map(Message::Stats)))
//!    .width(Length::Fixed(560.0))` (the panel itself returns an empty
//!    element while closed).
//!
//! The heatmap covers the last [`HEAT_WEEKS`] weeks ending at the current
//! week, weekday rows × week columns, bucketed by [`bucket_color`] (a
//! fixed-threshold alpha ramp off `tokens.primary`, so an OS-accent tint
//! propagates). Data comes as `"YYYY-MM-DD"` buckets from the engine; the
//! grid math is days-since-epoch based ([`grid_start`]/[`grid_index`],
//! pure + unit-tested), rendered via `crate::engine::stats::day_string`.

use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, pick_list, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::engine::stats::{day_string, Contributor, DayCount};

use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested; no panel state)
// ---------------------------------------------------------------------------

/// Heatmap size: the last ~26 weeks (182 cells, 7 rows × 26 columns).
pub const HEAT_WEEKS: usize = 26;
/// Heatmap cell size and gap, in points.
pub const CELL: f32 = 11.0;
pub const CELL_GAP: f32 = 2.0;
/// Contributors rendered in the leaderboard (the engine returns all).
pub const TOP_CONTRIBUTORS: usize = 15;
/// Leaderboard bar geometry, in points.
pub const BAR_MAX_WIDTH: f32 = 140.0;

/// Days-since-epoch of "today" (UTC — matches the engine's day bucketing
/// closely enough for a grid; the buckets themselves are keyed by string).
pub fn today_days() -> i64 {
    const DAY_SECS: i64 = 86_400;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64 / DAY_SECS)
}

/// Weekday of a days-since-epoch date: 0 = Sunday … 6 = Saturday
/// (the Unix epoch, 1970-01-01, was a Thursday).
pub fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// First day (days-since-epoch) of the heatmap grid: the start of the week
/// containing `today`, backed up `weeks - 1` whole weeks.
pub fn grid_start(today: i64, weeks: usize) -> i64 {
    let weeks = weeks.max(1) as i64;
    today - weekday(today) - (weeks - 1) * 7
}

/// Grid position of `day` inside the `weeks`-wide grid ending at `today`:
/// `(week column, weekday row)`, or `None` when outside the grid.
pub fn grid_index(day: i64, today: i64, weeks: usize) -> Option<(usize, usize)> {
    let offset = day - grid_start(today, weeks);
    let total = weeks.max(1) as i64 * 7;
    if offset < 0 || offset >= total {
        return None;
    }
    Some(((offset / 7) as usize, (offset % 7) as usize))
}

/// Heatmap bucket for a day's commit count: a fixed-threshold 5-step ramp
/// off `tokens.primary` (empty days use `tokens.muted`, so an OS-accent
/// override automatically recolors the ramp).
pub fn bucket_color(tokens: Tokens, count: u32) -> Color {
    match count {
        0 => tokens.muted,
        1..=2 => theme::with_alpha(tokens.primary, 0.30),
        3..=5 => theme::with_alpha(tokens.primary, 0.50),
        6..=9 => theme::with_alpha(tokens.primary, 0.75),
        _ => tokens.primary,
    }
}

/// Leaderboard bar length for `count` against the window's `max`, as a
/// fraction of [`BAR_MAX_WIDTH`] (0 for empty windows, ≥ 0.06 so every
/// contributor keeps a visible sliver).
pub fn bar_fraction(count: u32, max: u32) -> f32 {
    if max == 0 || count == 0 {
        return 0.0;
    }
    (count as f32 / max as f32).clamp(0.06, 1.0)
}

/// Canonical contributor ordering (count descending, ties by name then
/// email — mirrors `engine::stats`, applied defensively at render time).
pub fn order_contributors(mut list: Vec<Contributor>) -> Vec<Contributor> {
    list.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.email.cmp(&b.email))
    });
    list
}

// ---------------------------------------------------------------------------
// State + messages
// ---------------------------------------------------------------------------

/// Stats window length (the service takes `max_days: u32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    D90,
    D180,
    D365,
}

impl Window {
    /// Every option for the picker, in display order.
    pub const ALL: [Window; 3] = [Self::D90, Self::D180, Self::D365];

    /// `max_days` argument for the service calls.
    pub fn days(self) -> u32 {
        match self {
            Self::D90 => 90,
            Self::D180 => 180,
            Self::D365 => 365,
        }
    }

    /// Reverse of [`Window::days`] (unknown values clamp to the shortest).
    pub fn from_days(days: u32) -> Self {
        match days {
            180 => Self::D180,
            365 => Self::D365,
            _ => Self::D90,
        }
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Last {} days", self.days())
    }
}

/// Load state + data for the stats panel.
#[derive(Debug)]
pub struct StatsPanel {
    pub open: bool,
    /// True while the two reads (activity + contributors) are in flight.
    pub loading: bool,
    pub window: Window,
    /// Commits per local day inside the window (day ascending; empty days
    /// are omitted — the grid treats missing days as zero).
    pub activity: Option<Vec<DayCount>>,
    /// Per-author rollups (count descending).
    pub contributors: Option<Vec<Contributor>>,
    /// Load failure, rendered in the destructive colour.
    pub error: Option<String>,
    /// Reads still outstanding (0/1/2); drives `loading`.
    pub pending: u32,
}

impl Default for StatsPanel {
    fn default() -> Self {
        Self {
            open: false,
            loading: false,
            window: Window::D365,
            activity: None,
            contributors: None,
            error: None,
            pending: 0,
        }
    }
}

/// Panel messages: user input and shell-delivered service results.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Show/hide the panel (every open triggers a fresh load).
    ToggleOpen,
    /// The header ⟳ button: reload both reads for the current window.
    RefreshPressed,
    /// Window picker selection (also triggers a reload).
    WindowSelected(Window),
    /// `forge::commit_activity` result.
    ActivityLoaded(Result<Vec<DayCount>, String>),
    /// `forge::contributor_stats` result.
    ContributorsLoaded(Result<Vec<Contributor>, String>),
}

/// Intents the shell executes against the service layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Reload both stats reads for `window` days
    /// (`forge::commit_activity` + `forge::contributor_stats`).
    Refresh { window: u32 },
}

impl StatsPanel {
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ToggleOpen => {
                self.open = !self.open;
                self.open.then(|| self.refresh())
            }
            Msg::RefreshPressed => Some(self.refresh()),
            Msg::WindowSelected(window) => {
                if window == self.window {
                    return None;
                }
                self.window = window;
                Some(self.refresh())
            }
            Msg::ActivityLoaded(result) => {
                self.pending = self.pending.saturating_sub(1);
                match result {
                    Ok(days) => self.activity = Some(days),
                    Err(error) => self.error = Some(error),
                }
                self.settle();
                None
            }
            Msg::ContributorsLoaded(result) => {
                self.pending = self.pending.saturating_sub(1);
                match result {
                    Ok(contributors) => self.contributors = Some(contributors),
                    Err(error) => self.error = Some(error),
                }
                self.settle();
                None
            }
        }
    }

    fn refresh(&mut self) -> Action {
        self.loading = true;
        self.error = None;
        self.pending = 2;
        Action::Refresh {
            window: self.window.days(),
        }
    }

    fn settle(&mut self) {
        if self.pending == 0 {
            self.loading = false;
        }
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        if !self.open {
            return column![].into();
        }

        let mut header = row![text("Contributions")
            .size(theme::TEXT_BODY)
            .font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
        if self.loading {
            header = header.push(
                text("· loading…")
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            );
        }
        header = header
            .push(Space::new().width(Length::Fill))
            .push(
                pick_list(Window::ALL, Some(self.window), Msg::WindowSelected)
                    .text_size(theme::TEXT_SMALL)
                    .padding([2, 6])
                    .width(Length::Fixed(120.0)),
            )
            .push(theme::tip(
                tokens,
                button(text(theme::icon::REFRESH).size(theme::TEXT_SMALL))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::RefreshPressed),
                "Reload contributions and contributors",
            ))
            .push(theme::tip(
                tokens,
                button(text(theme::icon::CLOSE).size(theme::TEXT_BODY))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::ToggleOpen),
                "Close stats panel",
            ));

        let mut body = column![header].spacing(8).width(Length::Fill);
        if let Some(error) = &self.error {
            // Refresh re-reads both sections, so it doubles as the retry.
            body = body.push(
                row![
                    theme::note(tokens, error.as_str(), true, theme::NoteKind::Inline),
                    theme::mini_button(
                        tokens,
                        "Retry",
                        "Reload contributions and contributors",
                        Some(Msg::RefreshPressed),
                    ),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .width(Length::Fill),
            );
        }
        body = body.push(heatmap_section(self, tokens));
        body = body.push(contributors_section(self, tokens));

        container(
            scrollable(container(body).padding([8, 10]).width(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill),
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
}

/// Section title helper.
fn section_title<'a>(label: &str, tokens: Tokens) -> Element<'a, Msg> {
    text(label.to_owned())
        .size(theme::TEXT_SMALL)
        .font(theme::semibold())
        .color(tokens.muted_foreground)
        .into()
}

/// The contribution heatmap: 7 weekday rows × [`HEAT_WEEKS`] week columns,
/// plus the Less→More legend.
fn heatmap_section<'a>(panel: &'a StatsPanel, tokens: Tokens) -> Element<'a, Msg> {
    let counts: HashMap<&str, u32> = panel
        .activity
        .iter()
        .flatten()
        .map(|day| (day.day.as_str(), day.count))
        .collect();

    let today = today_days();
    let start = grid_start(today, HEAT_WEEKS);
    let mut grid = column![].spacing(CELL_GAP);
    for day_row in 0..7i64 {
        let mut cells = row![].spacing(CELL_GAP);
        for week in 0..HEAT_WEEKS {
            let day = start + week as i64 * 7 + day_row;
            let count = counts
                .get(day_string(day).as_str())
                .copied()
                .unwrap_or_default();
            let empty = count == 0;
            cells = cells.push(container(Space::new().width(CELL).height(CELL)).style(
                move |_theme| container::Style {
                    background: Some(Background::Color(bucket_color(tokens, count))),
                    border: Border {
                        color: tokens.border,
                        width: if empty { 1.0 } else { 0.0 },
                        radius: 2.0.into(),
                    },
                    ..container::Style::default()
                },
            ));
        }
        grid = grid.push(cells);
    }

    let mut legend = row![text("Less")
        .size(theme::TEXT_META)
        .color(tokens.muted_foreground)]
    .spacing(4)
    .align_y(Alignment::Center);
    for count in [0u32, 1, 3, 6, 10] {
        let color = bucket_color(tokens, count);
        legend = legend.push(container(Space::new().width(CELL).height(CELL)).style(
            move |_theme| container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 2.0.into(),
                },
                ..container::Style::default()
            },
        ));
    }
    legend = legend.push(
        text("More")
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
    );

    let body = if panel.activity.is_none() && !panel.loading {
        theme::note(
            tokens,
            "Open the panel to load contribution activity.",
            false,
            theme::NoteKind::Empty,
        )
    } else {
        column![grid, legend].spacing(6).into()
    };

    column![section_title("Contribution activity", tokens), body]
        .spacing(4)
        .width(Length::Fill)
        .into()
}

/// The contributor leaderboard: name, proportional commit bar, count.
fn contributors_section<'a>(panel: &'a StatsPanel, tokens: Tokens) -> Element<'a, Msg> {
    let mut body = column![].spacing(4).width(Length::Fill);
    match &panel.contributors {
        None if !panel.loading => {
            body = body.push(theme::note(
                tokens,
                "Open the panel to load contributor statistics.",
                false,
                theme::NoteKind::Empty,
            ));
        }
        None => {
            body = body.push(theme::note(
                tokens,
                "Loading contributors…",
                false,
                theme::NoteKind::Inline,
            ));
        }
        Some(list) if list.is_empty() => {
            body = body.push(theme::note(
                tokens,
                "No commits in this window.",
                false,
                theme::NoteKind::Empty,
            ));
        }
        Some(list) => {
            let max = list.iter().map(|c| c.count).max().unwrap_or_default();
            for contributor in order_contributors(list.clone())
                .into_iter()
                .take(TOP_CONTRIBUTORS)
            {
                let label = if contributor.name.is_empty() {
                    contributor.email.clone()
                } else {
                    contributor.name.clone()
                };
                let width = BAR_MAX_WIDTH * bar_fraction(contributor.count, max);
                let bar = if contributor.count == 0 {
                    Space::new().width(0).height(0)
                } else {
                    Space::new()
                        .width(Length::Fixed(width.max(4.0)))
                        .height(Length::Fixed(10.0))
                };
                body = body.push(
                    row![
                        text(label)
                            .size(theme::TEXT_SMALL)
                            .width(Length::Fixed(170.0)),
                        container(bar)
                            .width(Length::Fixed(BAR_MAX_WIDTH))
                            .style(move |_theme| container::Style {
                                background: Some(Background::Color(theme::with_alpha(
                                    tokens.primary,
                                    0.15
                                ),)),
                                ..container::Style::default()
                            }),
                        text(contributor.count.to_string())
                            .size(theme::TEXT_SMALL)
                            .color(tokens.muted_foreground)
                            .width(Length::Fixed(44.0))
                            .align_x(Alignment::End),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }
        }
    }

    column![section_title("Top contributors", tokens), body]
        .spacing(4)
        .width(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_is_sunday_first_and_epoch_is_thursday() {
        assert_eq!(weekday(0), 4); // 1970-01-01, Thursday
        assert_eq!(weekday(1), 5); // Friday
        assert_eq!(weekday(2), 6); // Saturday
        assert_eq!(weekday(3), 0); // 1970-01-04, Sunday
        assert_eq!(weekday(6), 3);
        assert_eq!(weekday(7), 4); // next Thursday
                                   // Negative days (pre-epoch) wrap cleanly.
        assert_eq!(weekday(-1), 3);
        assert_eq!(weekday(-4), 0);
    }

    #[test]
    fn grid_index_covers_the_week_ending_at_today() {
        // A one-week grid starting today (Thursday) spans Sun -4 … Sat 2.
        assert_eq!(grid_start(0, 1), -4);
        assert_eq!(grid_index(-4, 0, 1), Some((0, 0)));
        assert_eq!(grid_index(0, 0, 1), Some((0, 4)));
        assert_eq!(grid_index(2, 0, 1), Some((0, 6)));
        assert_eq!(grid_index(-5, 0, 1), None);
        assert_eq!(grid_index(3, 0, 1), None);
    }

    #[test]
    fn grid_index_round_trips_over_the_full_grid() {
        let today = 10_000;
        let start = grid_start(today, HEAT_WEEKS);
        assert_eq!(weekday(start), 0, "grid starts on a Sunday");
        for day in start..start + (HEAT_WEEKS as i64) * 7 {
            let (col, row) = grid_index(day, today, HEAT_WEEKS).expect("inside the grid");
            assert_eq!(start + col as i64 * 7 + row as i64, day);
        }
        assert_eq!(grid_index(start - 1, today, HEAT_WEEKS), None);
        assert_eq!(
            grid_index(start + (HEAT_WEEKS as i64) * 7, today, HEAT_WEEKS),
            None
        );
        // Today always lands in the last column, on its weekday row.
        assert_eq!(
            grid_index(today, today, HEAT_WEEKS),
            Some((HEAT_WEEKS - 1, weekday(today) as usize))
        );
    }

    #[test]
    fn grid_start_is_sane_for_week_boundaries() {
        // A today that is already a Sunday starts exactly at itself.
        let sunday = 3; // 1970-01-04
        assert_eq!(grid_start(sunday, 1), sunday);
        assert_eq!(grid_start(sunday, 26), sunday - 25 * 7);
    }

    #[test]
    fn bucket_ramp_is_stable_and_uses_primary() {
        let tokens = Tokens::DARK;
        assert_eq!(bucket_color(tokens, 0), tokens.muted);
        assert_eq!(
            bucket_color(tokens, 1),
            theme::with_alpha(tokens.primary, 0.30)
        );
        assert_eq!(
            bucket_color(tokens, 2),
            theme::with_alpha(tokens.primary, 0.30)
        );
        assert_eq!(
            bucket_color(tokens, 5),
            theme::with_alpha(tokens.primary, 0.50)
        );
        assert_eq!(
            bucket_color(tokens, 9),
            theme::with_alpha(tokens.primary, 0.75)
        );
        assert_eq!(bucket_color(tokens, 10), tokens.primary);
        assert_eq!(bucket_color(tokens, u32::MAX), tokens.primary);
        // The ramp ascends in alpha for the non-empty buckets.
        let alphas = [1u32, 3, 6, 10].map(|c| bucket_color(tokens, c).a);
        assert!(alphas.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn bar_fraction_clamps() {
        assert_eq!(bar_fraction(0, 10), 0.0);
        assert_eq!(bar_fraction(10, 0), 0.0);
        assert_eq!(bar_fraction(10, 10), 1.0);
        assert!((bar_fraction(5, 10) - 0.5).abs() < 1e-6);
        // Tiny counts stay visible.
        assert!((bar_fraction(1, 1000) - 0.06).abs() < 1e-6);
    }

    #[test]
    fn contributors_sort_count_desc_then_name_then_email() {
        let contributor = |name: &str, email: &str, count: u32| Contributor {
            name: name.to_owned(),
            email: email.to_owned(),
            count,
            first_day: "2025-01-01".to_owned(),
            last_day: "2025-01-31".to_owned(),
        };
        let ordered = order_contributors(vec![
            contributor("b", "b@x", 4),
            contributor("a", "a@x", 9),
            contributor("c", "c@x", 4),
            contributor("z", "z@x", 4),
        ]);
        let labels: Vec<(&str, u32)> = ordered.iter().map(|c| (c.name.as_str(), c.count)).collect();
        assert_eq!(labels, vec![("a", 9), ("b", 4), ("c", 4), ("z", 4)]);
        assert_eq!(order_contributors(Vec::new()), Vec::new());
    }

    #[test]
    fn window_mapping_round_trips() {
        for window in Window::ALL {
            assert_eq!(Window::from_days(window.days()), window);
        }
        assert_eq!(Window::from_days(0), Window::D90);
        assert_eq!(Window::from_days(999), Window::D90);
        assert_eq!(Window::D90.to_string(), "Last 90 days");
        assert_eq!(Window::D365.to_string(), "Last 365 days");
    }

    #[test]
    fn update_routes_loads_and_settles() {
        let mut panel = StatsPanel::default();
        // Opening kicks off both reads.
        assert_eq!(
            panel.update(Msg::ToggleOpen),
            Some(Action::Refresh { window: 365 })
        );
        assert!(panel.open && panel.loading && panel.pending == 2);

        // The first result keeps the spinner up…
        panel.update(Msg::ActivityLoaded(Ok(vec![DayCount {
            day: "2025-01-01".to_owned(),
            count: 3,
        }])));
        assert!(panel.loading && panel.pending == 1);
        // …the second settles it.
        panel.update(Msg::ContributorsLoaded(Ok(vec![Contributor {
            name: "a".to_owned(),
            email: "a@x".to_owned(),
            count: 3,
            first_day: "2025-01-01".to_owned(),
            last_day: "2025-01-01".to_owned(),
        }])));
        assert!(!panel.loading && panel.pending == 0);

        // Window change reloads; errors surface and unlock.
        assert_eq!(
            panel.update(Msg::WindowSelected(Window::D90)),
            Some(Action::Refresh { window: 90 })
        );
        assert!(panel.loading);
        panel.update(Msg::ActivityLoaded(Err("boom".to_owned())));
        panel.update(Msg::ContributorsLoaded(Err("boom".to_owned())));
        assert!(!panel.loading);
        assert_eq!(panel.error.as_deref(), Some("boom"));

        // Closing stops emitting actions.
        assert_eq!(panel.update(Msg::ToggleOpen), None);
        assert!(!panel.open);
        // Re-selecting the current window is a no-op.
        panel.update(Msg::ToggleOpen);
        assert_eq!(panel.update(Msg::WindowSelected(Window::D90)), None);
    }
}
