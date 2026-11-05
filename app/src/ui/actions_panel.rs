//! Shell actions panel: manage and run user-defined shell commands (M4,
//! contracts.md "Commands (M4)").
//!
//! Self-contained lane module (like [`super::health_panel`]): it owns its
//! state and its [`Msg`] enum, and returns [`Action`] intents from
//! [`update`] for the shell to translate into `service::misc` calls.
//! Nothing here touches services or the config store.
//!
//! # Persistence boundary
//!
//! The configured action list lives in the shell's user config — the panel
//! only edits a plain `Vec<ActionEntry>`: the shell loads it (usually on
//! first open) with [`ActionsPanel::set_actions`], persists the result of
//! each [`Action::Add`] / [`Action::Delete`], and feeds the authoritative
//! list back through [`ActionsPanel::set_actions`] alongside the `Added` /
//! `Deleted` confirmation messages. Validation (non-empty name/command,
//! unique name) is mirrored here so the Add button disables early; the
//! shell re-validates before persisting.
//!
//! # Run lifecycle
//!
//! `Action::Run` maps to `service::misc::action_run(services, repo_id,
//! name, command)` — a synchronous call returning the run id immediately
//! (`act-…` local, `ract-…` remote). `Ok(run_id)` is fed back as
//! `Msg::RunStarted { name, run_id }` and recorded in the running list;
//! `Err(e)` becomes `Msg::RunFailed(e)`. The backend enforces its own
//! per-repo concurrency cap (`actions::MAX_CONCURRENT_PER_REPO` = 3) and
//! reports violations through that error path. Output and exit arrive on
//! the backend event bus as `BackendEvent::ActionOutput` — see
//! [`super::output_panel`], the pane that displays them; the shell also
//! feeds the run's final `done: true` event here as
//! `Msg::RunFinished { run_id }` so the running list stays in sync.
//!
//! Keyboard shortcuts for actions (the old app's ctrl-N feature) are out of
//! scope for this panel — the orchestrator may wire global shortcuts in the
//! shell later.
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod actions_panel;` in `ui/mod.rs` (drop the temporary
//!    `#![allow(dead_code)]` at the top of this file in the same commit).
//! 2. Add `Actions(actions_panel::Msg)` to `app::Message` and hold one
//!    `ActionsPanel` per repo tab.
//! 3. Route messages: `Message::Actions(msg) => {
//!    let action = tab.actions.update(msg);
//!    Self::run_actions_action(repo_id, &mut tab.actions, action) }`.
//! 4. Execute actions:
//!    - `Action::Refresh`: load the configured list from the config store,
//!      then `tab.actions.set_actions(list)` (clears `loading`).
//!    - `Action::Add { name, command }`: persist into the config store,
//!      reload the list, `set_actions(updated)` and feed `Msg::Added`.
//!    - `Action::Delete { name }`: remove from the config store, reload,
//!      `set_actions(updated)` and feed `Msg::Deleted`.
//!    - `Action::Run { name, command }`:
//!      `service::misc::action_run(&services, repo_id, name, command)` →
//!      `Ok(run_id)`: feed `Msg::RunStarted { name, run_id }` and open the
//!      output pane with `output_panel::Msg::RunOpened { run_id, name }`;
//!      `Err(e)`: feed `Msg::RunFailed(e)`.
//!    - `Action::Cancel { run_id }`:
//!      `service::misc::action_cancel(&services, run_id)` (synchronous;
//!      `Ok(false)` just means the run already finished — ignore).
//! 5. Completion: on `BackendEvent::ActionOutput` with `done: true`, feed
//!    `Msg::RunFinished { run_id }` here (and the `Exit` message to the
//!    output panel — see its module docs).
//!
//! # Out of scope
//!
//! Keyboard shortcuts, command templates/placeholders and per-action
//! working-directory overrides are old-app features not carried over yet.

