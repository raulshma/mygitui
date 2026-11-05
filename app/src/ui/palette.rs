//! Command palette: a centered overlay that filters a registry of commands
//! and turns the selected row into an intent.
//!
//! Pure UI + intent module: no service calls, no `Task`. The orchestrator
//! wires it into the shell like this:
//!
//! 1. Own a [`State`] and register the commands the shell can execute via
//!    [`CommandSet::register`] / [`CommandSet::register_many`].
//! 2. Forward key presses by adding
//!    `iced::event::listen_with(palette::event_to_msg)` to the app
//!    subscription (alongside the existing modifiers listener) and route the
//!    resulting [`Msg`] through [`update`], translating the returned
//!    [`Action`] into shell messages. Keys are ignored while the palette is
//!    closed, so the subscription can stay unconditional.
//! 3. Layer [`view`] above the workspace while `state.open` is set, the same
//!    way `dialog::confirm` is layered today.
//! 4. Open it from the global hotkey (e.g. ctrl/⌘+P) via `Msg::Toggle` or
//!    `State::open` directly.
//!
//! Typing flows through `Msg::Key` (`Key::Character` appends, `Backspace`
//! deletes), so the query input in the view is a read-only display field and
//! keystrokes are never handled twice.

// Consumed by the app shell once the orchestrator registers this module;
// until then the crate cannot reach every entry point from its roots.
#![allow(dead_code)]

use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Element, Length};

use super::dialog::modal_with_cancel;
use super::theme::{self, Tokens};

/// Width of the palette card.
const PALETTE_WIDTH: f32 = 420.0;
/// Estimated height of one result row (used to size the scroll area).
const ROW_HEIGHT: f32 = 24.0;
/// Upper bound for the result list before it scrolls.
const LIST_MAX_HEIGHT: f32 = 320.0;
/// Rows jumped by the page up/page down keys.
const PAGE_SIZE: usize = 8;

/// A command the shell can execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
    pub section: String,
    pub keybind: Option<String>,
    pub enabled: bool,
    pub danger: bool,
}

impl Command {
    /// A plain enabled command without a keybind.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        section: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            section: section.into(),
            keybind: None,
            enabled: true,
            danger: false,
        }
    }

    /// Sets the shortcut hint shown right-aligned.
    pub fn keybind(mut self, keybind: impl Into<String>) -> Self {
        self.keybind = Some(keybind.into());
        self
    }

    /// Marks the command as unavailable: shown dimmed, not activatable.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Marks the command as destructive (tinted with the destructive token).
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// Registry of palette commands.
///
/// Deduplicated by [`Command::id`]: re-registering an id replaces the entry
/// in place. The set is always kept in stable *section, then insertion*
/// order (Rust's sort is stable, so equal sections keep registration order).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandSet {
    commands: Vec<Command>,
}

