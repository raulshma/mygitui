//! Health & maintenance panel: repo size/object census, maintenance ops,
//! archive export, sparse checkout and git-lfs — one dropdown surface over
//! `crate::service::maintenance` (M11/M12).
//!
//! Self-contained lane module (like [`super::worktree_panel`]): it owns its
//! state, its [`Msg`] enum, and returns [`Action`] intents from [`update`]
//! for the shell to translate into `service::maintenance` calls. Nothing
//! here touches services or `rfd` — [`Action::Archive`] only emits the
//! intent; the shell opens the native save dialog for the destination.
//!
//! Destructive or long ops run through one panel-internal confirm strip
//! (`PendingConfirm`, like the stash drop flow): the button parks the op,
//! the strip's Confirm turns it into the [`Action`], Cancel (or any load
//! result) clears it. While an op runs (`busy: Some`) every mutating button
//! is disabled and further requests are dropped, so ops never overlap.
//!
//! # Wiring (orchestrator checklist — none of this exists yet)
//!
//! 1. Register `mod health_panel;` in `ui/mod.rs` (drop the temporary
//!    `#![allow(dead_code)]` at the top of this file in the same commit).
//! 2. Add `Health(health_panel::Msg)` to `app::Message` and hold one
//!    `health_panel::HealthPanel` per tab (`tab.health: HealthPanel`).
//! 3. Route messages: `Message::Health(msg) => {
//!    let action = tab.health.update(msg);
//!    Self::run_health_action(repo_id, &mut tab.health, action) }`.
//! 4. Execute actions via `crate::service::maintenance`:
//!    - `Action::Refresh`: `repo_health` → `Msg::HealthLoaded`,
//!      `sparse_info` → `Msg::SparseLoaded`, `lfs_status` →
//!      `Msg::LfsLoaded` (all three; reads are fs-only and silent).
//!    - `Action::RunOp { op }`: `maintenance_run(repo_id, op)` →
//!      `Msg::OpDone(result)`. `op` is one of the `OP_*` constants:
//!      `gc`, `prune`, `commit_graph`, `pack_refs`, `count_objects`, `fsck`.
//!    - `Action::Archive { spec, format }`: open an rfd save dialog for the
//!      destination, then `archive(repo_id, spec, format, destination)` →
//!      `Msg::ArchiveDone(result)`. A cancelled dialog should come back as
//!      `Msg::ArchiveDone(Err(..))` so the panel unlocks.
//!    - `Action::SparseApply { patterns, add }`: `sparse_apply(repo_id,
//!      patterns, add)` → `Msg::SparseDone(result)` (empty patterns =
//!      disable sparse-checkout, back to full checkout).
//!    - `Action::LfsRun { subcommand }`: `lfs_run(repo_id, subcommand)` →
//!      `Msg::LfsDone(result)` (`pull` / `push` / `install` / `fetch`).
//! 5. View: in the repo overlay stack, like the forge panel —
//!    `container(container(tab.health.view(tokens).map(Message::Health)))
//!    .width(Length::Fixed(520.0))`.
//!
//! # Progress events the shell should surface
//!
//! The mutating calls (`maintenance_run`, `archive`, `sparse_apply`,
//! `lfs_run`) are op-queue mutations, so the footer already receives
//! `BackendEvent::OpProgress` for them with `kind` `"maintenance"`,
//! `"archive"`, `"sparse"` and `"lfs"`. Each of these ops is a single `git`
//! invocation that never calls `report()`, so there is no throttled
//! mid-run percentage: the only event per op is the final `done: true`
//! one (`pct: 100.0`, or `error: Some(..)` on failure, message
//! `"cancelled"` when skipped). No `OpLog` lines are produced. The reads
//! (`repo_health`, `sparse_info`, `lfs_status`) emit no events at all —
//! locally they run off-queue via `spawn_blocking`, remotely through
//! `enqueue_silent`.

use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_editor, text_input,
    Column, Space,
};
use iced::{Alignment, Background, Border, Element, Length};

use crate::engine::types::{LfsStatus, RepoHealth, SparseInfo};

use super::history_model::relative_age;
use super::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// Service op names (must match the strings `maintenance_run` / `lfs_run`
// dispatch on — see `service::maintenance.rs`)
// ---------------------------------------------------------------------------

/// `git gc` — repack and prune.
pub const OP_GC: &str = "gc";
/// `git prune` — drop unreachable loose objects.
pub const OP_PRUNE: &str = "prune";
/// `git commit-graph write` — revwalk acceleration.
pub const OP_COMMIT_GRAPH: &str = "commit_graph";
/// `git pack-refs --all` — consolidate loose refs.
pub const OP_PACK_REFS: &str = "pack_refs";
/// `git count-objects -v` — exact census; also seeds `RepoHealth
/// .packed_objects`. Read-only, runs without confirm.
pub const OP_COUNT_OBJECTS: &str = "count_objects";
/// `git fsck --dangling` — dangling census; seeds `RepoHealth
/// .fsck_dangling/.fsck_samples`. Walks every object (slow) → confirm.
pub const OP_FSCK: &str = "fsck";

/// Every op button the maintenance section renders, in display order.
pub const MAINTENANCE_OPS: [&str; 6] = [
    OP_GC,
    OP_PRUNE,
    OP_COMMIT_GRAPH,
    OP_PACK_REFS,
    OP_COUNT_OBJECTS,
    OP_FSCK,
];

/// Confirm-strip tag for a sparse apply with `set` semantics (replace).
pub const OP_SPARSE_SET: &str = "sparse_set";
/// Confirm-strip tag for a sparse apply with `add` semantics.
pub const OP_SPARSE_ADD: &str = "sparse_add";
/// Prefix for LFS confirm-strip tags: `lfs:<subcommand>`.
pub const LFS_OP_PREFIX: &str = "lfs:";

/// Button label for a maintenance op (also the busy label prefix).
pub fn op_label(op: &str) -> &'static str {
    match op {
        OP_GC => "Run gc",
        OP_PRUNE => "Prune loose",
        OP_COMMIT_GRAPH => "Commit-graph",
        OP_PACK_REFS => "Pack refs",
        OP_COUNT_OBJECTS => "Count objects",
        OP_FSCK => "Fsck census",
        _ => "Run op",
    }
}

