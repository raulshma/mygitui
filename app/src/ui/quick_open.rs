//! Quick Open (ctrl/cmd+P): a small centered overlay fuzzy-filtering the
//! repository's worktree files (from `workspace::repo_file_index`), Enter
//! opens the selection in the in-app editor.
//!
//! Pure UI + intent module: no service calls, no `Task`. The shell wires it
//! exactly like [`super::quick_switcher`]:
//!
//! 1. Own a [`State`]; on open, fetch the file index for the active repo
//!    (`repo_file_index`) and feed `state.entries` with the result (files
//!    only; directories are not openable).
//! 2. Forward key presses by adding
//!    `iced::event::listen_with(quick_open::event_to_msg)` to the app
//!    subscription (alongside the existing modifiers listener) and route the
//!    resulting [`Msg`] through [`update`], translating the returned
//!    [`Action`] into shell messages. Keys are ignored while the overlay is
//!    closed, so the subscription can stay unconditional.
//! 3. Layer [`view`] above the workspace while `state.open` is set, the same
//!    way `dialog::confirm` is layered today.
//! 4. Open it from the global hotkey (ctrl/cmd+P) via `Msg::Open`, and
//!    translate `Action::Open(path)` into the editor-open flow.
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

/// Width of the overlay card.
const CARD_WIDTH: f32 = 420.0;
/// Estimated height of one result row.
const ROW_HEIGHT: f32 = 24.0;
/// Upper bound for the result list before it scrolls.
const LIST_MAX_HEIGHT: f32 = 320.0;
/// Rows jumped by the page up/page down keys.
const PAGE_SIZE: usize = 8;
/// Result rows rendered (the best-ranked ones); the index can hold 50k
/// paths, the list shows the head of the ranking.
const MAX_RESULTS: usize = 200;

/// One openable file (workdir-relative path, forward slashes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
}

impl Entry {
    /// Last path segment, shown first in the row.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// Everything before the last segment (`""` for root-level files).
    pub fn dir(&self) -> &str {
        match self.path.rfind('/') {
            Some(pos) => &self.path[..pos],
            None => "",
        }
    }
}

/// Fuzzy subsequence score of `needle` inside `haystack`, case-insensitive;
/// higher is better, `None` = no match. Greedy left-to-right matching: each
/// matched char scores +1, a match at a path boundary (after a separator —
/// `/`, `_`, `-`, `.` … — or at the start) +8, a consecutive run +8, each
/// skipped character between matches -1, and the whole haystack length is
/// divided in as a small penalty so tighter paths rank above loose ones.
pub fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let needle: Vec<char> = needle.to_lowercase().chars().collect();

    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    let mut cursor = 0usize;
    for &wanted in &needle {
        let found = cursor + hay[cursor..].iter().position(|&c| c == wanted)?;
        score += 1;
        if found == 0 || !hay[found - 1].is_alphanumeric() {
            score += 8; // boundary: start of path or after a separator
        }
        if let Some(p) = prev {
            if found == p + 1 {
                score += 8; // consecutive run
            } else {
                score -= (found - p - 1) as i32; // skipped characters
            }
        }
        prev = Some(found);
        cursor = found + 1;
    }
    // Same score across different-length haystacks prefers the shorter,
    // tighter path (fewer filler characters between matches).
    Some(score - hay.len() as i32 / 16)
}

/// Indices into `entries`, best first.
///
/// An empty (or whitespace-only) query keeps every entry in index order
/// (the service hands them over in natural path order). A non-empty query
/// keeps the entries whose path fuzzy-matches it, ranked by score (ties
/// keep the original order). At most [`MAX_RESULTS`] rows are returned.
pub fn rank_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    let needle = query.trim();
    if needle.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut ranked: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| fuzzy_score(&entry.path, needle).map(|score| (score, index)))
        .collect();
    // Stable sort: equal scores keep the (natural) index order.
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    ranked
        .into_iter()
        .map(|(_, index)| index)
        .take(MAX_RESULTS)
        .collect()
}

/// Quick Open state owned by the shell.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub open: bool,
    /// True between the shell's open and the fed file index (the shell
    /// fetches/builds it when the overlay opens).
    pub loading: bool,
    pub query: String,
    /// Index into the ranked (visible) rows; see [`State::ranked`].
    pub selected: usize,
    pub entries: Vec<Entry>,
}

