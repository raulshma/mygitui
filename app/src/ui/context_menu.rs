//! Reusable context menu: a small bordered popup anchored at a cursor
//! position over a transparent full-window backdrop.
//!
//! Pure UI + intent module: no service calls, no `Task`. The orchestrator
//! wires it into the shell like this:
//!
//! 1. Own a [`State`]; on right-click (or any anchor event) build a
//!    [`Menu`], clamp the cursor position into the window with
//!    [`clamped_position`], and call `state.open_at(point, menu)`.
//! 2. Forward key presses by adding
//!    `iced::event::listen_with(context_menu::event_to_msg)` to the app
//!    subscription and route the resulting [`Msg`] through [`update`],
//!    translating the returned [`Action`] into shell messages. Keys are
//!    ignored while the menu is closed, so the subscription can stay
//!    unconditional.
//! 3. Layer [`view`] above the workspace while `state.open` is set. The
//!    backdrop press closes the menu; the shell may additionally react to
//!    [`Action::Close`] (e.g. restore focus).
//!
//! Selection only ever lands on activatable items: navigation and the
//! initial selection skip separators, headers and disabled commands.

// Consumed by the app shell once the orchestrator registers this module;
// until then the crate cannot reach every entry point from its roots.
#![allow(dead_code)]

use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{
    Alignment, Background, Border, Element, Length, Point, Rectangle, Shadow, Size, Vector,
};

use super::theme::{self, Tokens};

/// Width of the menu card.
const MENU_WIDTH: f32 = 210.0;

/// One entry of a context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Command {
        id: String,
        label: String,
        keybind: Option<String>,
        enabled: bool,
        danger: bool,
    },
    Separator,
    Header(String),
}

impl Item {
    /// Whether pressing enter (or clicking) can activate this item.
    pub fn is_activatable(&self) -> bool {
        matches!(self, Item::Command { enabled: true, .. })
    }
}

/// A context menu: an ordered list of [`Item`]s with a builder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    pub items: Vec<Item>,
}

impl Menu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an enabled command without a keybind.
    pub fn command(self, id: impl Into<String>, label: impl Into<String>) -> Self {
        self.command_with(id, label, None, true, false)
    }

    /// Appends a command with full control over keybind, enabled and danger.
    pub fn command_with(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        keybind: Option<String>,
        enabled: bool,
        danger: bool,
    ) -> Self {
        self.items.push(Item::Command {
            id: id.into(),
            label: label.into(),
            keybind,
            enabled,
            danger,
        });
        self
    }

    /// Appends a thin divider.
    pub fn separator(mut self) -> Self {
        self.items.push(Item::Separator);
        self
    }

    /// Appends a small non-interactive group caption.
    pub fn header(mut self, label: impl Into<String>) -> Self {
        self.items.push(Item::Header(label.into()));
        self
    }
}

/// Context menu state owned by the shell.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub open: bool,
    /// Top-left anchor of the popup; clamp with [`clamped_position`] before
    /// calling [`State::open_at`] so the menu stays inside the window.
    pub position: Point,
    pub menu: Menu,
    /// Index into [`Menu::items`]; only ever points at activatable items
    /// (separators, headers and disabled commands are skipped).
    pub selected: usize,
}

impl State {
    /// Opens `menu` anchored at `position` and selects the first
    /// activatable item.
    pub fn open_at(&mut self, position: Point, menu: Menu) {
        self.open = true;
        self.position = position;
        self.menu = menu;
        self.selected = self
            .menu
            .items
            .iter()
            .position(Item::is_activatable)
            .unwrap_or(0);
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

/// Clamps a desired menu origin so a menu of `size` stays inside `viewport`.
///
/// The menu is top-left anchored: the origin is clamped into the viewport
/// shrunk by the menu size; a menu larger than the viewport is pinned to the
/// viewport origin.
pub fn clamped_position(position: Point, size: Size, viewport: Rectangle) -> Point {
    let max_x = (viewport.x + viewport.width - size.width).max(viewport.x);
    let max_y = (viewport.y + viewport.height - size.height).max(viewport.y);
    Point::new(
        position.x.clamp(viewport.x, max_x),
        position.y.clamp(viewport.y, max_y),
    )
}

/// Messages the shell forwards to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Key press forwarded from the shell's event listener.
    Key(Key, Modifiers),
    /// Press on a menu row; the index is into [`Menu::items`].
    ItemClicked(usize),
    /// Press on the backdrop outside the menu.
    BackdropPressed,
}

/// Intents the shell translates into service calls / shell messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run the menu command with this id.
    Command(String),
    /// The menu dismissed itself (escape or backdrop); `open` is already
    /// `false`.
    Close,
}