/// One-line description used for the confirm strip of a maintenance op.
pub fn op_description(op: &str) -> &'static str {
    match op {
        OP_GC => "git gc — repack and prune the object store (may take a while)",
        OP_PRUNE => "git prune — delete unreachable loose objects",
        OP_COMMIT_GRAPH => "git commit-graph write — rewrite the commit-graph file",
        OP_PACK_REFS => "git pack-refs --all — consolidate loose refs into packed-refs",
        OP_COUNT_OBJECTS => "git count-objects -v — measure the exact object census",
        OP_FSCK => "git fsck — walk every object to count dangling ones (slow)",
        _ => "run the maintenance op",
    }
}

/// Whether the op goes through the confirm strip: `count_objects` is a
/// fast read-only census and runs directly, everything else mutates the
/// repository or walks every object (`fsck`).
pub fn op_needs_confirm(op: &str) -> bool {
    op != OP_COUNT_OBJECTS
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// An op parked on the panel-internal confirm strip. `op` is the service
/// identifier (maintenance `OP_*` name, `OP_SPARSE_*` tag or
/// `lfs:<subcommand>`); `label` is the human wording shown in the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConfirm {
    pub op: String,
    pub label: String,
}

/// Archive container format (`service::maintenance::archive` accepts
/// `zip` | `tar` | `tar.gz`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArchiveFormat {
    #[default]
    Zip,
    Tar,
    TarGz,
}

impl ArchiveFormat {
    /// Every option for the picker, in display order.
    pub const ALL: [ArchiveFormat; 3] =
        [ArchiveFormat::Zip, ArchiveFormat::Tar, ArchiveFormat::TarGz];

    /// Value passed straight to `archive`'s `format` argument.
    pub fn as_str(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
        }
    }
}

impl std::fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Load state and form buffers for the health/maintenance panel.
#[derive(Debug)]
pub struct HealthPanel {
    pub open: bool,
    /// True while the three status reads (health/sparse/LFS) are in flight.
    pub loading: bool,
    pub health: Option<RepoHealth>,
    pub sparse: Option<SparseInfo>,
    pub lfs: Option<LfsStatus>,
    /// Load or op failure, rendered in the destructive colour.
    pub error: Option<String>,
    /// Human label of the op currently running; locks every mutating
    /// button until the matching `*Done` message arrives.
    pub busy: Option<String>,
    /// One-line summary of the last successful op.
    pub last_result: Option<String>,
    /// Op parked on the inline confirm strip.
    pub confirm: Option<PendingConfirm>,

    // Archive form.
    pub archive_ref: String,
    pub archive_format: ArchiveFormat,

    // Sparse form (multi-line pattern editor, one directory per line).
    pub sparse_body: text_editor::Content,
    /// `false` = `set` (replace the pattern list), `true` = `add`.
    pub sparse_add: bool,
}

impl Default for HealthPanel {
    fn default() -> Self {
        Self {
            open: false,
            loading: false,
            health: None,
            sparse: None,
            lfs: None,
            error: None,
            busy: None,
            last_result: None,
            confirm: None,
            archive_ref: "HEAD".to_string(),
            archive_format: ArchiveFormat::default(),
            sparse_body: text_editor::Content::new(),
            sparse_add: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Messages + actions
// ---------------------------------------------------------------------------

/// Panel messages: user input, shell-delivered service results.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Show/hide the panel (first open triggers a full load).
    ToggleOpen,
    /// The header ⟳ button: reload health, sparse and LFS state.
    RefreshPressed,
    /// `repo_health` result.
    HealthLoaded(Result<RepoHealth, String>),
    /// `sparse_info` result (Ok also pre-fills the pattern editor when empty).
    SparseLoaded(Result<SparseInfo, String>),
    /// `lfs_status` result.
    LfsLoaded(Result<LfsStatus, String>),

    // Maintenance ops (`op` is an `OP_*` constant).
    OpRequested(String),
    OpConfirmed,
    OpCanceled,

    // Archive form.
    ArchiveRefChanged(String),
    ArchiveFormatChanged(ArchiveFormat),
    ArchivePressed,
    /// `archive` result (destination picked in the shell's save dialog —
    /// a cancelled dialog arrives as `Err` and unlocks the panel).
    ArchiveDone(Result<(), String>),

    // Sparse form.
    /// Programmatic whole-text set (shell prefill); keystrokes flow
    /// through [`Msg::SparseEdited`].
    #[allow(dead_code)]
    SparseTextChanged(String),
    /// Keystrokes in the multi-line pattern editor.
    SparseEdited(text_editor::Action),
    SparseAddToggled(bool),
    SparsePressed,
    SparseConfirmed,

    // LFS ops (`sub` is `pull`/`push`/`install`/`fetch`).
    LfsOpRequested(String),
    LfsOpConfirmed,

    /// Shared cancel for the sparse/LFS confirm strips.
    ConfirmCanceled,

    /// `maintenance_run` result.
    OpDone(Result<String, String>),
    /// `sparse_apply` result.
    SparseDone(Result<(), String>),
    /// `lfs_run` result.
    LfsDone(Result<String, String>),
}

/// Intents the shell executes against the service layer (see the module
/// docs for the exact service mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Reload all three status reads (health, sparse, LFS).
    Refresh,
    /// Run one maintenance op via `maintenance_run` (`op` = `OP_*` name).
    RunOp { op: String, label: String },
    /// Export `spec` as `format`; the shell picks the destination with the
    /// native save dialog and then calls `service::maintenance::archive`.
    Archive { spec: String, format: String },
    /// Apply sparse-checkout patterns (empty = back to full checkout).
    SparseApply { patterns: Vec<String>, add: bool },
    /// `git lfs <subcommand>` (`pull`/`push`/`install`/`fetch`).
    LfsRun { subcommand: String },
}

// ---------------------------------------------------------------------------
// Pure decision helpers (unit-tested; `update` has no other inputs)
// ---------------------------------------------------------------------------

/// Split the multi-line pattern editor contents into the `Vec<String>` the
/// service takes: one pattern per line, trimmed, empty lines dropped,
/// duplicates removed (first occurrence wins — `sparse-checkout` tolerates
/// repeats, but a unique list keeps the confirm label honest).
pub fn parse_patterns(text: &str) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || patterns.iter().any(|known| known == line) {
            continue;
        }
        patterns.push(line.to_owned());
    }
    patterns
}