impl CommandSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one command, replacing an existing entry with the same id.
    pub fn register(&mut self, command: Command) {
        match self
            .commands
            .iter_mut()
            .find(|existing| existing.id == command.id)
        {
            Some(existing) => *existing = command,
            None => self.commands.push(command),
        }
        self.commands.sort_by(|a, b| a.section.cmp(&b.section));
    }

    /// Registers several commands (see [`Self::register`]).
    pub fn register_many(&mut self, commands: impl IntoIterator<Item = Command>) {
        for command in commands {
            self.register(command);
        }
    }

    /// The commands in display order (section, then insertion).
    pub fn as_slice(&self) -> &[Command] {
        &self.commands
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Match tier of `needle` inside `haystack`, case-insensitive:
/// `0` exact prefix, `1` word-boundary match, `2` substring, no match.
fn match_rank(haystack: &str, needle: &str) -> Option<u8> {
    let haystack = haystack.to_lowercase();
    let index = haystack.find(needle)?;
    Some(if index == 0 {
        0
    } else if haystack[..index]
        .chars()
        .next_back()
        .is_some_and(is_word_separator)
    {
        1
    } else {
        2
    })
}

/// Any non-alphanumeric character starts a word (` `, `-`, `_`, `/`, …).
fn is_word_separator(c: char) -> bool {
    !c.is_alphanumeric()
}

/// Indices into `commands` of the entries matching `query`, best first.
///
/// A command matches when the query hits its label, section or keybind; the
/// best tier across those three fields decides (exact prefix, then
/// word-boundary match, then substring), ties keep the original order. An
/// empty (or whitespace-only) query yields every index in the original
/// order. Disabled commands rank like any other; whether they can be
/// activated is decided by [`update`], not here.
pub fn rank_commands(commands: &[Command], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return (0..commands.len()).collect();
    }
    let mut ranked: Vec<(u8, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            let tier = [
                match_rank(&command.label, &needle),
                match_rank(&command.section, &needle),
                command
                    .keybind
                    .as_deref()
                    .and_then(|keybind| match_rank(keybind, &needle)),
            ]
            .into_iter()
            .flatten()
            .min()?;
            Some((tier, index))
        })
        .collect();
    ranked.sort_by_key(|entry| (entry.0, entry.1));
    ranked.into_iter().map(|(_, index)| index).collect()
}

/// Palette state owned by the shell.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub open: bool,
    pub query: String,
    /// Index into the ranked (visible) rows; see [`State::ranked`].
    pub selected: usize,
    pub commands: CommandSet,
}

impl State {
    /// Opens the palette with a fresh query and selection.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Indices into `State::commands` of the visible rows, best first.
    pub fn ranked(&self) -> Vec<usize> {
        rank_commands(self.commands.as_slice(), &self.query)
    }
}

/// Messages the shell forwards to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Open the palette (global hotkey).
    Open,
    /// Dismiss the palette; also produced by pressing the backdrop.
    Close,
    /// Open when closed, close when open (global hotkey).
    Toggle,
    /// Key press forwarded from the shell's event listener.
    Key(Key, Modifiers),
    /// Press on a visible row; the index is into [`State::ranked`].
    RowClicked(usize),
}

/// Intents the shell translates into service calls / shell messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run the command with this id.
    RunCommand(String),
    /// The palette dismissed itself (escape or backdrop); the shell may
    /// restore focus to the workspace. `open` is already `false`.
    Close,
}

/// Maps a raw iced event to a palette message; wire with
/// `iced::event::listen_with(palette::event_to_msg)`.
pub fn event_to_msg(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Msg> {
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Msg::Key(key, modifiers))
        }
        _ => None,
    }
}

/// Steps the palette; returns the intent for the shell to translate, if any.
///
/// `Msg::Open` / `Msg::Toggle` are shell-initiated and return `None`; a
/// user-initiated dismissal (escape, backdrop, `Msg::Close`) yields
/// [`Action::Close`], activation yields [`Action::RunCommand`]. Disabled
/// commands are visible but not activatable: enter or click on one is a
/// no-op and keeps the palette open.
pub fn update(state: &mut State, msg: Msg) -> Option<Action> {
    match msg {
        Msg::Open => {
            state.open();
            None
        }
        Msg::Toggle => {
            state.toggle();
            None
        }
        Msg::Close if state.open => {
            state.close();
            Some(Action::Close)
        }
        Msg::Close => None,
        Msg::Key(key, modifiers) if state.open => handle_key(state, key, modifiers),
        Msg::RowClicked(position) if state.open => activate(state, position),
        Msg::Key(..) | Msg::RowClicked(_) => None,
    }
}

