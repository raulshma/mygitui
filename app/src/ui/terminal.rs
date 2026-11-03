//! Embedded terminal pane: native port of the old Svelte PTY panel.
//!
//! The pane is a *monitor with input* over the existing PTY backend
//! ([`crate::pty`], `pty-output` / `pty-exit` on the backend event bus; the
//! SSH shells from [`crate::remote::term`] emit the same events, so
//! `rshell-*` session ids work unchanged). It keeps a styled line scrollback
//! (`Vec<Run>` per line), renders it as monospace rows on a fixed dark
//! surface, forwards keystrokes/paste to the shell and forwards grid resizes
//! to the pty. It is intentionally NOT a full vt100/xterm emulator — see
//! "Scope" below.
//!
//! # Scope
//!
//! Output is accumulated as styled lines and the parser handles C0 controls
//! (CR, LF/VT/FF, BS, TAB, BEL), escape-sequence chunk splitting across
//! [`Scrollback::feed`] calls, OSC/DCS/APC string swallowing and the SGR
//! subset `0` / `1` / `22` / `30-37` / `39` / `90-97` (plus `38;5;n` for the
//! first 16 palette entries). Everything else — cursor addressing, scroll
//! regions, alternate screens, background colors, the 256-color cube — is
//! stripped or ignored. Consequences: full-screen TUIs (vim, less) degrade to
//! a linear text flow, erased cells become plain spaces, and `bold`
//! brightens the 8 base colors instead of switching font weight per glyph
//! (`Run::bold` still carries the flag for rendering). That matches this
//! pane's job — watching shell/git output and typing into the shell — not an
//! xterm.js replacement.
//!
//! # Update shape
//!
//! [`update`] is deliberately pure: `update(&mut State, Msg) -> Option<Action>`
//! with no `&AppServices`. Nothing in the state machine needs to read
//! services, and every PTY call (`pty_create`/`pty_write`/`pty_resize`/
//! `pty_kill`) is a cheap synchronous function the app executes from the
//! returned [`Action`]. That mirrors the pure-model idiom of `status_model` /
//! `history_model` and keeps everything unit-testable headless.
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod terminal;` in `ui/mod.rs`.
//! 2. Add a `Terminal(terminal::Msg)` variant to `app::Message` and hold one
//!    `terminal::State` per tab (built with the repo root so the pane can
//!    spawn a shell: `terminal::State::new(Some(root.into()))`).
//! 3. Route messages: `Message::Terminal(msg) => {
//!    let action = terminal::update(&mut tab.terminal, msg);
//!    Self::run_terminal_action(&mut tab.terminal, action) }`.
//! 4. Execute actions — all PTY calls are synchronous, so call them inline
//!    inside `update` and return `Task::done`/`Task::none`:
//!    - `Action::Spawn { repo_root }`: map the root to the open tab's
//!      `RepoId`, call `crate::pty::pty_create(&services, repo_id,
//!      Some(state.dims().1), Some(state.dims().0))` (rows, cols order) and
//!      deliver the returned session id as `Msg::Opened(id)`.
//!    - `Action::Write { session, data }`: `crate::pty::pty_write`.
//!    - `Action::Resize { session, rows, cols }`: `crate::pty::pty_resize`.
//!    - `Action::Kill { session }`: `crate::pty::pty_kill` (also the right
//!      call when a tab closes; errors mean the session is already gone).
//!    - `Action::Restart { old_session, repo_root }`: best-effort kill of
//!      `old_session`, then spawn like `Spawn` and deliver the new id via
//!      `Msg::Opened`.
//!    - `Action::FollowScroll { id }`: [`terminal::snap_to_bottom`] pins the
//!      scrollback to the newest output (`snap_to_end` operation) — batch or
//!      chain it like any other task.
//! 5. Subscribe per live session, keyed like `App::auto_fetch_subscription`:
//!    `terminal::subscription(&session_id).map(Message::Terminal)`.
//! 6. Input plumbing: while the pane is focused feed key presses as
//!    `Msg::Key(key, modifiers)` (e.g. via `iced::keyboard::on_key_press`;
//!    window-shortcut combos are ignored by [`update`]), clipboard text as
//!    `Msg::Paste`, and clicks as `Msg::Click` to mark focus. The shell
//!    releases focus via [`State::blur`] when another typing surface takes
//!    over (tab switches, commit editor, filter inputs).

use std::path::PathBuf;

use iced::futures::SinkExt as _;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::operation::snap_to_end;
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::Id;
use iced::widget::{
    button, column, container, mouse_area, rich_text, row, scrollable, span, text, Space,
};
use iced::{Background, Border, Color, Element, Length, Subscription, Task};
use tokio::sync::broadcast::error::RecvError;

use crate::service::events::{global, BackendEvent};

use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Font size of terminal rows (contracts kept small so ~120 columns fit a
/// half-width pane).
pub const FONT_SIZE: f32 = 13.0;

/// Exact row height. The virtualizer's pixel math depends on this being
/// absolute (see [`visible_window`]).
pub const LINE_HEIGHT: f32 = 18.0;

/// Scrollback cap in lines; the oldest lines are dropped beyond it.
pub const MAX_SCROLLBACK_LINES: usize = 10_000;

/// Rows rendered before/after the visible range while scrolled into history
/// (same idea as `history_panel::OVERSCAN`).
const OVERSCAN: usize = 20;

/// Tab stops (classic 8-column terminals).
const TAB_WIDTH: usize = 8;

/// Follow-mode snaps within this pixel slack of the bottom.
const FOLLOW_EPSILON: f32 = 0.5;

/// Upper bound for one CSI parameter payload before it is treated as
/// malformed and dropped.
const MAX_CSI_BYTES: usize = 64;

/// Fixed terminal surface, independent of the light/dark theme (a terminal
/// is always dark, like the old panel).
const TERMINAL_BG: Color = rgb(0x0d0d0d);

/// Default text color on the dark surface.
const DEFAULT_FG: Color = rgb(0xd4d4d4);

/// Base 8 colors (Tomorrow Night; "black" is lightened for legibility on the
/// near-black surface).
const PALETTE_NORMAL: [Color; 8] = [
    rgb(0x4e4e4e),
    rgb(0xcc6666),
    rgb(0xb5bd68),
    rgb(0xf0c674),
    rgb(0x81a2be),
    rgb(0xb294bb),
    rgb(0x8abeb7),
    rgb(0xc5c8c6),
];

/// Bright 8 colors (SGR 90-97, and SGR 1 brightening 30-37).
const PALETTE_BRIGHT: [Color; 8] = [
    rgb(0x666669),
    rgb(0xd54e53),
    rgb(0xb9ca4a),
    rgb(0xe7c547),
    rgb(0x7aa6da),
    rgb(0xc397d8),
    rgb(0x70c0b1),
    rgb(0xffffff),
];

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// Resolve a normalized palette index (0-15) to a color; `bold` brightens the
/// 8 base colors (contracts.md SGR subset: "1 bold → brighter").
fn palette_color(index: u8, bold: bool) -> Color {
    let base = (index % 8) as usize;
    if index >= 8 || bold {
        PALETTE_BRIGHT[base]
    } else {
        PALETTE_NORMAL[base]
    }
}

// ---------------------------------------------------------------------------
// Scrollback model + ANSI parser
// ---------------------------------------------------------------------------

/// One styled run of terminal text.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    /// Resolved foreground color; `None` renders with [`DEFAULT_FG`].
    pub color: Option<Color>,
    /// SGR 1 was active (see module docs for how bold is rendered).
    pub bold: bool,
}

/// One scrollback line: styled runs in order.
pub type Line = Vec<Run>;

/// Active SGR state while feeding a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    /// Normalized palette index 0-15; `None` = default foreground.
    fg: Option<u8>,
    bold: bool,
}

fn run_color(style: Style) -> Option<Color> {
    style.fg.map(|index| palette_color(index, style.bold))
}

/// Escape-sequence parser states. All states survive chunk boundaries: a
/// sequence split across [`Scrollback::feed`] calls is reassembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Parse {
    #[default]
    Ground,
    /// ESC seen; waiting for the selector byte.
    Esc,
    /// Consume exactly one byte (charset selectors like `ESC ( B`, RIS, ...).
    EscOne,
    /// CSI: collecting parameters until a final byte (0x40-0x7E).
    Csi,
    /// OSC/DCS/APC/PM string: consume until BEL or ST.
    String,
    /// Saw ESC inside a string: expect the `\` of ST.
    StringEsc,
}

/// Styled scrollback with the ANSI parser state attached. `feed` accepts
/// arbitrary chunk boundaries; the chunked state machine reassembles escape
/// sequences spanning feeds (unit-tested).
#[derive(Debug, Clone, Default)]
pub struct Scrollback {
    lines: Vec<Line>,
    /// Cursor row (absolute index into `lines`; append-only except for
    /// `ED`/trim adjustments — see module docs for the monitor scope).
    row: usize,
    /// Cursor column in chars.
    col: usize,
    /// `CSI s` / `CSI u` saved position.
    saved: Option<(usize, usize)>,
    state: Parse,
    csi: Vec<u8>,
    /// Printable text of the current chunk not yet committed to a line;
    /// flushed on style changes, control chars and at the end of `feed`.
    pending: String,
    pending_style: Style,
    fg: Option<u8>,
    bold: bool,
}