/// Mirror of `service::maintenance::archive_arg_ok` (keep in sync): the ref
/// must be non-empty, must not look like an option, and must be free of
/// shell metacharacters and control characters. The service re-validates
/// regardless; mirroring the rules here lets the Export button disable
/// early instead of failing on the backend.
pub fn archive_arg_ok(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.chars().any(|c| {
            matches!(
                c,
                ';' | '&'
                    | '|'
                    | '$'
                    | '`'
                    | '('
                    | ')'
                    | '<'
                    | '>'
                    | '"'
                    | '\''
                    | '\n'
                    | '\r'
                    | '\0'
            )
        })
}

/// Whether the archive form can be submitted: not busy and the trimmed ref
/// passes [`archive_arg_ok`].
pub fn can_archive(panel: &HealthPanel) -> bool {
    panel.busy.is_none() && archive_arg_ok(panel.archive_ref.trim())
}

/// Human-readable byte size (`512 B`, `2.0 KiB`, `5.0 MiB`, `1.3 GiB`).
pub fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * KIB;
    const GIB: f64 = MIB * KIB;
    let bytes = bytes as f64;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{:.1} KiB", bytes / KIB)
    } else if bytes < GIB {
        format!("{:.1} MiB", bytes / MIB)
    } else {
        format!("{:.1} GiB", bytes / GIB)
    }
}

/// Which section owns the pending confirm (drives which messages the
/// confirm strip sends).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmSection {
    Maintenance,
    Sparse,
    Lfs,
}

impl HealthPanel {
    fn confirm_section(&self) -> Option<ConfirmSection> {
        let op = self.confirm.as_ref()?.op.as_str();
        Some(match op {
            OP_SPARSE_SET | OP_SPARSE_ADD => ConfirmSection::Sparse,
            op if op.starts_with(LFS_OP_PREFIX) => ConfirmSection::Lfs,
            _ => ConfirmSection::Maintenance,
        })
    }

