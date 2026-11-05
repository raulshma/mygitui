//! Output panel: live, per-run output streams for custom shell actions
//! (M4 `action-output` events) and generally for op logs.
//!
//! The backend streams each run's output on the event bus as
//! `BackendEvent::ActionOutput(ActionOutput { run_id, repo_id, name, line,
//! done, exit_code })` — one event per line, plus a final `done: true`
//! event carrying the exit code (lines always land before it). The shell
//! routes those events into this panel's [`Msg::Output`] / [`Msg::Exit`]
//! and the `action_run` result into [`Msg::RunOpened`]; [`update`] is pure
//! and only returns [`Action`] intents (cancel / follow-scroll) for the
//! shell to execute.
//!
//! Bookkeeping: runs are keyed by run id (`act-…` local, `ract-…` remote).
//! A `RunOpened` for an id that already has buffered lines (output raced
//! ahead of the launch result) keeps the lines and just fixes the label; an
//! `Output` for a never-opened run creates a placeholder section named by
//! run id so nothing is silently dropped. Lines are capped per run at
//! [`MAX_LINES_PER_RUN`] (oldest dropped, see [`cap_lines`]) on top of the
//! backend's own emission cap (`actions::MAX_OUTPUT_LINES`), and output
//! lines arriving after a run's `Exit` are dropped (the backend drops them
//! for killed process trees; this is the belt-and-braces half).
//!
//! View: one stacked section per run — name + status chip + cancel button,
//! then a monospace `scrollable` of the lines. New output re-pins the
//! scrollable to the bottom via [`Action::FollowScroll`] (the shell runs
//! [`snap_to_bottom`]); unlike the terminal pane there is no scroll-up
//! follow-mode release — this pane is a log watcher, so it always follows.
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod output_panel;` in `ui/mod.rs` (drop the temporary
//!    `#![allow(dead_code)]` at the top of this file in the same commit).
//! 2. Add `Output(output_panel::Msg)` to `app::Message` and hold one
//!    `OutputPanel` per repo tab (or one global — run ids are unique).
//! 3. Route messages: `Message::Output(msg) => {
//!    let action = tab.output.update(msg);
//!    Self::run_output_action(&mut tab.output, action) }`.
//! 4. Feed it from the backend event stream (wherever `BackendEvent` is
//!    dispatched):
//!    - `BackendEvent::ActionOutput(event)` with `!event.done` →
//!      `Msg::Output { run_id: event.run_id, line: event.line }`.
//!    - `BackendEvent::ActionOutput(event)` with `event.done == true` →
//!      `Msg::Exit { run_id: event.run_id, code: event.exit_code }` AND
//!      `actions_panel::Msg::RunFinished { run_id: event.run_id }` so the
//!      actions panel's running list stays in sync.
//!    - `service::misc::action_run` returning `Ok(run_id)` →
//!      `Msg::RunOpened { run_id, name }` (the same result also feeds the
//!      actions panel's `RunStarted`).
//! 5. Execute actions:
//!    - `Action::Cancel { run_id }`:
//!      `service::misc::action_cancel(&services, run_id)` (synchronous;
//!      `Ok(false)` = already finished — ignore).
//!    - `Action::FollowScroll { id }`: `output_panel::snap_to_bottom(id)`,
//!      batched/chained like any other task.

// Registered for gates before the shell wires this module in; drop this
// allow once `app.rs` routes `Msg`/`Action`.
#![allow(dead_code)]

use iced::widget::operation::snap_to_end;
use iced::widget::Id;
use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Task};

use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

/// Buffered lines kept per run; the oldest are dropped beyond it (pure
/// [`cap_lines`], unit-tested). The backend caps *emitted* lines at
/// `actions::MAX_OUTPUT_LINES` (10k); this keeps the UI side bounded too.
pub const MAX_LINES_PER_RUN: usize = 5_000;

/// Height of one expanded run's scroll area (stacked sections share the
/// panel, so each gets a fixed slice instead of competing for fill).
const RUN_BODY_HEIGHT: f32 = 220.0;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One run's accumulated output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    /// Run id from `action_run` (`act-…` / `ract-…`); unique per process.
    pub run_id: String,
    /// The action definition's display name (falls back to the run id for
    /// lines that arrived before the launch result was processed).
    pub name: String,
    /// Output lines, oldest first, capped at [`MAX_LINES_PER_RUN`].
    pub lines: Vec<String>,
    /// Exit status once the run's `done: true` event arrived: the exit code
    /// as a string, or `"signal"` when the backend reports no code (the
    /// process was killed by a signal — the backend encodes that as `-1`).
    pub exited: Option<String>,
    /// Whether the section's line list is rendered.
    pub expanded: bool,
}

