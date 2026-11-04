//! Interactive rebase planner dialog: per-commit actions for `rebase_start`.
//!
//! Self-contained like `bisect_panel`: the state ([`Planner`]) lives on
//! `Tab::pending_rebase_planner`, the shell routes [`Msg`] into
//! [`Planner::update`] (pure — no service calls) and maps the returned
//! [`Action`] onto the rebase service fns (`service::merge::rebase_plan`
//! loads the initial pick list, `autosquash_plan` rebuilds it,
//! `rebase_start` runs it), feeding results back through shell messages.
//!
//! The rows map 1:1 onto the engine's [`RebaseStep`] list (`sha`, `action`
//! and optional `new_message`); the dialog only adds display state (commit
//! summaries) and the pure action/reorder logic below.

use iced::widget::{button, column, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length};

use crate::engine::types::RebaseStep;

use super::app::Message;
use super::dialog;
use super::repo_view::short_sha;
use super::theme::{self, Tokens};

/// The per-commit actions the planner offers. `exec` stays engine-only: the
/// planner authors steps from existing commits, so there is nothing to
/// execute yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAction {
    Pick,
    Squash,
    Fixup,
    Drop,
    Edit,
    Reword,
}

/// Every action the planner offers, in cycle order (pick → squash → fixup →
/// drop → edit → reword).
pub const PLAN_ACTIONS: [PlanAction; 6] = [
    PlanAction::Pick,
    PlanAction::Squash,
    PlanAction::Fixup,
    PlanAction::Drop,
    PlanAction::Edit,
    PlanAction::Reword,
];

impl PlanAction {
    /// The action string the engine's `RebaseStep::action` expects.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
            Self::Edit => "edit",
            Self::Reword => "reword",
        }
    }

    /// The next action in cycle order (the row's action button steps
    /// through [`PLAN_ACTIONS`]: pick → squash → fixup → drop → edit →
    /// reword → pick).
    pub fn cycle(self) -> Self {
        let index = PLAN_ACTIONS
            .iter()
            .position(|action| *action == self)
            .unwrap_or(0);
        PLAN_ACTIONS[(index + 1) % PLAN_ACTIONS.len()]
    }

    /// Parses an engine action string; unknown actions (e.g. `exec`, which
    /// the planner never authors) surface as the pick default.
    pub fn parse(action: &str) -> Self {
        match action {
            "squash" => Self::Squash,
            "fixup" => Self::Fixup,
            "drop" => Self::Drop,
            "edit" => Self::Edit,
            "reword" => Self::Reword,
            _ => Self::Pick,
        }
    }
}

/// One planned commit: the engine step plus display state.
#[derive(Debug, Clone)]
pub struct PlanRow {
    pub sha: String,
    /// Commit subject from the loaded history (display only; empty when the
    /// commit is not in the streamed pages).
    pub summary: String,
    pub action: PlanAction,
    /// Reword replacement message (only meaningful for `reword`).
    pub new_message: Option<String>,
}

impl PlanRow {
    fn from_step(step: &RebaseStep) -> Self {
        Self {
            sha: step.sha.clone(),
            summary: String::new(),
            action: PlanAction::parse(&step.action),
            new_message: step.new_message.clone(),
        }
    }

    /// The engine step this row authors. Only `reword` carries a message
    /// (the engine ignores it for the other actions).
    pub fn to_step(&self) -> RebaseStep {
        RebaseStep {
            sha: self.sha.clone(),
            action: self.action.label().to_string(),
            new_message: (self.action == PlanAction::Reword)
                .then(|| self.new_message.clone().unwrap_or_default()),
        }
    }

    /// Cycles the action; entering `reword` prefills the replacement with
    /// the commit subject (an existing draft message is kept).
    pub fn cycle_action(&mut self) {
        self.action = self.action.cycle();
        if self.action == PlanAction::Reword
            && self
                .new_message
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            self.new_message = Some(self.summary.clone());
        }
    }
}

/// Planner rows from an engine plan (`rebase_plan` / `autosquash_plan`
/// output), in the same order.
pub fn rows_from_steps(steps: &[RebaseStep]) -> Vec<PlanRow> {
    steps.iter().map(PlanRow::from_step).collect()
}