impl Scrollback {
    pub fn new() -> Self {
        Self::default()
    }

    /// Styled lines, oldest first.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Cursor position as `(row, col)` — row indexes [`Scrollback::lines`].
    /// Parser-state introspection (tests); the view renders from `lines`.
    #[allow(dead_code)]
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Whether no output has arrived yet (tests / empty-state checks).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Forget everything (fresh session).
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Feed one output chunk (lossy UTF-8 from the backend, escape sequences
    /// possibly split anywhere).
    pub fn feed(&mut self, data: &str) {
        for ch in data.chars() {
            match self.state {
                Parse::Ground => self.ground(ch),
                Parse::Esc => match ch {
                    '[' => {
                        self.csi.clear();
                        self.state = Parse::Csi;
                    }
                    ']' | 'P' | 'X' | '^' | '_' => self.state = Parse::String,
                    '\u{1b}' => {}
                    _ => self.state = Parse::EscOne,
                },
                Parse::EscOne => self.state = Parse::Ground,
                Parse::Csi => match ch {
                    '\u{1b}' => self.state = Parse::Esc,
                    '\u{18}' | '\u{1a}' => self.state = Parse::Ground, // CAN/SUB abort
                    c if ('\u{40}'..='\u{7e}').contains(&c) => {
                        let params = std::mem::take(&mut self.csi);
                        self.dispatch_csi(&params, c);
                        self.state = Parse::Ground;
                    }
                    _ => {
                        // Oversized payloads are discarded but the sequence is
                        // still scanned to its final byte, so nothing leaks as
                        // printable text.
                        if self.csi.len() < MAX_CSI_BYTES {
                            self.csi.push(ch as u8);
                        }
                    }
                },
                Parse::String => match ch {
                    '\u{7}' | '\u{9c}' => self.state = Parse::Ground,
                    '\u{1b}' => self.state = Parse::StringEsc,
                    _ => {} // title/hyperlink payloads: swallowed
                },
                Parse::StringEsc => self.state = Parse::Ground, // ST (`ESC \`) or recovery
            }
        }
        // The pending buffer only spans one feed call: everything printable
        // must land in `lines` before the next render.
        self.flush_pending();
    }

    /// Drop lines beyond [`MAX_SCROLLBACK_LINES`], keeping the newest, and
    /// re-anchor the cursor. The pure half is [`trim_oldest`].
    pub fn trim_to_cap(&mut self) {
        let dropped = trim_oldest(&mut self.lines, MAX_SCROLLBACK_LINES);
        if dropped > 0 {
            self.row = self.row.saturating_sub(dropped);
            if let Some((row, col)) = self.saved {
                self.saved = Some((row.saturating_sub(dropped), col));
            }
        }
    }

    // -- ground-state handling ------------------------------------------------

    fn ground(&mut self, ch: char) {
        match ch {
            '\u{8}' => {
                // BS moves the cursor only; the next write overwrites.
                self.flush_pending();
                self.col = self.col.saturating_sub(1);
            }
            '\u{9}' => {
                self.flush_pending();
                self.col = (self.col / TAB_WIDTH + 1) * TAB_WIDTH;
            }
            '\u{a}' | '\u{b}' | '\u{c}' => {
                // LF/VT/FF move down; the column survives (CRLF arrives as
                // two controls, so shells still get col 0 via the CR).
                self.flush_pending();
                self.row += 1;
                self.ensure_line();
            }
            '\u{d}' => {
                self.flush_pending();
                self.col = 0;
            }
            '\u{1b}' => {
                self.flush_pending();
                self.state = Parse::Esc;
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => {} // BEL + other C0/DEL
            c => self.push_printable(c),
        }
    }

    fn push_printable(&mut self, c: char) {
        let style = Style {
            fg: self.fg,
            bold: self.bold,
        };
        if !self.pending.is_empty() && self.pending_style != style {
            self.flush_pending();
        }
        if self.pending.is_empty() {
            self.pending_style = style;
        }
        self.pending.push(c);
    }

    fn flush_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending);
        let style = self.pending_style;
        self.write_at(&text, style);
    }

    // -- line editing ---------------------------------------------------------

    fn ensure_line(&mut self) {
        while self.lines.len() <= self.row {
            self.lines.push(Vec::new());
        }
    }

    /// Commit `text` at the cursor (overwrite semantics), then advance the
    /// cursor past it.
    fn write_at(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.ensure_line();
        let len = char_len(&self.lines[self.row]);
        if self.col >= len {
            let line = &mut self.lines[self.row];
            if self.col > len {
                // Jumped past the end (tabs, CHA): pad with blank cells.
                push_run(line, " ".repeat(self.col - len), Style::default());
            }
            push_run(line, text.to_owned(), style);
        } else {
            let mut line = std::mem::take(&mut self.lines[self.row]);
            splice(&mut line, self.col, text, style);
            self.lines[self.row] = line;
        }
        self.col += text.chars().count();
    }

    /// Blank a char range of the cursor line (eraser cells become plain
    /// spaces — see module docs).
    fn blank_range(&mut self, from: usize, to: usize) {
        if to <= from {
            return;
        }
        let mut line = std::mem::take(&mut self.lines[self.row]);
        splice(&mut line, from, &" ".repeat(to - from), Style::default());
        self.lines[self.row] = line;
    }

    fn erase_in_line(&mut self, mode: u32) {
        self.ensure_line();
        let len = char_len(&self.lines[self.row]);
        match mode {
            0 => self.blank_range(self.col.min(len), len),
            1 => self.blank_range(0, (self.col + 1).min(len)),
            2 => self.blank_range(0, len),
            _ => {}
        }
    }

    fn erase_in_display(&mut self, mode: u32) {
        match mode {
            0 => {
                self.ensure_line();
                let len = char_len(&self.lines[self.row]);
                let from = self.col.min(len);
                self.blank_range(from, len);
                self.lines.truncate(self.row + 1);
            }
            1 => {
                if self.row > 0 {
                    self.lines.drain(0..self.row);
                    self.row = 0;
                }
                self.ensure_line();
                let len = char_len(&self.lines[0]);
                self.blank_range(0, (self.col + 1).min(len));
            }
            // 3 also clears the scrollback per xterm; same net effect here.
            2 | 3 => {
                self.lines = vec![Vec::new()];
                self.row = 0;
                self.col = 0;
            }
            _ => {}
        }
    }

    // -- CSI dispatch ---------------------------------------------------------

    fn dispatch_csi(&mut self, params: &[u8], final_byte: char) {
        match final_byte {
            'm' => self.sgr(params),
            'K' => self.erase_in_line(first_param(params)),
            'J' => self.erase_in_display(first_param(params)),
            'G' | '`' => {
                let column = first_param(params).max(1);
                self.col = (column - 1) as usize;
            }
            'C' => self.col += first_param(params).max(1) as usize,
            'D' => self.col = self.col.saturating_sub(first_param(params).max(1) as usize),
            'X' => {
                let count = first_param(params).max(1) as usize;
                self.ensure_line();
                let len = char_len(&self.lines[self.row]);
                let from = self.col.min(len);
                self.blank_range(from, (from + count).min(len));
            }
            's' => self.saved = Some((self.row, self.col)),
            'u' => {
                if let Some((row, col)) = self.saved {
                    self.row = row.min(self.lines.len().saturating_sub(1));
                    self.col = col;
                }
            }
            // Cursor addressing (A/B/H/f/d), scroll regions, modes, DSR, ...:
            // stripped — monitor scope (module docs).
            _ => {}
        }
    }

    /// SGR subset: 0 reset, 1 bold (→ brighter), 22 normal, 30-37/90-97 fg,
    /// 39 default; extended color (38/48) tails are consumed so subsequent
    /// parameters still apply, with `38;5;0-15` mapped onto the 16-color
    /// palette and everything else ignored (no background support).
    fn sgr(&mut self, params: &[u8]) {
        let values = csi_params(params);
        let mut i = 0;
        while i < values.len() {
            match values[i] {
                0 => {
                    self.fg = None;
                    self.bold = false;
                }
                1 => self.bold = true,
                2 | 21 | 22 => self.bold = false,
                30..=37 => self.fg = Some((values[i] - 30) as u8),
                39 => self.fg = None,
                90..=97 => self.fg = Some((values[i] - 90 + 8) as u8),
                38 | 48 => match values.get(i + 1) {
                    Some(5) => {
                        if values[i] == 38 {
                            if let Some(index @ 0..=15) = values.get(i + 2).copied() {
                                self.fg = Some(index as u8);
                            }
                        }
                        i += 2;
                    }
                    // 38;2;r;g;b truecolor: consumed, color ignored.
                    Some(2) => i += 4,
                    // Malformed tail: drop the rest of the sequence.
                    _ => break,
                },
                _ => {} // underline/blink/reverse/bg colors: out of scope
            }
            i += 1;
        }
    }
}