    /// The whole pattern editor as one string (for parsing at confirm time).
    fn sparse_text(&self) -> String {
        self.sparse_body.text()
    }
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

impl HealthPanel {
    /// Pure state machine: apply `msg`, return the intent (if any) for the
    /// shell to execute. No service calls happen here.
    pub fn update(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::ToggleOpen => {
                self.open = !self.open;
                // First open (or a panel that never loaded) kicks off a
                // full load; a closed panel clears any pending confirm.
                if self.open {
                    if self.health.is_none() && self.error.is_none() && !self.loading {
                        self.loading = true;
                        return Some(Action::Refresh);
                    }
                } else {
                    self.confirm = None;
                }
                None
            }
            Msg::RefreshPressed => {
                self.loading = true;
                self.error = None;
                Some(Action::Refresh)
            }
            Msg::HealthLoaded(result) => {
                self.loading = false;
                match result {
                    Ok(health) => {
                        self.error = None;
                        self.health = Some(health);
                    }
                    Err(error) => {
                        self.health = None;
                        self.error = Some(error);
                    }
                }
                None
            }
            Msg::SparseLoaded(result) => {
                self.loading = false;
                match result {
                    Ok(info) => {
                        self.error = None;
                        self.sparse = Some(info);
                        // Pre-fill the editor with the current patterns so
                        // `set` edits them naturally; never clobber typing.
                        if self.sparse_text().trim().is_empty() {
                            let patterns = self
                                .sparse
                                .as_ref()
                                .map(|info| info.patterns.join("\n"))
                                .unwrap_or_default();
                            self.sparse_body = text_editor::Content::with_text(&patterns);
                        }
                    }
                    Err(error) => {
                        self.sparse = None;
                        self.error = Some(error);
                    }
                }
                None
            }
            Msg::LfsLoaded(result) => {
                self.loading = false;
                match result {
                    Ok(status) => {
                        self.error = None;
                        self.lfs = Some(status);
                    }
                    Err(error) => {
                        self.lfs = None;
                        self.error = Some(error);
                    }
                }
                None
            }

            Msg::OpRequested(op) => {
                if self.busy.is_some() {
                    return None;
                }
                if op_needs_confirm(&op) {
                    self.confirm = Some(PendingConfirm {
                        label: op_description(&op).to_string(),
                        op,
                    });
                    return None;
                }
                // Read-only census: run directly.
                let label = op_label(&op).to_string();
                self.busy = Some(label.clone());
                Some(Action::RunOp { op, label })
            }
            Msg::OpConfirmed => {
                if self.busy.is_some() {
                    self.confirm = None;
                    return None;
                }
                let confirm = self.confirm.take()?;
                self.busy = Some(op_label(&confirm.op).to_string());
                Some(Action::RunOp {
                    op: confirm.op,
                    label: confirm.label,
                })
            }
            Msg::OpCanceled => {
                self.confirm = None;
                None
            }

            Msg::ArchiveRefChanged(spec) => {
                self.archive_ref = spec;
                None
            }
            Msg::ArchiveFormatChanged(format) => {
                self.archive_format = format;
                None
            }
            Msg::ArchivePressed => {
                let spec = self.archive_ref.trim().to_string();
                if !can_archive(self) {
                    return None;
                }
                self.busy = Some(format!("Exporting {} ({})…", spec, self.archive_format));
                Some(Action::Archive {
                    spec,
                    format: self.archive_format.as_str().to_string(),
                })
            }
            Msg::ArchiveDone(result) => {
                self.busy = None;
                match result {
                    Ok(()) => self.last_result = Some("archive exported".to_string()),
                    Err(error) => self.error = Some(error),
                }
                // The archive is written outside the repository — nothing
                // in the panel's status reads changes, so no Refresh.
                None
            }

            Msg::SparseTextChanged(text) => {
                self.sparse_body = text_editor::Content::with_text(&text);
                None
            }
            Msg::SparseEdited(action) => {
                self.sparse_body.perform(action);
                None
            }
            Msg::SparseAddToggled(add) => {
                self.sparse_add = add;
                None
            }
            Msg::SparsePressed => {
                if self.busy.is_some() {
                    return None;
                }
                let op = if self.sparse_add {
                    OP_SPARSE_ADD
                } else {
                    OP_SPARSE_SET
                };
                let label = if parse_patterns(&self.sparse_text()).is_empty() {
                    "sparse-checkout disable — back to a full checkout".to_string()
                } else if self.sparse_add {
                    "sparse-checkout add — extend the pattern list and update the worktree"
                        .to_string()
                } else {
                    "sparse-checkout set — replace the pattern list and update the worktree"
                        .to_string()
                };
                self.confirm = Some(PendingConfirm {
                    op: op.to_string(),
                    label,
                });
                None
            }
            Msg::SparseConfirmed => {
                if self.busy.is_some() {
                    self.confirm = None;
                    return None;
                }
                let confirm = self.confirm.take()?;
                if confirm.op != OP_SPARSE_SET && confirm.op != OP_SPARSE_ADD {
                    return None;
                }
                let add = confirm.op == OP_SPARSE_ADD;
                let patterns = parse_patterns(&self.sparse_text());
                self.busy = Some(if add {
                    "Applying sparse-checkout (add)…".to_string()
                } else {
                    "Applying sparse-checkout…".to_string()
                });
                Some(Action::SparseApply { patterns, add })
            }

            Msg::LfsOpRequested(sub) => {
                if self.busy.is_some() {
                    return None;
                }
                self.confirm = Some(PendingConfirm {
                    op: format!("{LFS_OP_PREFIX}{sub}"),
                    label: format!("git lfs {sub}"),
                });
                None
            }
            Msg::LfsOpConfirmed => {
                if self.busy.is_some() {
                    self.confirm = None;
                    return None;
                }
                let confirm = self.confirm.take()?;
                let subcommand = confirm.op.strip_prefix(LFS_OP_PREFIX)?.to_string();
                self.busy = Some(format!("Running git lfs {subcommand}…"));
                Some(Action::LfsRun { subcommand })
            }
            Msg::ConfirmCanceled => {
                self.confirm = None;
                None
            }

            Msg::OpDone(result) => {
                self.busy = None;
                match result {
                    Ok(summary) => {
                        self.error = None;
                        self.last_result = Some(summary);
                        // gc/prune/… change the object store; reload the
                        // census so the health section reflects it.
                        Some(Action::Refresh)
                    }
                    Err(error) => {
                        self.error = Some(error);
                        None
                    }
                }
            }
            Msg::SparseDone(result) => {
                self.busy = None;
                match result {
                    Ok(()) => {
                        self.error = None;
                        self.last_result = Some("sparse-checkout updated".to_string());
                        Some(Action::Refresh)
                    }
                    Err(error) => {
                        self.error = Some(error);
                        None
                    }
                }
            }
            Msg::LfsDone(result) => {
                self.busy = None;
                match result {
                    Ok(summary) => {
                        self.error = None;
                        self.last_result = Some(summary);
                        Some(Action::Refresh)
                    }
                    Err(error) => {
                        self.error = Some(error);
                        None
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

impl HealthPanel {
    pub fn view(&self, tokens: Tokens) -> Element<'_, Msg> {
        if !self.open {
            return column![].into();
        }

        let mut header = row![text("Health & maintenance")
            .size(12)
            .font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
        if self.loading {
            header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
        }
        header = header
            .push(Space::new().width(Length::Fill))
            .push(
                button(text("⟳").size(11))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Msg::RefreshPressed),
            )
            .push(
                button(text("×").size(12))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Msg::ToggleOpen),
            );

        let mut body = column![header].spacing(6).width(Length::Fill);
        if let Some(line) = status_line(self, tokens) {
            body = body.push(line);
        }
        if let Some(strip) = confirm_strip(self, tokens) {
            body = body.push(strip);
        }
        body = body
            .push(health_section(self, tokens))
            .push(maintenance_section(self, tokens))
            .push(archive_section(self, tokens))
            .push(sparse_section(self, tokens))
            .push(lfs_section(self, tokens));

        container(
            scrollable(container(body).padding([8, 10]).width(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill),
        )
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
        .into()
    }
}

/// Busy line, last result or error — whichever is newest.
fn status_line<'a>(panel: &'a HealthPanel, tokens: Tokens) -> Option<Element<'a, Msg>> {
    if let Some(busy) = &panel.busy {
        return Some(
            row![
                text("⏳").size(10),
                text(busy.as_str()).size(10).color(tokens.muted_foreground)
            ]
            .spacing(4)
            .into(),
        );
    }
    if let Some(error) = &panel.error {
        return Some(note(tokens, error.clone(), true).into());
    }
    panel
        .last_result
        .as_ref()
        .map(|result| note(tokens, result.clone(), false).into())
}

/// The one inline confirm strip for whichever section requested it.
fn confirm_strip<'a>(panel: &'a HealthPanel, tokens: Tokens) -> Option<Element<'a, Msg>> {
    let confirm = panel.confirm.as_ref()?;
    let (press, cancel) = match panel.confirm_section()? {
        ConfirmSection::Maintenance => (Msg::OpConfirmed, Msg::OpCanceled),
        ConfirmSection::Sparse => (Msg::SparseConfirmed, Msg::ConfirmCanceled),
        ConfirmSection::Lfs => (Msg::LfsOpConfirmed, Msg::ConfirmCanceled),
    };
    Some(
        container(
            row![
                text(confirm.label.as_str())
                    .size(10)
                    .color(tokens.foreground)
                    .width(Length::Fill),
                button(text("Cancel").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(cancel),
                button(text("Confirm").size(9))
                    .style(theme::button_style(tokens, theme::ButtonKind::Danger))
                    .padding([2, 8])
                    .on_press(press),
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
        })
        .into(),
    )
}

/// Section 1: the `RepoHealth` census.
fn health_section<'a>(panel: &HealthPanel, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = section_header(tokens, "Health");