fn handle_key(state: &mut State, key: Key, modifiers: Modifiers) -> Option<Action> {
    // Plain typing (no ctrl/alt/⌘) edits the query; modifier combos are left
    // for the shell's own hotkeys.
    if let Key::Character(input) = &key {
        if !(modifiers.control() || modifiers.alt() || modifiers.command()) {
            state.query.push_str(input);
            state.selected = 0;
        }
        return None;
    }

    let named = match key {
        Key::Named(named) => named,
        // (Characters were already consumed above; the arm is still required
        // for exhaustiveness.)
        Key::Character(_) | Key::Unidentified => return None,
    };

    match named {
        Named::Escape => {
            state.close();
            Some(Action::Close)
        }
        Named::Enter => activate(state, state.selected),
        Named::Backspace => {
            state.query.pop();
            state.selected = 0;
            None
        }
        Named::ArrowDown => {
            move_selection(state, 1);
            None
        }
        Named::ArrowUp => {
            move_selection(state, -1);
            None
        }
        Named::PageDown => {
            clamp_selection(state, PAGE_SIZE as i32);
            None
        }
        Named::PageUp => {
            clamp_selection(state, -(PAGE_SIZE as i32));
            None
        }
        Named::Home => {
            state.selected = 0;
            None
        }
        Named::End => {
            state.selected = state.ranked().len().saturating_sub(1);
            None
        }
        _ => None,
    }
}

/// Moves the selection by `delta` visible rows, wrapping around both ends.
fn move_selection(state: &mut State, delta: i32) {
    let len = state.ranked().len();
    if len == 0 {
        state.selected = 0;
        return;
    }
    let current = state.selected.min(len - 1) as i32;
    state.selected = (current + delta).rem_euclid(len as i32) as usize;
}

/// Moves the selection by `delta` visible rows, clamped to the list bounds.
fn clamp_selection(state: &mut State, delta: i32) {
    let len = state.ranked().len();
    if len == 0 {
        state.selected = 0;
        return;
    }
    let current = state.selected.min(len - 1) as i32;
    state.selected = (current + delta).clamp(0, len as i32 - 1) as usize;
}

/// Activates the visible row at `position`, if it exists and is enabled.
fn activate(state: &mut State, position: usize) -> Option<Action> {
    let id = {
        let ranked = state.ranked();
        let index = *ranked.get(position)?;
        let command = state.commands.as_slice().get(index)?;
        if !command.enabled {
            return None;
        }
        command.id.clone()
    };
    state.close();
    Some(Action::RunCommand(id))
}

/// Overlay for the current state; layer above the workspace while
/// `state.open` is set (a zero-sized element when closed).
pub fn view(state: &State, tokens: &Tokens) -> Element<'static, Msg> {
    if !state.open {
        return Space::new().width(0).height(0).into();
    }
    // Own the tokens so style closures capture a `Copy` value instead of the
    // reference (keeping the element `'static`).
    let tokens = *tokens;

    let query_row = row![
        text("›").size(13).color(tokens.muted_foreground),
        text_input("Type a command…", state.query.as_str())
            .size(theme::TEXT_BODY)
            .padding([4, 4])
            .width(Length::Fill),
        text("esc to close")
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let ranked = state.ranked();
    let selected = state.selected.min(ranked.len().saturating_sub(1));
    let commands = state.commands.as_slice();

    let list: Element<'static, Msg> = if ranked.is_empty() {
        container(
            text(if state.commands.is_empty() {
                "No commands registered."
            } else {
                "No matching commands."
            })
            .size(theme::TEXT_SMALL)
            .color(tokens.muted_foreground),
        )
        .padding([6, 2])
        .into()
    } else {
        let height = (ranked.len() as f32 * ROW_HEIGHT).clamp(ROW_HEIGHT, LIST_MAX_HEIGHT);
        let mut rows = column![].spacing(1).width(Length::Fill);
        for (position, index) in ranked.into_iter().enumerate() {
            rows = rows.push(command_row(
                &commands[index],
                position,
                position == selected,
                &tokens,
            ));
        }
        scrollable(rows)
            .height(Length::Fixed(height))
            .width(Length::Fill)
            .into()
    };

    // Shared modal scaffold (its card look replaces the popover tint so
    // every overlay reads the same): the query input and row buttons consume
    // their own presses; presses on the scrim or the card's padding close
    // the palette.
    modal_with_cancel(
        tokens,
        PALETTE_WIDTH,
        column![query_row, Space::new().height(4), list]
            .spacing(6)
            .into(),
        Space::new().width(0).into(),
        Msg::Close,
    )
}