/// The engine plan the rows author (the `rebase_start` argument).
pub fn steps_from_rows(rows: &[PlanRow]) -> Vec<RebaseStep> {
    rows.iter().map(PlanRow::to_step).collect()
}

/// Fills empty row summaries from a commit lookup (the streamed history;
/// shas missing from the loaded pages keep the empty summary and the dialog
/// shows the short sha).
pub fn fill_summaries(rows: &mut [PlanRow], lookup: impl Fn(&str) -> Option<String>) {
    for row in rows {
        if row.summary.is_empty() {
            if let Some(summary) = lookup(&row.sha) {
                row.summary = summary;
            }
        }
    }
}

/// Swaps a row with its neighbor (`up` moves toward the plan's start);
/// out-of-bounds indices and edge moves are no-ops.
pub fn move_row(rows: &mut [PlanRow], index: usize, up: bool) {
    let target = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1)
    };
    if let Some(target) = target {
        if target < rows.len() {
            rows.swap(index, target);
        }
    }
}

/// Planner messages (shell → [`Planner::update`]).
#[derive(Debug, Clone)]
pub enum Msg {
    /// The row's action button was pressed: cycle pick → … → reword.
    ActionCycled(usize),
    MoveUp(usize),
    MoveDown(usize),
    /// The reword input for a row changed.
    RewordChanged {
        index: usize,
        message: String,
    },
    /// Replace the plan with the `autosquash_plan` output (shell runs it).
    AutosquashPressed,
    /// Start the rebase with the authored steps (shell runs it).
    StartPressed,
}

/// Intents returned by [`Planner::update`] for the shell to execute.
#[derive(Debug, Clone)]
pub enum Action {
    /// Rebuild the plan via `service::autosquash_plan(repo_id, onto)`.
    Autosquash,
    /// Start the rebase via `service::merge::rebase_start`.
    Start {
        onto: String,
        steps: Vec<RebaseStep>,
    },
}

/// Pending interactive-rebase planner: the target commit the current branch
/// is rewritten onto plus the editable plan rows.
#[derive(Debug)]
pub struct Planner {
    /// The commit the plan replays onto (`rebase_start`'s `onto`).
    pub onto: String,
    /// Display subject of `onto` (from the history; may be empty).
    pub onto_summary: String,
    pub rows: Vec<PlanRow>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Planner {
    /// Opens the planner for rebasing onto `onto`; the shell loads the
    /// initial pick plan right after.
    pub fn new(onto: String, onto_summary: String) -> Self {
        Self {
            onto,
            onto_summary,
            rows: Vec::new(),
            loading: true,
            error: None,
        }
    }

    /// Applies a loaded plan (initial `rebase_plan` or `autosquash_plan`
    /// output) as the planner rows.
    pub fn plan_loaded(&mut self, steps: Vec<RebaseStep>) {
        self.rows = rows_from_steps(&steps);
        self.loading = false;
        self.error = None;
    }

    /// Pure state transition; returns the intent for the shell to run.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ActionCycled(index) => {
                if let Some(row) = self.rows.get_mut(index) {
                    row.cycle_action();
                }
                None
            }
            Msg::MoveUp(index) => {
                move_row(&mut self.rows, index, true);
                None
            }
            Msg::MoveDown(index) => {
                move_row(&mut self.rows, index, false);
                None
            }
            Msg::RewordChanged { index, message } => {
                if let Some(row) = self.rows.get_mut(index) {
                    row.new_message = Some(message);
                }
                None
            }
            Msg::AutosquashPressed => {
                if self.loading || self.rows.is_empty() {
                    return None;
                }
                self.loading = true;
                Some(Action::Autosquash)
            }
            Msg::StartPressed => {
                if self.loading {
                    return None;
                }
                Some(Action::Start {
                    onto: self.onto.clone(),
                    steps: steps_from_rows(&self.rows),
                })
            }
        }
    }

    /// What currently blocks starting the rebase, for the dialog hint (the
    /// shell also refuses to run a Start while these hold). Mirrors the
    /// engine's plan validation: squash/fixup fold into a previous commit
    /// (none exists at step 0) and reword needs a message.
    pub fn start_blocker(&self, dirty: bool) -> Option<&'static str> {
        if dirty {
            return Some("Stash your changes first");
        }
        if self.loading {
            return Some("Loading plan…");
        }
        if self.rows.is_empty() {
            return Some("Nothing to rebase");
        }
        if matches!(self.rows[0].action, PlanAction::Squash | PlanAction::Fixup) {
            return Some("The first step folds into its parent — start with a pick");
        }
        let reword_missing = self.rows.iter().any(|row| {
            row.action == PlanAction::Reword
                && row
                    .new_message
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
        });
        if reword_missing {
            return Some("Every reword needs a message");
        }
        None
    }
}