// Registered for gates before the shell wires this module in; drop this
// allow once `app.rs` routes `Msg`/`Action`.
#![allow(dead_code)]

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One configured shell action (the user config's shape; the shell owns
/// persistence, the panel only edits the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionEntry {
    /// Display name (unique within the list; also the run's label in the
    /// output panel and on `BackendEvent::ActionOutput`).
    pub name: String,
    /// Shell command text, run with cwd = repo root through the platform
    /// shell (`sh -c` / `cmd /C` — see `actions::build_shell_command`).
    pub command: String,
}

impl ActionEntry {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
        }
    }
}

/// One in-flight run (mirror of the backend's run registry for the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningAction {
    /// Run id from `action_run` (`act-…` local / `ract-…` remote); doubles
    /// as the `action_cancel` token and the output panel's run key.
    pub run_id: String,
    /// The action definition's display name.
    pub name: String,
    /// Lines the output panel already held for this run id when the start
    /// arrived — 0 for a fresh run; the shell snapshots the output panel's
    /// buffer size and patches it via [`ActionsPanel::set_started_line_count`]
    /// when attaching to a run that was already streaming.
    pub started_line_count: usize,
}

/// Panel state: the configured list, the add form and the running set.
#[derive(Debug, Default)]
pub struct ActionsPanel {
    pub open: bool,
    /// True while the shell reloads the configured list.
    pub loading: bool,
    pub actions: Vec<ActionEntry>,
    pub running: Vec<RunningAction>,
    /// Add-form validation failure or shell-fed failure (`RunFailed`),
    /// rendered in the destructive colour.
    pub error: Option<String>,
    pub new_name: String,
    pub new_command: String,
    /// Name parked on the inline delete confirm (one at a time).
    pub confirm_delete: Option<String>,
}

impl ActionsPanel {
    /// Replace the configured list (shell feeds it after loading or
    /// mutating the config store); clears `loading`.
    pub fn set_actions(&mut self, actions: Vec<ActionEntry>) {
        self.loading = false;
        self.actions = actions;
    }

    /// Patch the recorded output offset of a running action (see
    /// [`RunningAction::started_line_count`]). Returns `false` when the run
    /// is unknown.
    pub fn set_started_line_count(&mut self, run_id: &str, count: usize) -> bool {
        match self.running.iter_mut().find(|r| r.run_id == run_id) {
            Some(run) => {
                run.started_line_count = count;
                true
            }
            None => false,
        }
    }

    /// Whether an action with this name is currently running (the view
    /// disables its Run button).
    pub fn is_running(&self, name: &str) -> bool {
        self.running.iter().any(|r| r.name == name)
    }

    /// The configured list as slice (the shell reads it to persist).
    pub fn actions(&self) -> &[ActionEntry] {
        &self.actions
    }
}

// ---------------------------------------------------------------------------
// Pure validation helper (unit-tested; `update` has no other inputs)
// ---------------------------------------------------------------------------

/// Why the add form cannot be submitted yet, or `None` when it can: name
/// and command must be non-empty (trimmed) and the name must not collide
/// with a configured action.
pub fn add_validation_error(actions: &[ActionEntry], name: &str, command: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Name is required.".to_string());
    }
    if command.trim().is_empty() {
        return Some("Command is required.".to_string());
    }
    if actions.iter().any(|entry| entry.name == name) {
        return Some(format!("An action named `{name}` already exists."));
    }
    None
}

/// Clip a long string for display (same idea as health_panel's helper).
fn clip(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let cut: String = value.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

// ---------------------------------------------------------------------------
// Messages + actions
// ---------------------------------------------------------------------------

/// Panel messages: user input plus the `Done` results the shell feeds back.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Show/hide the panel.
    ToggleOpen,
    /// Reload the configured list from the shell's config store.
    RefreshPressed,

    // Add form.
    NameChanged(String),
    CommandChanged(String),
    AddPressed,

    // Delete (inline confirm on the row).
    DeleteRequested(String),
    DeleteConfirmed,
    DeleteCanceled,

    // Runs.
    RunPressed(String),
    CancelPressed(String),

    // Done messages the shell feeds after executing an `Action`.
    /// The add was persisted and the list refreshed.
    Added,
    /// The delete was persisted and the list refreshed.
    Deleted,
    /// `action_run` returned this run id for the action.
    RunStarted {
        name: String,
        run_id: String,
    },
    /// The run's final `done: true` event arrived on the bus.
    RunFinished {
        run_id: String,
    },
    /// `action_run` rejected the launch (empty command, concurrency cap, …).
    RunFailed(String),
}

