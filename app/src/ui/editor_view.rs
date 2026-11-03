//! Center-pane file editor over the mtime-guarded worktree writer
//! (`repo_write_file`) with conflict resolution and open-in-external-editor.
//!
//! # Save semantics (the mtime guard)
//!
//! `service::workspace::repo_write_file(services, repo_id, path, content,
//! expected_mtime)` refuses to write when the file's current mtime does not
//! match `expected_mtime` — `Some(stored)` must equal the on-disk mtime,
//! `Some(0)` means "create" (the file must be missing), and `None`
//! overwrites unconditionally (tooling escape hatch, used only by the
//! conflict Overwrite button here). The panel therefore keeps
//! `base_mtime: Option<u128>` — the mtype captured at load time in the
//! service's own unit (milliseconds since the epoch as `u128`; note this
//! panel deliberately mirrors the service signature instead of narrowing to
//! `u64`) — and sends it as `expected_mtime` on every save. A successful
//! save returns the fresh `FileMeta`, whose `mtime_ms` becomes the new
//! baseline, so consecutive saves keep guarding.
//!
//! # Load semantics (stat-then-read)
//!
//! `repo_read_file` returns raw bytes without any mtime, so the shell loads
//! a file as `repo_stat_file` FIRST (captures `FileMeta.mtime_ms`), then
//! `repo_read_file` (content), and feeds both via [`Msg::Opened`]. The
//! order is load-bearing: if an external write lands between the two calls,
//! the stale mtime + fresh content fail *closed* — the next save is refused
//! as a conflict instead of silently clobbering the external edit. The
//! shell should also check `FileMeta.kind` and refuse to feed non-text
//! files through (the content string is lossy UTF-8).
//!
//! # Conflict UX
//!
//! A save refused by the guard surfaces as an inline strip: "changed on
//! disk — Reload / Overwrite". Reload re-runs the load flow (fresh content
//! and mtime; the local edits are replaced, which the service error text
//! says the same); Overwrite re-saves the current buffer with
//! `expected_mtime = None`. Conflict detection classifies the backend error
//! string via [`is_mtime_conflict_error`] (the exact phrase comes from
//! `workspace::write_file_impl`, used identically by the local and remote
//! write paths).
//!
//! # Open in external editor
//!
//! The toolbar carries a pick list of apps from
//! `crate::editors::detect_apps()` (cheap, no subprocesses) fed in via
//! [`Msg::AppsLoaded`]; the Open button emits
//! [`Action::OpenExternal`] and the shell resolves it by re-detecting and
//! calling `crate::editors::open_path_with(&app, repo_root.join(path))` —
//! only machine-verified paths spawn (the same rule the
//! `crate::editors::open_with` service fn enforces for the repo root).
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod editor_view;` in `ui/mod.rs` (drop the temporary
//!    `#![allow(dead_code)]` at the top of this file in the same commit).
//! 2. Add `Editor(editor_view::Msg)` to `app::Message` and hold one
//!    `EditorView` per repo tab (or one global — it shows one file).
//! 3. Route messages: `Message::Editor(msg) => {
//!    let action = tab.editor.update(msg);
//!    Self::run_editor_action(repo_id, &mut tab.editor, action) }`.
//! 4. Execute actions:
//!    - `Action::Load { path }` / `Action::Reload { path }` (same handling):
//!      `repo_stat_file(&services, repo_id, path.clone())` →
//!      `repo_read_file(&services, repo_id, path.clone())` (stat FIRST —
//!      see the module docs) → feed `Msg::Opened { path, content:
//!      String::from_utf8_lossy(&bytes).into_owned(), mtime:
//!      meta.mtime_ms }`. Refuse `kind != Text` with an error instead.
//!    - `Action::Save { path, content, expected_mtime }`:
//!      `repo_write_file(&services, repo_id, path, content.into_bytes(),
//!      expected_mtime)` → feed `Msg::Saved(result)`.
//!    - `Action::OpenExternal { path, app_id }`:
//!      `crate::editors::detect_apps()` → find `app_id` →
//!      `crate::editors::open_path_with(&app, &repo_root.join(&path))`
//!      (resolve `path` against the repo root; surface any `Err(String)` as
//!      `Msg`-less — e.g. log it or reuse the panel's error via a shell-side
//!      variant).
//! 5. Feed `Msg::AppsLoaded(crate::editors::detect_apps())` once when the
//!    editor first opens (detection is a handful of `stat`s).
//! 6. Focus: the shell should blur the editor (`text_editor` keeps focus)
//!    when overlays open, like the commit message editor.