    let rows: Element<'a, Msg> = match &panel.health {
        Some(health) => {
            let now = now_seconds();
            let packed = match health.packed_objects {
                Some(kib) => format!("{kib} KiB"),
                None => "not measured — run Count objects".to_string(),
            };
            let graph = if health.has_commit_graph {
                format!("yes ({})", human_size(health.commit_graph_bytes))
            } else {
                "no".to_string()
            };
            let last_gc = match health.last_gc {
                Some(at) => relative_age(at, now),
                None => "never recorded".to_string(),
            };
            let dangling = match (&health.fsck_dangling, health.fsck_samples.first()) {
                (Some(count), Some(sample)) => {
                    format!("{count} dangling (e.g. {})", clip(sample, 12))
                }
                (Some(count), None) => format!("{count} dangling"),
                (None, _) => "not measured — run Fsck census".to_string(),
            };
            column![
                kv(tokens, "git dir size", human_size(health.git_size_bytes)),
                kv(
                    tokens,
                    "worktree size",
                    human_size(health.worktree_size_bytes)
                ),
                kv(tokens, "loose objects", health.loose_objects.to_string()),
                kv(tokens, "packed objects", packed),
                kv(tokens, "pack files", health.pack_files.to_string()),
                kv(tokens, "commit-graph", graph),
                kv(
                    tokens,
                    "packed-refs",
                    if health.packed_refs {
                        "yes".to_string()
                    } else {
                        "no".to_string()
                    }
                ),
                kv(tokens, "last gc", last_gc),
                kv(tokens, "fsck", dangling),
            ]
            .spacing(2)
            .width(Length::Fill)
            .into()
        }
        None if panel.error.is_some() => {
            note(tokens, "Health unavailable — see the error above.", false).into()
        }
        None => note(tokens, "Loading health…", false).into(),
    };
    section = section.push(rows);
    section_card(section, tokens)
}

/// Section 2: maintenance op buttons (two rows of three).
fn maintenance_section<'a>(panel: &HealthPanel, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = section_header(tokens, "Maintenance");
    let locked = panel.busy.is_some();

    let mut op_rows = column![].spacing(4).width(Length::Fill);
    for chunk in MAINTENANCE_OPS.chunks(3) {
        let mut buttons = row![].spacing(4);
        for op in chunk {
            buttons = buttons.push(
                button(text(op_label(op)).size(10))
                    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                    .padding([4, 8])
                    .width(Length::Fill)
                    .on_press_maybe((!locked).then(|| Msg::OpRequested((*op).to_string()))),
            );
        }
        op_rows = op_rows.push(buttons);
    }
    section = section.push(op_rows);
    if locked {
        section = section.push(note(
            tokens,
            "An op is running — buttons stay disabled until it finishes.",
            false,
        ));
    } else {
        section = section.push(note(
            tokens,
            "Count objects is a read-only census; the other ops confirm first.",
            false,
        ));
    }
    section_card(section, tokens)
}

