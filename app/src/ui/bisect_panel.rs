//! Bisect stepper panel: start a run, mark probes good/bad/skip, stop.
//!
//! Self-contained surface: owns its messages ([`Msg`]) and intents
//! ([`Action`]). The shell routes `Msg` into [`BisectPanel::update`] (pure —
//! no service calls) and maps the returned [`Action`] onto the bisect
//! service fns (`service::merge::{bisect_state, bisect_start, bisect_mark,
//! bisect_reset, bisect_log}`), feeding results back as
//! `Msg::StateLoaded` / `Msg::LogLoaded`.
//!
//! Reuses the engine's [`BisectState`] / [`BisectLogEntry`] / [`BisectMark`]
//! types directly; the panel never duplicates them.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{BisectLogEntry, BisectMark, BisectState};

use super::history_model::relative_age;
use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// Panel state: load state, engine bisect state and the start-form buffers.
#[derive(Debug)]
pub struct BisectPanel {
    pub open: bool,
    pub loading: bool,
    pub state: Option<BisectState>,
    pub error: Option<String>,
    /// "bad" ref for the start form (prefilled with HEAD).
    pub start_bad: String,
    /// Optional "good" ref for the start form.
    pub start_good: String,
    /// Mark history from `bisect_log` (live run's log, else the last
    /// archived run). Falls back to [`BisectState::log`] while active.
    pub log: Option<Vec<BisectLogEntry>>,
}

impl Default for BisectPanel {
    fn default() -> Self {
        Self {
            open: false,
            loading: false,
            state: None,
            error: None,
            start_bad: "HEAD".to_string(),
            start_good: String::new(),
            log: None,
        }
    }
}

/// Panel messages (shell → [`BisectPanel::update`]).
#[derive(Debug, Clone)]
pub enum Msg {
    ToggleOpen,
    /// Result of `service::bisect_state` / `bisect_start` / `bisect_mark` /
    /// `bisect_reset` (all return the new state).
    StateLoaded(Result<BisectState, String>),
    StartBadChanged(String),
    StartGoodChanged(String),
    StartPressed,
    MarkGood,
    MarkBad,
    MarkSkip,
    ResetPressed,
    /// Result of `service::bisect_log`.
    LogLoaded(Vec<BisectLogEntry>),
    RefreshPressed,
}

/// Intents returned by [`BisectPanel::update`] for the shell to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Fetch the current state (`bisect_state`), feed back `StateLoaded`.
    Refresh,
    /// Start a run (`bisect_start`); `good` is `None` when blank.
    Start { bad: String, good: Option<String> },
    /// Mark the current probe (`bisect_mark`), feed back `StateLoaded`.
    Mark(BisectMark),
    /// Stop the run (`bisect_reset`).
    Reset,
    /// Fetch the mark history (`bisect_log`), feed back `LogLoaded`.
    LoadLog,
}

impl BisectPanel {
    /// A run is in progress (`BisectState::active`).
    pub fn is_active(&self) -> bool {
        self.state.as_ref().is_some_and(|state| state.active)
    }

    /// The run converged: HEAD sits on the first bad commit. Marks are
    /// disabled and the result is highlighted with a "Stop bisect" button.
    /// Shell-facing check (tests use the free [`first_bad_reached`]); the
    /// view reads `state` directly.
    #[allow(dead_code)]
    pub fn is_first_bad(&self) -> bool {
        self.state.as_ref().is_some_and(first_bad_reached)
    }