impl State {
    /// Opens the overlay with a fresh query and selection.
    pub fn open(&mut self) {
        self.open = true;
        self.loading = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.loading = false;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Replaces the index with a fresh one (files only; the caller filters
    /// directories out of the service result) and ends the loading state.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.loading = false;
        self.selected = 0;
    }

    /// Indices into `State::entries` of the visible rows, best first.
    pub fn ranked(&self) -> Vec<usize> {
        rank_entries(&self.entries, &self.query)
    }
}

/// Messages the shell forwards to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Open the overlay (global hotkey, palette command).
    Open,
    /// Dismiss the overlay; also produced by pressing the backdrop.
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
    /// Open the file at this workdir-relative path in the in-app editor.
    Open(String),
    /// The overlay dismissed itself (escape or backdrop); `open` is already
    /// `false`.
    Close,
}

/// Maps a raw iced event to a Quick Open message; wire with
/// `iced::event::listen_with(quick_open::event_to_msg)`.
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

/// Steps the overlay; returns the intent for the shell to translate, if any.
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
        text_input("Go to file…", state.query.as_str())
            .size(theme::TEXT_BODY)
            .padding([4, 4])
            .width(Length::Fill),
        text("enter to open")
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let ranked = state.ranked();
    let selected = state.selected.min(ranked.len().saturating_sub(1));

    let list: Element<'static, Msg> = if state.loading && ranked.is_empty() {
        container(
            text("Building file index…")
                .size(theme::TEXT_SMALL)
                .color(tokens.muted_foreground),
        )
        .padding([6, 2])
        .into()
    } else if ranked.is_empty() {
        container(
            text(if state.entries.is_empty() {
                "No files in this repository."
            } else {
                "No matching files."
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

    // Shared modal scaffold (its card look replaces the popover tint so
    // every overlay reads the same): the query input and row buttons consume
    // their own presses; presses on the scrim or the card's padding close
    // the overlay.
    modal_with_cancel(
        tokens,
        CARD_WIDTH,
        column![query_row, Space::new().height(4), list]
            .spacing(6)
            .into(),
        Space::new().width(0).into(),
        Msg::Close,
    )
}

fn entry_row(
    entry: &Entry,
    position: usize,
    selected: bool,
    tokens: &Tokens,
) -> Element<'static, Msg> {
    let tokens = *tokens;
    let dir = entry.dir();
    let mut content = column![text(entry.name().to_string())
        .size(theme::TEXT_SMALL)
        .color(tokens.foreground)]
    .spacing(1)
    .width(Length::Fill);
    if !dir.is_empty() {
        content = content.push(
            text(dir.to_string())
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
        );
    }

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

    fn entries(paths: &[&str]) -> Vec<Entry> {
        paths
            .iter()
            .map(|path| Entry {
                path: path.to_string(),
            })
            .collect()
    }

    fn open_state(paths: &[&str]) -> State {
        let mut state = State {
            entries: entries(paths),
            ..State::default()
        };
        state.loading = false;
        state.open();
        state.loading = false;
        state
    }

    #[test]
    fn entry_name_and_dir() {
        let entry = Entry {
            path: "src/ui/app.rs".to_string(),
        };
        assert_eq!(entry.name(), "app.rs");
        assert_eq!(entry.dir(), "src/ui");
        let root = Entry {
            path: "README.md".to_string(),
        };
        assert_eq!(root.name(), "README.md");
        assert_eq!(root.dir(), "");
    }

    #[test]
    fn fuzzy_matches_subsequences() {
        // Subsequence matches.
        assert!(fuzzy_score("src/app/main.rs", "appm").is_some());
        assert_eq!(fuzzy_score("src/app/main.rs", "zzz"), None);
        // Empty query matches everything with the neutral score.
        assert_eq!(fuzzy_score("anything", ""), Some(0));
        assert_eq!(fuzzy_score("anything", "   "), Some(0));
        // Case-insensitive.
        assert!(fuzzy_score("README.md", "readme").is_some());
    }

    #[test]
    fn fuzzy_prefers_boundaries_and_consecutive_runs() {
        // Boundary hit (after '/') beats a scattered mid-word match.
        let boundary = fuzzy_score("src/main.rs", "mai");
        let scattered = fuzzy_score("somadir/umain.rs", "mai");
        assert!(boundary.unwrap() > scattered.unwrap());
        // Consecutive run beats one with gaps.
        let consecutive = fuzzy_score("src/app.rs", "ap");
        let gapped = fuzzy_score("src/aXp.rs", "ap");
        assert!(consecutive.unwrap() > gapped.unwrap());
    }

    #[test]
    fn rank_empty_query_keeps_natural_order() {
        let list = entries(&["b.rs", "a.rs", "c/d.rs"]);
        assert_eq!(rank_entries(&list, ""), vec![0, 1, 2]);
        assert_eq!(rank_entries(&list, "  "), vec![0, 1, 2]);
    }

    #[test]
    fn rank_query_prefers_tight_boundary_matches() {
        let list = entries(&["docs/notes.md", "src/app.rs", "src/app_main_long_name.rs"]);
        // "app": "src/app.rs" has a consecutive boundary run and is shorter
        // than "src/app_main_long_name.rs" (length penalty); "docs/notes.md"
        // does not match.
        assert_eq!(rank_entries(&list, "app"), vec![1, 2]);
    }

    #[test]
    fn rank_query_filters_non_matches() {
        let list = entries(&["src/main.rs", "README.md"]);
        assert!(rank_entries(&list, "zzz").is_empty());
    }

    #[test]
    fn rank_query_is_case_insensitive() {
        let list = entries(&["src/App.rs"]);
        assert_eq!(rank_entries(&list, "app"), vec![0]);
        assert_eq!(rank_entries(&list, "APP"), vec![0]);
    }

    #[test]
    fn rank_caps_results() {
        let list: Vec<Entry> = (0..500)
            .map(|n| Entry {
                path: format!("file{n}.rs"),
            })
            .collect();
        assert_eq!(rank_entries(&list, "").len(), 500, "empty query: all");
        assert_eq!(
            rank_entries(&list, "file").len(),
            MAX_RESULTS,
            "query: capped"
        );
    }

    #[test]
    fn open_resets_query_selection_and_sets_loading() {
        let mut state = open_state(&["a.rs"]);
        state.query.push_str("leftover");
        state.selected = 4;
        state.close();
        state.open();
        assert!(state.open);
        assert!(state.loading);
        assert_eq!(state.query, "");
        assert_eq!(state.selected, 0);
        // Feeding the index clears loading and resets the selection.
        state.selected = 7;
        state.set_entries(entries(&["b.rs", "c.rs"]));
        assert!(!state.loading);
        assert_eq!(state.selected, 0);
        assert_eq!(state.entries.len(), 2);
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
        let mut state = open_state(&["src/app.rs", "src/nothing_here.rs"]);
        state.selected = 1;

        update(&mut state, character("a"));
        assert_eq!(state.query, "a");
        assert_eq!(state.selected, 0);
        assert_eq!(state.ranked(), vec![0]);

        update(&mut state, key(Named::Backspace));
        assert_eq!(state.query, "");
        assert_eq!(state.ranked().len(), 2);
    }

    #[test]
    fn selection_wraps_with_arrows() {
        let mut state = open_state(&["a.rs", "b.rs", "c.rs"]);
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
        let paths: Vec<String> = (0..20).map(|n| format!("file{n}.rs")).collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut state = open_state(&paths);

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
        let mut state = open_state(&[]);
        update(&mut state, key(Named::ArrowDown));
        update(&mut state, key(Named::PageUp));
        update(&mut state, key(Named::End));
        assert_eq!(state.selected, 0);
        assert_eq!(update(&mut state, key(Named::Enter)), None);
        assert!(state.open);
    }

    #[test]
    fn enter_activates_and_closes() {
        let mut state = open_state(&["src/alpha.rs", "src/beta.rs"]);
        update(&mut state, key(Named::ArrowDown));
        let action = update(&mut state, key(Named::Enter));
        assert_eq!(action, Some(Action::Open("src/beta.rs".to_string())));
        assert!(!state.open);
    }

    #[test]
    fn row_click_activates() {
        let mut state = open_state(&["src/alpha.rs"]);
        assert_eq!(
            update(&mut state, Msg::RowClicked(0)),
            Some(Action::Open("src/alpha.rs".to_string()))
        );
        assert!(!state.open);
        assert_eq!(update(&mut state, Msg::RowClicked(99)), None);
    }

    #[test]
    fn escape_and_backdrop_close_with_intent() {
        let mut state = open_state(&["a.rs"]);
        assert_eq!(update(&mut state, key(Named::Escape)), Some(Action::Close));
        assert!(!state.open);

        state.open();
        assert_eq!(update(&mut state, Msg::Close), Some(Action::Close));
        assert!(!state.open);
        // Already closed: no further intent.
        assert_eq!(update(&mut state, Msg::Close), None);
    }

    #[test]
    fn closed_overlay_ignores_keys_and_clicks() {
        let mut state = State {
            entries: entries(&["a.rs"]),
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