/// Maps a raw iced event to a menu message; wire with
/// `iced::event::listen_with(context_menu::event_to_msg)`.
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

/// Steps the menu; returns the intent for the shell to translate, if any.
///
/// Everything is ignored while the menu is closed. Dismissals (escape,
/// backdrop) yield [`Action::Close`], activation yields
/// [`Action::Command`]. Clicks on separators, headers or disabled commands
/// are no-ops.
pub fn update(state: &mut State, msg: Msg) -> Option<Action> {
    if !state.open {
        return None;
    }
    match msg {
        Msg::BackdropPressed => {
            state.close();
            Some(Action::Close)
        }
        Msg::Key(key, modifiers) => handle_key(state, key, modifiers),
        Msg::ItemClicked(index) => {
            let id = match state.menu.items.get(index) {
                Some(Item::Command {
                    id, enabled: true, ..
                }) => id.clone(),
                _ => return None,
            };
            state.close();
            Some(Action::Command(id))
        }
    }
}

fn handle_key(state: &mut State, key: Key, modifiers: Modifiers) -> Option<Action> {
    let named = match key {
        Key::Named(named) => named,
        Key::Character(_) | Key::Unidentified => return None,
    };
    match named {
        Named::Escape => {
            state.close();
            Some(Action::Close)
        }
        Named::Enter if is_plain(modifiers) => {
            let id = match state.menu.items.get(state.selected) {
                Some(Item::Command {
                    id, enabled: true, ..
                }) => id.clone(),
                _ => return None,
            };
            state.close();
            Some(Action::Command(id))
        }
        Named::ArrowDown if is_plain(modifiers) => {
            if let Some(next) = next_activatable(&state.menu.items, state.selected, 1) {
                state.selected = next;
            }
            None
        }
        Named::ArrowUp if is_plain(modifiers) => {
            if let Some(previous) = next_activatable(&state.menu.items, state.selected, -1) {
                state.selected = previous;
            }
            None
        }
        _ => None,
    }
}

/// Navigation keys ignore ctrl/alt/⌘ combos so the shell keeps its hotkeys.
fn is_plain(modifiers: Modifiers) -> bool {
    !(modifiers.control() || modifiers.alt() || modifiers.command())
}

/// Index of the next activatable item stepping from `start` by `delta`,
/// wrapping once around the list; `None` when nothing is activatable.
fn next_activatable(items: &[Item], start: usize, delta: i32) -> Option<usize> {
    let len = items.len();
    if len == 0 {
        return None;
    }
    let mut index = start.min(len - 1);
    for _ in 0..len {
        index = (index as i32 + delta).rem_euclid(len as i32) as usize;
        if items[index].is_activatable() {
            return Some(index);
        }
    }
    None
}

/// Popup for the current state; layer above the workspace while
/// `state.open` is set (a zero-sized element when closed).
pub fn view(state: &State, tokens: &Tokens) -> Element<'static, Msg> {
    if !state.open {
        return Space::new().width(0).height(0).into();
    }
    // Own the tokens so style closures capture a `Copy` value instead of the
    // reference (keeping the element `'static`).
    let tokens = *tokens;

    let mut body = column![].spacing(2).width(Length::Fill);
    for (index, item) in state.menu.items.iter().enumerate() {
        match item {
            Item::Command {
                label,
                keybind,
                enabled,
                danger,
                ..
            } => {
                body = body.push(item_row(
                    index,
                    label,
                    keybind.as_deref(),
                    *enabled,
                    *danger,
                    index == state.selected,
                    &tokens,
                ))
            }
            Item::Separator => body = body.push(separator(&tokens)),
            Item::Header(label) => {
                body = body.push(
                    container(
                        text(label.clone())
                            .size(9)
                            .font(theme::semibold())
                            .color(tokens.muted_foreground),
                    )
                    .padding([2, 4]),
                )
            }
        }
    }

    let card = container(body)
        .width(Length::Fixed(MENU_WIDTH))
        .padding([6, 4])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.popover)),
            text_color: Some(tokens.popover_foreground),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 8.0.into(),
            },
            shadow: Shadow {
                color: theme::with_alpha(iced::Color::BLACK, 0.2),
                offset: Vector::new(0.0, 4.0),
                blur_radius: 12.0,
            },
            snap: true,
        });

    // Transparent full-window backdrop; the card is offset into place via
    // padding. Row buttons consume their own presses, so only presses on the
    // backdrop (or non-interactive card areas, as in dialog.rs) dismiss.
    let anchored = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Start)
        .align_y(Alignment::Start)
        .padding(iced::Padding {
            top: state.position.y,
            left: state.position.x,
            ..iced::Padding::ZERO
        });

    mouse_area(anchored).on_press(Msg::BackdropPressed).into()
}