    /// Pure state transition; returns the intent for the shell to run.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ToggleOpen => {
                self.open = !self.open;
                if !self.open || self.loading {
                    return None;
                }
                if self.state.is_none() {
                    self.loading = true;
                    return Some(Action::Refresh);
                }
                if self.log.is_none() {
                    return Some(Action::LoadLog);
                }
                None
            }
            Msg::StateLoaded(Ok(state)) => {
                self.loading = false;
                self.error = None;
                if !state.active {
                    // Run finished or reset: clear stale history and reset
                    // the start form for the next run.
                    self.log = None;
                    self.start_bad = "HEAD".to_string();
                    self.start_good.clear();
                }
                self.state = Some(state);
                None
            }
            Msg::StateLoaded(Err(err)) => {
                self.loading = false;
                self.error = Some(err);
                None
            }
            Msg::StartBadChanged(bad) => {
                self.start_bad = bad;
                None
            }
            Msg::StartGoodChanged(good) => {
                self.start_good = good;
                None
            }
            Msg::StartPressed => self.start(),
            Msg::MarkGood => self.mark(BisectMark::Good),
            Msg::MarkBad => self.mark(BisectMark::Bad),
            Msg::MarkSkip => self.mark(BisectMark::Skip),
            Msg::ResetPressed => {
                if !self.is_active() || self.loading {
                    return None;
                }
                self.loading = true;
                self.error = None;
                Some(Action::Reset)
            }
            Msg::LogLoaded(log) => {
                self.log = Some(log);
                None
            }
            Msg::RefreshPressed => {
                if self.loading {
                    return None;
                }
                self.loading = true;
                Some(Action::Refresh)
            }
        }
    }

    /// Start guard: requires an inactive panel and a non-empty bad ref; a
    /// blank good ref defaults to `None` (whole history is suspect).
    fn start(&mut self) -> Option<Action> {
        if self.is_active() || self.loading {
            return None;
        }
        let bad = self.start_bad.trim().to_string();
        if bad.is_empty() {
            return None;
        }
        let good = self.start_good.trim().to_string();
        self.loading = true;
        self.error = None;
        Some(Action::Start {
            bad,
            good: (!good.is_empty()).then_some(good),
        })
    }

    /// Mark guard: only an active run can be marked (a blank state or a
    /// finished run has nothing to probe).
    fn mark(&mut self, mark: BisectMark) -> Option<Action> {
        if !self.is_active() || self.loading {
            return None;
        }
        self.loading = true;
        self.error = None;
        Some(Action::Mark(mark))
    }

    /// Overlay card, or `None` while closed (the shell decides placement).
    pub fn view(&self, tokens: Tokens) -> Option<Element<'_, Msg>> {
        if !self.open {
            return None;
        }

        let mut header = row![text("Bisect")
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
            .push(theme::tip(
                tokens,
                button(text(theme::icon::REFRESH).size(theme::TEXT_SMALL))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::RefreshPressed),
                "Reload bisect state",
            ))
            .push(theme::tip(
                tokens,
                button(text(theme::icon::CLOSE).size(theme::TEXT_BODY))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([4, 8])
                    .on_press(Msg::ToggleOpen),
                "Close bisect panel",
            ));

        let body: Element<'_, Msg> = match &self.state {
            Some(state) if state.active => self.active_body(state, tokens),
            Some(_) => self.start_form(tokens),
            None if self.error.is_some() => {
                // Refresh re-reads the bisect state, so it doubles as retry.
                let retry = theme::mini_button(
                    tokens,
                    "Retry",
                    "Reload bisect state",
                    Some(Msg::RefreshPressed),
                );
                row![
                    theme::note(
                        tokens,
                        self.error.as_deref().unwrap_or_default(),
                        true,
                        theme::NoteKind::Inline
                    ),
                    retry,
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .into()
            }
            None => theme::note(
                tokens,
                "Loading bisect state…",
                false,
                theme::NoteKind::Inline,
            ),
        };

        let mut card = column![header].spacing(6);
        if let Some(err) = &self.error {
            if self.state.is_some() {
                card = card.push(theme::note(
                    tokens,
                    err.as_str(),
                    true,
                    theme::NoteKind::Inline,
                ));
            }
        }
        card = card.push(body);

        Some(
            container(card.padding([8, 10]))
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
                .into(),
        )
    }

    /// Inactive: compact start form (bad ref prefilled with HEAD, optional
    /// good ref).
    fn start_form(&self, tokens: Tokens) -> Element<'_, Msg> {
        let can_start = !self.loading && !self.start_bad.trim().is_empty();
        row![
            column![
                text("Bad (first known bad)")
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
                text_input("HEAD", self.start_bad.as_str())
                    .on_input(Msg::StartBadChanged)
                    .on_submit(Msg::StartPressed)
                    .size(theme::TEXT_SMALL)
                    .padding([4, 6])
                    .width(Length::Fixed(150.0)),
            ]
            .spacing(2),
            column![
                text("Good (last known good, optional)")
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
                text_input("tag or sha", self.start_good.as_str())
                    .on_input(Msg::StartGoodChanged)
                    .on_submit(Msg::StartPressed)
                    .size(theme::TEXT_SMALL)
                    .padding([4, 6])
                    .width(Length::Fixed(170.0)),
            ]
            .spacing(2),
            button(text("Start bisect").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                .padding([4, 10])
                .on_press_maybe(can_start.then_some(Msg::StartPressed)),
        ]
        .spacing(10)
        .align_y(Alignment::End)
        .into()
    }

    /// Active: status banner with probe buttons (or the first-bad result),
    /// then the mark-history log.
    fn active_body<'a>(&'a self, state: &'a BisectState, tokens: Tokens) -> Element<'a, Msg> {
        let converged = first_bad_reached(state);
        let marks_enabled = marks_enabled(state, self.loading);

        let mut body = column![].spacing(6);

        if converged {
            body = body.push(first_bad_banner(state, self.loading, tokens));
        } else {
            let mut status = row![
                text("Bisecting")
                    .size(theme::TEXT_SMALL)
                    .font(theme::semibold())
                    .color(tokens.foreground),
                text(short_sha(&state.bad))
                    .size(theme::TEXT_SMALL)
                    .font(theme::mono())
                    .color(tokens.foreground),
                text(remaining_label(state))
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if let Some(branch) = state.orig_branch.as_deref() {
                status = status.push(
                    text(format!("started from {branch}"))
                        .size(theme::TEXT_META)
                        .color(tokens.muted_foreground),
                );
            }
            body = body.push(status);
        }

        body = body.push(
            row![
                button(text("Good").size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([3, 10])
                    .on_press_maybe(marks_enabled.then_some(Msg::MarkGood)),
                button(text("Bad").size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                    .padding([3, 10])
                    .on_press_maybe(marks_enabled.then_some(Msg::MarkBad)),
                button(text("Skip").size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([3, 10])
                    .on_press_maybe(marks_enabled.then_some(Msg::MarkSkip)),
                Space::new().width(Length::Fill),
                button(text(if converged { "Stop bisect" } else { "Stop" }).size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([3, 10])
                    .on_press_maybe((!self.loading).then_some(Msg::ResetPressed)),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        );

        body = body.push(self.log_view(state, tokens));
        body.into()
    }

    /// Mark history: the panel-loaded log, falling back to the state's own
    /// log while a run is active. The first-bad commit is highlighted once
    /// the run converges.
    fn log_view<'a>(&'a self, state: &'a BisectState, tokens: Tokens) -> Element<'a, Msg> {
        let entries: &[BisectLogEntry] = match &self.log {
            Some(log) => log,
            None => &state.log,
        };
        if entries.is_empty() {
            return theme::note(
                tokens,
                "No marks recorded yet.",
                false,
                theme::NoteKind::Empty,
            );
        }
        let highlight_sha = first_bad_reached(state).then_some(state.bad.as_str());
        let now = now_seconds();
        let mut rows = column![].spacing(1).width(Length::Fill);
        for (step, entry) in entries.iter().enumerate() {
            rows = rows.push(log_row(
                step + 1,
                entry,
                highlight_sha.is_some_and(|sha| sha == entry.sha),
                now,
                tokens,
            ));
        }
        let height = (entries.len() as f32 * 22.0).clamp(22.0, 140.0);
        scrollable(rows)
            .height(Length::Fixed(height))
            .width(Length::Fill)
            .into()
    }
}

/// The run converged: an active state with no candidates left and a
/// resolved tip (guards the blank default state).
pub fn first_bad_reached(state: &BisectState) -> bool {
    state.active && state.remaining == 0 && !state.bad.is_empty()
}

/// Whether the Good/Bad/Skip buttons should be enabled: probing is over
/// once the first bad commit is found, and in-flight ops disable them.
pub fn marks_enabled(state: &BisectState, loading: bool) -> bool {
    !first_bad_reached(state) && !loading
}

/// "first bad commit" / "1 candidate left" / "N candidates left".
fn remaining_label(state: &BisectState) -> String {
    if first_bad_reached(state) {
        return "first bad commit".to_string();
    }
    match state.remaining {
        1 => "1 candidate left".to_string(),
        n => format!("{n} candidates left"),
    }
}

fn first_bad_banner<'a>(state: &'a BisectState, loading: bool, tokens: Tokens) -> Element<'a, Msg> {
    row![
        text("First bad commit")
            .size(theme::TEXT_META)
            .font(theme::semibold())
            .color(tokens.deleted),
        text(short_sha(&state.bad))
            .size(theme::TEXT_SMALL)
            .font(theme::mono())
            .color(tokens.foreground),
        Space::new().width(Length::Fill),
        button(text("Stop bisect").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([3, 10])
            .on_press_maybe((!loading).then_some(Msg::ResetPressed)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn log_row<'a>(
    step: usize,
    entry: &'a BisectLogEntry,
    highlight: bool,
    now: i64,
    tokens: Tokens,
) -> Element<'a, Msg> {
    let (mark_label, mark_color) = match entry.mark.as_str() {
        "good" => ("good", tokens.added),
        "bad" => ("bad", tokens.deleted),
        _ => ("skip", tokens.muted_foreground),
    };
    let line = row![
        text(format!("#{step}"))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(24.0)),
        text(mark_label)
            .size(theme::TEXT_META)
            .color(mark_color)
            .width(Length::Fixed(28.0)),
        text(short_sha(&entry.sha))
            .size(theme::TEXT_META)
            .font(theme::mono())
            .color(tokens.foreground),
        Space::new().width(6),
        text(relative_age(entry.at, now))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(line)
        .padding([2, 4])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: highlight
                .then(|| Background::Color(theme::with_alpha(tokens.deleted, 0.18))),
            border: Border {
                color: if highlight {
                    theme::with_alpha(tokens.deleted, 0.5)
                } else {
                    theme::with_alpha(tokens.border, 0.4)
                },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_state(active: bool, remaining: usize) -> BisectState {
        BisectState {
            active,
            bad: if active {
                "c0ffee1bad".to_string()
            } else {
                String::new()
            },
            good: None,
            current: None,
            remaining,
            skipped: Vec::new(),
            orig_head: "deadbeef".to_string(),
            orig_branch: Some("main".to_string()),
            log: Vec::new(),
        }
    }

    fn panel_with(state: BisectState) -> BisectPanel {
        let mut panel = BisectPanel {
            open: true,
            ..BisectPanel::default()
        };
        panel.update(Msg::StateLoaded(Ok(state)));
        panel
    }

    fn mark_entry(mark: &str, sha: &str) -> BisectLogEntry {
        BisectLogEntry {
            mark: mark.to_string(),
            sha: sha.to_string(),
            at: 1_700_000_000,
        }
    }

    #[test]
    fn default_prefills_bad_with_head() {
        let panel = BisectPanel::default();
        assert_eq!(panel.start_bad, "HEAD");
        assert!(panel.start_good.is_empty());
        assert!(!panel.open);
    }

    #[test]
    fn toggle_open_first_time_emits_refresh() {
        let mut panel = BisectPanel::default();
        let action = panel.update(Msg::ToggleOpen);
        assert!(panel.open);
        assert!(panel.loading);
        assert_eq!(action, Some(Action::Refresh));
    }

    #[test]
    fn toggle_open_twice_emits_one_refresh() {
        let mut panel = BisectPanel::default();
        assert_eq!(panel.update(Msg::ToggleOpen), Some(Action::Refresh));
        // Close and reopen while the fetch is still in flight: the reopen
        // must not queue a second Refresh.
        panel.update(Msg::ToggleOpen);
        assert!(!panel.open);
        assert_eq!(panel.update(Msg::ToggleOpen), None);
        assert!(panel.open);
        assert!(panel.loading);
    }

    #[test]
    fn toggle_open_with_cached_state_loads_log_once() {
        // State arrives while the panel is still closed (shell preload).
        let mut panel = BisectPanel::default();
        panel.update(Msg::StateLoaded(Ok(fixture_state(true, 3))));
        assert_eq!(panel.update(Msg::ToggleOpen), Some(Action::LoadLog));
        panel.update(Msg::LogLoaded(vec![mark_entry("bad", "c0ffee1bad")]));
        assert_eq!(panel.update(Msg::ToggleOpen), None); // closed
        assert_eq!(panel.update(Msg::ToggleOpen), None); // reopen, cached
        assert!(panel.open);
    }

    #[test]
    fn start_requires_non_empty_bad() {
        let mut panel = panel_with(fixture_state(false, 0));
        panel.start_bad = "   ".to_string();
        assert_eq!(panel.update(Msg::StartPressed), None);
        assert!(!panel.loading);
    }

    #[test]
    fn start_blocked_while_active_or_loading() {
        let mut panel = panel_with(fixture_state(true, 3));
        assert_eq!(panel.update(Msg::StartPressed), None);
        panel = panel_with(fixture_state(false, 0));
        panel.loading = true;
        assert_eq!(panel.update(Msg::StartPressed), None);
    }

    #[test]
    fn start_trims_bad_and_defaults_good_to_none() {
        let mut panel = panel_with(fixture_state(false, 0));
        panel.start_bad = "  HEAD ".to_string();
        panel.start_good = "   ".to_string();
        assert_eq!(
            panel.update(Msg::StartPressed),
            Some(Action::Start {
                bad: "HEAD".to_string(),
                good: None,
            })
        );
        assert!(panel.loading);
    }

    #[test]
    fn start_passes_trimmed_good() {
        let mut panel = panel_with(fixture_state(false, 0));
        panel.start_good = " v1.2.0 ".to_string();
        assert_eq!(
            panel.update(Msg::StartPressed),
            Some(Action::Start {
                bad: "HEAD".to_string(),
                good: Some("v1.2.0".to_string()),
            })
        );
    }

    #[test]
    fn marks_require_active_state() {
        let mut panel = BisectPanel {
            open: true,
            ..BisectPanel::default()
        };
        assert_eq!(panel.update(Msg::MarkGood), None);
        assert_eq!(panel.update(Msg::MarkBad), None);
        assert_eq!(panel.update(Msg::MarkSkip), None);

        panel = panel_with(fixture_state(false, 0));
        assert_eq!(panel.update(Msg::MarkGood), None);

        panel = panel_with(fixture_state(true, 3));
        assert_eq!(
            panel.update(Msg::MarkGood),
            Some(Action::Mark(BisectMark::Good))
        );
        assert!(panel.loading);
        panel.loading = false;
        assert_eq!(
            panel.update(Msg::MarkBad),
            Some(Action::Mark(BisectMark::Bad))
        );
        panel.loading = false;
        assert_eq!(
            panel.update(Msg::MarkSkip),
            Some(Action::Mark(BisectMark::Skip))
        );
    }

    #[test]
    fn marks_disabled_once_first_bad_reached() {
        let state = fixture_state(true, 0);
        assert!(first_bad_reached(&state));
        assert!(!marks_enabled(&state, false));
        // Loading also disables, even before convergence.
        let probing = fixture_state(true, 2);
        assert!(marks_enabled(&probing, false));
        assert!(!marks_enabled(&probing, true));
        // Guard against the blank default state counting as converged.
        assert!(!first_bad_reached(&BisectState::default()));
    }

    #[test]
    fn reset_only_when_active() {
        let mut panel = panel_with(fixture_state(false, 0));
        assert_eq!(panel.update(Msg::ResetPressed), None);
        panel = panel_with(fixture_state(true, 1));
        assert_eq!(panel.update(Msg::ResetPressed), Some(Action::Reset));
        assert!(panel.loading);
    }

    #[test]
    fn state_loaded_error_clears_loading_keeps_panel() {
        let mut panel = panel_with(fixture_state(true, 2));
        panel.loading = true;
        panel.update(Msg::StateLoaded(Err("boom".to_string())));
        assert!(!panel.loading);
        assert_eq!(panel.error.as_deref(), Some("boom"));
        // A successful load clears the error and keeps the state.
        panel.update(Msg::StateLoaded(Ok(fixture_state(false, 0))));
        assert!(panel.error.is_none());
        assert!(!panel.is_active());
    }

    #[test]
    fn inactive_state_resets_start_form() {
        let mut panel = panel_with(fixture_state(true, 1));
        panel.start_bad = "v0.9".to_string();
        panel.start_good = "v0.8".to_string();
        panel.update(Msg::LogLoaded(vec![mark_entry("bad", "c0ffee1bad")]));
        panel.update(Msg::StateLoaded(Ok(fixture_state(false, 0))));
        assert_eq!(panel.start_bad, "HEAD");
        assert!(panel.start_good.is_empty());
        assert!(panel.log.is_none());
    }

    #[test]
    fn log_loaded_stores_entries() {
        let mut panel = BisectPanel::default();
        panel.update(Msg::LogLoaded(vec![
            mark_entry("good", "aaa"),
            mark_entry("bad", "bbb"),
        ]));
        let log = panel.log.expect("log stored");
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].mark, "bad");
    }

    #[test]
    fn refresh_emits_once_until_state_arrives() {
        let mut panel = panel_with(fixture_state(true, 2));
        assert_eq!(panel.update(Msg::RefreshPressed), Some(Action::Refresh));
        assert_eq!(panel.update(Msg::RefreshPressed), None); // still loading
        panel.update(Msg::StateLoaded(Ok(fixture_state(true, 2))));
        assert_eq!(panel.update(Msg::RefreshPressed), Some(Action::Refresh));
    }

    #[test]
    fn remaining_label_covers_all_phases() {
        assert_eq!(remaining_label(&fixture_state(true, 0)), "first bad commit");
        assert_eq!(remaining_label(&fixture_state(true, 1)), "1 candidate left");
        assert_eq!(
            remaining_label(&fixture_state(true, 5)),
            "5 candidates left"
        );
    }
}