/// Intents the shell executes (see the module docs for the exact
/// `service::misc` mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Load the configured action list from the shell's config store.
    Refresh,
    /// Persist a new action (`name`/`command` are pre-validated).
    Add { name: String, command: String },
    /// Remove the configured action with this name.
    Delete { name: String },
    /// Run the action via `service::misc::action_run`.
    Run { name: String, command: String },
    /// Kill a running action via `service::misc::action_cancel`.
    Cancel { run_id: String },
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

impl ActionsPanel {
    /// Pure state machine: apply `msg`, return the intent (if any) for the
    /// shell to execute. No service or config calls happen here.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ToggleOpen => {
                self.open = !self.open;
                if !self.open {
                    self.confirm_delete = None;
                }
                None
            }
            Msg::RefreshPressed => {
                self.loading = true;
                self.error = None;
                Some(Action::Refresh)
            }

            Msg::NameChanged(name) => {
                self.new_name = name;
                None
            }
            Msg::CommandChanged(command) => {
                self.new_command = command;
                None
            }
            Msg::AddPressed => {
                if let Some(error) =
                    add_validation_error(&self.actions, &self.new_name, &self.new_command)
                {
                    self.error = Some(error);
                    return None;
                }
                self.error = None;
                Some(Action::Add {
                    name: self.new_name.trim().to_string(),
                    command: self.new_command.trim().to_string(),
                })
            }

            Msg::DeleteRequested(name) => {
                if self.confirm_delete.as_deref() != Some(name.as_str()) {
                    self.confirm_delete = Some(name);
                }
                None
            }
            Msg::DeleteConfirmed => {
                let name = self.confirm_delete.take()?;
                Some(Action::Delete { name })
            }
            Msg::DeleteCanceled => {
                self.confirm_delete = None;
                None
            }

            Msg::RunPressed(name) => {
                // One instance per action at a time (guards double-clicks;
                // the backend's per-repo cap governs the rest).
                if self.is_running(&name) {
                    return None;
                }
                let command = self
                    .actions
                    .iter()
                    .find(|entry| entry.name == name)?
                    .command
                    .clone();
                Some(Action::Run { name, command })
            }
            Msg::CancelPressed(run_id) => self
                .running
                .iter()
                .any(|run| run.run_id == run_id)
                .then_some(Action::Cancel { run_id }),

            // Done messages.
            Msg::Added => {
                self.new_name.clear();
                self.new_command.clear();
                self.error = None;
                None
            }
            Msg::Deleted => {
                self.confirm_delete = None;
                None
            }
            Msg::RunStarted { name, run_id } => {
                // The shell may re-feed (e.g. a replayed launch result).
                if self.running.iter().any(|run| run.run_id == run_id) {
                    return None;
                }
                self.running.push(RunningAction {
                    run_id,
                    name,
                    started_line_count: 0,
                });
                None
            }
            Msg::RunFinished { run_id } => {
                self.running.retain(|run| run.run_id != run_id);
                None
            }
            Msg::RunFailed(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

impl ActionsPanel {
    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        if !self.open {
            return column![].into();
        }

        let mut header = row![text("Shell actions")
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
                button(text("⟳").size(theme::TEXT_SMALL))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Msg::RefreshPressed),
            )
            .push(
                button(text("×").size(theme::TEXT_BODY))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Msg::ToggleOpen),
            );

        let mut body = column![header].spacing(8).width(Length::Fill);
        if let Some(error) = &self.error {
            body = body.push(
                text(error.as_str())
                    .size(theme::TEXT_META)
                    .color(tokens.destructive)
                    .width(Length::Fill),
            );
        }
        body = body
            .push(self.configured_section(tokens))
            .push(self.add_section(tokens))
            .push(self.running_section(tokens));

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

    /// The configured list with per-row Run/Delete (and the inline delete
    /// confirm on the parked row).
    fn configured_section(&self, tokens: Tokens) -> Element<'_, Msg> {
        let mut section = column![text("Configured")
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground)]
        .spacing(4)
        .width(Length::Fill);

        if self.actions.is_empty() {
            section = section.push(
                text("No actions configured yet — add one below; it is stored in your config.")
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            );
            return section.into();
        }

        for entry in &self.actions {
            if self.confirm_delete.as_deref() == Some(entry.name.as_str()) {
                // Inline confirm on the parked row.
                section = section.push(
                    container(
                        row![
                            text(format!("Delete `{}`?", entry.name))
                                .size(theme::TEXT_META)
                                .width(Length::Fill),
                            button(text("Cancel").size(theme::TEXT_META))
                                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                                .padding([2, 8])
                                .on_press(Msg::DeleteCanceled),
                            button(text("Delete").size(theme::TEXT_META))
                                .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                                .padding([2, 8])
                                .on_press(Msg::DeleteConfirmed),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                    )
                    .padding([4, 6])
                    .style(move |_theme| container::Style {
                        background: Some(Background::Color(theme::with_alpha(
                            tokens.destructive,
                            0.08,
                        ))),
                        border: Border {
                            color: theme::with_alpha(tokens.destructive, 0.4),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..container::Style::default()
                    }),
                );
                continue;
            }

            let running = self.is_running(&entry.name);
            section = section.push(
                row![
                    column![
                        text(entry.name.as_str())
                            .size(theme::TEXT_SMALL)
                            .font(theme::semibold()),
                        text(clip(&entry.command, 72))
                            .size(theme::TEXT_META)
                            .font(theme::mono())
                            .color(tokens.muted_foreground),
                    ]
                    .spacing(1)
                    .width(Length::Fill),
                    button(text(if running { "Running" } else { "Run" }).size(theme::TEXT_META))
                        .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                        .padding([3, 10])
                        .on_press_maybe((!running).then(|| Msg::RunPressed(entry.name.clone()))),
                    button(text("Delete").size(theme::TEXT_META))
                        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                        .padding([3, 8])
                        .on_press(Msg::DeleteRequested(entry.name.clone())),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .width(Length::Fill),
            );
        }

        section.into()
    }

    /// The add form (name + command, validated non-empty and unique).
    fn add_section(&self, tokens: Tokens) -> Element<'_, Msg> {
        let valid =
            add_validation_error(&self.actions, &self.new_name, &self.new_command).is_none();
        column![
            text("Add action")
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
            row![
                text_input("name", self.new_name.as_str())
                    .on_input(Msg::NameChanged)
                    .size(theme::TEXT_SMALL)
                    .padding([4, 6])
                    .width(Length::Fixed(150.0)),
                text_input("command (cwd = repo root)", self.new_command.as_str())
                    .on_input(Msg::CommandChanged)
                    .on_submit_maybe(valid.then_some(Msg::AddPressed))
                    .size(theme::TEXT_SMALL)
                    .padding([4, 6])
                    .width(Length::Fill),
                button(text("Add").size(theme::TEXT_META))
                    .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                    .padding([4, 12])
                    .on_press_maybe(valid.then_some(Msg::AddPressed)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        ]
        .spacing(4)
        .width(Length::Fill)
        .into()
    }

    /// Live runs with their cancel affordances (output itself streams in
    /// the output panel).
    fn running_section(&self, tokens: Tokens) -> Element<'_, Msg> {
        let mut section = column![text("Running")
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground)]
        .spacing(4)
        .width(Length::Fill);

        if self.running.is_empty() {
            section = section.push(
                text("Nothing running.")
                    .size(theme::TEXT_META)
                    .color(tokens.muted_foreground),
            );
            return section.into();
        }

        for run in &self.running {
            section = section.push(
                row![
                    column![
                        text(run.name.as_str()).size(theme::TEXT_SMALL),
                        text(clip(&run.run_id, 24))
                            .size(theme::TEXT_META)
                            .font(theme::mono())
                            .color(tokens.muted_foreground),
                    ]
                    .spacing(1)
                    .width(Length::Fill),
                    button(text("Cancel").size(theme::TEXT_META))
                        .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                        .padding([3, 10])
                        .on_press(Msg::CancelPressed(run.run_id.clone())),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .width(Length::Fill),
            );
        }

        section.into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, command: &str) -> ActionEntry {
        ActionEntry::new(name, command)
    }

    // -- add validation -------------------------------------------------------

    #[test]
    fn add_requires_name_and_command() {
        let actions = vec![entry("test", "cargo test")];
        assert_eq!(
            add_validation_error(&actions, "", "cargo build"),
            Some("Name is required.".to_string())
        );
        assert_eq!(
            add_validation_error(&actions, "  ", "cargo build"),
            Some("Name is required.".to_string())
        );
        assert_eq!(
            add_validation_error(&actions, "build", "   "),
            Some("Command is required.".to_string())
        );
        assert_eq!(add_validation_error(&actions, "build", "cargo build"), None);
    }

    #[test]
    fn add_rejects_duplicate_names() {
        let actions = vec![entry("test", "cargo test")];
        assert_eq!(
            add_validation_error(&actions, "test", "cargo test --all"),
            Some("An action named `test` already exists.".to_string())
        );
        // Validation matches the trimmed name, like the Add path.
        assert_eq!(
            add_validation_error(&actions, " test ", "x"),
            Some("An action named `test` already exists.".to_string())
        );
        assert_eq!(add_validation_error(&actions, "Test", "x"), None);
    }

    #[test]
    fn add_pressed_validates_then_emits_trimmed_add() {
        let mut panel = ActionsPanel {
            open: true,
            ..Default::default()
        };
        panel.set_actions(vec![entry("test", "cargo test")]);

        // Invalid: parks the error, no intent.
        panel.update(Msg::NameChanged("test".into()));
        panel.update(Msg::CommandChanged("cargo build".into()));
        assert_eq!(panel.update(Msg::AddPressed), None);
        assert!(panel.error.is_some());

        // Valid: trimmed Add intent, error cleared.
        panel.update(Msg::NameChanged(" build ".into()));
        panel.update(Msg::CommandChanged("  cargo build  ".into()));
        assert_eq!(
            panel.update(Msg::AddPressed),
            Some(Action::Add {
                name: "build".to_string(),
                command: "cargo build".to_string(),
            })
        );
        assert!(panel.error.is_none());
    }

    #[test]
    fn added_clears_the_form() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::NameChanged("lint".into()));
        panel.update(Msg::CommandChanged("clippy".into()));
        panel.update(Msg::Added);
        assert!(panel.new_name.is_empty());
        assert!(panel.new_command.is_empty());
    }

    // -- delete flow ------------------------------------------------------------

    #[test]
    fn delete_needs_the_inline_confirm() {
        let mut panel = ActionsPanel::default();
        assert_eq!(panel.update(Msg::DeleteRequested("lint".into())), None);
        assert_eq!(panel.confirm_delete.as_deref(), Some("lint"));
        assert_eq!(
            panel.update(Msg::DeleteConfirmed),
            Some(Action::Delete {
                name: "lint".to_string(),
            })
        );
        assert!(panel.confirm_delete.is_none());
    }

    #[test]
    fn delete_confirm_can_be_canceled() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::DeleteRequested("lint".into()));
        panel.update(Msg::DeleteCanceled);
        assert!(panel.confirm_delete.is_none());
        assert_eq!(panel.update(Msg::DeleteConfirmed), None);
    }

    // -- run mapping --------------------------------------------------------------

    #[test]
    fn run_pressed_maps_name_to_configured_command() {
        let mut panel = ActionsPanel::default();
        panel.set_actions(vec![entry("test", "cargo test")]);
        assert_eq!(
            panel.update(Msg::RunPressed("test".into())),
            Some(Action::Run {
                name: "test".to_string(),
                command: "cargo test".to_string(),
            })
        );
    }

    #[test]
    fn run_pressed_ignores_unknown_or_already_running_actions() {
        let mut panel = ActionsPanel::default();
        panel.set_actions(vec![entry("test", "cargo test")]);

        assert_eq!(panel.update(Msg::RunPressed("nope".into())), None);

        panel.update(Msg::RunStarted {
            name: "test".into(),
            run_id: "act-1".into(),
        });
        assert!(panel.is_running("test"));
        assert_eq!(panel.update(Msg::RunPressed("test".into())), None);
    }

    #[test]
    fn cancel_pressed_only_fires_for_known_runs() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::RunStarted {
            name: "test".into(),
            run_id: "act-1".into(),
        });
        assert_eq!(
            panel.update(Msg::CancelPressed("act-1".into())),
            Some(Action::Cancel {
                run_id: "act-1".to_string(),
            })
        );
        assert_eq!(panel.update(Msg::CancelPressed("act-9".into())), None);
    }

    // -- running-list bookkeeping ---------------------------------------------------

    #[test]
    fn run_started_appends_and_dedupes() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::RunStarted {
            name: "test".into(),
            run_id: "act-1".into(),
        });
        panel.update(Msg::RunStarted {
            name: "test".into(),
            run_id: "act-1".into(),
        });
        assert_eq!(panel.running.len(), 1);
        assert_eq!(panel.running[0].started_line_count, 0);
    }

    #[test]
    fn run_finished_removes_only_that_run() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::RunStarted {
            name: "test".into(),
            run_id: "act-1".into(),
        });
        panel.update(Msg::RunStarted {
            name: "lint".into(),
            run_id: "ract-2".into(),
        });
        panel.update(Msg::RunFinished {
            run_id: "act-1".into(),
        });
        assert_eq!(panel.running.len(), 1);
        assert_eq!(panel.running[0].run_id, "ract-2");
        assert!(!panel.is_running("test"));
        assert!(panel.is_running("lint"));
    }

    #[test]
    fn started_line_count_patch_hits_known_runs_only() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::RunStarted {
            name: "test".into(),
            run_id: "act-1".into(),
        });
        assert!(panel.set_started_line_count("act-1", 42));
        assert_eq!(panel.running[0].started_line_count, 42);
        assert!(!panel.set_started_line_count("act-9", 1));
    }

    #[test]
    fn run_failed_parks_the_error() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::RunFailed(
            "repo r already has 3 actions running; cancel one first".into(),
        ));
        assert!(panel.error.is_some());
    }

    // -- misc ------------------------------------------------------------------

    #[test]
    fn set_actions_replaces_and_unlocks_loading() {
        let mut panel = ActionsPanel {
            loading: true,
            ..Default::default()
        };
        panel.set_actions(vec![entry("a", "b"), entry("c", "d")]);
        assert!(!panel.loading);
        assert_eq!(panel.actions().len(), 2);
        assert_eq!(panel.actions()[0], entry("a", "b"));
    }

    #[test]
    fn toggle_open_resets_the_delete_confirm() {
        let mut panel = ActionsPanel::default();
        panel.update(Msg::ToggleOpen); // opens
        assert!(panel.open);
        panel.update(Msg::DeleteRequested("lint".into()));
        panel.update(Msg::ToggleOpen); // closes
        assert!(!panel.open);
        assert!(panel.confirm_delete.is_none());
    }
}