// Registered for gates before the shell wires this module in; drop this
// allow once `app.rs` routes `Msg`/`Action`.
#![allow(dead_code)]

use std::fmt;

use iced::widget::{button, column, container, pick_list, row, text, text_editor, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::editors::AppEntry;
use crate::workspace::FileMeta;

use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// External apps
// ---------------------------------------------------------------------------

/// One detected external app for the "Open in…" pick list. `AppEntry` does
/// not implement `PartialEq` (so it cannot be a pick-list option directly);
/// this mirror adds the equality the picker needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalApp {
    /// Stable app id (`vscode`, `zed`, `explorer`, …).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Machine-verified executable/bundle path (display only here).
    pub path: String,
}

impl ExternalApp {
    pub fn from_entry(entry: &AppEntry) -> Self {
        Self {
            id: entry.id.clone(),
            name: entry.name.clone(),
            path: entry.path.clone(),
        }
    }
}

impl fmt::Display for ExternalApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Whether a `repo_write_file` error is the mtime-guard refusal (as opposed
/// to an I/O failure). Matches `workspace::write_file_impl`'s message
/// "`<path>` changed on disk (external modification); reload before saving"
/// — the local and the remote SFTP write paths use the same wording, so a
/// phrase match is the classification the service layer supports today.
pub fn is_mtime_conflict_error(error: &str) -> bool {
    error.contains("changed on disk")
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Editor state: the loaded file, its load-time mtime baseline, dirty/
/// saving flags and the conflict strip.
#[derive(Debug)]
pub struct EditorView {
    pub open: bool,
    /// Workdir-relative path (forward slashes) of the open file.
    pub path: Option<String>,
    pub content: text_editor::Content,
    /// True while a load (`Opened` not yet fed) is in flight.
    pub loading: bool,
    /// Set by [`Msg::ContentChanged`], cleared by a successful save or a
    /// fresh `Opened`.
    pub dirty: bool,
    /// Load/save failure that is not a conflict, rendered destructive.
    pub error: Option<String>,
    /// True between `SavePressed`/`ConflictOverwrite` and the fed `Saved`.
    pub saving: bool,
    /// The file's mtime (ms since epoch, `u128` per `FileMeta`) captured at
    /// load time; sent as `expected_mtime` on saves. `Some(0)` marks a
    /// create. `None` only on a never-loaded panel.
    pub base_mtime: Option<u128>,
    /// The mtime-guard refusal text while a conflict strip is showing.
    pub conflict: Option<String>,

    /// Detected external apps (shell feeds `detect_apps()` once).
    pub apps: Vec<ExternalApp>,
    /// Pick-list selection for "Open in…".
    pub selected_app: Option<ExternalApp>,
}

impl Default for EditorView {
    fn default() -> Self {
        Self {
            open: false,
            path: None,
            content: text_editor::Content::new(),
            loading: false,
            dirty: false,
            error: None,
            saving: false,
            base_mtime: None,
            conflict: None,
            apps: Vec::new(),
            selected_app: None,
        }
    }
}

impl EditorView {
    /// Whether the Save button can fire: a file is loaded, was edited, and
    /// no load/save is in flight.
    pub fn can_save(&self) -> bool {
        self.path.is_some() && self.dirty && !self.loading && !self.saving
    }

    /// Drop the current file but keep the detected apps list (closing and
    /// reopening the editor should not re-probe the machine).
    fn reset_file(&mut self) {
        self.path = None;
        self.content = text_editor::Content::new();
        self.loading = false;
        self.dirty = false;
        self.error = None;
        self.saving = false;
        self.base_mtime = None;
        self.conflict = None;
    }
}

// ---------------------------------------------------------------------------
// Messages + actions
// ---------------------------------------------------------------------------

/// Editor messages: user input plus the results the shell feeds back.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Open `path` (workdir-relative) in the editor; emits
    /// [`Action::Load`].
    OpenRequested(String),
    /// Close the editor (drops the buffer; the shell may also want to
    /// reclaim the center pane).
    Close,
    /// Load result: content (lossy UTF-8 of the raw bytes) plus the mtime
    /// captured by the stat-before-read sequence.
    Opened {
        path: String,
        content: String,
        mtime: u128,
    },
    /// Keystrokes/edits in the text editor widget.
    ContentChanged(text_editor::Action),
    /// Save button (guarded, see [`EditorView::can_save`]).
    SavePressed,
    /// `repo_write_file` result.
    Saved(Result<FileMeta, String>),
    /// Conflict strip: re-load from disk (fresh content + mtime).
    ConflictReload,
    /// Conflict strip: overwrite the external change (`expected_mtime =
    /// None` on the re-save).
    ConflictOverwrite,

    // External editors.
    /// `crate::editors::detect_apps()` result (shell feeds once).
    AppsLoaded(Vec<AppEntry>),
    /// Pick-list selection changed.
    AppSelected(ExternalApp),
    /// "Open in…" button for the selected app.
    OpenExternalPressed,
}

/// Intents the shell executes (see the module docs for the exact
/// service mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `repo_stat_file` + `repo_read_file` → `Msg::Opened`.
    Load { path: String },
    /// Same service calls as [`Action::Load`]; distinct intent so the
    /// shell can tell a conflict reload from a fresh open (e.g. for
    /// logging). Fed by [`Msg::ConflictReload`].
    Reload { path: String },
    /// `repo_write_file(path, content, expected_mtime)` → `Msg::Saved`.
    Save {
        path: String,
        content: String,
        /// The load-time mtime, `Some(0)` for creates, `None` only from the
        /// conflict Overwrite path (unconditional overwrite).
        expected_mtime: Option<u128>,
    },
    /// Open the file in the external app with this id (the shell re-detects
    /// so only machine-verified paths spawn).
    OpenExternal { path: String, app_id: String },
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

impl EditorView {
    /// Pure state machine: apply `msg`, return the intent (if any) for the
    /// shell to execute. No service calls happen here.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::OpenRequested(path) => {
                self.open = true;
                self.reset_file();
                self.path = Some(path.clone());
                self.loading = true;
                Some(Action::Load { path })
            }
            Msg::Close => {
                self.open = false;
                self.reset_file();
                None
            }
            Msg::Opened {
                path,
                content,
                mtime,
            } => {
                // Stale load for a file the user has already navigated away
                // from (a second `OpenRequested` reset everything): drop it.
                if self.path.as_deref() != Some(path.as_str()) {
                    return None;
                }
                self.loading = false;
                self.content = text_editor::Content::with_text(&content);
                self.base_mtime = Some(mtime);
                self.dirty = false;
                self.error = None;
                self.conflict = None;
                None
            }
            Msg::ContentChanged(action) => {
                self.content.perform(action);
                self.dirty = true;
                None
            }
            Msg::SavePressed => {
                if !self.can_save() {
                    return None;
                }
                let path = self.path.clone().expect("can_save checked the path");
                self.saving = true;
                self.error = None;
                Some(Action::Save {
                    path,
                    content: self.content.text(),
                    expected_mtime: self.base_mtime,
                })
            }
            Msg::Saved(Ok(meta)) => {
                if self.path.as_deref() != Some(meta.path.as_str()) {
                    return None; // stale save result for another file
                }
                self.saving = false;
                self.dirty = false;
                self.base_mtime = Some(meta.mtime_ms);
                self.conflict = None;
                self.error = None;
                None
            }
            Msg::Saved(Err(error)) => {
                if !self.saving {
                    return None; // stale failure after a reset: drop
                }
                self.saving = false;
                if is_mtime_conflict_error(&error) {
                    self.conflict = Some(error);
                } else {
                    self.error = Some(error);
                }
                None
            }
            Msg::ConflictReload => {
                let path = self.path.clone()?;
                self.conflict = None;
                self.loading = true;
                Some(Action::Reload { path })
            }
            Msg::ConflictOverwrite => {
                let path = self.path.clone()?;
                self.conflict = None;
                self.saving = true;
                // The service overwrites unconditionally with `None` — the
                // deliberate conflict-resolution escape hatch.
                Some(Action::Save {
                    path,
                    content: self.content.text(),
                    expected_mtime: None,
                })
            }

            Msg::AppsLoaded(apps) => {
                self.apps = apps.iter().map(ExternalApp::from_entry).collect();
                // Keep a selection that still exists; otherwise default to
                // the platform file manager (always first).
                let still_there = self
                    .selected_app
                    .as_ref()
                    .is_some_and(|selected| self.apps.iter().any(|app| app == selected));
                if !still_there {
                    self.selected_app = self.apps.first().cloned();
                }
                None
            }
            Msg::AppSelected(app) => {
                self.selected_app = Some(app);
                None
            }
            Msg::OpenExternalPressed => {
                let path = self.path.clone()?;
                let app = self.selected_app.clone()?;
                Some(Action::OpenExternal {
                    path,
                    app_id: app.id,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

impl EditorView {
    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        if !self.open {
            return column![].into();
        }

        let Some(path) = self.path.as_deref() else {
            return column![].into();
        };

        let header = row![
            text(path).size(12).font(theme::mono()),
            text(if self.dirty { "•" } else { " " })
                .size(12)
                .color(tokens.modified),
            Space::new().width(Length::Fill),
            button(text("×").size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([2, 8])
                .on_press(Msg::Close),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill);

        let mut body = column![].spacing(6).width(Length::Fill);
        body = body.push(header);

        // Status / error line.
        if self.loading {
            body = body.push(text("loading…").size(10).color(tokens.muted_foreground));
        } else if self.saving {
            body = body.push(text("saving…").size(10).color(tokens.muted_foreground));
        } else if let Some(error) = &self.error {
            body = body.push(
                text(error.as_str())
                    .size(10)
                    .color(tokens.destructive)
                    .width(Length::Fill),
            );
        }

        // Conflict strip (mtime guard refused the save).
        if let Some(conflict) = &self.conflict {
            body = body.push(
                container(
                    column![
                        text(conflict.as_str()).size(10).width(Length::Fill),
                        row![
                            Space::new().width(Length::Fill),
                            button(text("Overwrite").size(9))
                                .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                                .padding([2, 8])
                                .on_press(Msg::ConflictOverwrite),
                            button(text("Reload").size(9))
                                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                                .padding([2, 8])
                                .on_press(Msg::ConflictReload),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                    ]
                    .spacing(4),
                )
                .padding([6, 8])
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
        }

        // Toolbar: save + open-in-external-editor.
        let mut toolbar = row![
            button(text("Save").size(10))
                .style(theme::button_style(
                    tokens,
                    if self.can_save() {
                        theme::ButtonKind::Primary
                    } else {
                        theme::ButtonKind::Secondary
                    }
                ))
                .padding([4, 12])
                .on_press_maybe(self.can_save().then_some(Msg::SavePressed)),
            Space::new().width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if !self.apps.is_empty() {
            toolbar = toolbar
                .push(
                    pick_list(
                        self.apps.clone(),
                        self.selected_app.clone(),
                        Msg::AppSelected,
                    )
                    .text_size(10)
                    .padding([3, 6])
                    .width(Length::Fixed(170.0)),
                )
                .push(
                    button(text("Open in…").size(10))
                        .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                        .padding([4, 10])
                        .on_press_maybe(
                            self.selected_app
                                .is_some()
                                .then_some(Msg::OpenExternalPressed),
                        ),
                );
        }
        body = body.push(toolbar);

        body = body.push(
            text_editor(&self.content)
                .placeholder("Empty file")
                .on_action(Msg::ContentChanged)
                .size(12)
                .padding(8)
                .font(theme::mono())
                .height(Length::Fill),
        );

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([8, 10])
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn open_panel(path: &str) -> (EditorView, Action) {
        let mut editor = EditorView::default();
        let action = editor.update(Msg::OpenRequested(path.to_string()));
        (editor, action.expect("open emits Load"))
    }

    fn feed_opened(editor: &mut EditorView, path: &str, content: &str, mtime: u128) {
        let action = editor.update(Msg::Opened {
            path: path.to_string(),
            content: content.to_string(),
            mtime,
        });
        assert!(action.is_none(), "Opened must not emit an intent");
    }

    /// Type `text` at the end of the buffer one char at a time (the iced
    /// 0.14 edit shape: `Action::Edit(Edit::Insert(char))`; note
    /// `Content::with_text` leaves the cursor at position 0, so seek first).
    fn type_text(editor: &mut EditorView, text: &str) {
        editor.update(Msg::ContentChanged(text_editor::Action::Move(
            text_editor::Motion::DocumentEnd,
        )));
        for ch in text.chars() {
            editor.update(Msg::ContentChanged(text_editor::Action::Edit(
                text_editor::Edit::Insert(ch),
            )));
        }
    }

    // -- open / close ------------------------------------------------------------

    #[test]
    fn open_requested_starts_a_load() {
        let (editor, action) = open_panel("src/main.rs");
        assert!(editor.open);
        assert_eq!(editor.path.as_deref(), Some("src/main.rs"));
        assert!(editor.loading);
        assert!(!editor.dirty);
        assert_eq!(
            action,
            Action::Load {
                path: "src/main.rs".to_string(),
            }
        );
    }

    #[test]
    fn opened_replaces_content_and_resets_flags() {
        let (mut editor, _) = open_panel("README.md");
        type_text(&mut editor, "stale");
        feed_opened(&mut editor, "README.md", "hello\nworld", 1_723_456_789_012);
        assert!(!editor.loading);
        assert!(!editor.dirty);
        assert_eq!(editor.content.text(), "hello\nworld");
        assert_eq!(editor.base_mtime, Some(1_723_456_789_012));
        assert!(editor.error.is_none());
        assert!(editor.conflict.is_none());
    }

    #[test]
    fn opened_for_a_stale_path_is_dropped() {
        let (mut editor, _) = open_panel("a.txt");
        // A second open request resets the state and starts a new load.
        editor.update(Msg::OpenRequested("b.txt".into()));
        editor.update(Msg::Opened {
            path: "a.txt".into(),
            content: "old".into(),
            mtime: 1,
        });
        assert!(editor.loading, "stale Opened must not clear the new load");
        assert_eq!(editor.content.text(), "", "stale content dropped");
    }

    #[test]
    fn close_resets_the_file_but_keeps_detected_apps() {
        let (mut editor, _) = open_panel("a.txt");
        editor.update(Msg::AppsLoaded(vec![AppEntry {
            id: "zed".into(),
            name: "Zed".into(),
            path: "/usr/bin/zed".into(),
        }]));
        feed_opened(&mut editor, "a.txt", "text", 42);
        type_text(&mut editor, "x");
        editor.update(Msg::Close);
        assert!(!editor.open);
        assert!(editor.path.is_none());
        assert!(editor.base_mtime.is_none());
        assert!(!editor.dirty);
        assert!(!editor.loading);
        assert_eq!(editor.apps.len(), 1, "detection survives close");
        assert_eq!(editor.selected_app.as_ref().expect("selected").id, "zed");
    }

    // -- dirty tracking -------------------------------------------------------------

    #[test]
    fn content_changed_marks_dirty() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "hello", 7);
        assert!(!editor.dirty);
        type_text(&mut editor, " world");
        assert!(editor.dirty);
        assert_eq!(editor.content.text(), "hello world");
    }

    #[test]
    fn a_successful_save_clears_dirty_and_advances_the_mtime_baseline() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "v1", 100);
        type_text(&mut editor, "v2");

        assert!(editor.can_save());
        assert_eq!(
            editor.update(Msg::SavePressed),
            Some(Action::Save {
                path: "a.txt".to_string(),
                content: "v1v2".to_string(),
                expected_mtime: Some(100),
            })
        );
        assert!(editor.saving);

        editor.update(Msg::Saved(Ok(FileMeta {
            path: "a.txt".into(),
            size: 4,
            kind: crate::workspace::FileContentKind::Text,
            readonly: false,
            mtime_ms: 200,
        })));
        assert!(!editor.saving);
        assert!(!editor.dirty);
        assert_eq!(editor.base_mtime, Some(200), "next save guards on 200");
        assert!(editor.conflict.is_none());
    }

    #[test]
    fn save_pressed_is_gated_on_dirty_and_in_flight_flags() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "text", 5);
        assert_eq!(editor.update(Msg::SavePressed), None, "not dirty");

        type_text(&mut editor, "!");
        assert!(editor.update(Msg::SavePressed).is_some());

        // Saving locks a second press until the result arrives.
        assert_eq!(editor.update(Msg::SavePressed), None);
    }

    #[test]
    fn stale_save_results_are_dropped() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "text", 5);
        type_text(&mut editor, "!");
        assert!(editor.update(Msg::SavePressed).is_some());

        // The user switched files while the save was in flight.
        editor.update(Msg::OpenRequested("b.txt".into()));
        editor.update(Msg::Saved(Ok(FileMeta {
            path: "a.txt".into(),
            size: 4,
            kind: crate::workspace::FileContentKind::Text,
            readonly: false,
            mtime_ms: 9,
        })));
        assert!(editor.loading, "stale Ok must not touch the new load");
    }

    // -- conflict handling --------------------------------------------------------------

    #[test]
    fn conflict_error_opens_the_strip_and_other_errors_do_not() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "text", 5);
        type_text(&mut editor, "!");
        editor.update(Msg::SavePressed);

        editor.update(Msg::Saved(Err(
            "`a.txt` changed on disk (external modification); reload before saving".into(),
        )));
        assert!(!editor.saving);
        assert!(editor.conflict.is_some());
        assert!(editor.error.is_none());

        // A plain I/O failure keeps the strip closed and parks the error.
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "text", 5);
        type_text(&mut editor, "!");
        editor.update(Msg::SavePressed);
        editor.update(Msg::Saved(Err("saving `a.txt`: permission denied".into())));
        assert!(editor.conflict.is_none());
        assert_eq!(
            editor.error.as_deref(),
            Some("saving `a.txt`: permission denied")
        );
    }

    #[test]
    fn conflict_overwrite_resaves_without_the_guard() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "text", 5);
        type_text(&mut editor, "!");
        editor.update(Msg::SavePressed);
        editor.update(Msg::Saved(Err(
            "`a.txt` changed on disk (external modification); reload before saving".into(),
        )));

        assert_eq!(
            editor.update(Msg::ConflictOverwrite),
            Some(Action::Save {
                path: "a.txt".to_string(),
                content: "text!".to_string(),
                expected_mtime: None,
            })
        );
        assert!(editor.saving);
        assert!(editor.conflict.is_none(), "strip closed while re-saving");

        // The overwrite result advances the baseline like any save.
        editor.update(Msg::Saved(Ok(FileMeta {
            path: "a.txt".into(),
            size: 5,
            kind: crate::workspace::FileContentKind::Text,
            readonly: false,
            mtime_ms: 300,
        })));
        assert!(!editor.saving);
        assert!(!editor.dirty);
        assert_eq!(editor.base_mtime, Some(300));
    }

    #[test]
    fn conflict_reload_refetches_the_file() {
        let (mut editor, _) = open_panel("a.txt");
        feed_opened(&mut editor, "a.txt", "mine", 5);
        type_text(&mut editor, "!");
        editor.update(Msg::SavePressed);
        editor.update(Msg::Saved(Err(
            "`a.txt` changed on disk (external modification); reload before saving".into(),
        )));

        assert_eq!(
            editor.update(Msg::ConflictReload),
            Some(Action::Reload {
                path: "a.txt".to_string(),
            })
        );
        assert!(editor.loading);
        assert!(editor.conflict.is_none());

        // The reload result replaces the buffer (local edits are gone).
        feed_opened(&mut editor, "a.txt", "theirs", 6);
        assert_eq!(editor.content.text(), "theirs");
        assert!(!editor.dirty);
        assert_eq!(editor.base_mtime, Some(6));
    }

    #[test]
    fn conflict_actions_do_nothing_without_an_open_file() {
        let mut editor = EditorView::default();
        assert_eq!(editor.update(Msg::ConflictReload), None);
        assert_eq!(editor.update(Msg::ConflictOverwrite), None);
    }

    // -- error classification ---------------------------------------------------------

    #[test]
    fn mtime_conflict_phrase_is_recognized() {
        assert!(is_mtime_conflict_error(
            "`x.txt` changed on disk (external modification); reload before saving"
        ));
        assert!(!is_mtime_conflict_error(
            "reading `x.txt` failed: no such file"
        ));
        assert!(!is_mtime_conflict_error(""));
    }

    // -- external apps ------------------------------------------------------------------

    #[test]
    fn apps_loaded_defaults_to_the_first_app_and_keeps_valid_selections() {
        let mut editor = EditorView::default();
        editor.update(Msg::AppsLoaded(vec![
            AppEntry {
                id: "explorer".into(),
                name: "Files".into(),
                path: "xdg-open".into(),
            },
            AppEntry {
                id: "zed".into(),
                name: "Zed".into(),
                path: "/usr/bin/zed".into(),
            },
        ]));
        assert_eq!(
            editor.selected_app.as_ref().expect("default").id,
            "explorer"
        );

        editor.update(Msg::AppSelected(ExternalApp {
            id: "zed".into(),
            name: "Zed".into(),
            path: "/usr/bin/zed".into(),
        }));
        // Re-feed with the same set: the selection survives.
        editor.update(Msg::AppsLoaded(vec![
            AppEntry {
                id: "explorer".into(),
                name: "Files".into(),
                path: "xdg-open".into(),
            },
            AppEntry {
                id: "zed".into(),
                name: "Zed".into(),
                path: "/usr/bin/zed".into(),
            },
        ]));
        assert_eq!(editor.selected_app.as_ref().expect("kept").id, "zed");
    }

    #[test]
    fn apps_loaded_resets_a_selection_that_disappeared() {
        let mut editor = EditorView::default();
        editor.update(Msg::AppsLoaded(vec![AppEntry {
            id: "zed".into(),
            name: "Zed".into(),
            path: "/usr/bin/zed".into(),
        }]));
        editor.update(Msg::AppsLoaded(vec![AppEntry {
            id: "explorer".into(),
            name: "Files".into(),
            path: "xdg-open".into(),
        }]));
        assert_eq!(editor.selected_app.as_ref().expect("reset").id, "explorer");
    }

    #[test]
    fn open_external_emits_the_selected_app() {
        let (mut editor, _) = open_panel("a.txt");
        assert_eq!(editor.update(Msg::OpenExternalPressed), None, "no apps yet");

        editor.update(Msg::AppsLoaded(vec![AppEntry {
            id: "vscode".into(),
            name: "VS Code".into(),
            path: "/usr/bin/code".into(),
        }]));
        assert_eq!(
            editor.update(Msg::OpenExternalPressed),
            Some(Action::OpenExternal {
                path: "a.txt".to_string(),
                app_id: "vscode".to_string(),
            })
        );
    }
}