/// Section 3: archive export (destination picked by the shell's save
/// dialog — the panel only emits the intent).
fn archive_section<'a>(panel: &HealthPanel, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = section_header(tokens, "Archive export");

    let spec = panel.archive_ref.trim();
    let valid = archive_arg_ok(spec);
    section = section.push(
        row![
            text_input("ref (HEAD, branch, tag…)", panel.archive_ref.as_str())
                .on_input(Msg::ArchiveRefChanged)
                .on_submit_maybe(can_archive(panel).then_some(Msg::ArchivePressed))
                .size(11)
                .padding([4, 6])
                .width(Length::Fill),
            pick_list(
                ArchiveFormat::ALL.as_slice(),
                Some(panel.archive_format),
                Msg::ArchiveFormatChanged
            )
            .text_size(11)
            .padding([3, 6])
            .width(Length::Fixed(110.0)),
            button(text("Export…").size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                .padding([4, 10])
                .on_press_maybe(can_archive(panel).then_some(Msg::ArchivePressed)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    );

    let hint = if valid {
        "The destination file is picked in the native save dialog."
    } else {
        "Invalid ref: no leading dash, shell metacharacters or control characters."
    };
    section = section.push(note(tokens, hint, !valid));
    section_card(section, tokens)
}

/// Section 4: sparse checkout state + pattern editor.
fn sparse_section<'a>(panel: &'a HealthPanel, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = section_header(tokens, "Sparse checkout");

    if let Some(info) = &panel.sparse {
        let mut chips = row![].spacing(4).align_y(Alignment::Center);
        chips = chips.push(chip_view(
            tokens,
            if info.enabled {
                "enabled"
            } else {
                "full checkout"
            },
            false,
        ));
        chips = chips.push(chip_view(
            tokens,
            if info.cone { "cone mode" } else { "no cone" },
            !info.cone,
        ));
        section = section.push(chips);

        let mut pattern_rows = column![].spacing(1).width(Length::Fill);
        for pattern in info.patterns.iter().take(6) {
            pattern_rows = pattern_rows.push(
                text(pattern.as_str())
                    .size(9)
                    .font(theme::mono())
                    .color(tokens.muted_foreground),
            );
        }
        if info.patterns.len() > 6 {
            pattern_rows = pattern_rows.push(text(format!("+{} more", info.patterns.len() - 6)));
        }
        if !info.patterns.is_empty() {
            section = section.push(pattern_rows);
        }
    } else {
        section = section.push(note(tokens, "Sparse state not loaded yet.", false));
    }

    section = section.push(
        text_editor(&panel.sparse_body)
            .placeholder("one directory pattern per line (empty = full checkout)")
            .on_action(Msg::SparseEdited)
            .size(11)
            .padding(6)
            .height(Length::Fixed(64.0)),
    );
    section = section.push(
        row![
            checkbox(panel.sparse_add)
                .label("add (keep existing patterns)")
                .size(12)
                .text_size(10)
                .spacing(4)
                .on_toggle(Msg::SparseAddToggled),
            Space::new().width(Length::Fill),
            button(text("Apply…").size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Primary))
                .padding([4, 10])
                .on_press_maybe((panel.busy.is_none()).then_some(Msg::SparsePressed)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    );
    section_card(section, tokens)
}

/// Section 5: git-lfs availability, tracked patterns and run ops.
fn lfs_section<'a>(panel: &HealthPanel, tokens: Tokens) -> Element<'a, Msg> {
    let mut section = section_header(tokens, "Git LFS");

    match &panel.lfs {
        Some(status) => {
            let mut chips = row![].spacing(4).align_y(Alignment::Center);
            chips = chips.push(chip_view(
                tokens,
                if status.installed {
                    "installed"
                } else {
                    "not installed"
                },
                !status.installed,
            ));
            if let Some(version) = status.version.as_deref() {
                chips = chips.push(
                    text(clip(version, 34))
                        .size(9)
                        .font(theme::mono())
                        .color(tokens.muted_foreground),
                );
            }
            section = section.push(chips);

            if status.tracked_patterns.is_empty() {
                section = section.push(note(tokens, "No tracked LFS patterns.", false));
            } else {
                let mut patterns = row![].spacing(4);
                for pattern in status.tracked_patterns.iter().take(8) {
                    patterns = patterns.push(chip_view(tokens, pattern, false));
                }
                if status.tracked_patterns.len() > 8 {
                    patterns =
                        patterns.push(text(format!("+{} more", status.tracked_patterns.len() - 8)));
                }
                section = section.push(patterns);
            }
        }
        None => section = section.push(note(tokens, "LFS status not loaded yet.", false)),
    }

    let locked = panel.busy.is_some();
    let mut buttons = row![].spacing(4);
    for sub in ["pull", "push", "fetch", "install"] {
        buttons = buttons.push(
            button(text(sub).size(10))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([4, 8])
                .on_press_maybe((!locked).then(|| Msg::LfsOpRequested(sub.to_string()))),
        );
    }
    section = section.push(buttons);
    section_card(section, tokens)
}

// ---------------------------------------------------------------------------
// Small view helpers (label/value row, section card, chips, notes)
// ---------------------------------------------------------------------------

fn section_header<'a>(tokens: Tokens, label: &str) -> Column<'a, Msg> {
    column![text(label.to_string())
        .size(10)
        .font(theme::semibold())
        .color(tokens.muted_foreground)]
    .spacing(4)
    .width(Length::Fill)
}

fn section_card<'a>(section: Column<'a, Msg>, tokens: Tokens) -> Element<'a, Msg> {
    container(section.spacing(4).padding([6, 8]))
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(tokens.muted, 0.06))),
            border: Border {
                color: theme::with_alpha(tokens.border, 0.6),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn kv<'a>(tokens: Tokens, label: &str, value: String) -> Element<'a, Msg> {
    row![
        text(label.to_string())
            .size(10)
            .color(tokens.muted_foreground)
            .width(Length::Fixed(120.0)),
        text(value).size(10).color(tokens.foreground),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

fn chip_view<'a>(tokens: Tokens, label: &str, danger: bool) -> Element<'a, Msg> {
    let color = if danger {
        tokens.destructive
    } else {
        tokens.muted_foreground
    };
    container(text(label.to_string()).size(9).color(color))
        .padding([1, 6])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(theme::with_alpha(color, 0.12))),
            border: Border {
                color: theme::with_alpha(color, 0.4),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn note<'a>(
    tokens: Tokens,
    label: impl Into<String>,
    error: bool,
) -> container::Container<'a, Msg> {
    let color = if error {
        tokens.destructive
    } else {
        tokens.muted_foreground
    };
    container(text(label.into()).size(10).color(color)).padding([2, 4])
}

fn clip(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> HealthPanel {
        HealthPanel::default()
    }

    fn run(panel: &mut HealthPanel, msg: Msg) -> Option<Action> {
        panel.update(msg)
    }

    #[test]
    fn default_panel_opens_with_a_head_ref_and_refreshes_once() {
        let mut panel = panel();
        assert_eq!(panel.archive_ref, "HEAD");
        assert_eq!(run(&mut panel, Msg::ToggleOpen), Some(Action::Refresh));
        assert!(panel.open && panel.loading);

        // With data loaded, closing and reopening does not re-refresh.
        panel.loading = false;
        panel.health = Some(RepoHealth::default());
        assert_eq!(run(&mut panel, Msg::ToggleOpen), None);
        assert!(!panel.open);
        assert!(panel.confirm.is_none(), "close clears a pending confirm");
        assert_eq!(run(&mut panel, Msg::ToggleOpen), None);
        assert!(panel.open);
    }

    #[test]
    fn refresh_pressed_clears_error_and_requests_reload() {
        let mut panel = panel();
        panel.open = true;
        panel.error = Some("stale".to_string());
        assert_eq!(run(&mut panel, Msg::RefreshPressed), Some(Action::Refresh));
        assert!(panel.loading);
        assert!(panel.error.is_none());
    }

    #[test]
    fn section_loads_apply_success_and_failure() {
        let mut panel = panel();
        panel.open = true;

        assert_eq!(run(&mut panel, Msg::HealthLoaded(Err("boom".into()))), None);
        assert!(panel.health.is_none());
        assert_eq!(panel.error.as_deref(), Some("boom"));
        assert!(!panel.loading, "a failed load also ends the loading state");

        let health = RepoHealth {
            loose_objects: 3,
            pack_files: 2,
            ..RepoHealth::default()
        };
        assert_eq!(run(&mut panel, Msg::HealthLoaded(Ok(health))), None);
        assert_eq!(panel.health.as_ref().unwrap().loose_objects, 3);
        assert!(panel.error.is_none());

        assert_eq!(
            run(&mut panel, Msg::LfsLoaded(Err("lfs down".into()))),
            None
        );
        assert!(panel.lfs.is_none());
        assert_eq!(panel.error.as_deref(), Some("lfs down"));
        assert_eq!(
            run(
                &mut panel,
                Msg::LfsLoaded(Ok(LfsStatus {
                    installed: true,
                    version: Some("git-lfs/3.0".into()),
                    tracked_patterns: vec!["*.bin".into()],
                }))
            ),
            None
        );
        assert!(panel.lfs.as_ref().unwrap().installed);
    }

    #[test]
    fn sparse_load_prefills_editor_only_when_empty() {
        let mut panel = panel();
        panel.open = true;

        let info = SparseInfo {
            enabled: true,
            cone: true,
            patterns: vec!["src/ui".into(), "lib".into()],
        };
        assert_eq!(run(&mut panel, Msg::SparseLoaded(Ok(info))), None);
        assert_eq!(panel.sparse_body.text(), "src/ui\nlib");

        // A failure does not wipe the editor, and a later load with more
        // patterns does not clobber user edits.
        assert_eq!(run(&mut panel, Msg::SparseLoaded(Err("x".into()))), None);
        assert_eq!(panel.sparse_body.text(), "src/ui\nlib");
        let edited = SparseInfo {
            patterns: vec!["other".into()],
            ..SparseInfo::default()
        };
        assert_eq!(run(&mut panel, Msg::SparseLoaded(Ok(edited))), None);
        assert_eq!(panel.sparse_body.text(), "src/ui\nlib");
    }

    #[test]
    fn maintenance_confirm_flow_request_confirm_clear() {
        let mut panel = panel();
        panel.open = true;

        assert_eq!(run(&mut panel, Msg::OpRequested(OP_GC.to_string())), None);
        let confirm = panel.confirm.as_ref().expect("confirm parked");
        assert_eq!(confirm.op, OP_GC);
        assert!(panel.busy.is_none());

        assert_eq!(
            run(&mut panel, Msg::OpConfirmed),
            Some(Action::RunOp {
                op: OP_GC.to_string(),
                label: op_description(OP_GC).to_string(),
            })
        );
        assert!(panel.confirm.is_none(), "confirm cleared on execution");
        assert!(panel.busy.is_some());

        assert_eq!(
            run(&mut panel, Msg::OpDone(Ok("gc done".into()))),
            Some(Action::Refresh)
        );
        assert!(panel.busy.is_none());
        assert_eq!(panel.last_result.as_deref(), Some("gc done"));

        // Cancel path clears without running anything.
        assert_eq!(run(&mut panel, Msg::OpRequested(OP_FSCK.to_string())), None);
        assert_eq!(run(&mut panel, Msg::OpCanceled), None);
        assert!(panel.confirm.is_none());
        assert!(panel.busy.is_none());
    }

    #[test]
    fn count_objects_runs_directly_fsck_confirms() {
        let mut panel = panel();
        panel.open = true;

        assert_eq!(
            run(&mut panel, Msg::OpRequested(OP_COUNT_OBJECTS.to_string())),
            Some(Action::RunOp {
                op: OP_COUNT_OBJECTS.to_string(),
                label: op_label(OP_COUNT_OBJECTS).to_string(),
            })
        );
        assert!(panel.confirm.is_none());
        assert!(panel.busy.is_some());
        panel.busy = None;

        assert_eq!(run(&mut panel, Msg::OpRequested(OP_FSCK.to_string())), None);
        assert_eq!(panel.confirm.as_ref().unwrap().op, OP_FSCK);
    }

    #[test]
    fn op_failure_surfaces_in_error_without_refresh() {
        let mut panel = panel();
        panel.open = true;
        run(&mut panel, Msg::OpRequested(OP_GC.to_string()));
        run(&mut panel, Msg::OpConfirmed);
        assert_eq!(
            run(&mut panel, Msg::OpDone(Err("git gc failed".into()))),
            None
        );
        assert!(panel.busy.is_none());
        assert_eq!(panel.error.as_deref(), Some("git gc failed"));
        assert!(panel.last_result.is_none());
    }

    #[test]
    fn busy_lock_blocks_concurrent_ops() {
        let mut panel = panel();
        panel.open = true;
        panel.busy = Some("Running git gc…".to_string());

        assert_eq!(run(&mut panel, Msg::OpRequested(OP_GC.to_string())), None);
        assert!(panel.confirm.is_none(), "no confirm while busy");
        assert_eq!(run(&mut panel, Msg::ArchivePressed), None);
        assert_eq!(run(&mut panel, Msg::SparsePressed), None);
        assert_eq!(run(&mut panel, Msg::LfsOpRequested("pull".into())), None);
        assert!(panel.confirm.is_none());

        // A stale confirm (busy + confirm can only co-exist artificially)
        // cannot start a second op and is dropped.
        panel.confirm = Some(PendingConfirm {
            op: OP_PRUNE.to_string(),
            label: "Pack refs".to_string(),
        });
        assert_eq!(run(&mut panel, Msg::OpConfirmed), None);
        assert!(panel.confirm.is_none());
        assert!(panel.busy.is_some(), "busy unchanged by the dropped op");
    }

    #[test]
    fn sparse_text_parsing_splits_trims_and_dedupes() {
        assert_eq!(
            parse_patterns("src/ui\n\n   lib \ncore\nsrc/ui"),
            vec!["src/ui", "lib", "core"]
        );
        assert_eq!(parse_patterns("  solo  "), vec!["solo"]);
        assert!(parse_patterns("").is_empty());
        assert!(parse_patterns(" \n\t\n").is_empty());
    }

    #[test]
    fn sparse_confirm_flow_set_add_and_disable() {
        let mut panel = panel();
        panel.open = true;
        run(&mut panel, Msg::SparseTextChanged("a\n b \na\n\n".into()));

        assert_eq!(run(&mut panel, Msg::SparsePressed), None);
        let confirm = panel.confirm.as_ref().unwrap();
        assert_eq!(confirm.op, OP_SPARSE_SET);
        assert!(confirm.label.contains("replace"));

        match run(&mut panel, Msg::SparseConfirmed) {
            Some(Action::SparseApply { patterns, add }) => {
                assert_eq!(patterns, vec!["a".to_string(), "b".to_string()]);
                assert!(!add);
            }
            other => panic!("expected SparseApply, got {other:?}"),
        }
        assert!(panel.busy.is_some());
        assert_eq!(
            run(&mut panel, Msg::SparseDone(Ok(()))),
            Some(Action::Refresh)
        );
        assert_eq!(
            panel.last_result.as_deref(),
            Some("sparse-checkout updated")
        );

        // add semantics flow through the same confirm strip.
        run(&mut panel, Msg::SparseAddToggled(true));
        assert_eq!(run(&mut panel, Msg::SparsePressed), None);
        assert_eq!(panel.confirm.as_ref().unwrap().op, OP_SPARSE_ADD);
        match run(&mut panel, Msg::SparseConfirmed) {
            Some(Action::SparseApply { add, .. }) => assert!(add),
            other => panic!("expected SparseApply, got {other:?}"),
        }
        panel.busy = None;

        // Empty editor = disable sparse-checkout (empty patterns, set
        // semantics).
        run(&mut panel, Msg::SparseAddToggled(false));
        run(&mut panel, Msg::SparseTextChanged("".into()));
        assert_eq!(run(&mut panel, Msg::SparsePressed), None);
        match run(&mut panel, Msg::SparseConfirmed) {
            Some(Action::SparseApply { patterns, add }) => {
                assert!(patterns.is_empty());
                assert!(!add);
            }
            other => panic!("expected SparseApply, got {other:?}"),
        }
    }

    #[test]
    fn sparse_and_lfs_cancels_clear_their_confirms() {
        let mut panel = panel();
        panel.open = true;
        run(&mut panel, Msg::SparsePressed);
        assert!(panel.confirm.is_some());
        assert_eq!(run(&mut panel, Msg::ConfirmCanceled), None);
        assert!(panel.confirm.is_none());
        assert!(panel.busy.is_none());

        run(&mut panel, Msg::LfsOpRequested("push".into()));
        assert!(panel.confirm.is_some());
        assert_eq!(run(&mut panel, Msg::ConfirmCanceled), None);
        assert!(panel.confirm.is_none());
    }

    #[test]
    fn lfs_confirm_flow_routes_the_subcommand() {
        let mut panel = panel();
        panel.open = true;

        assert_eq!(run(&mut panel, Msg::LfsOpRequested("pull".into())), None);
        let confirm = panel.confirm.as_ref().unwrap();
        assert_eq!(confirm.op, "lfs:pull");
        assert_eq!(confirm.label, "git lfs pull");

        assert_eq!(
            run(&mut panel, Msg::LfsOpConfirmed),
            Some(Action::LfsRun {
                subcommand: "pull".to_string(),
            })
        );
        assert!(panel.busy.is_some());
        assert_eq!(
            run(&mut panel, Msg::LfsDone(Ok("2 objects fetched".into()))),
            Some(Action::Refresh)
        );
        assert!(panel.busy.is_none());
        assert_eq!(panel.last_result.as_deref(), Some("2 objects fetched"));

        // Sparse confirms are not consumed by the LFS confirm message.
        run(&mut panel, Msg::SparsePressed);
        assert!(run(&mut panel, Msg::LfsOpConfirmed).is_none());
        assert!(panel.busy.is_none());
    }

    #[test]
    fn archive_ref_validation_mirrors_the_service_guard() {
        for ok in ["HEAD", "main", "v1.0.1", "origin/main", "refs/heads/x"] {
            assert!(archive_arg_ok(ok), "`{ok}` should be accepted");
        }
        for bad in [
            "", "-x", "a;b", "a&b", "a|b", "a$b", "a`b", "a(b", "a)b", "a<b", "a>b", "a\"b", "a'b",
            "a\nb", "a\rb",
        ] {
            assert!(!archive_arg_ok(bad), "`{bad}` should be rejected");
        }
        // A NUL byte is a control character the service rejects too.
        assert!(!archive_arg_ok("a\0b"));
    }

    #[test]
    fn archive_button_gate_and_done_flow() {
        let mut panel = panel();
        panel.open = true;
        assert!(can_archive(&panel), "default HEAD passes the guard");

        panel.archive_ref = "-o evil".into();
        assert!(!can_archive(&panel));
        assert_eq!(run(&mut panel, Msg::ArchivePressed), None);

        panel.archive_ref = "v1.0".into();
        assert_eq!(
            run(&mut panel, Msg::ArchivePressed),
            Some(Action::Archive {
                spec: "v1.0".to_string(),
                format: "zip".to_string(),
            })
        );
        assert!(panel.busy.is_some());

        assert_eq!(run(&mut panel, Msg::ArchiveDone(Ok(()))), None);
        assert!(panel.busy.is_none());
        assert_eq!(panel.last_result.as_deref(), Some("archive exported"));
        assert_eq!(
            run(&mut panel, Msg::ArchiveDone(Err("cancelled".into()))),
            None
        );
        assert_eq!(panel.error.as_deref(), Some("cancelled"));

        // Format picker feeds the emitted format string.
        assert_eq!(
            run(&mut panel, Msg::ArchiveFormatChanged(ArchiveFormat::TarGz)),
            None
        );
        assert_eq!(
            run(&mut panel, Msg::ArchivePressed),
            Some(Action::Archive {
                spec: "v1.0".to_string(),
                format: "tar.gz".to_string(),
            })
        );
    }

    #[test]
    fn confirm_strip_routes_per_section() {
        let mut panel = panel();
        panel.open = true;

        run(&mut panel, Msg::OpRequested(OP_GC.to_string()));
        assert_eq!(panel.confirm_section(), Some(ConfirmSection::Maintenance));
        assert_eq!(run(&mut panel, Msg::OpCanceled), None);

        run(&mut panel, Msg::SparsePressed);
        assert_eq!(panel.confirm_section(), Some(ConfirmSection::Sparse));
        assert_eq!(run(&mut panel, Msg::ConfirmCanceled), None);

        run(&mut panel, Msg::LfsOpRequested("fetch".into()));
        assert_eq!(panel.confirm_section(), Some(ConfirmSection::Lfs));
        assert_eq!(
            run(&mut panel, Msg::LfsOpConfirmed),
            Some(Action::LfsRun {
                subcommand: "fetch".to_string(),
            })
        );
    }

    #[test]
    fn human_size_formats_bytes_kib_mib_gib() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn every_maintenance_op_has_labels() {
        for op in MAINTENANCE_OPS {
            assert!(!op_label(op).is_empty(), "{op} missing a button label");
            assert!(
                !op_description(op).is_empty(),
                "{op} missing a confirm description"
            );
        }
        assert!(op_needs_confirm(OP_GC));
        assert!(!op_needs_confirm(OP_COUNT_OBJECTS));
    }

    #[test]
    fn archive_format_round_trips() {
        assert_eq!(ArchiveFormat::default(), ArchiveFormat::Zip);
        assert_eq!(ArchiveFormat::Zip.to_string(), "zip");
        assert_eq!(ArchiveFormat::Tar.to_string(), "tar");
        assert_eq!(ArchiveFormat::TarGz.to_string(), "tar.gz");
    }
}