impl RunView {
    /// Whether the run is still streaming.
    pub fn is_running(&self) -> bool {
        self.exited.is_none()
    }
}

/// Panel state: open flag plus one section per known run (oldest first).
#[derive(Debug, Default)]
pub struct OutputPanel {
    pub open: bool,
    pub runs: Vec<RunView>,
}

impl OutputPanel {
    /// The run view for `run_id`, if known.
    pub fn run(&self, run_id: &str) -> Option<&RunView> {
        self.runs.iter().find(|run| run.run_id == run_id)
    }

    /// Find-or-create the run section for `run_id` (placeholder name until
    /// a `RunOpened` fixes the label).
    fn ensure_run(&mut self, run_id: &str) -> &mut RunView {
        if let Some(index) = self.runs.iter().position(|run| run.run_id == run_id) {
            return &mut self.runs[index];
        }
        self.runs.push(RunView {
            run_id: run_id.to_owned(),
            name: run_id.to_owned(),
            lines: Vec::new(),
            exited: None,
            expanded: true,
        });
        self.runs.last_mut().expect("just pushed")
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Drop the oldest lines beyond `cap` (keeping the newest); returns how
/// many were dropped. Pure half of the per-run trim (mirrors
/// `terminal::trim_oldest`).
pub fn cap_lines(lines: &mut Vec<String>, cap: usize) -> usize {
    let excess = lines.len().saturating_sub(cap.max(1));
    if excess > 0 {
        lines.drain(0..excess);
    }
    excess
}

/// Stable scrollable id for a run (unique per section).
pub fn scroll_id_for(run_id: &str) -> Id {
    Id::from(format!("mygitui-output-{run_id}"))
}

/// Task pinning a run's scrollable to the newest line (the shell executes
/// [`Action::FollowScroll`] with this).
pub fn snap_to_bottom(id: Id) -> Task<Msg> {
    snap_to_end(id)
}

/// Display string for the backend's final `exit_code` (`None` = the process
/// had no exit code — killed by a signal).
fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(code) => code.to_string(),
        None => "signal".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Messages + actions
// ---------------------------------------------------------------------------

/// Panel messages: the shell feeds `RunOpened` / `Output` / `Exit` from the
/// `action_run` result and the `BackendEvent::ActionOutput` stream; the
/// rest is user input.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Show/hide the panel.
    ToggleOpen,
    /// `action_run` returned this run id (label the section; keeps any
    /// lines that already raced in under the placeholder name).
    RunOpened { run_id: String, name: String },
    /// One output line for a run.
    Output { run_id: String, line: String },
    /// The run's final `done: true` event; `code` is the backend's
    /// `exit_code` (`None` = killed by a signal).
    Exit { run_id: String, code: Option<i32> },
    /// Collapse/expand one run's line list.
    ExpandToggled(String),
    /// Cancel button on a running section.
    CancelPressed(String),
    /// Drop every finished run's section (running ones are kept).
    ClearFinished,
}

/// Intents the shell executes (see the module docs for the exact mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `service::misc::action_cancel(&services, run_id)`.
    Cancel { run_id: String },
    /// UI-only intent: pin the run's scrollable to the newest line with
    /// [`snap_to_bottom`].
    FollowScroll { id: Id },
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Pure state machine: apply `msg` to `panel`, return the intent (if any)
/// for the shell to execute. No service calls happen here.
pub fn update(panel: &mut OutputPanel, msg: Msg) -> Option<Action> {
    match msg {
        Msg::ToggleOpen => {
            panel.open = !panel.open;
            None
        }
        Msg::RunOpened { run_id, name } => {
            panel.ensure_run(&run_id).name = name;
            follow_if_visible(panel, &run_id)
        }
        Msg::Output { run_id, line } => {
            let run = panel.ensure_run(&run_id);
            if run.exited.is_some() {
                return None; // post-exit line (cancelled process tree): drop
            }
            run.lines.push(line);
            cap_lines(&mut run.lines, MAX_LINES_PER_RUN);
            follow_if_visible(panel, &run_id)
        }
        Msg::Exit { run_id, code } => {
            let run = panel.ensure_run(&run_id);
            run.exited = Some(exit_label(code));
            None
        }
        Msg::ExpandToggled(run_id) => {
            if let Some(run) = panel.runs.iter_mut().find(|run| run.run_id == run_id) {
                run.expanded = !run.expanded;
            }
            None
        }
        Msg::CancelPressed(run_id) => panel
            .run(run_id.as_str())
            .filter(|run| run.is_running())
            .map(|_| Action::Cancel { run_id }),
        Msg::ClearFinished => {
            panel.runs.retain(|run| run.is_running());
            None
        }
    }
}

/// Follow intent for a run's scrollable when the panel is open and the
/// section expanded (new output should keep the newest line visible).
fn follow_if_visible(panel: &OutputPanel, run_id: &str) -> Option<Action> {
    let follows = panel
        .run(run_id)
        .is_some_and(|run| run.expanded && run.exited.is_none());
    (panel.open && follows).then(|| Action::FollowScroll {
        id: scroll_id_for(run_id),
    })
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Render the panel: one stacked section per run (oldest first).
pub fn view(panel: &OutputPanel, tokens: Tokens) -> Element<'_, Msg> {
    if !panel.open {
        return column![].into();
    }

    let mut header = row![text("Action output")
        .size(theme::TEXT_BODY)
        .font(theme::semibold())]
    .spacing(6)
    .align_y(Alignment::Center);
    header = header
        .push(Space::new().width(Length::Fill))
        .push(
            button(text("Clear finished").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press_maybe(
                    panel
                        .runs
                        .iter()
                        .any(|run| !run.is_running())
                        .then_some(Msg::ClearFinished),
                ),
        )
        .push(
            button(text("×").size(theme::TEXT_BODY))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::ToggleOpen),
        );

    let mut body = column![header].spacing(8).width(Length::Fill);
    if panel.runs.is_empty() {
        body = body.push(
            text("No output yet — run a shell action and its lines stream here.")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }
    for run in &panel.runs {
        body = body.push(run_section(run, tokens));
    }

    container(
        scrollable(container(body).padding([8, 10]).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
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

/// One run section: header (expander, name, status chip, cancel) plus the
/// capped monospace line list when expanded.
fn run_section<'a>(run: &'a RunView, tokens: Tokens) -> Element<'a, Msg> {
    let arrow = if run.expanded { "▾" } else { "▸" };
    let running = run.is_running();

    let mut section = column![row![
        button(text(arrow).size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 6])
            .on_press(Msg::ExpandToggled(run.run_id.clone())),
        text(run.name.as_str())
            .size(theme::TEXT_SMALL)
            .font(theme::semibold())
            .width(Length::Fill),
        status_chip(run, tokens),
        button(text("Cancel").size(theme::TEXT_META))
            .style(theme::button_style(tokens, theme::ButtonKind::Danger))
            .padding([3, 10])
            .on_press_maybe(running.then(|| Msg::CancelPressed(run.run_id.clone()))),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill),]
    .spacing(4)
    .width(Length::Fill);

    if run.expanded {
        let mut lines = column![].width(Length::Fill);
        if run.lines.is_empty() {
            lines = lines.push(
                text(if running {
                    "waiting for output…"
                } else {
                    "(no output)"
                })
                .size(theme::TEXT_META)
                .font(theme::mono())
                .color(tokens.muted_foreground),
            );
        }
        for line in &run.lines {
            lines = lines.push(
                text(line.as_str())
                    .size(theme::TEXT_SMALL)
                    .font(theme::mono())
                    .width(Length::Fill),
            );
        }
        section = section.push(
            container(
                scrollable(lines)
                    .id(scroll_id_for(&run.run_id))
                    .width(Length::Fill),
            )
            .height(Length::Fixed(RUN_BODY_HEIGHT))
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(Background::Color(theme::with_alpha(
                    tokens.foreground,
                    if tokens.dark { 0.04 } else { 0.02 },
                ))),
                border: Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            })
            .padding([4, 6]),
        );
    }