fn command_row(
    command: &Command,
    position: usize,
    selected: bool,
    tokens: &Tokens,
) -> Element<'static, Msg> {
    let tokens = *tokens;
    let (label_color, meta_color) = if !command.enabled {
        (tokens.disabled_fg, tokens.disabled_fg)
    } else if command.danger {
        (tokens.destructive, tokens.muted_foreground)
    } else {
        (tokens.foreground, tokens.muted_foreground)
    };

    let line = row![
        text(command.section.clone())
            .size(theme::TEXT_META)
            .color(meta_color),
        text(command.label.clone())
            .size(theme::TEXT_SMALL)
            .color(label_color),
        Space::new().width(Length::Fill),
        text(command.keybind.clone().unwrap_or_default())
            .size(theme::TEXT_META)
            .font(theme::mono())
            .color(meta_color),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(
        button(container(line))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .width(Length::Fill)
            .on_press_maybe(command.enabled.then_some(Msg::RowClicked(position))),
    )
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(if selected {
            tokens.active
        } else {
            theme::with_alpha(tokens.popover, 0.0)
        })),
        ..container::Style::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Named;

    fn command(id: &str, label: &str, section: &str) -> Command {
        Command::new(id, label, section)
    }

    fn key(named: Named) -> Msg {
        Msg::Key(Key::Named(named), Modifiers::default())
    }

    fn character(input: &str) -> Msg {
        Msg::Key(Key::Character(input.into()), Modifiers::default())
    }

    fn open_state(commands: Vec<Command>) -> State {
        let mut state = State::default();
        state.commands.register_many(commands);
        state.open();
        state
    }

    #[test]
    fn registry_dedupes_by_id_keeping_position() {
        let mut set = CommandSet::new();
        set.register(command("stage", "Stage", "Git"));
        set.register(command("stash", "Stash", "Git"));
        set.register(command("open", "Open repo", "App"));
        set.register(Command::new("stage", "Stage all", "Git"));

        assert_eq!(set.len(), 3);
        let ids: Vec<&str> = set.as_slice().iter().map(|c| c.id.as_str()).collect();
        // Section order first ("App" < "Git"), then insertion order.
        assert_eq!(ids, ["open", "stage", "stash"]);
        assert_eq!(set.as_slice()[1].label, "Stage all");
    }

    #[test]
    fn register_many_dedupes() {
        let mut set = CommandSet::new();
        set.register_many(vec![
            command("a", "First", "S"),
            command("b", "Second", "S"),
            command("a", "First v2", "S"),
        ]);
        assert_eq!(set.len(), 2);
        assert_eq!(set.as_slice()[0].label, "First v2");
        assert_eq!(set.as_slice()[1].id, "b");
    }

    #[test]
    fn rank_empty_query_keeps_original_order() {
        let commands = vec![
            command("1", "Zebra", "B"),
            command("2", "Apple", "A"),
            command("3", "Mango", "C"),
        ];
        assert_eq!(rank_commands(&commands, ""), vec![0, 1, 2]);
        assert_eq!(rank_commands(&commands, "   "), vec![0, 1, 2]);
    }

    #[test]
    fn rank_prefers_prefix_over_word_boundary_over_substring() {
        let commands = vec![
            command("1", "Unstage all", "Git"), // "stage" inside "Unstage" → substring
            command("2", "stage file", "Git"),  // prefix
            command("3", "Stage all files", "Git"), // prefix (case-insensitive)
        ];
        assert_eq!(rank_commands(&commands, "stage"), vec![1, 2, 0]);
    }

    #[test]
    fn rank_word_boundary_beats_substring() {
        let commands = vec![
            command("1", "Small talk", "Chat"), // "all" inside "Small" → substring
            command("2", "Stage all files", "Git"), // " all" → word boundary
            command("3", "all pins", "Git"),    // prefix
        ];
        assert_eq!(rank_commands(&commands, "all"), vec![2, 1, 0]);
    }

    #[test]
    fn rank_matches_section_and_keybind() {
        let commands = vec![
            Command::new("1", "Frobnicate", "View").keybind("Ctrl+Shift+F"),
            command("2", "Refresh", "Git"),
        ];
        assert_eq!(rank_commands(&commands, "git"), vec![1]);
        assert_eq!(rank_commands(&commands, "ctrl"), vec![0]);
        assert_eq!(rank_commands(&commands, "shift"), vec![0]);
        assert!(rank_commands(&commands, "nomatch").is_empty());
    }

    #[test]
    fn rank_is_case_insensitive_and_stable_within_tier() {
        let commands = vec![
            command("1", "Alpha", "S"),
            command("2", "ALPHA", "S"),
            command("3", "Beta", "S"),
        ];
        assert_eq!(rank_commands(&commands, "alp"), vec![0, 1]);
    }

    #[test]
    fn disabled_commands_stay_listed() {
        let commands = vec![
            command("1", "Push", "Git").disabled(),
            command("2", "Pull", "Git"),
        ];
        assert_eq!(rank_commands(&commands, ""), vec![0, 1]);
        assert_eq!(rank_commands(&commands, "push"), vec![0]);
    }

    #[test]
    fn open_resets_query_and_selection() {
        let mut state = open_state(vec![command("a", "A", "S")]);
        state.query.push_str("leftover");
        state.selected = 7;
        state.close();
        state.open();
        assert!(state.open);
        assert_eq!(state.query, "");
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn toggle_roundtrip() {
        let mut state = State::default();
        update(&mut state, Msg::Toggle);
        assert!(state.open);
        update(&mut state, Msg::Toggle);
        assert!(!state.open);
    }

    #[test]
    fn typing_filters_and_resets_selection() {
        let mut state = open_state(vec![
            command("1", "Stage", "Git"),
            command("2", "Stash", "Git"),
            command("3", "Commit", "Git"),
        ]);
        state.selected = 2;

        update(&mut state, character("s"));
        assert_eq!(state.query, "s");
        assert_eq!(state.selected, 0);
        assert_eq!(state.ranked(), vec![0, 1]);

        update(&mut state, character("t"));
        assert_eq!(state.query, "st");

        update(&mut state, key(Named::Backspace));
        assert_eq!(state.query, "s");
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn ctrl_typed_characters_are_ignored() {
        let mut state = open_state(vec![]);
        let msg = Msg::Key(Key::Character("a".into()), Modifiers::CTRL);
        update(&mut state, msg);
        assert_eq!(state.query, "");
    }

    #[test]
    fn selection_wraps_with_arrows() {
        let mut state = open_state(vec![
            command("1", "One", "S"),
            command("2", "Two", "S"),
            command("3", "Three", "S"),
        ]);
        assert_eq!(state.selected, 0);

        update(&mut state, key(Named::ArrowDown));
        assert_eq!(state.selected, 1);
        update(&mut state, key(Named::ArrowUp));
        assert_eq!(state.selected, 0);
        update(&mut state, key(Named::ArrowUp));
        assert_eq!(state.selected, 2);
        update(&mut state, key(Named::ArrowDown));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn navigation_on_empty_list_is_safe() {
        let mut state = open_state(vec![]);
        update(&mut state, key(Named::ArrowDown));
        update(&mut state, key(Named::PageUp));
        update(&mut state, key(Named::End));
        assert_eq!(state.selected, 0);
        assert_eq!(update(&mut state, key(Named::Enter)), None);
        assert!(state.open);
    }

    #[test]
    fn home_end_and_page_keys_clamp() {
        let mut state = open_state(
            (0..20)
                .map(|n| command(&format!("c{n}"), &format!("Command {n}"), "S"))
                .collect(),
        );

        update(&mut state, key(Named::End));
        assert_eq!(state.selected, 19);
        update(&mut state, key(Named::PageUp));
        assert_eq!(state.selected, 11);
        update(&mut state, key(Named::Home));
        assert_eq!(state.selected, 0);
        update(&mut state, key(Named::PageDown));
        assert_eq!(state.selected, 8);
        update(&mut state, key(Named::PageDown));
        assert_eq!(state.selected, 16);
        update(&mut state, key(Named::PageDown));
        assert_eq!(state.selected, 19);
    }

    #[test]
    fn enter_activates_and_closes() {
        let mut state = open_state(vec![command("stage.all", "Stage all", "Git")]);
        let action = update(&mut state, key(Named::Enter));
        assert_eq!(action, Some(Action::RunCommand("stage.all".into())));
        assert!(!state.open);
    }

    #[test]
    fn enter_on_disabled_command_is_ignored() {
        let mut state = open_state(vec![command("push", "Push", "Git").disabled()]);
        assert_eq!(update(&mut state, key(Named::Enter)), None);
        assert!(state.open);
    }

    #[test]
    fn row_click_activates_enabled_rows_only() {
        let mut state = open_state(vec![
            command("a", "A", "S"),
            command("b", "B", "S").disabled(),
        ]);
        // Row 0 is "a"; row 1 is "b" (disabled).
        assert_eq!(
            update(&mut state, Msg::RowClicked(0)),
            Some(Action::RunCommand("a".into()))
        );
        assert!(!state.open);

        state.open();
        assert_eq!(update(&mut state, Msg::RowClicked(1)), None);
        assert!(state.open);
        assert_eq!(update(&mut state, Msg::RowClicked(99)), None);
    }

    #[test]
    fn escape_and_backdrop_close_with_intent() {
        let mut state = open_state(vec![command("a", "A", "S")]);
        assert_eq!(update(&mut state, key(Named::Escape)), Some(Action::Close));
        assert!(!state.open);

        state.open();
        assert_eq!(update(&mut state, Msg::Close), Some(Action::Close));
        assert!(!state.open);
        // Already closed: no further intent.
        assert_eq!(update(&mut state, Msg::Close), None);
    }

    #[test]
    fn closed_palette_ignores_keys_and_clicks() {
        let mut state = State::default();
        state.commands.register(command("a", "A", "S"));

        assert_eq!(update(&mut state, key(Named::Escape)), None);
        assert_eq!(update(&mut state, key(Named::ArrowDown)), None);
        assert_eq!(update(&mut state, key(Named::Enter)), None);
        assert_eq!(update(&mut state, character("a")), None);
        assert_eq!(update(&mut state, Msg::RowClicked(0)), None);
        assert!(!state.open);
        assert_eq!(state.query, "");
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn event_to_msg_maps_key_presses() {
        let event = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: Key::Named(Named::Escape),
            modified_key: Key::Named(Named::Escape),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::Escape),
            location: iced::keyboard::Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        });
        assert_eq!(
            event_to_msg(
                event,
                iced::event::Status::Ignored,
                iced::window::Id::unique()
            ),
            Some(Msg::Key(Key::Named(Named::Escape), Modifiers::default()))
        );
        assert_eq!(
            event_to_msg(
                iced::Event::Keyboard(
                    iced::keyboard::Event::ModifiersChanged(Modifiers::default())
                ),
                iced::event::Status::Ignored,
                iced::window::Id::unique(),
            ),
            None
        );
    }
}
