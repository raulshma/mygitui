//! Repo quick-switcher (cmd/ctrl+K): a small centered overlay listing known
//! repositories, filtered as you type.
//!
//! Pure UI + intent module: no service calls, no `Task`. The orchestrator
//! wires it into the shell like this:
//!
//! 1. Own a [`State`] and feed `state.entries` from the persisted recents
//!    (`config::RecentRepo` → [`Entry`]) whenever the config changes.
//! 2. Forward key presses by adding
//!    `iced::event::listen_with(quick_switcher::event_to_msg)` to the app
//!    subscription (alongside the existing modifiers listener) and route the
//!    resulting [`Msg`] through [`update`], translating the returned
//!    [`Action`] into shell messages. Keys are ignored while the switcher is
//!    closed, so the subscription can stay unconditional.
//! 3. Layer [`view`] above the workspace while `state.open` is set, the same
//!    way `dialog::confirm` is layered today.
//! 4. Open it from the global hotkey (cmd/ctrl+K) via `Msg::Toggle` or
//!    `State::open` directly, and translate `Action::Open(path)` into the
//!    shell's open-repository flow.
//!
//! Typing flows through `Msg::Key` (`Key::Character` appends, `Backspace`
//! deletes), so the query input in the view is a read-only display field and
//! keystrokes are never handled twice.

// Consumed by the app shell once the orchestrator registers this module;
// until then the crate cannot reach every entry point from its roots.
#![allow(dead_code)]

use std::path::PathBuf;

use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::{
    button, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{Alignment, Background, Border, Element, Length, Shadow, Vector};

use super::theme::{self, Tokens};

/// Width of the switcher card.
const SWITCHER_WIDTH: f32 = 380.0;
/// Estimated height of one result row (two text lines).
const ROW_HEIGHT: f32 = 30.0;
/// Upper bound for the result list before it scrolls.
const LIST_MAX_HEIGHT: f32 = 300.0;
/// Rows jumped by the page up/page down keys.
const PAGE_SIZE: usize = 8;

/// One switchable repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub pinned: bool,
    /// Unix seconds of the last open; `None` sorts last among unpinned.
    pub last_opened: Option<u64>,
}

impl Entry {
    /// Derives the display name from the path's file name.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self {
            path,
            name,
            pinned: false,
            last_opened: None,
        }
    }

    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn last_opened(mut self, last_opened: Option<u64>) -> Self {
        self.last_opened = last_opened;
        self
    }
}

/// Match tier of `needle` inside `haystack`, case-insensitive:
/// `0` exact prefix, `1` word-boundary match, `2` substring, no match.
/// (Small duplicate of the palette's ranker on purpose: the modules stay
/// independent with no cross-module dependencies.)
fn match_rank(haystack: &str, needle: &str) -> Option<u8> {
    let haystack = haystack.to_lowercase();
    let index = haystack.find(needle)?;
    Some(if index == 0 {
        0
    } else if haystack[..index]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_alphanumeric())
    {
        1
    } else {
        2
    })
}

/// Indices into `entries`, best first.
///
/// An empty (or whitespace-only) query keeps every entry: pinned first, then
/// by recency (`last_opened` descending, missing last), each group stable in
/// original order. A non-empty query keeps the entries whose name or path
/// contains it (case-insensitive), ranked by match tier over the name first
/// and the path second (exact prefix, then word-boundary match, then
/// substring), then by recency, then original order.
pub fn rank_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&entries[a], &entries[b]);
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_opened.unwrap_or(0).cmp(&a.last_opened.unwrap_or(0)))
        });
        return order;
    }

    let mut ranked: Vec<(u8, u8, u64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let name_tier = match_rank(&entry.name, &needle);
            let path_tier = match_rank(&entry.path.to_string_lossy(), &needle);
            let (name_tier, path_tier) = match (name_tier, path_tier) {
                (Some(name), Some(path)) => (name, path),
                (Some(name), None) => (name, 3),
                (None, Some(path)) => (3, path),
                (None, None) => return None,
            };
            Some((name_tier, path_tier, entry.last_opened.unwrap_or(0), index))
        })
        .collect();
    ranked.sort_by_key(|entry| (entry.0, entry.1, std::cmp::Reverse(entry.2), entry.3));
    ranked.into_iter().map(|(_, _, _, index)| index).collect()
}

/// Switcher state owned by the shell.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub open: bool,
    pub query: String,
    /// Index into the ranked (visible) rows; see [`State::ranked`].
    pub selected: usize,
    pub entries: Vec<Entry>,
}

impl State {
    /// Opens the switcher with a fresh query and selection.
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

    /// Indices into `State::entries` of the visible rows, best first.
    pub fn ranked(&self) -> Vec<usize> {
        rank_entries(&self.entries, &self.query)
    }
}