fn item_row(
    index: usize,
    label: &str,
    keybind: Option<&str>,
    enabled: bool,
    danger: bool,
    selected: bool,
    tokens: &Tokens,
) -> Element<'static, Msg> {
    let tokens = *tokens;
    let label_color = if !enabled {
        tokens.disabled_fg
    } else if danger {
        tokens.destructive
    } else {
        tokens.foreground
    };

    let line = row![
        text(label.to_string()).size(11).color(label_color),
        Space::new().width(Length::Fill),
        text(keybind.unwrap_or_default().to_string())
            .size(9)
            .font(theme::mono())
            .color(tokens.muted_foreground),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(
        button(container(line))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .width(Length::Fill)
            .on_press_maybe(enabled.then_some(Msg::ItemClicked(index))),
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

/// Thin divider with a little breathing room on both sides.
fn separator(tokens: &Tokens) -> Element<'static, Msg> {
    let tokens = *tokens;
    container(
        container(Space::new().width(Length::Fill))
            .height(Length::Fixed(1.0))
            .style(move |_theme| container::Style {
                background: Some(Background::Color(tokens.border)),
                ..container::Style::default()
            }),
    )
    .padding([2, 4])
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(named: Named) -> Msg {
        Msg::Key(Key::Named(named), Modifiers::default())
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
        Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    fn sample_menu() -> Menu {
        Menu::new()
            .header("Staging")
            .command("stage", "Stage")
            .separator()
            .command_with("pull", "Pull", Some("Ctrl+U".into()), false, false)
            .command_with("reset", "Hard reset", None, true, true)
    }

    #[test]
    fn builder_composes_items_in_order() {
        let menu = sample_menu();
        assert_eq!(
            menu.items,
            vec![
                Item::Header("Staging".into()),
                Item::Command {
                    id: "stage".into(),
                    label: "Stage".into(),
                    keybind: None,
                    enabled: true,
                    danger: false,
                },
                Item::Separator,
                Item::Command {
                    id: "pull".into(),
                    label: "Pull".into(),
                    keybind: Some("Ctrl+U".into()),
                    enabled: false,
                    danger: false,
                },
                Item::Command {
                    id: "reset".into(),
                    label: "Hard reset".into(),
                    keybind: None,
                    enabled: true,
                    danger: true,
                },
            ]
        );
    }

    #[test]
    fn open_selects_first_activatable_item() {
        let mut state = State::default();
        state.open_at(Point::ORIGIN, sample_menu());
        assert!(state.open);
        // Item 0 is a header, item 1 the enabled "Stage" command: that is
        // the first activatable item.
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn navigation_skips_separators_headers_and_disabled() {
        let menu = Menu::new()
            .command("a", "A")
            .header("Group")
            .separator()
            .command_with("b", "B", None, false, false)
            .command("c", "C")
            .command_with("d", "D", None, false, false);

        let mut state = State::default();
        state.open_at(Point::ORIGIN, menu);
        assert_eq!(state.selected, 0);

        update(&mut state, key(Named::ArrowDown));
        assert_eq!(state.selected, 4); // skips header, separator, disabled "b"

        update(&mut state, key(Named::ArrowDown));
        assert_eq!(state.selected, 0); // wraps past disabled "d"

        update(&mut state, key(Named::ArrowUp));
        assert_eq!(state.selected, 4);

        update(&mut state, key(Named::ArrowUp));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn single_item_navigation_stays_put() {
        let mut state = State::default();
        state.open_at(Point::ORIGIN, Menu::new().command("only", "Only"));
        update(&mut state, key(Named::ArrowDown));
        update(&mut state, key(Named::ArrowUp));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn enter_activates_and_closes() {
        let mut state = State::default();
        state.open_at(Point::ORIGIN, sample_menu());
        let action = update(&mut state, key(Named::Enter));
        assert_eq!(action, Some(Action::Command("stage".into())));
        assert!(!state.open);
    }

    #[test]
    fn enter_with_nothing_activatable_is_ignored() {
        let menu = Menu::new()
            .command_with("a", "A", None, false, false)
            .separator();
        let mut state = State::default();
        state.open_at(Point::ORIGIN, menu);
        assert_eq!(update(&mut state, key(Named::Enter)), None);
        assert!(state.open);
    }

    #[test]
    fn item_click_activates_enabled_commands_only() {
        let mut state = State::default();
        state.open_at(Point::ORIGIN, sample_menu());

        // Index 1 is the enabled "Stage" command.
        assert_eq!(
            update(&mut state, Msg::ItemClicked(1)),
            Some(Action::Command("stage".into()))
        );
        assert!(!state.open);

        state.open_at(Point::ORIGIN, sample_menu());
        // Separator, disabled command and header are not activatable.
        assert_eq!(update(&mut state, Msg::ItemClicked(2)), None);
        assert_eq!(update(&mut state, Msg::ItemClicked(3)), None);
        assert_eq!(update(&mut state, Msg::ItemClicked(0)), None);
        assert_eq!(update(&mut state, Msg::ItemClicked(99)), None);
        assert!(state.open);
    }

    #[test]
    fn escape_and_backdrop_close_with_intent() {
        let mut state = State::default();
        state.open_at(Point::ORIGIN, sample_menu());
        assert_eq!(update(&mut state, key(Named::Escape)), Some(Action::Close));
        assert!(!state.open);

        state.open_at(Point::ORIGIN, sample_menu());
        assert_eq!(
            update(&mut state, Msg::BackdropPressed),
            Some(Action::Close)
        );
        assert!(!state.open);
    }

    #[test]
    fn closed_menu_ignores_everything() {
        let mut state = State::default();
        assert_eq!(update(&mut state, key(Named::Escape)), None);
        assert_eq!(update(&mut state, key(Named::Enter)), None);
        assert_eq!(update(&mut state, key(Named::ArrowDown)), None);
        assert_eq!(update(&mut state, Msg::ItemClicked(0)), None);
        assert_eq!(update(&mut state, Msg::BackdropPressed), None);
        assert!(!state.open);
    }

    #[test]
    fn clamping_inside_viewport_keeps_position() {
        let viewport = rect(0.0, 0.0, 500.0, 500.0);
        let size = Size::new(100.0, 80.0);
        assert_eq!(
            clamped_position(Point::new(200.0, 210.0), size, viewport),
            Point::new(200.0, 210.0)
        );
    }

    #[test]
    fn clamping_right_edge() {
        let viewport = rect(0.0, 0.0, 500.0, 500.0);
        let size = Size::new(100.0, 80.0);
        assert_eq!(
            clamped_position(Point::new(480.0, 100.0), size, viewport),
            Point::new(400.0, 100.0)
        );
    }

    #[test]
    fn clamping_bottom_edge() {
        let viewport = rect(0.0, 0.0, 500.0, 500.0);
        let size = Size::new(100.0, 80.0);
        assert_eq!(
            clamped_position(Point::new(100.0, 470.0), size, viewport),
            Point::new(100.0, 420.0)
        );
    }

    #[test]
    fn clamping_top_left_negative() {
        let viewport = rect(0.0, 0.0, 500.0, 500.0);
        let size = Size::new(100.0, 80.0);
        assert_eq!(
            clamped_position(Point::new(-20.0, -30.0), size, viewport),
            Point::new(0.0, 0.0)
        );
    }

    #[test]
    fn clamping_offset_viewport() {
        let viewport = rect(50.0, 60.0, 200.0, 100.0);
        let size = Size::new(20.0, 20.0);
        assert_eq!(
            clamped_position(Point::new(400.0, 400.0), size, viewport),
            Point::new(230.0, 140.0)
        );
        assert_eq!(
            clamped_position(Point::new(10.0, 10.0), size, viewport),
            Point::new(50.0, 60.0)
        );
    }

    #[test]
    fn clamping_oversized_menu_pins_to_origin() {
        let viewport = rect(0.0, 0.0, 500.0, 500.0);
        let size = Size::new(600.0, 600.0);
        assert_eq!(
            clamped_position(Point::new(100.0, 100.0), size, viewport),
            Point::new(0.0, 0.0)
        );
    }

    #[test]
    fn event_to_msg_maps_key_presses() {
        let event = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: Key::Named(Named::ArrowDown),
            modified_key: Key::Named(Named::ArrowDown),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::ArrowDown),
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
            Some(Msg::Key(Key::Named(Named::ArrowDown), Modifiers::default()))
        );
    }
}