    section.into()
}

/// Status chip text + colour: muted "running", green "exited (0)",
/// destructive "exited (N)" otherwise.
fn status_chip<'a>(run: &'a RunView, tokens: Tokens) -> Element<'a, Msg> {
    let (label, color): (String, Color) = match &run.exited {
        None => ("running".to_string(), tokens.muted_foreground),
        Some(code) if code == "0" => ("exited (0)".to_string(), tokens.added),
        Some(code) => (format!("exited ({code})"), tokens.destructive),
    };
    text(label)
        .size(theme::TEXT_META)
        .font(theme::mono())
        .color(color)
        .into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(panel: &mut OutputPanel, run_id: &str, name: &str) {
        update(
            panel,
            Msg::RunOpened {
                run_id: run_id.to_string(),
                name: name.to_string(),
            },
        );
    }

    // -- line capping -----------------------------------------------------------

    #[test]
    fn cap_lines_drops_the_oldest_beyond_the_cap() {
        let mut lines: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        assert_eq!(cap_lines(&mut lines, 6), 4);
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "4");
        assert_eq!(lines[5], "9");
    }

    #[test]
    fn cap_lines_below_cap_is_a_noop() {
        let mut lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(cap_lines(&mut lines, 10), 0);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn cap_lines_treats_zero_cap_as_one_like_the_terminal_trim() {
        // `cap.max(1)` mirrors `terminal::trim_oldest`: a zero cap keeps
        // exactly one line instead of panicking on an empty drain range.
        let mut lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(cap_lines(&mut lines, 0), 1);
        assert_eq!(lines, vec!["b".to_string()]);
    }

    #[test]
    fn output_is_capped_per_run() {
        let mut panel = OutputPanel {
            open: true,
            ..Default::default()
        };
        opened(&mut panel, "act-1", "test");
        for i in 0..(MAX_LINES_PER_RUN + 25) {
            update(
                &mut panel,
                Msg::Output {
                    run_id: "act-1".into(),
                    line: i.to_string(),
                },
            );
        }
        let run = panel.run("act-1").expect("run exists");
        assert_eq!(run.lines.len(), MAX_LINES_PER_RUN);
        assert_eq!(run.lines[0], "25", "oldest lines dropped");
        assert_eq!(
            run.lines.last().map(String::as_str),
            Some((MAX_LINES_PER_RUN + 24).to_string().as_str())
        );
    }

    // -- run opened / output bookkeeping -----------------------------------------

    #[test]
    fn run_opened_creates_an_expanded_running_section() {
        let mut panel = OutputPanel::default();
        opened(&mut panel, "act-1", "test");
        let run = panel.run("act-1").expect("run exists");
        assert_eq!(run.name, "test");
        assert!(run.is_running());
        assert!(run.expanded);
        assert!(run.lines.is_empty());
    }

    #[test]
    fn output_for_unknown_run_creates_a_placeholder_section() {
        let mut panel = OutputPanel::default();
        update(
            &mut panel,
            Msg::Output {
                run_id: "ract-7".into(),
                line: "hello".into(),
            },
        );
        let run = panel.run("ract-7").expect("placeholder exists");
        assert_eq!(run.name, "ract-7", "placeholder named by run id");
        assert_eq!(run.lines, vec!["hello".to_string()]);
    }

    #[test]
    fn run_opened_labels_a_placeholder_and_keeps_lines() {
        let mut panel = OutputPanel::default();
        update(
            &mut panel,
            Msg::Output {
                run_id: "act-1".into(),
                line: "early line".into(),
            },
        );
        opened(&mut panel, "act-1", "test");
        let run = panel.run("act-1").expect("run exists");
        assert_eq!(run.name, "test");
        assert_eq!(run.lines, vec!["early line".to_string()]);
    }

    #[test]
    fn multi_run_streams_stay_separate() {
        let mut panel = OutputPanel::default();
        opened(&mut panel, "act-1", "test");
        opened(&mut panel, "ract-2", "remote");
        update(
            &mut panel,
            Msg::Output {
                run_id: "act-1".into(),
                line: "local".into(),
            },
        );
        update(
            &mut panel,
            Msg::Output {
                run_id: "ract-2".into(),
                line: "remote".into(),
            },
        );
        assert_eq!(panel.runs.len(), 2);
        assert_eq!(
            panel.run("act-1").expect("a1").lines,
            vec!["local".to_string()]
        );
        assert_eq!(
            panel.run("ract-2").expect("r2").lines,
            vec!["remote".to_string()]
        );
    }

    // -- exit handling -------------------------------------------------------------

    #[test]
    fn exit_records_the_code_and_stops_following() {
        let mut panel = OutputPanel {
            open: true,
            ..Default::default()
        };
        opened(&mut panel, "act-1", "test");

        // While running, output follows the bottom.
        assert!(matches!(
            update(
                &mut panel,
                Msg::Output {
                    run_id: "act-1".into(),
                    line: "line".into(),
                },
            ),
            Some(Action::FollowScroll { .. })
        ));

        assert_eq!(
            update(
                &mut panel,
                Msg::Exit {
                    run_id: "act-1".into(),
                    code: Some(0),
                },
            ),
            None
        );
        let run = panel.run("act-1").expect("run exists");
        assert_eq!(run.exited.as_deref(), Some("0"));
        assert!(!run.is_running());

        // Lines after the final event are dropped.
        assert_eq!(
            update(
                &mut panel,
                Msg::Output {
                    run_id: "act-1".into(),
                    line: "late".into(),
                },
            ),
            None
        );
        assert_eq!(panel.run("act-1").expect("run").lines.len(), 1);
    }

    #[test]
    fn exit_without_a_code_reports_a_signal() {
        let mut panel = OutputPanel::default();
        opened(&mut panel, "act-1", "test");
        update(
            &mut panel,
            Msg::Exit {
                run_id: "act-1".into(),
                code: None,
            },
        );
        assert_eq!(
            panel.run("act-1").expect("run").exited.as_deref(),
            Some("signal")
        );
    }

    #[test]
    fn exit_creates_a_section_for_an_unknown_run_too() {
        let mut panel = OutputPanel::default();
        update(
            &mut panel,
            Msg::Exit {
                run_id: "act-404".into(),
                code: Some(3),
            },
        );
        assert_eq!(
            panel.run("act-404").expect("run").exited.as_deref(),
            Some("3")
        );
    }

    // -- cancel + clear finished ------------------------------------------------------

    #[test]
    fn cancel_only_fires_for_running_sections() {
        let mut panel = OutputPanel::default();
        opened(&mut panel, "act-1", "test");
        opened(&mut panel, "act-2", "lint");
        update(
            &mut panel,
            Msg::Exit {
                run_id: "act-2".into(),
                code: Some(1),
            },
        );
        assert_eq!(
            update(&mut panel, Msg::CancelPressed("act-1".into())),
            Some(Action::Cancel {
                run_id: "act-1".to_string(),
            })
        );
        assert_eq!(update(&mut panel, Msg::CancelPressed("act-2".into())), None);
        assert_eq!(update(&mut panel, Msg::CancelPressed("nope".into())), None);
    }

    #[test]
    fn clear_finished_keeps_running_sections() {
        let mut panel = OutputPanel::default();
        opened(&mut panel, "act-1", "test");
        opened(&mut panel, "act-2", "lint");
        opened(&mut panel, "act-3", "build");
        update(
            &mut panel,
            Msg::Exit {
                run_id: "act-2".into(),
                code: Some(0),
            },
        );
        update(&mut panel, Msg::ClearFinished);
        assert_eq!(panel.runs.len(), 2);
        assert!(panel.run("act-1").is_some());
        assert!(panel.run("act-3").is_some());
        assert!(panel.run("act-2").is_none());
    }

    // -- expand + follow -----------------------------------------------------------------

    #[test]
    fn expand_toggle_flips_and_gates_the_follow_intent() {
        let mut panel = OutputPanel {
            open: true,
            ..Default::default()
        };
        opened(&mut panel, "act-1", "test");
        update(&mut panel, Msg::ExpandToggled("act-1".into()));
        assert!(!panel.run("act-1").expect("run").expanded);
        assert_eq!(
            update(
                &mut panel,
                Msg::Output {
                    run_id: "act-1".into(),
                    line: "line".into(),
                },
            ),
            None,
            "collapsed sections do not follow"
        );
        update(&mut panel, Msg::ExpandToggled("act-1".into()));
        assert!(matches!(
            update(
                &mut panel,
                Msg::Output {
                    run_id: "act-1".into(),
                    line: "line".into(),
                },
            ),
            Some(Action::FollowScroll { .. })
        ));
    }

    #[test]
    fn follow_intent_requires_the_panel_to_be_open() {
        let mut panel = OutputPanel {
            open: false,
            ..Default::default()
        };
        opened(&mut panel, "act-1", "test");
        opened(&mut panel, "act-1", "test");
        assert_eq!(
            update(
                &mut panel,
                Msg::Output {
                    run_id: "act-1".into(),
                    line: "line".into(),
                },
            ),
            None
        );
    }

    #[test]
    fn scroll_ids_are_unique_per_run() {
        assert_ne!(scroll_id_for("act-1"), scroll_id_for("act-2"));
    }
}