/// Messages the shell forwards to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Open the switcher (global hotkey).
    Open,
    /// Dismiss the switcher; also produced by pressing the backdrop.
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
    /// Open the repository at this path.
    Open(PathBuf),
    /// The switcher dismissed itself (escape or backdrop); the shell may
    /// restore focus to the workspace. `open` is already `false`.
    Close,
}

/// Maps a raw iced event to a switcher message; wire with
/// `iced::event::listen_with(quick_switcher::event_to_msg)`.
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

/// Steps the switcher; returns the intent for the shell to translate, if any.
///
/// `Msg::Open` / `Msg::Toggle` are shell-initiated and return `None`; a
/// user-initiated dismissal (escape, backdrop, `Msg::Close`) yields
/// [`Action::Close`], activation yields [`Action::Open`].
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

/// Activates the visible row at `position`, if present.
fn activate(state: &mut State, position: usize) -> Option<Action> {
    let path = {
        let ranked = state.ranked();
        let index = *ranked.get(position)?;
        state.entries.get(index)?.path.clone()
    };
    state.close();
    Some(Action::Open(path))
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
        text_input("Switch repository…", state.query.as_str())
            .size(12)
            .padding([4, 4])
            .width(Length::Fill),
        text("enter to open").size(9).color(tokens.muted_foreground),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let ranked = state.ranked();
    let selected = state.selected.min(ranked.len().saturating_sub(1));

    let list: Element<'static, Msg> = if ranked.is_empty() {
        container(
            text(if state.entries.is_empty() {
                "No repositories registered."
            } else {
                "No matching repositories."
            })
            .size(11)
            .color(tokens.muted_foreground),
        )
        .padding([6, 2])
        .into()
    } else {
        let height = (ranked.len() as f32 * ROW_HEIGHT).clamp(ROW_HEIGHT, LIST_MAX_HEIGHT);
        let mut rows = column![].spacing(1).width(Length::Fill);
        for (position, index) in ranked.into_iter().enumerate() {
            rows = rows.push(entry_row(
                &state.entries[index],
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

    let card = container(column![query_row, Space::new().height(4), list].spacing(6))
        .width(Length::Fixed(SWITCHER_WIDTH))
        .padding([12, 14])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.popover)),
            text_color: Some(tokens.popover_foreground),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: Shadow {
                color: theme::with_alpha(iced::Color::BLACK, 0.25),
                offset: Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            snap: true,
        });

    let scrim = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(
                iced::Color::BLACK,
                0.4,
            ))),
            ..container::Style::default()
        });

    // As in dialog.rs, the query input and row buttons consume their own
    // presses; presses on the scrim or the card's padding close the switcher.
    mouse_area(scrim).on_press(Msg::Close).into()
}