/// Char length of a line across its runs.
fn char_len(line: &Line) -> usize {
    line.iter().map(|run| run.text.chars().count()).sum()
}

/// Append `text` to a line, merging into the last run when the style matches
/// (keeps runs coarse across feed boundaries).
fn push_run(line: &mut Line, text: String, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = line.last_mut() {
        if last.bold == style.bold && last.color == run_color(style) {
            last.text.push_str(&text);
            return;
        }
    }
    line.push(Run {
        text,
        color: run_color(style),
        bold: style.bold,
    });
}

/// Overwrite `[start, start + text)` with `text` in `style`, splitting
/// boundary runs and keeping everything else intact.
fn splice(line: &mut Line, start: usize, text: &str, style: Style) {
    let count = text.chars().count();
    let end = start + count;
    let mut out: Vec<Run> = Vec::with_capacity(line.len() + 2);
    let mut inserted = false;
    let mut pos = 0usize;

    for run in std::mem::take(line) {
        let len = run.text.chars().count();
        let run_end = pos + len;
        if pos < start {
            // Untouched head before the replaced range (may be the whole run).
            let head = (start - pos).min(len);
            if head > 0 {
                out.push(Run {
                    text: run.text.chars().take(head).collect(),
                    ..run.clone()
                });
            }
        }
        if !inserted && run_end >= start {
            out.push(Run {
                text: text.to_owned(),
                color: run_color(style),
                bold: style.bold,
            });
            inserted = true;
        }
        if run_end > end {
            // Untouched tail after the replaced range.
            let skip = (end.saturating_sub(pos)).min(len);
            if skip < len {
                out.push(Run {
                    text: run.text.chars().skip(skip).collect(),
                    ..run.clone()
                });
            }
        }
        pos = run_end;
    }
    if !inserted {
        out.push(Run {
            text: text.to_owned(),
            color: run_color(style),
            bold: style.bold,
        });
    }
    *line = out;
    normalize(line);
}

/// Merge adjacent same-style runs and drop empties (keeps `Line`s compact).
fn normalize(line: &mut Line) {
    line.retain(|run| !run.text.is_empty());
    let mut merged: Line = Vec::with_capacity(line.len());
    for run in line.drain(..) {
        match merged.last_mut() {
            Some(last) if last.bold == run.bold && last.color == run.color => {
                last.text.push_str(&run.text);
            }
            _ => merged.push(run),
        }
    }
    *line = merged;
}

/// Parse a CSI parameter payload into u32s (empty segments are 0; non-digit
/// bytes — private markers, intermediates — are skipped; capped).
fn csi_params(params: &[u8]) -> Vec<u32> {
    let mut values: Vec<u32> = Vec::new();
    let mut current: Option<u32> = None;
    for &byte in params {
        match byte {
            b'0'..=b'9' => {
                current = Some(
                    current
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(u32::from(byte - b'0')),
                );
            }
            b';' => values.push(current.take().unwrap_or(0)),
            _ => {}
        }
        if values.len() >= 32 {
            return values;
        }
    }
    values.push(current.unwrap_or(0));
    values
}

/// First CSI parameter (0 when absent), for dispatches with a default of 0.
fn first_param(params: &[u8]) -> u32 {
    csi_params(params).first().copied().unwrap_or(0)
}

/// Pure scrollback trim: drop the oldest lines beyond `cap` (keeping the
/// newest), return how many were dropped.
pub fn trim_oldest(lines: &mut Vec<Line>, cap: usize) -> usize {
    let excess = lines.len().saturating_sub(cap.max(1));
    if excess > 0 {
        lines.drain(0..excess);
    }
    excess
}

// ---------------------------------------------------------------------------
// Messages, actions, state
// ---------------------------------------------------------------------------

/// Pane messages. `Opened`/`Output`/`Exit` mirror the backend PTY events;
/// the rest are user input or view events.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A session was spawned; the pane resets its scrollback and follows it.
    /// Also carries remote (`rshell-*`) session ids — the event shapes match.
    Opened(String),
    /// One coalesced `pty-output` window for `session`.
    Output(String, String),
    /// The final `pty-exit` for `session`; the string is the exit code
    /// (`None` reserved for unknown/degraded terminations).
    Exit(String, Option<String>),
    /// Key press while the pane is focused (orchestrator wires the source).
    Key(Key, Modifiers),
    /// Single printable character (convenience input path; the shell
    /// currently routes everything through [`Msg::Key`]).
    #[allow(dead_code)]
    Char(char),
    /// Clipboard paste; newlines are normalized to `\r` like real terminals.
    Paste(String),
    /// The pane's grid changed (orchestrator derives cols/rows from pixels).
    /// Note the field order: cols first. Unwired while the pane uses the
    /// spawn-time grid; live-resize tracking is a follow-up.
    #[allow(dead_code)]
    Resize { cols: u16, rows: u16 },
    /// The scrollback scrolled: absolute y offset and viewport height.
    /// (Adapted from the contract's `Scroll(i32)` to the `DiffScrolled` /
    /// `HistoryScrolled` viewport idiom this codebase already uses.)
    Scroll { y: f32, viewport: f32 },
    /// The pane was clicked; marks it focused.
    Click,
    /// The "Open shell" affordance was pressed (only rendered pre-session).
    SpawnRequested,
    /// The Restart button on the exited strip was pressed.
    RestartPressed,
}

/// Intents the app executes. All PTY calls are synchronous; the app performs
/// them inline in its `update` (see module docs for the exact mapping).
#[derive(Debug, Clone)]
pub enum Action {
    /// Spawn the default shell rooted at `repo_root`
    /// (`crate::pty::pty_create`; the app maps the root to its open
    /// `RepoId`). The resulting session id comes back as [`Msg::Opened`].
    Spawn { repo_root: PathBuf },
    /// `crate::pty::pty_write`: keystrokes/paste for the session.
    Write { session: String, data: String },
    /// `crate::pty::pty_resize`: field order is cols, rows (pty order is
    /// `pty_resize(.., rows, cols)` — mind the swap when forwarding).
    Resize {
        session: String,
        cols: u16,
        rows: u16,
    },
    /// `crate::pty::pty_kill` (also used on tab close; errors mean the
    /// session is already gone). The shell kills directly on tab close, so
    /// the pane itself never emits this today.
    #[allow(dead_code)]
    Kill { session: String },
    /// Kill `old_session` best-effort, then spawn a fresh shell at
    /// `repo_root` and deliver the new id via [`Msg::Opened`].
    Restart {
        old_session: Option<String>,
        repo_root: PathBuf,
    },
    /// UI-only intent: run [`snap_to_bottom`] for the pane's scrollable so
    /// follow mode stays pinned to the newest output.
    FollowScroll { id: Id },
}

/// Per-pane terminal state (one per repo tab).
#[derive(Debug, Clone)]
pub struct State {
    repo_root: Option<PathBuf>,
    session: Option<String>,
    /// Exit code string once `pty-exit` arrived.
    exit: Option<String>,
    /// Scrollable id, derived from the session so multiple panes stay unique.
    scroll_id: Option<Id>,
    scrollback: Scrollback,
    cols: u16,
    rows: u16,
    scroll_y: f32,
    viewport_height: f32,
    /// Follow mode: pinned to the newest output (re-armed via
    /// [`Action::FollowScroll`] and released by any upward scroll).
    follow: bool,
    focused: bool,
}

impl State {
    /// `repo_root` is the local workdir the shell spawns in (`None` renders
    /// the pane without a spawn affordance; remote projects route through
    /// `rshell_create` on the app side instead).
    pub fn new(repo_root: Option<PathBuf>) -> Self {
        Self {
            repo_root,
            session: None,
            exit: None,
            scroll_id: None,
            scrollback: Scrollback::new(),
            cols: 80,
            rows: 24,
            scroll_y: 0.0,
            viewport_height: 0.0,
            follow: true,
            focused: false,
        }
    }

    /// Live session id, if any.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Exit code string once the process exited.
    pub fn exit_code(&self) -> Option<&str> {
        self.exit.as_deref()
    }