/// The planner modal card (over a dimmed scrim via [`dialog::modal`]).
pub fn dialog<'a>(tokens: Tokens, planner: &'a Planner, dirty: bool) -> Element<'a, Message> {
    let step_of = |msg: Msg| Message::RebasePlanner(msg);
    let interactive = !planner.loading;

    let mut list = column![].spacing(2).width(Length::Fill);
    for (index, row) in planner.rows.iter().enumerate() {
        let move_button = |label: &'static str, up: bool| {
            button(text(label).size(10).color(tokens.muted_foreground))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([1, 5])
                .on_press_maybe(interactive.then(|| {
                    step_of(if up {
                        Msg::MoveUp(index)
                    } else {
                        Msg::MoveDown(index)
                    })
                }))
        };
        let action_color = if row.action == PlanAction::Drop {
            tokens.deleted
        } else {
            tokens.foreground
        };
        let action = button(text(row.action.label()).size(10).color(action_color))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([2, 6])
            .on_press_maybe(interactive.then(|| step_of(Msg::ActionCycled(index))));
        let mut line: Element<'a, Message> = row![
            move_button("↑", true),
            move_button("↓", false),
            action,
            text(short_sha(&row.sha))
                .size(10)
                .font(theme::mono())
                .color(tokens.muted_foreground),
            text(row.summary.as_str())
                .size(11)
                .color(tokens.foreground)
                .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into();
        if row.action == PlanAction::Reword {
            line = column![
                line,
                text_input(
                    "New message",
                    row.new_message.as_deref().unwrap_or_default()
                )
                .on_input(move |message| step_of(Msg::RewordChanged { index, message }))
                .size(11)
                .padding([3, 6]),
            ]
            .spacing(2)
            .width(Length::Fill)
            .into();
        }
        list = list.push(line);
    }

    let mut body = column![
        text(format!("Rebase onto {}", short_sha(&planner.onto)))
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text(match (planner.rows.len(), planner.onto_summary.as_str()) {
            (0, _) => format!(
                "Replay the current branch's commits on top of {}.",
                short_sha(&planner.onto)
            ),
            (count, "") => format!(
                "Rewrite {count} commit{} of the current branch on top of “{}”. This changes their commit ids.",
                if count == 1 { "" } else { "s" },
                short_sha(&planner.onto)
            ),
            (count, summary) => format!(
                "Rewrite {count} commit{} of the current branch on top of “{summary}”. This changes their commit ids.",
                if count == 1 { "" } else { "s" },
            ),
        })
        .size(12)
        .color(tokens.muted_foreground),
    ]
    .spacing(6);

    if planner.loading {
        body = body.push(
            text("Loading plan…")
                .size(11)
                .color(tokens.muted_foreground),
        );
    }
    if let Some(error) = &planner.error {
        body = body.push(
            text(error.as_str())
                .size(11)
                .color(tokens.deleted)
                .width(Length::Fill),
        );
    }
    if !planner.rows.is_empty() {
        body = body.push(
            scrollable(list)
                .height(Length::Fixed(320.0))
                .width(Length::Fill),
        );
        body = body.push(
            button(text("Autosquash fixups").size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([3, 8])
                .on_press_maybe(interactive.then(|| step_of(Msg::AutosquashPressed))),
        );
    }
    if let Some(blocker) = planner.start_blocker(dirty) {
        body = body.push(text(blocker).size(10).color(tokens.deleted));
    }

    let actions = dialog::action_row(
        tokens,
        "Start rebase",
        (planner.start_blocker(dirty).is_none()).then(|| step_of(Msg::StartPressed)),
    );

    dialog::modal(tokens, 540.0, body.into(), actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(sha: &str, action: &str) -> RebaseStep {
        RebaseStep {
            sha: sha.to_string(),
            action: action.to_string(),
            new_message: None,
        }
    }

    fn row(sha: &str, action: PlanAction) -> PlanRow {
        PlanRow {
            sha: sha.to_string(),
            summary: String::new(),
            action,
            new_message: None,
        }
    }

    #[test]
    fn fill_summaries_only_touches_empty_summaries() {
        let mut rows = rows_from_steps(&[step("1111111", "pick"), step("2222222", "pick")]);
        rows[1].summary = "kept".to_string();
        fill_summaries(&mut rows, |sha| match sha {
            "1111111" => Some("first subject".to_string()),
            "2222222" => Some("must not overwrite".to_string()),
            _ => None,
        });
        assert_eq!(rows[0].summary, "first subject");
        assert_eq!(rows[1].summary, "kept");
        // Unknown shas stay empty (the dialog shows the short sha).
        rows.push(row("3333333", PlanAction::Pick));
        fill_summaries(&mut rows, |_| None);
        assert_eq!(rows[2].summary, "");
    }

    #[test]
    fn action_cycle_visits_every_action_and_wraps() {
        let start = PlanAction::Pick;
        let mut seen = vec![start];
        let mut current = start;
        for _ in 0..PLAN_ACTIONS.len() {
            current = current.cycle();
            seen.push(current);
        }
        // After six cycles it is back at pick, and every action appeared.
        assert_eq!(current, PlanAction::Pick);
        let mut unique = seen.clone();
        unique.sort_by_key(|action| PLAN_ACTIONS.iter().position(|a| *a == *action).unwrap());
        unique.dedup();
        assert_eq!(unique.len(), PLAN_ACTIONS.len());
    }

    #[test]
    fn action_labels_match_the_engine_vocabulary() {
        // The engine accepts exactly these action strings (engine/rebase.rs).
        let expected = ["pick", "squash", "fixup", "drop", "edit", "reword"];
        for action in PLAN_ACTIONS {
            assert!(expected.contains(&action.label()));
        }
        // Parse round-trips, and unknown actions fall back to pick.
        for action in PLAN_ACTIONS {
            assert_eq!(PlanAction::parse(action.label()), action);
        }
        assert_eq!(PlanAction::parse("exec"), PlanAction::Pick);
        assert_eq!(PlanAction::parse("nonsense"), PlanAction::Pick);
    }

    #[test]
    fn move_row_swaps_neighbors_and_ignores_edges() {
        let mut rows = vec![
            row("a", PlanAction::Pick),
            row("b", PlanAction::Fixup),
            row("c", PlanAction::Drop),
        ];
        move_row(&mut rows, 1, true);
        assert_eq!(
            rows.iter().map(|r| r.sha.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        move_row(&mut rows, 0, true);
        assert_eq!(
            rows.iter().map(|r| r.sha.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        move_row(&mut rows, 2, false);
        assert_eq!(
            rows.iter().map(|r| r.sha.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        move_row(&mut rows, 2, true);
        assert_eq!(
            rows.iter().map(|r| r.sha.as_str()).collect::<Vec<_>>(),
            ["b", "c", "a"]
        );
        // Out-of-range indices are no-ops.
        move_row(&mut rows, 9, true);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn steps_round_trip_preserves_sha_action_and_reword_message() {
        let mut rows = rows_from_steps(&[
            step("1111111", "pick"),
            step("2222222", "fixup"),
            step("3333333", "drop"),
            RebaseStep {
                sha: "4444444".to_string(),
                action: "reword".to_string(),
                new_message: Some("renamed".to_string()),
            },
        ]);
        assert_eq!(rows[3].action, PlanAction::Reword);
        // Unknown engine actions surface as pick (exec is engine-only).
        rows.push(PlanRow::from_step(&step("5555555", "exec")));
        let steps = steps_from_rows(&rows);
        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0].action, "pick");
        assert_eq!(steps[0].new_message, None);
        assert_eq!(steps[1].action, "fixup");
        assert_eq!(steps[2].action, "drop");
        assert_eq!(steps[3].action, "reword");
        assert_eq!(steps[3].new_message.as_deref(), Some("renamed"));
        // Non-reword rows carry no message.
        assert_eq!(steps[4].action, "pick");
        assert_eq!(steps[4].new_message, None);
    }

    #[test]
    fn cycling_into_reword_prefills_the_subject() {
        let mut row = row("1111111", PlanAction::Pick);
        row.summary = "add feature".to_string();
        row.cycle_action();
        assert_eq!(row.action, PlanAction::Squash);
        assert_eq!(row.new_message, None);
        row.cycle_action();
        assert_eq!(row.action, PlanAction::Fixup);
        row.cycle_action();
        assert_eq!(row.action, PlanAction::Drop);
        row.cycle_action();
        assert_eq!(row.action, PlanAction::Edit);
        row.cycle_action();
        assert_eq!(row.action, PlanAction::Reword);
        assert_eq!(row.new_message.as_deref(), Some("add feature"));
        // An existing draft message survives leaving and re-entering reword.
        row.new_message = Some("typed by hand".to_string());
        for _ in 0..PLAN_ACTIONS.len() {
            row.cycle_action();
        }
        assert_eq!(row.action, PlanAction::Reword);
        assert_eq!(row.new_message.as_deref(), Some("typed by hand"));
    }

    #[test]
    fn update_routes_row_edits_and_emits_intents() {
        let mut planner = Planner::new("9999999".to_string(), String::new());
        planner.plan_loaded(vec![step("1111111", "pick"), step("2222222", "pick")]);

        assert!(planner.update(Msg::ActionCycled(0)).is_none());
        assert_eq!(planner.rows[0].action, PlanAction::Squash);
        assert!(planner.update(Msg::MoveDown(0)).is_none());
        assert_eq!(planner.rows[0].sha, "2222222");
        assert!(planner
            .update(Msg::RewordChanged {
                index: 1,
                message: "new text".into()
            })
            .is_none());
        assert_eq!(planner.rows[1].new_message.as_deref(), Some("new text"));

        // Start emits the authored steps and the onto target.
        planner.plan_loaded(vec![step("1111111", "pick")]);
        match planner.update(Msg::StartPressed) {
            Some(Action::Start { onto, steps }) => {
                assert_eq!(onto, "9999999");
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].sha, "1111111");
                assert_eq!(steps[0].action, "pick");
                assert_eq!(steps[0].new_message, None);
            }
            other => panic!("expected a start intent, got {other:?}"),
        }

        // Autosquash emits its intent (and flips the loading flag).
        planner.plan_loaded(vec![step("1111111", "pick")]);
        assert!(matches!(
            planner.update(Msg::AutosquashPressed),
            Some(Action::Autosquash)
        ));
        assert!(planner.loading);
        // While loading, further intents are suppressed.
        assert!(planner.update(Msg::StartPressed).is_none());
        assert!(planner.update(Msg::AutosquashPressed).is_none());
    }

    #[test]
    fn start_blocker_guards_dirty_tree_and_engine_validation() {
        let mut planner = Planner::new("9999999".to_string(), String::new());
        // Loading plans cannot start (dirty wins for the hint text).
        assert_eq!(
            planner.start_blocker(true),
            Some("Stash your changes first")
        );
        assert_eq!(planner.start_blocker(false), Some("Loading plan…"));

        planner.plan_loaded(vec![step("1111111", "pick")]);
        assert_eq!(
            planner.start_blocker(false),
            None,
            "clean tree + picks start"
        );
        assert_eq!(
            planner.start_blocker(true),
            Some("Stash your changes first")
        );

        // The engine rejects a leading squash/fixup (no previous commit).
        planner.rows.insert(0, row("0000000", PlanAction::Fixup));
        assert_eq!(
            planner.start_blocker(false),
            Some("The first step folds into its parent — start with a pick")
        );
        planner.rows[0].action = PlanAction::Squash;
        assert!(planner.start_blocker(false).is_some());

        // Reword rows need a non-empty message.
        planner.rows[0].action = PlanAction::Pick;
        planner.rows.push(PlanRow {
            sha: "2222222".to_string(),
            summary: "subject".to_string(),
            action: PlanAction::Reword,
            new_message: Some("  ".to_string()),
        });
        assert_eq!(
            planner.start_blocker(false),
            Some("Every reword needs a message")
        );
        planner.rows[2].new_message = Some("real message".to_string());
        assert_eq!(planner.start_blocker(false), None);

        // An empty plan has nothing to rebase.
        planner.rows.clear();
        assert_eq!(planner.start_blocker(false), Some("Nothing to rebase"));
    }
}