fn entry_row(
    entry: &Entry,
    position: usize,
    selected: bool,
    tokens: &Tokens,
) -> Element<'static, Msg> {
    let tokens = *tokens;
    let mut name_line = row![].spacing(4).align_y(Alignment::Center);
    if entry.pinned {
        name_line = name_line.push(text("★").size(9).color(tokens.modified));
    }
    name_line = name_line.push(text(entry.name.clone()).size(11).color(tokens.foreground));

    let content = column![
        name_line,
        text(entry.path.display().to_string())
            .size(9)
            .color(tokens.muted_foreground),
    ]
    .spacing(1)
    .width(Length::Fill);

    container(
        button(container(content))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .width(Length::Fill)
            .on_press(Msg::RowClicked(position)),
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

    fn key(named: Named) -> Msg {
        Msg::Key(Key::Named(named), Modifiers::default())
    }

    fn character(input: &str) -> Msg {
        Msg::Key(Key::Character(input.into()), Modifiers::default())
    }

    fn entry(path: &str, pinned: bool, last_opened: Option<u64>) -> Entry {
        Entry::new(path).pinned(pinned).last_opened(last_opened)
    }

    fn open_state(entries: Vec<Entry>) -> State {
        let mut state = State {
            entries,
            ..State::default()
        };
        state.open();
        state
    }

    #[test]
    fn entry_new_derives_name_from_path() {
        assert_eq!(Entry::new("/home/dev/mygitui").name, "mygitui");
        assert_eq!(
            Entry::new("/home/dev/mygitui").path,
            PathBuf::from("/home/dev/mygitui")
        );
        assert_eq!(Entry::new("/").name, "/");
    }

    #[test]
    fn rank_empty_query_pins_first_then_recency() {
        let entries = vec![
            entry("/w/alpha", false, Some(100)),
            entry("/w/beta", false, Some(300)),
            entry("/w/gamma", true, Some(5)),
            entry("/w/delta", false, None),
        ];
        assert_eq!(rank_entries(&entries, ""), vec![2, 1, 0, 3]);
        assert_eq!(rank_entries(&entries, "   "), vec![2, 1, 0, 3]);
    }

    #[test]
    fn rank_empty_query_is_stable_for_equal_keys() {
        let entries = vec![
            entry("/w/a", false, Some(100)),
            entry("/w/b", false, Some(100)),
            entry("/w/c", false, Some(100)),
        ];
        assert_eq!(rank_entries(&entries, ""), vec![0, 1, 2]);
    }

    #[test]
    fn rank_query_prefers_name_tier_then_path_tier() {
        let entries = vec![
            entry("/w/notes", false, Some(500)),       // no match
            entry("/w/mygitui", false, Some(500)),     // name substring, path substring
            entry("/w/gittools", false, Some(1)),      // name prefix
            entry("/x/githome/dotfiles", false, None), // path word-boundary only
        ];
        // Tiers: (2,2) for mygitui, (0,0) for gittools, (3,1) for dotfiles.
        assert_eq!(rank_entries(&entries, "git"), vec![2, 1, 3]);
    }

    #[test]
    fn rank_query_word_boundary_beats_substring() {
        let entries = vec![
            entry("/w/smalltalk", false, None), // "all" inside "smalltalk" → substring
            entry("/w/allhands", false, None),  // "all" at the start → prefix
            entry("/w/best-of-all", false, None), // "all" after '-' → word boundary
        ];
        assert_eq!(rank_entries(&entries, "all"), vec![1, 2, 0]);
    }

    #[test]
    fn rank_query_is_case_insensitive() {
        let entries = vec![entry("/w/MyGitUI", false, None)];
        assert_eq!(rank_entries(&entries, "git"), vec![0]);
        assert_eq!(rank_entries(&entries, "MYGIT"), vec![0]);
    }

    #[test]
    fn rank_query_recency_tiebreak_within_tier() {
        let entries = vec![
            entry("/w/older", false, Some(100)),
            entry("/w/newer", false, Some(300)),
        ];
        // Same (2,2) tier, so the more recent entry wins.
        assert_eq!(rank_entries(&entries, "w/"), vec![1, 0]);
    }

    #[test]
    fn rank_query_no_match_excludes_everything() {
        let entries = vec![entry("/w/alpha", false, None)];
        assert!(rank_entries(&entries, "zzz").is_empty());
    }

    #[test]
    fn open_resets_query_and_selection() {
        let mut state = open_state(vec![entry("/w/a", false, None)]);
        state.query.push_str("leftover");
        state.selected = 4;
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
            entry("/w/mygitui", false, None),
            entry("/w/dotfiles", false, None),
        ]);
        state.selected = 1;

        update(&mut state, character("g"));
        assert_eq!(state.query, "g");
        assert_eq!(state.selected, 0);
        assert_eq!(state.ranked(), vec![0]);

        update(&mut state, key(Named::Backspace));
        assert_eq!(state.query, "");
        assert_eq!(state.ranked().len(), 2);
    }

    #[test]
    fn selection_wraps_with_arrows() {
        let mut state = open_state(vec![
            entry("/w/a", false, None),
            entry("/w/b", false, None),
            entry("/w/c", false, None),
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
    fn home_end_and_page_keys_clamp() {
        let entries: Vec<Entry> = (0..20)
            .map(|n| entry(&format!("/w/repo{n}"), false, None))
            .collect();
        let mut state = open_state(entries);

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
    fn enter_activates_and_closes() {
        let mut state = open_state(vec![
            entry("/w/alpha", false, None),
            entry("/w/beta", false, None),
        ]);
        update(&mut state, key(Named::ArrowDown));
        let action = update(&mut state, key(Named::Enter));
        assert_eq!(action, Some(Action::Open(PathBuf::from("/w/beta"))));
        assert!(!state.open);
    }

    #[test]
    fn row_click_activates() {
        let mut state = open_state(vec![entry("/w/alpha", false, None)]);
        assert_eq!(
            update(&mut state, Msg::RowClicked(0)),
            Some(Action::Open(PathBuf::from("/w/alpha")))
        );
        assert!(!state.open);
        assert_eq!(update(&mut state, Msg::RowClicked(99)), None);
    }

    #[test]
    fn escape_and_backdrop_close_with_intent() {
        let mut state = open_state(vec![entry("/w/a", false, None)]);
        assert_eq!(update(&mut state, key(Named::Escape)), Some(Action::Close));
        assert!(!state.open);

        state.open();
        assert_eq!(update(&mut state, Msg::Close), Some(Action::Close));
        assert!(!state.open);
        // Already closed: no further intent.
        assert_eq!(update(&mut state, Msg::Close), None);
    }

    #[test]
    fn closed_switcher_ignores_keys_and_clicks() {
        let mut state = State {
            entries: vec![entry("/w/a", false, None)],
            ..State::default()
        };

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
            key: Key::Named(Named::Enter),
            modified_key: Key::Named(Named::Enter),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::Enter),
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
            Some(Msg::Key(Key::Named(Named::Enter), Modifiers::default()))
        );
    }
}