    /// Process-exit introspection (tests; the view reads `exit_code`).
    #[allow(dead_code)]
    pub fn is_exited(&self) -> bool {
        self.exit.is_some()
    }

    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Whether the pane currently holds keyboard focus (set by [`Msg::Click`],
    /// cleared by [`State::blur`]). The shell gates its key-capture
    /// subscription on this.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Releases keyboard focus (the shell calls this when another typing
    /// surface takes over: tab switches, commit editor, filter inputs…).
    pub fn blur(&mut self) {
        self.focused = false;
    }

    /// Styled scrollback lines, oldest first.
    pub fn lines(&self) -> &[Line] {
        self.scrollback.lines()
    }

    /// Parser cursor as `(row, col)` (tests; the view renders `lines`).
    #[allow(dead_code)]
    pub fn cursor(&self) -> (usize, usize) {
        self.scrollback.cursor()
    }

    /// Grid dimensions as `(cols, rows)`.
    pub fn dims(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Whether follow mode is currently pinned to the newest output (tests
    /// and shell diagnostics; the view tracks follow internally).
    #[allow(dead_code)]
    pub fn at_bottom(&self) -> bool {
        self.follow
    }

    /// The scrollable's id (present once a session was opened).
    /// The scrollable's id (tests / shell diagnostics; the view reads the
    /// field directly).
    #[allow(dead_code)]
    pub fn scrollable_id(&self) -> Option<Id> {
        self.scroll_id.clone()
    }

    fn scrollable_id_or_fallback(&self) -> Id {
        self.scroll_id
            .clone()
            .unwrap_or_else(|| Id::from("mygitui-terminal-unattached"))
    }

    /// Session that can accept input: live and not exited.
    fn writable_session(&self) -> Option<&str> {
        let session = self.session.as_deref()?;
        self.exit.is_none().then_some(session)
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Pure state machine: apply `msg` to `state`, return the intent (if any)
/// for the app to execute. See the module docs for the update-shape decision.
pub fn update(state: &mut State, msg: Msg) -> Option<Action> {
    match msg {
        Msg::Opened(session) => {
            let id = scrollable_id_for(&session);
            state.session = Some(session);
            state.exit = None;
            state.scrollback.clear();
            state.follow = true;
            state.scroll_id = Some(id.clone());
            // Arm relative-offset follow before the first output arrives.
            Some(Action::FollowScroll { id })
        }
        Msg::Output(session, data) => {
            if state.session.as_deref() != Some(session.as_str()) {
                return None; // stale chunk from a previous session
            }
            state.scrollback.feed(&data);
            state.scrollback.trim_to_cap();
            state.follow.then(|| Action::FollowScroll {
                id: state.scrollable_id_or_fallback(),
            })
        }
        Msg::Exit(session, code) => {
            if state.session.as_deref() == Some(session.as_str()) {
                state.exit = code;
            }
            None
        }
        Msg::Key(key, modifiers) => {
            let session = state.writable_session()?.to_owned();
            key_sequence(&key, modifiers).map(|data| Action::Write { session, data })
        }
        Msg::Char(c) => {
            let session = state.writable_session()?.to_owned();
            Some(Action::Write {
                session,
                data: c.to_string(),
            })
        }
        Msg::Paste(text) => {
            let session = state.writable_session()?.to_owned();
            Some(Action::Write {
                session,
                data: normalize_paste(&text),
            })
        }
        Msg::Resize { cols, rows } => {
            state.cols = cols.max(1);
            state.rows = rows.max(1);
            state.session.as_ref().map(|session| Action::Resize {
                session: session.clone(),
                cols: state.cols,
                rows: state.rows,
            })
        }
        Msg::Scroll { y, viewport } => {
            state.scroll_y = y.max(0.0);
            if viewport > 0.0 {
                state.viewport_height = viewport;
            }
            let content_height = state.scrollback.lines.len() as f32 * LINE_HEIGHT;
            state.follow = state.scroll_y + viewport >= content_height - FOLLOW_EPSILON;
            // Re-arm relative-offset follow whenever we sit at the bottom
            // (wheel-down lands on Absolute offsets, which would freeze).
            state.follow.then(|| Action::FollowScroll {
                id: state.scrollable_id_or_fallback(),
            })
        }
        Msg::Click => {
            state.focused = true;
            None
        }
        Msg::SpawnRequested => {
            if state.session.is_none() {
                state.repo_root.as_ref().map(|root| Action::Spawn {
                    repo_root: root.clone(),
                })
            } else {
                None
            }
        }
        Msg::RestartPressed => {
            if state.exit.is_some() {
                state.repo_root.as_ref().map(|root| Action::Restart {
                    old_session: state.session.clone(),
                    repo_root: root.clone(),
                })
            } else {
                None
            }
        }
    }
}

/// Stable scrollable id for a session (unique per pane).
fn scrollable_id_for(session: &str) -> Id {
    Id::from(format!("mygitui-terminal-{session}"))
}

/// Task pinning a pane's scrollable to the newest output (relative offset
/// END — iced keeps a relative offset glued to the content bottom as it
/// grows, which is what follow mode builds on). The app executes
/// [`Action::FollowScroll`] with this.
pub fn snap_to_bottom(id: Id) -> Task<Msg> {
    snap_to_end(id)
}

/// Normalize pasted text for a pty: terminals send Enter as `\r`, so
/// `\r\n` / `\n` become `\r`.
fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\r").replace('\n', "\r")
}

// ---------------------------------------------------------------------------
// Keyboard → byte sequences (xterm conventions for the supported subset)
// ---------------------------------------------------------------------------

/// Map a key press to the byte sequence a terminal would send. `None` means
/// "not terminal input" (window shortcuts, IME compositions, unknown keys).
fn key_sequence(key: &Key, modifiers: Modifiers) -> Option<String> {
    // Window shortcuts win (the app's ctrl/cmd bindings are not shell input).
    if modifiers.logo() {
        return None;
    }
    match key {
        Key::Named(named) => named_sequence(*named, modifiers),
        Key::Character(text) => {
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None; // IME/dead-key composition: ignore
            }
            let base = if modifiers.control() {
                char::from_u32(u32::from(ctrl_byte(c)?))?
            } else {
                c
            };
            // Alt-prefix (ESC-prefixed) like xterm.
            Some(if modifiers.alt() {
                format!("\u{1b}{base}")
            } else {
                base.to_string()
            })
        }
        Key::Unidentified => None,
    }
}

/// Control-character byte for ctrl+`c` (xterm mapping for the common keys).
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_uppercase() {
        'A'..='Z' => c.to_ascii_uppercase() as u8 - b'A' + 1,
        '@' | ' ' => 0,
        '[' => 27,
        '\\' => 28,
        ']' => 29,
        '^' => 30,
        '_' | '/' => 31,
        '?' => 127,
        _ => return None,
    })
}

/// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn modifier_param(modifiers: Modifiers) -> u32 {
    1 + u32::from(modifiers.shift())
        + 2 * u32::from(modifiers.alt())
        + 4 * u32::from(modifiers.control())
}

fn csi_letter(letter: char, modifiers: Modifiers) -> String {
    let param = modifier_param(modifiers);
    if param == 1 {
        format!("\u{1b}[{letter}")
    } else {
        format!("\u{1b}[1;{param}{letter}")
    }
}

fn csi_tilde(code: u32, modifiers: Modifiers) -> String {
    let param = modifier_param(modifiers);
    if param == 1 {
        format!("\u{1b}[{code}~")
    } else {
        format!("\u{1b}[{code};{param}~")
    }
}

fn named_sequence(named: Named, modifiers: Modifiers) -> Option<String> {
    use Named::*;
    Some(match named {
        Enter => "\r".to_owned(),
        Backspace => "\u{7f}".to_owned(),
        Tab => {
            if modifiers.shift() {
                "\u{1b}[Z".to_owned()
            } else {
                "\t".to_owned()
            }
        }
        Escape => "\u{1b}".to_owned(),
        Space => " ".to_owned(),
        ArrowUp => csi_letter('A', modifiers),
        ArrowDown => csi_letter('B', modifiers),
        ArrowRight => csi_letter('C', modifiers),
        ArrowLeft => csi_letter('D', modifiers),
        Home => csi_letter('H', modifiers),
        End => csi_letter('F', modifiers),
        Insert => csi_tilde(2, modifiers),
        Delete => csi_tilde(3, modifiers),
        PageUp => csi_tilde(5, modifiers),
        PageDown => csi_tilde(6, modifiers),
        F1 => "\u{1b}OP".to_owned(),
        F2 => "\u{1b}OQ".to_owned(),
        F3 => "\u{1b}OR".to_owned(),
        F4 => "\u{1b}OS".to_owned(),
        F5 => csi_tilde(15, modifiers),
        F6 => csi_tilde(17, modifiers),
        F7 => csi_tilde(18, modifiers),
        F8 => csi_tilde(19, modifiers),
        F9 => csi_tilde(20, modifiers),
        F10 => csi_tilde(21, modifiers),
        F11 => csi_tilde(23, modifiers),
        F12 => csi_tilde(24, modifiers),
        // Numpad digits/operators arrive as `Key::Character` in iced (winit
        // reports them as text), so no Numpad* arms are needed here.
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Render the pane: fixed dark surface, exit strip when the process ended,
/// monospace scrollback rows, spawn affordance before a session exists.
pub fn view<'a>(state: &'a State, tokens: Tokens) -> Element<'a, Msg> {
    let border_color = if state.focused {
        tokens.ring
    } else {
        tokens.border
    };
    let body: Element<'a, Msg> = if state.has_session() {
        let mut body = column![];
        if let Some(code) = state.exit_code() {
            body = body.push(exit_strip(code, tokens));
        }
        body.push(scroll_area(state))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    } else {
        empty_state(state, tokens)
    };

    // Click-to-focus: any press inside the pane grabs the keyboard (the
    // shell's key-capture subscription is gated on this flag).
    mouse_area(
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(Background::Color(TERMINAL_BG)),
                text_color: Some(DEFAULT_FG),
                border: Border {
                    color: border_color,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            }),
    )
    .on_press(Msg::Click)
    .into()
}

/// "process exited (code N)" strip with the Restart affordance.
fn exit_strip<'a>(code: &str, tokens: Tokens) -> Element<'a, Msg> {
    row![
        text(format!("process exited (code {code})"))
            .size(11)
            .color(tokens.destructive),
        Space::new().width(Length::Fill),
        button(text("Restart").size(10))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([3, 10])
            .on_press(Msg::RestartPressed),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .width(Length::Fill)
    .padding([6, 10])
    .into()
}

/// The scrollback: virtualized monospace rows (same scheme as
/// `history_panel` — fixed [`LINE_HEIGHT`], top/bottom pads, overscan).
fn scroll_area<'a>(state: &'a State) -> Element<'a, Msg> {
    let lines = state.lines();
    let win = visible_window(
        lines.len(),
        state.scroll_y,
        state.viewport_height,
        state.follow,
    );

    let mut content = column![].width(Length::Fill);
    if win.top_pad > 0.0 {
        content = content.push(Space::new().height(Length::Fixed(win.top_pad)));
    }
    for line in &lines[win.first..win.end] {
        content = content.push(line_row(line));
    }
    if win.bottom_pad > 0.0 {
        content = content.push(Space::new().height(Length::Fixed(win.bottom_pad)));
    }

    scrollable(content)
        .id(state.scrollable_id_or_fallback())
        .on_scroll(|viewport| Msg::Scroll {
            y: viewport.absolute_offset().y,
            viewport: viewport.bounds().height,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One scrollback line as rich text with an exact absolute line height.
fn line_row<'a>(line: &'a Line) -> Element<'a, Msg> {
    if line.is_empty() {
        // Blank rows still occupy exactly one line of the virtual space.
        return Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(LINE_HEIGHT))
            .into();
    }
    let spans: Vec<iced::widget::text::Span<'a, (), iced::Font>> = line
        .iter()
        .map(|run| {
            let font = theme::mono();
            span(run.text.as_str())
                .font(if run.bold {
                    iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..font
                    }
                } else {
                    font
                })
                .color_maybe(run.color)
        })
        .collect();
    rich_text(spans)
        .size(FONT_SIZE)
        .font(theme::mono())
        .line_height(LineHeight::Absolute(LINE_HEIGHT.into()))
        .wrapping(Wrapping::None)
        .width(Length::Fill)
        .into()
}

/// Placeholder before a session exists (with the spawn affordance when the
/// pane knows a repo root).
fn empty_state<'a>(state: &'a State, tokens: Tokens) -> Element<'a, Msg> {
    let mut prompt = column![text("No shell session")
        .size(12)
        .color(tokens.muted_foreground)]
    .spacing(10)
    .align_x(iced::Alignment::Center);
    if state.repo_root.is_some() {
        prompt = prompt.push(
            button(text("Open shell").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([4, 12])
                .on_press(Msg::SpawnRequested),
        );
    }
    container(prompt).center(Length::Fill).into()
}

/// Visible-row window over the virtual scrollback (pads in pixels), keeping
/// the rendered content exactly `lines * LINE_HEIGHT` tall so scrollbar math
/// stays truthful. Follow mode renders the newest rows (the scrollable's
/// offset is pinned to the bottom by [`snap_to_bottom`]); history mode
/// windows around the scroll offset like `history_panel`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleWindow {
    pub first: usize,
    pub end: usize,
    pub top_pad: f32,
    pub bottom_pad: f32,
}

pub fn visible_window(total: usize, scroll_y: f32, viewport: f32, follow: bool) -> VisibleWindow {
    if total == 0 {
        return VisibleWindow {
            first: 0,
            end: 0,
            top_pad: 0.0,
            bottom_pad: 0.0,
        };
    }
    if follow {
        let visible = (viewport / LINE_HEIGHT).ceil() as usize;
        let count = (visible + OVERSCAN).min(total);
        let first = total - count;
        return VisibleWindow {
            first,
            end: total,
            top_pad: first as f32 * LINE_HEIGHT,
            bottom_pad: 0.0,
        };
    }
    let first_line = (scroll_y / LINE_HEIGHT).floor().max(0.0) as usize;
    let first = first_line.saturating_sub(OVERSCAN).min(total);
    let end_line = ((scroll_y + viewport) / LINE_HEIGHT).ceil().max(0.0) as usize;
    let end = end_line.saturating_add(OVERSCAN).clamp(first, total);
    VisibleWindow {
        first,
        end,
        top_pad: first as f32 * LINE_HEIGHT,
        bottom_pad: (total - end) as f32 * LINE_HEIGHT,
    }
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// Bridge the backend PTY events for one session into pane messages. Keyed
/// by session id (run one per live session, `Subscription::batch`-ed like
/// `App::auto_fetch_subscription`); the stream ends after the session's
/// `pty-exit`.
pub fn subscription(session_id: &str) -> Subscription<Msg> {
    Subscription::run_with(session_id.to_owned(), |session: &String| {
        let session = session.clone();
        iced::stream::channel(
            64,
            move |mut output: iced::futures::channel::mpsc::Sender<Msg>| async move {
                let mut receiver = global().subscribe();
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            let is_exit = matches!(&event, BackendEvent::PtyExit(payload) if payload.session_id == session);
                            let Some(msg) = session_msg(&session, event) else {
                                continue;
                            };
                            if output.send(msg).await.is_err() {
                                break; // consumer gone
                            }
                            // A session's exit is its final event.
                            if is_exit {
                                break;
                            }
                        }
                        Err(RecvError::Lagged(skipped)) => {
                            tracing::warn!(
                                skipped,
                                session = %session,
                                "terminal event stream lagged; skipping events"
                            );
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
            },
        )
    })
}

/// Pure event filter/mapping behind [`subscription`] (unit-tested): PTY
/// events for `session` become messages; everything else (other sessions'
/// output, repo events) is dropped. Local (`pty-*`) and remote (`rshell-*`)
/// sessions share the same event shapes, so both work unchanged.
fn session_msg(session: &str, event: BackendEvent) -> Option<Msg> {
    match event {
        BackendEvent::PtyOutput(payload) if payload.session_id == session => {
            Some(Msg::Output(payload.session_id, payload.data))
        }
        BackendEvent::PtyExit(payload) if payload.session_id == session => Some(Msg::Exit(
            payload.session_id,
            Some(payload.code.to_string()),
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    // Named variants (this shadows the widget `Space` re-exported by `super::*`,
    // which the tests do not use).
    use iced::keyboard::key::Named::*;

    // -- parser: plain flow ---------------------------------------------------

    fn feed_text(data: &str) -> Scrollback {
        let mut sb = Scrollback::new();
        sb.feed(data);
        sb
    }

    fn line_text(sb: &Scrollback, row: usize) -> String {
        sb.lines()[row]
            .iter()
            .map(|run| run.text.as_str())
            .collect()
    }

    #[test]
    fn plain_text_lands_as_single_run() {
        let sb = feed_text("hello world");
        assert_eq!(sb.lines().len(), 1);
        assert_eq!(sb.lines()[0].len(), 1);
        assert_eq!(line_text(&sb, 0), "hello world");
        assert_eq!(sb.lines()[0][0].color, None);
        assert!(!sb.lines()[0][0].bold);
        assert_eq!(sb.cursor(), (0, 11));
    }

    #[test]
    fn crlf_starts_new_lines() {
        let sb = feed_text("a\r\nb\r\nc");
        assert_eq!(sb.lines().len(), 3);
        assert_eq!(line_text(&sb, 0), "a");
        assert_eq!(line_text(&sb, 1), "b");
        assert_eq!(line_text(&sb, 2), "c");
    }

    #[test]
    fn bare_lf_keeps_column_like_a_raw_tty() {
        // The pty's line discipline turns \n into \r\n; a bare \n (raw-mode
        // programs) moves down without resetting the column, faithfully.
        let sb = feed_text("abc\ndef");
        assert_eq!(sb.lines().len(), 2);
        assert_eq!(line_text(&sb, 1), "   def");
    }

    #[test]
    fn carriage_return_overwrites_from_column_zero() {
        let sb = feed_text("abc\rX");
        assert_eq!(line_text(&sb, 0), "Xbc");
    }

    #[test]
    fn carriage_return_progress_bar_rewrites() {
        // Progress bars end the line with CR + (padded) rewrite or erase.
        let sb = feed_text("progress 10%\r\u{1b}[K100%");
        assert_eq!(line_text(&sb, 0), "100%        ");
        assert_eq!(sb.cursor(), (0, 4));
    }

    #[test]
    fn backspace_overwrites_backwards() {
        let sb = feed_text("abc\u{8}\u{8}Z");
        assert_eq!(line_text(&sb, 0), "aZc");
        assert_eq!(sb.cursor(), (0, 2));
    }

    #[test]
    fn backspace_at_column_zero_is_clamped() {
        let sb = feed_text("\u{8}\u{8}x");
        assert_eq!(line_text(&sb, 0), "x");
    }

    #[test]
    fn tab_expands_to_eight_column_stops() {
        let sb = feed_text("a\tb");
        assert_eq!(line_text(&sb, 0), "a       b");
        assert_eq!(sb.cursor(), (0, 9));
    }

    #[test]
    fn tab_on_tab_stop_advances_a_full_stop() {
        let sb = feed_text("12345678\tX");
        assert_eq!(line_text(&sb, 0), "12345678        X");
    }

    #[test]
    fn bel_and_other_c0_are_ignored() {
        let sb = feed_text("a\u{7}b\u{1}c");
        assert_eq!(line_text(&sb, 0), "abc");
    }

    #[test]
    fn multibyte_text_survives() {
        let sb = feed_text("héllo ✓");
        assert_eq!(line_text(&sb, 0), "héllo ✓");
        assert_eq!(sb.cursor(), (0, 7));
    }

    // -- parser: SGR colors ----------------------------------------------------

    #[test]
    fn sgr_31_sets_red() {
        let sb = feed_text("\u{1b}[31mred\u{1b}[0m plain");
        assert_eq!(sb.lines()[0].len(), 2);
        assert_eq!(sb.lines()[0][0].color, Some(PALETTE_NORMAL[1]));
        assert_eq!(sb.lines()[0][1].text, " plain");
        assert_eq!(sb.lines()[0][1].color, None);
    }

    #[test]
    fn sgr_bold_brightens_base_colors() {
        let sb = feed_text("\u{1b}[1;32mgreen");
        assert_eq!(sb.lines()[0][0].color, Some(PALETTE_BRIGHT[2]));
        assert!(sb.lines()[0][0].bold);
    }

    #[test]
    fn sgr_92_is_bright_green_directly() {
        let sb = feed_text("\u{1b}[92mg");
        assert_eq!(sb.lines()[0][0].color, Some(PALETTE_BRIGHT[2]));
    }

    #[test]
    fn sgr_39_restores_default_and_0_resets_bold() {
        let sb = feed_text("\u{1b}[1;34mblue\u{1b}[39mplain\u{1b}[0m\u{1b}[31mred");
        let runs = &sb.lines()[0];
        assert_eq!(
            runs[0].color,
            Some(PALETTE_BRIGHT[4]),
            "bold brightens blue"
        );
        assert!(runs[0].bold);
        assert_eq!(runs[1].color, None);
        assert!(runs[1].bold, "39 must not reset bold");
        assert_eq!(runs[2].color, Some(PALETTE_NORMAL[1]));
        assert!(!runs[2].bold, "0 must reset bold");
    }

    #[test]
    fn sgr_22_drops_bold_only() {
        let sb = feed_text("\u{1b}[1;34mb\u{1b}[22mb");
        let runs = &sb.lines()[0];
        assert!(runs[0].bold);
        assert!(!runs[1].bold);
        assert_eq!(runs[1].color, Some(PALETTE_NORMAL[4]));
    }

    #[test]
    fn sgr_256_color_tail_is_consumed_and_0_15_mapped() {
        // 38;5;196 (out of palette) is consumed without leaking text…
        let sb = feed_text("\u{1b}[38;5;196;1mX");
        let runs = &sb.lines()[0];
        assert_eq!(line_text(&sb, 0), "X");
        assert!(runs[0].bold);
        assert_eq!(runs[0].color, None);
        // …and 38;5;n for n < 16 maps onto the 16-color palette.
        let sb = feed_text("\u{1b}[38;5;9mY");
        assert_eq!(sb.lines()[0][0].color, Some(PALETTE_BRIGHT[1]));
    }

    #[test]
    fn sgr_truecolor_tail_is_consumed() {
        let sb = feed_text("\u{1b}[38;2;12;34;56;4;31mZ");
        // 4 (underline) ignored, 31 still applied after the truecolor tail.
        assert_eq!(sb.lines()[0][0].color, Some(PALETTE_NORMAL[1]));
        assert_eq!(line_text(&sb, 0), "Z");
    }

    #[test]
    fn sgr_background_and_attributes_are_ignored() {
        let sb = feed_text("\u{1b}[41;4;7;5mtext");
        assert_eq!(line_text(&sb, 0), "text");
        assert_eq!(sb.lines()[0][0].color, None);
    }

    #[test]
    fn empty_sgr_resets() {
        let sb = feed_text("\u{1b}[1;31ma\u{1b}[mb");
        let runs = &sb.lines()[0];
        assert!(runs[0].bold);
        assert!(!runs[1].bold);
        assert_eq!(runs[1].color, None);
    }

    // -- parser: chunked input (the hard part) ---------------------------------

    #[test]
    fn escape_sequence_split_across_feeds() {
        let mut sb = Scrollback::new();
        sb.feed("ok\u{1b}[3");
        sb.feed("1mred");
        let runs = &sb.lines()[0];
        assert_eq!(runs[0].text, "ok");
        assert_eq!(runs[0].color, None);
        assert_eq!(runs[1].text, "red");
        assert_eq!(runs[1].color, Some(PALETTE_NORMAL[1]));
    }

    #[test]
    fn escape_sequence_split_byte_by_byte() {
        let mut sb = Scrollback::new();
        for chunk in ["a", "\u{1b}", "[", "9", "1", ";", "3", "2", "m", "b"] {
            sb.feed(chunk);
        }
        let runs = &sb.lines()[0];
        assert_eq!(runs[0].text, "a");
        assert_eq!(runs[1].text, "b");
        assert_eq!(runs[1].color, Some(PALETTE_NORMAL[2]), "last SGR fg wins");
    }

    #[test]
    fn osc_title_split_across_feeds() {
        let mut sb = Scrollback::new();
        sb.feed("\u{1b}]0;win");
        sb.feed("dow title\u{7}tail");
        assert_eq!(line_text(&sb, 0), "tail");
    }

    #[test]
    fn osc_terminated_by_st() {
        let sb = feed_text("\u{1b}]8;;https://x\u{1b}\\after");
        assert_eq!(line_text(&sb, 0), "after");
    }

    #[test]
    fn charset_selector_consumed_across_feeds() {
        let mut sb = Scrollback::new();
        sb.feed("a\u{1b}(B");
        sb.feed("b");
        assert_eq!(line_text(&sb, 0), "ab");
    }

    #[test]
    fn csi_abort_and_restart_recovers() {
        // CAN aborts a CSI; the following sequence still applies.
        let sb = feed_text("\u{1b}[31\u{18}\u{1b}[32mgreen");
        assert_eq!(sb.lines()[0][0].color, Some(PALETTE_NORMAL[2]));
    }

    #[test]
    fn control_chars_around_chunk_boundaries() {
        let mut sb = Scrollback::new();
        sb.feed("one\r");
        sb.feed("\ntwo\u{8}");
        sb.feed("\u{8}!");
        // "two" then two backspaces (one per chunk boundary) then "!"
        // overwrites at column 1.
        assert_eq!(line_text(&sb, 0), "one");
        assert_eq!(line_text(&sb, 1), "t!o");
    }

    #[test]
    fn style_runs_merge_across_feeds() {
        let mut sb = Scrollback::new();
        sb.feed("\u{1b}[31mred");
        sb.feed("der");
        assert_eq!(sb.lines()[0].len(), 1, "same-style runs merge");
        assert_eq!(line_text(&sb, 0), "redder");
    }

    // -- parser: erasing and cursor addressing ---------------------------------

    #[test]
    fn erase_in_line_mode_2_then_write() {
        let sb = feed_text("hello\u{1b}[2Khi");
        // K2 blanks the line, the column survives, so "hi" lands at col 5.
        assert_eq!(line_text(&sb, 0), "     hi");
    }

    #[test]
    fn erase_in_line_mode_0_truncates_visually() {
        let sb = feed_text("hello world\u{1b}[6G\u{1b}[KX");
        // Cursor to col 5, erase to end (blank cells), overwrite with X.
        assert_eq!(line_text(&sb, 0), "helloX     ");
    }

    #[test]
    fn cursor_horizontal_absolute() {
        let sb = feed_text("abc\u{1b}[1GX");
        assert_eq!(line_text(&sb, 0), "Xbc");
    }

    #[test]
    fn cursor_forward_and_back() {
        let sb = feed_text("ab\u{1b}[2C-\u{1b}[1D+");
        // ab, forward 2 (col 4, padded), '-', back 1, '+' -> overwrite '-'.
        assert_eq!(line_text(&sb, 0), "ab  +");
    }

    #[test]
    fn erase_display_clears_everything() {
        let sb = feed_text("one\ntwo\u{1b}[2Jthree");
        assert_eq!(sb.lines().len(), 1);
        assert_eq!(line_text(&sb, 0), "three");
        assert_eq!(sb.cursor(), (0, 5));
    }

    #[test]
    fn save_and_restore_cursor() {
        // Save at col 2, advance with CD, restore, overwrite the 'C'.
        let sb = feed_text("ab\u{1b}[sCD\u{1b}[uX");
        assert_eq!(line_text(&sb, 0), "abXD");
    }

    #[test]
    fn unknown_sequences_do_not_leak_text() {
        let sb = feed_text("a\u{1b}[?1049h\u{1b}[?25l\u{1b}[2;3Hb");
        assert_eq!(line_text(&sb, 0), "ab");
    }

    #[test]
    fn oversized_csi_is_dropped_safely() {
        let garbage = format!("a\u{1b}[{}mb", "0;".repeat(100));
        let sb = feed_text(&garbage);
        assert_eq!(line_text(&sb, 0), "ab");
    }

    // -- trim -------------------------------------------------------------------

    #[test]
    fn trim_oldest_drops_the_oldest_lines() {
        let mut lines: Vec<Line> = (0..15)
            .map(|i| {
                vec![Run {
                    text: i.to_string(),
                    color: None,
                    bold: false,
                }]
            })
            .collect();
        let dropped = trim_oldest(&mut lines, 10);
        assert_eq!(dropped, 5);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0][0].text, "5");
        assert_eq!(lines[9][0].text, "14");
    }

    #[test]
    fn trim_oldest_below_cap_is_a_noop() {
        let mut lines: Vec<Line> = vec![Vec::new(); 3];
        assert_eq!(trim_oldest(&mut lines, 10), 0);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn scrollback_trims_and_keeps_cursor_pinned() {
        let mut sb = Scrollback::new();
        sb.feed("\n".repeat(MAX_SCROLLBACK_LINES + 50).as_str());
        sb.trim_to_cap();
        assert_eq!(sb.lines().len(), MAX_SCROLLBACK_LINES);
        assert_eq!(sb.cursor().0, MAX_SCROLLBACK_LINES - 1);
        assert_eq!(sb.lines()[0].len(), 0, "oldest lines dropped");
    }

    // -- keyboard mapping ---------------------------------------------------------

    fn key_of(key: Key, modifiers: Modifiers) -> Option<String> {
        key_sequence(&key, modifiers)
    }

    #[test]
    fn printable_character_passes_through() {
        assert_eq!(
            key_of(Key::Character("a".into()), Modifiers::empty()),
            Some("a".into())
        );
        assert_eq!(
            key_of(Key::Character("A".into()), Modifiers::SHIFT),
            Some("A".into())
        );
        assert_eq!(
            key_of(Key::Named(Space), Modifiers::empty()),
            Some(" ".into())
        );
    }

    #[test]
    fn enter_backspace_tab_escape() {
        assert_eq!(
            key_of(Key::Named(Enter), Modifiers::empty()),
            Some("\r".into())
        );
        assert_eq!(
            key_of(Key::Named(Backspace), Modifiers::empty()),
            Some("\u{7f}".into())
        );
        assert_eq!(
            key_of(Key::Named(Tab), Modifiers::empty()),
            Some("\t".into())
        );
        assert_eq!(
            key_of(Key::Named(Tab), Modifiers::SHIFT),
            Some("\u{1b}[Z".into())
        );
        assert_eq!(
            key_of(Key::Named(Escape), Modifiers::empty()),
            Some("\u{1b}".into())
        );
    }

    #[test]
    fn control_characters() {
        assert_eq!(
            key_of(Key::Character("c".into()), Modifiers::CTRL),
            Some("\u{3}".into())
        );
        assert_eq!(
            key_of(Key::Character("C".into()), Modifiers::CTRL),
            Some("\u{3}".into())
        );
        assert_eq!(
            key_of(Key::Character("d".into()), Modifiers::CTRL),
            Some("\u{4}".into())
        );
        assert_eq!(
            key_of(Key::Character("[".into()), Modifiers::CTRL),
            Some("\u{1b}".into())
        );
        assert_eq!(
            key_of(Key::Character(" ".into()), Modifiers::CTRL),
            Some("\u{0}".into())
        );
        assert_eq!(key_of(Key::Character("1".into()), Modifiers::CTRL), None);
    }

    #[test]
    fn alt_prefixes_with_escape() {
        assert_eq!(
            key_of(Key::Character("b".into()), Modifiers::ALT),
            Some("\u{1b}b".into())
        );
        assert_eq!(
            key_of(Key::Character("b".into()), Modifiers::ALT | Modifiers::CTRL),
            Some("\u{1b}\u{2}".into())
        );
    }

    #[test]
    fn arrow_and_navigation_sequences() {
        assert_eq!(
            key_of(Key::Named(ArrowUp), Modifiers::empty()),
            Some("\u{1b}[A".into())
        );
        assert_eq!(
            key_of(Key::Named(ArrowDown), Modifiers::empty()),
            Some("\u{1b}[B".into())
        );
        assert_eq!(
            key_of(Key::Named(ArrowRight), Modifiers::empty()),
            Some("\u{1b}[C".into())
        );
        assert_eq!(
            key_of(Key::Named(ArrowLeft), Modifiers::empty()),
            Some("\u{1b}[D".into())
        );
        assert_eq!(
            key_of(Key::Named(ArrowUp), Modifiers::CTRL),
            Some("\u{1b}[1;5A".into())
        );
        assert_eq!(
            key_of(Key::Named(ArrowLeft), Modifiers::ALT | Modifiers::SHIFT),
            Some("\u{1b}[1;4D".into())
        );
        assert_eq!(
            key_of(Key::Named(Home), Modifiers::empty()),
            Some("\u{1b}[H".into())
        );
        assert_eq!(
            key_of(Key::Named(End), Modifiers::empty()),
            Some("\u{1b}[F".into())
        );
        assert_eq!(
            key_of(Key::Named(Delete), Modifiers::empty()),
            Some("\u{1b}[3~".into())
        );
        assert_eq!(
            key_of(Key::Named(PageUp), Modifiers::empty()),
            Some("\u{1b}[5~".into())
        );
        assert_eq!(
            key_of(Key::Named(PageDown), Modifiers::CTRL),
            Some("\u{1b}[6;5~".into())
        );
    }

    #[test]
    fn function_keys() {
        assert_eq!(
            key_of(Key::Named(F1), Modifiers::empty()),
            Some("\u{1b}OP".into())
        );
        assert_eq!(
            key_of(Key::Named(F5), Modifiers::empty()),
            Some("\u{1b}[15~".into())
        );
        assert_eq!(
            key_of(Key::Named(F12), Modifiers::empty()),
            Some("\u{1b}[24~".into())
        );
    }

    #[test]
    fn logo_combinations_are_window_shortcuts() {
        assert_eq!(key_of(Key::Character("k".into()), Modifiers::LOGO), None);
        assert_eq!(key_of(Key::Named(Enter), Modifiers::LOGO), None);
    }

    #[test]
    fn ime_composition_and_unidentified_are_ignored() {
        assert_eq!(
            key_of(Key::Character("ab".into()), Modifiers::empty()),
            None
        );
        assert_eq!(key_of(Key::Unidentified, Modifiers::empty()), None);
    }

    // -- paste ------------------------------------------------------------------

    #[test]
    fn paste_normalizes_newlines_to_cr() {
        assert_eq!(normalize_paste("a\nb\r\nc\rd"), "a\rb\rc\rd");
    }

    // -- update ------------------------------------------------------------------

    fn state_with_session() -> State {
        let mut state = State::new(Some(PathBuf::from("/repo")));
        update(&mut state, Msg::Opened("pty-1".into()));
        state
    }

    #[test]
    fn opened_resets_state_and_arms_follow() {
        let mut state = State {
            exit: Some("1".into()),
            ..State::default()
        };
        let action = update(&mut state, Msg::Opened("pty-2".into()));
        assert_eq!(state.session(), Some("pty-2"));
        assert_eq!(state.exit_code(), None);
        assert!(state.at_bottom());
        assert!(state.scrollback.is_empty());
        assert!(matches!(action, Some(Action::FollowScroll { .. })));
    }

    #[test]
    fn output_feeds_scrollback_and_arms_follow() {
        let mut state = state_with_session();
        let action = update(&mut state, Msg::Output("pty-1".into(), "hi".into()));
        assert_eq!(line_text(&state.scrollback, 0), "hi");
        assert!(matches!(action, Some(Action::FollowScroll { .. })));
    }

    #[test]
    fn output_for_other_sessions_is_ignored() {
        let mut state = state_with_session();
        assert!(update(&mut state, Msg::Output("pty-9".into(), "hi".into())).is_none());
        assert!(state.scrollback.is_empty());
        assert!(update(&mut state, Msg::Exit("pty-9".into(), Some("0".into()))).is_none());
        assert_eq!(state.exit_code(), None);
    }

    #[test]
    fn exit_records_code_and_blocks_input() {
        let mut state = state_with_session();
        update(&mut state, Msg::Exit("pty-1".into(), Some("127".into())));
        assert_eq!(state.exit_code(), Some("127"));
        assert!(state.is_exited());
        assert!(update(&mut state, Msg::Char('x')).is_none());
        assert!(update(&mut state, Msg::Key(Key::Named(Enter), Modifiers::empty())).is_none());
    }

    #[test]
    fn key_presses_write_xterm_sequences() {
        let mut state = state_with_session();
        let action = update(&mut state, Msg::Key(Key::Named(Enter), Modifiers::empty()));
        assert!(matches!(
            action,
            Some(Action::Write { data, .. }) if data == "\r"
        ));
        let action = update(
            &mut state,
            Msg::Key(Key::Character("c".into()), Modifiers::CTRL),
        );
        assert!(matches!(
            action,
            Some(Action::Write { data, .. }) if data == "\u{3}"
        ));
    }

    #[test]
    fn key_press_without_session_is_ignored() {
        let mut state = State::default();
        assert!(update(&mut state, Msg::Char('x')).is_none());
    }

    #[test]
    fn char_and_paste_forward_writes() {
        let mut state = state_with_session();
        assert!(matches!(
            update(&mut state, Msg::Char('x')),
            Some(Action::Write { data, .. }) if data == "x"
        ));
        assert!(matches!(
            update(&mut state, Msg::Paste("a\nb".into())),
            Some(Action::Write { data, .. }) if data == "a\rb"
        ));
    }

    #[test]
    fn resize_updates_dims_and_forwards() {
        let mut state = state_with_session();
        let action = update(
            &mut state,
            Msg::Resize {
                cols: 120,
                rows: 40,
            },
        );
        assert_eq!(state.dims(), (120, 40));
        assert!(matches!(
            action,
            Some(Action::Resize {
                cols: 120,
                rows: 40,
                ..
            })
        ));
        // Without a session the dims still update, nothing is forwarded.
        let mut idle = State::default();
        assert!(update(&mut idle, Msg::Resize { cols: 0, rows: 10 }).is_none());
        assert_eq!(idle.dims(), (1, 10), "dims clamp at 1");
    }

    #[test]
    fn scroll_releases_and_rearms_follow() {
        let mut state = State::new(None);
        update(&mut state, Msg::Opened("pty-1".into()));
        // 199 newlines -> 200 lines * 18px = 3600px of content.
        update(&mut state, Msg::Output("pty-1".into(), "\n".repeat(199)));
        // Scrolling up releases follow.
        let action = update(
            &mut state,
            Msg::Scroll {
                y: 0.0,
                viewport: 400.0,
            },
        );
        assert!(!state.at_bottom());
        assert!(action.is_none());
        // Scrolling back to the bottom re-arms follow (and re-pins).
        let action = update(
            &mut state,
            Msg::Scroll {
                y: 3600.0 - 400.0,
                viewport: 400.0,
            },
        );
        assert!(state.at_bottom());
        assert!(matches!(action, Some(Action::FollowScroll { .. })));
    }

    #[test]
    fn spawn_request_needs_root_and_no_session() {
        let mut state = State::new(None);
        assert!(update(&mut state, Msg::SpawnRequested).is_none());
        let mut state = State::new(Some(PathBuf::from("/repo")));
        let action = update(&mut state, Msg::SpawnRequested);
        assert!(matches!(
            action,
            Some(Action::Spawn { repo_root }) if repo_root == Path::new("/repo")
        ));
        assert!(update(&mut state_with_session(), Msg::SpawnRequested).is_none());
    }

    #[test]
    fn restart_only_after_exit_and_carries_both_halves() {
        let mut state = State::new(Some(PathBuf::from("/repo")));
        update(&mut state, Msg::Opened("pty-1".into()));
        assert!(update(&mut state, Msg::RestartPressed).is_none());
        update(&mut state, Msg::Exit("pty-1".into(), Some("0".into())));
        let action = update(&mut state, Msg::RestartPressed);
        assert!(matches!(
            action,
            Some(Action::Restart { old_session: Some(id), repo_root })
                if id == "pty-1" && repo_root == Path::new("/repo")
        ));
    }

    #[test]
    fn click_marks_focused() {
        let mut state = State::default();
        assert!(update(&mut state, Msg::Click).is_none());
        assert!(state.focused);
    }

    // -- visible window ------------------------------------------------------------

    #[test]
    fn window_follow_renders_the_newest_rows() {
        let win = visible_window(1000, 0.0, 360.0, true);
        // ceil(360/18)=20 visible + overscan, clamped to the tail.
        assert_eq!(win.first, 1000 - 40);
        assert_eq!(win.end, 1000);
        assert_eq!(win.top_pad, 960.0 * LINE_HEIGHT);
        assert_eq!(win.bottom_pad, 0.0);
    }

    #[test]
    fn window_follow_with_small_scrollback_fills_from_zero() {
        let win = visible_window(5, 0.0, 360.0, true);
        assert_eq!(win.first, 0);
        assert_eq!(win.end, 5);
        assert_eq!(win.top_pad, 0.0);
        assert_eq!(win.bottom_pad, 0.0);
    }

    #[test]
    fn window_history_windows_around_the_offset() {
        let win = visible_window(1000, 5000.0, 360.0, false);
        // floor(5000/18) = 277 -> minus overscan; ceil(5360/18) = 298 -> plus.
        assert_eq!(win.first, 257);
        assert_eq!(win.end, 318);
        assert_eq!(
            win.top_pad + (win.end - win.first) as f32 * LINE_HEIGHT + win.bottom_pad,
            1000.0 * LINE_HEIGHT,
            "rendered height must equal the virtual content height"
        );
    }

    #[test]
    fn window_history_with_unknown_viewport_renders_overscan() {
        // Before the first on_scroll event (viewport = 0) we still render a
        // useful band without panicking.
        let win = visible_window(1000, 0.0, 0.0, false);
        assert_eq!(win.first, 0);
        assert_eq!(win.end, OVERSCAN);
        let win = visible_window(10, 0.0, 0.0, false);
        assert_eq!(win.end, 10);
    }

    #[test]
    fn window_handles_stale_offsets_beyond_content() {
        // e.g. right after Msg::Opened reset the scrollback.
        let win = visible_window(3, 99_999.0, 360.0, false);
        assert_eq!(win.first, 3);
        assert_eq!(win.end, 3);
        assert_eq!(win.top_pad, 3.0 * LINE_HEIGHT);
        assert_eq!(win.bottom_pad, 0.0);
    }

    #[test]
    fn window_empty_scrollback_is_empty() {
        let win = visible_window(0, 0.0, 0.0, true);
        assert_eq!(
            (win.first, win.end, win.top_pad, win.bottom_pad),
            (0, 0, 0.0, 0.0)
        );
    }

    // -- subscription mapping ---------------------------------------------------------

    #[test]
    fn session_msg_filters_by_session_and_maps_events() {
        use crate::pty::{PtyExit, PtyOutput};

        let output = session_msg(
            "pty-1",
            BackendEvent::PtyOutput(PtyOutput {
                session_id: "pty-1".into(),
                data: "hi".into(),
            }),
        );
        assert!(matches!(output, Some(Msg::Output(id, data)) if id == "pty-1" && data == "hi"));

        let other = session_msg(
            "pty-1",
            BackendEvent::PtyOutput(PtyOutput {
                session_id: "pty-2".into(),
                data: "hi".into(),
            }),
        );
        assert!(other.is_none());

        let exit = session_msg(
            "pty-1",
            BackendEvent::PtyExit(PtyExit {
                session_id: "pty-1".into(),
                code: 1,
            }),
        );
        assert!(matches!(exit, Some(Msg::Exit(id, Some(code))) if id == "pty-1" && code == "1"));

        let unrelated = session_msg(
            "pty-1",
            BackendEvent::RepoChanged(crate::watcher::RepoChangedEvent {
                repo_id: crate::engine::types::RepoId("r".into()),
                paths: vec![],
                head_moved: false,
                full: false,
            }),
        );
        assert!(unrelated.is_none());
    }

    // -- palette ------------------------------------------------------------------------

    #[test]
    fn palette_bold_brightens_only_base_colors() {
        assert_eq!(palette_color(1, false), PALETTE_NORMAL[1]);
        assert_eq!(palette_color(1, true), PALETTE_BRIGHT[1]);
        assert_eq!(palette_color(9, true), PALETTE_BRIGHT[1]);
        assert_eq!(palette_color(9, false), PALETTE_BRIGHT[1]);
    }
}
