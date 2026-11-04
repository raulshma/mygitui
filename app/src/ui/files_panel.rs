//! Files panel: a lazy working-tree browser rendered as a center mode.
//!
//! One directory level is fetched at a time through
//! `workspace::repo_list_dir` (root first, children on expand); listings are
//! cached per directory and dropped wholesale when the repo changes (the
//! watcher-driven `RepoChanged` path re-issues the visible loads). Entries
//! carry a git-state dot derived from the shell's status snapshot — same
//! colors as the status panel (untracked/modified/deleted/staged/conflicted),
//! directories aggregate the strongest state below them.
//!
//! Right-clicks emit `Message::FilesRowMenu`; the shell builds the context
//! menu (open / file history / copy path / external editor / new file… /
//! new folder… / rename… / delete) and translates the chosen `files.*`
//! command back into panel state. New/rename targets are typed into an
//! inline prompt row (same pattern as the branch panel's create field);
//! deletes go through the tab's confirmation dialog.
//!
//! Pure UI + intent module: no service calls, no `Task`. The shell routes
//! `Message::Files` through [`update`] and executes the returned
//! [`Action`]s (`repo_list_dir`, `repo_create_entry`, `repo_rename_entry`,
//! `repo_delete_entry`, editor/history opens).

use std::collections::{HashMap, HashSet};

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::engine::types::ChangeKind;
use crate::workspace::{DirEntryKind, DirListing, NewEntryKind};

use super::app::Message;
use super::status_model::StatusRow;
use super::theme::{self, Tokens};

/// Recursion cap for the flatten walk; a listing can only nest as deep as
/// the filesystem, so this only guards against malformed service data.
const MAX_DEPTH: usize = 64;

/// Indent per nesting level.
const INDENT_WIDTH: f32 = 14.0;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Files panel state (one per repo tab).
#[derive(Debug, Default)]
pub struct FilesPanel {
    pub open: bool,
    /// Cached listings by requested directory path ("" = workdir root).
    pub dirs: HashMap<String, DirListing>,
    /// Directories with a load in flight (guards duplicate requests).
    pub loading: HashSet<String>,
    /// Expanded directory paths (persisted across reloads).
    pub expanded: HashSet<String>,
    /// Last load failure (rendered like the other panels' error states).
    pub error: Option<String>,
    /// Inline new-file / new-folder / rename prompt, if one is open.
    pub prompt: Option<Prompt>,
}

impl FilesPanel {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Drops every cached listing (expanded state survives); the shell
    /// re-issues loads for the root and the expanded directories.
    pub fn invalidate(&mut self) {
        self.dirs.clear();
        self.loading.clear();
        self.error = None;
    }

    /// Stores a completed directory load.
    pub fn accept_dir(&mut self, dir: &str, result: Result<DirListing, String>) {
        self.loading.remove(dir);
        match result {
            Ok(listing) => {
                self.dirs.insert(dir.to_string(), listing);
                self.error = None;
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// Whether a load for `dir` is needed (not cached, not in flight).
    pub fn needs_load(&self, dir: &str) -> bool {
        !self.dirs.contains_key(dir) && !self.loading.contains(dir)
    }
}

/// What the inline prompt is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// Create an empty file inside `dir` ("" = root).
    NewFile { dir: String },
    /// Create a directory inside `dir` ("" = root).
    NewFolder { dir: String },
    /// Rename/move `path` (the prompt value is the new name; a value with a
    /// `/` moves the entry into a sibling subdirectory).
    Rename { path: String },
}

/// Inline prompt state (a text row at the bottom of the panel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub kind: PromptKind,
    pub value: String,
}

// ---------------------------------------------------------------------------
// Git-state decorations
// ---------------------------------------------------------------------------

/// Working-tree state badge for one tree entry, collapsed from the status
/// snapshot to a single mark per path. Ordering is the aggregation priority
/// for directories: a directory shows the strongest state below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileState {
    /// Untracked in the worktree.
    Untracked,
    /// Modified (or added/renamed/copied/typechange) in the worktree.
    Modified,
    /// Deleted from the worktree or index.
    Deleted,
    /// Staged (index differs from HEAD) — includes staged+modified files.
    Staged,
    /// Merge conflict.
    Conflicted,
}

/// Single-path state of one status row (status-panel visual language).
pub fn file_state_of(row: &StatusRow) -> FileState {
    if matches!(row.index, ChangeKind::Conflicted) || matches!(row.worktree, ChangeKind::Conflicted)
    {
        return FileState::Conflicted;
    }
    let staged = !matches!(row.index, ChangeKind::Unmodified | ChangeKind::Ignored);
    if staged {
        return FileState::Staged;
    }
    match row.worktree {
        ChangeKind::Deleted => FileState::Deleted,
        ChangeKind::Untracked => FileState::Untracked,
        _ => FileState::Modified,
    }
}

/// Path → state for every changed file in the status snapshot.
pub fn file_states(rows: &[StatusRow]) -> HashMap<String, FileState> {
    rows.iter()
        .map(|row| (row.path.clone(), file_state_of(row)))
        .collect()
}

/// Aggregates file states into their ancestor directories: each directory
/// shows the strongest state of any changed file below it. A file and a
/// directory can never share a path, so the maps merge without collision.
pub fn aggregate_dir_states(files: &HashMap<String, FileState>) -> HashMap<String, FileState> {
    let mut out: HashMap<String, FileState> = HashMap::new();
    for (path, state) in files {
        let mut rest = path.as_str();
        while let Some(pos) = rest.rfind('/') {
            let dir = &rest[..pos];
            let slot = out.entry(dir.to_string()).or_insert(*state);
            if *state > *slot {
                *slot = *state;
            }
            rest = dir;
        }
    }
    out
}

/// Combined path → state map (files plus their ancestor directories).
pub fn states(rows: &[StatusRow]) -> HashMap<String, FileState> {
    let files = file_states(rows);
    let dirs = aggregate_dir_states(&files);
    files.into_iter().chain(dirs).collect()
}

/// Token color of a state mark (matches `status_panel::kind_color`).
pub fn state_color(tokens: Tokens, state: FileState) -> Color {
    match state {
        FileState::Untracked => tokens.untracked,
        FileState::Modified => tokens.modified,
        FileState::Deleted => tokens.deleted,
        FileState::Staged => tokens.added,
        FileState::Conflicted => tokens.conflicted,
    }
}

// ---------------------------------------------------------------------------
// Tree model
// ---------------------------------------------------------------------------

/// One rendered row of the flattened tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Repository-relative path.
    pub path: String,
    pub name: String,
    /// Nesting level (root entries are 0).
    pub depth: usize,
    pub kind: DirEntryKind,
    /// Whether this directory currently shows its children (dirs only).
    pub expanded: bool,
    /// Git-state dot, if the path (or anything below it) is changed.
    pub state: Option<FileState>,
}

impl Row {
    /// Whether the row is an expandable directory (submodules are listed but
    /// not browsed — open them as repos instead).
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, DirEntryKind::Dir | DirEntryKind::Submodule)
    }
}

/// Flattens the cached listings into visible rows: the root listing's
/// entries in service order (dirs first, natural sort), with the children of
/// every expanded directory interleaved in place. Directories whose listing
/// is not loaded (or not fetched yet) still render, without children.
pub fn flatten(
    dirs: &HashMap<String, DirListing>,
    expanded: &HashSet<String>,
    states: &HashMap<String, FileState>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    flatten_into(dirs, expanded, states, "", 0, &mut rows);
    rows
}

fn flatten_into(
    dirs: &HashMap<String, DirListing>,
    expanded: &HashSet<String>,
    states: &HashMap<String, FileState>,
    dir: &str,
    depth: usize,
    rows: &mut Vec<Row>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(listing) = dirs.get(dir) else {
        return;
    };
    for entry in &listing.entries {
        let is_dir = matches!(entry.kind, DirEntryKind::Dir | DirEntryKind::Submodule);
        let row_expanded = is_dir && expanded.contains(&entry.path);
        rows.push(Row {
            path: entry.path.clone(),
            name: entry.name.clone(),
            depth,
            kind: entry.kind,
            expanded: row_expanded,
            state: states.get(&entry.path).copied(),
        });
        if row_expanded {
            flatten_into(dirs, expanded, states, &entry.path, depth + 1, rows);
        }
    }
}

// ---------------------------------------------------------------------------
// Name helpers (pure; unit-tested)
// ---------------------------------------------------------------------------

/// Joins a prompt directory ("" = root) and a typed name into the target
/// path: forward slashes, one separator between segments, empty segments
/// dropped (`"src"` + `" / a / "` → `"src/a"`).
pub fn join_rel(dir: &str, name: &str) -> String {
    let mut segments: Vec<&str> = dir.split('/').collect();
    segments.extend(name.split('/'));
    let segments: Vec<&str> = segments
        .into_iter()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect();
    segments.join("/")
}

/// Base name (last path segment) of a repository-relative path.
pub fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Target path for a rename prompt: the typed name resolved inside the
/// renamed entry's current parent (`a/b.txt` + `c.txt` → `a/c.txt`; a name
/// with a slash moves the entry into a sibling subdirectory).
pub fn rename_target(path: &str, name: &str) -> String {
    let name = name.trim();
    match path.rfind('/') {
        Some(pos) => join_rel(&path[..pos], name),
        None => name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Messages + actions
// ---------------------------------------------------------------------------

/// Files panel messages (user input; routed from the shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Show/hide the panel (⋯ menu "Files…", palette `view.files`).
    Toggle,
    /// Click on a row: toggles directories (fetching their listing when
    /// needed), opens files.
    RowOpen { path: String, is_dir: bool },
    /// ⟳ header button: drop all cached listings and reload.
    Reload,
    /// Open the inline prompt (context menu / header buttons).
    PromptOpen(PromptKind),
    /// Prompt text changed.
    PromptChanged(String),
    /// Prompt submitted (Enter or button).
    PromptConfirm,
    /// Prompt canceled.
    PromptCancel,
}

/// Intents the shell executes (see the module docs for the service mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The panel became visible; the shell closes the other center modes.
    Opened,
    /// Load one directory level ("" = root).
    LoadDir { dir: String },
    /// Drop the caches and reload the visible directories.
    Reload,
    /// Open a file in the in-app editor.
    OpenEditor { path: String },
    /// Open a file's history view.
    OpenHistory { path: String },
    /// `repo_create_entry`.
    CreateEntry { path: String, kind: NewEntryKind },
    /// `repo_rename_entry(from, to)`.
    RenameEntry { from: String, to: String },
    /// `repo_delete_entry` (the shell asks for confirmation first).
    DeleteEntry { path: String },
}

/// Pure state machine: apply `msg`, return the intent (if any) for the
/// shell to execute. No service calls happen here.
pub fn update(panel: &mut FilesPanel, msg: Msg) -> Option<Action> {
    match msg {
        Msg::Toggle => {
            panel.toggle();
            if panel.open {
                panel.error = None;
                Some(Action::Opened)
            } else {
                None
            }
        }
        Msg::RowOpen { path, is_dir } => {
            if is_dir {
                toggle_dir(panel, path)
            } else {
                Some(Action::OpenEditor { path })
            }
        }
        Msg::Reload => {
            panel.invalidate();
            Some(Action::Reload)
        }
        Msg::PromptOpen(kind) => {
            // Rename prompts start from the entry's current name; creates
            // start empty.
            let value = match &kind {
                PromptKind::Rename { path } => base_name(path).to_string(),
                _ => String::new(),
            };
            panel.prompt = Some(Prompt { kind, value });
            None
        }
        Msg::PromptChanged(value) => {
            if let Some(prompt) = &mut panel.prompt {
                prompt.value = value;
            }
            None
        }
        Msg::PromptConfirm => {
            let prompt = panel.prompt.clone()?;
            let value = prompt.value.trim().trim_matches('/').to_string();
            if value.is_empty() || value.split('/').any(|segment| segment.is_empty()) {
                return None;
            }
            match prompt.kind {
                PromptKind::NewFile { dir } => {
                    let path = join_rel(&dir, &value);
                    panel.prompt = None;
                    Some(Action::CreateEntry {
                        path,
                        kind: NewEntryKind::File,
                    })
                }
                PromptKind::NewFolder { dir } => {
                    let path = join_rel(&dir, &value);
                    panel.prompt = None;
                    Some(Action::CreateEntry {
                        path,
                        kind: NewEntryKind::Dir,
                    })
                }
                PromptKind::Rename { path } => {
                    if value == path {
                        // No-op rename: just dismiss.
                        panel.prompt = None;
                        return None;
                    }
                    let to = rename_target(&path, &value);
                    panel.prompt = None;
                    Some(Action::RenameEntry { from: path, to })
                }
            }
        }
        Msg::PromptCancel => {
            panel.prompt = None;
            None
        }
    }
}

/// Expands `dir` (fetching its listing when needed) or collapses it.
fn toggle_dir(panel: &mut FilesPanel, dir: String) -> Option<Action> {
    if panel.expanded.remove(&dir) {
        return None; // was expanded: collapse
    }
    panel.expanded.insert(dir.clone());
    if panel.needs_load(&dir) {
        Some(Action::LoadDir { dir })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Center-mode body for the current state (routed with `Message::Files`).
pub fn view<'a>(
    panel: &'a FilesPanel,
    status_rows: &'a [StatusRow],
    tokens: Tokens,
) -> Element<'a, Message> {
    let states = states(status_rows);

    let mut header = row![
        text("Files").size(12).font(theme::semibold()),
        Space::new().width(Length::Fill),
        button(text("+ File").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 8])
            .on_press_maybe(new_entry_message(panel, NewEntryKind::File)),
        button(text("+ Folder").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 8])
            .on_press_maybe(new_entry_message(panel, NewEntryKind::Dir)),
        button(text("⟳").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 8])
            .on_press(Message::Files(Msg::Reload)),
        button(text("×").size(12))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([2, 8])
            .on_press(Message::Files(Msg::Toggle)),
    ]
    .align_y(Alignment::Center)
    .spacing(4);

    if !panel.dirs.contains_key("") {
        header = header.push(text("· loading…").size(10).color(tokens.muted_foreground));
    }

    let body: Element<'a, Message> = if let Some(err) = &panel.error {
        empty_state(
            err,
            if panel.dirs.contains_key("") {
                tokens.deleted
            } else {
                tokens.foreground
            },
        )
    } else if !panel.dirs.contains_key("") {
        empty_state("Loading files…", tokens.muted_foreground)
    } else {
        let rows = flatten(&panel.dirs, &panel.expanded, &states);
        if rows.is_empty() {
            empty_state("Empty working tree.", tokens.muted_foreground)
        } else {
            let mut list = column![].spacing(1).width(Length::Fill);
            for tree_row in &rows {
                list = list.push(row_view(tree_row, tokens));
            }
            scrollable(list)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    };

    let prompt_row: Element<'a, Message> = match &panel.prompt {
        Some(prompt) => prompt_view(prompt, tokens),
        None => Space::new().height(0).into(),
    };

    container(
        column![header, body, prompt_row]
            .spacing(6)
            .padding([8, 10]),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(tokens.sidebar)),
        border: Border {
            color: tokens.sidebar_border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// The root-level "+ File"/"+ Folder" intent, only when no prompt is open
/// (a second click while the prompt shows would reset the typed value).
fn new_entry_message(panel: &FilesPanel, kind: NewEntryKind) -> Option<Message> {
    if panel.prompt.is_some() {
        return None;
    }
    let kind = match kind {
        NewEntryKind::File => PromptKind::NewFile { dir: String::new() },
        NewEntryKind::Dir => PromptKind::NewFolder { dir: String::new() },
    };
    Some(Message::Files(Msg::PromptOpen(kind)))
}

fn empty_state<'a>(label: &'a str, color: Color) -> Element<'a, Message> {
    container(text(label.to_string()).size(12).color(color))
        .padding([12, 4])
        .width(Length::Fill)
        .into()
}

/// One tree row: indent, caret, name, state dot, right-click menu. All
/// content is owned, so the element is `'static`.
fn row_view(tree_row: &Row, tokens: Tokens) -> Element<'static, Message> {
    let is_dir = tree_row.is_dir();

    let caret = if is_dir {
        if tree_row.expanded {
            "▾"
        } else {
            "▸"
        }
    } else {
        "·"
    };
    let caret_color = if is_dir {
        tokens.primary
    } else {
        tokens.muted_foreground
    };

    let mut line = row![Space::new().width(Length::Fixed(tree_row.depth as f32 * INDENT_WIDTH))]
        .spacing(4)
        .align_y(Alignment::Center);
    line = line.push(
        text(caret)
            .size(9)
            .color(caret_color)
            .width(Length::Fixed(10.0)),
    );
    line = line.push(
        text(tree_row.name.clone())
            .size(11)
            .font(theme::mono())
            .color(tokens.foreground),
    );
    if matches!(tree_row.kind, DirEntryKind::Submodule) {
        line = line.push(text("(submodule)").size(9).color(tokens.muted_foreground));
    }
    if let Some(state) = tree_row.state {
        line = line.push(text("●").size(9).color(state_color(tokens, state)));
    }

    let open_msg = Message::Files(Msg::RowOpen {
        path: tree_row.path.clone(),
        is_dir,
    });

    mouse_area(
        button(container(line).padding([3, 4]).width(Length::Fill))
            .style(move |_theme, status| {
                let background = match status {
                    button::Status::Hovered => theme::mix(tokens.sidebar, tokens.hover, 0.5),
                    _ => tokens.sidebar,
                };
                button::Style {
                    background: Some(Background::Color(background)),
                    text_color: tokens.foreground,
                    border: Border::default(),
                    shadow: iced::Shadow::default(),
                    snap: true,
                }
            })
            .padding(0)
            .width(Length::Fill)
            .on_press(open_msg),
    )
    .on_right_press(Message::FilesRowMenu {
        path: tree_row.path.clone(),
        is_dir,
    })
    .into()
}

/// The inline new/rename prompt (branch-panel create-field pattern).
fn prompt_view<'a>(prompt: &'a Prompt, tokens: Tokens) -> Element<'a, Message> {
    let (placeholder, confirm_label) = match &prompt.kind {
        PromptKind::NewFile { .. } => ("new-file.txt", "Create file"),
        PromptKind::NewFolder { .. } => ("new-folder", "Create folder"),
        PromptKind::Rename { path } => (base_name(path), "Rename"),
    };
    row![
        text_input(placeholder, prompt.value.as_str())
            .on_input(|value| Message::Files(Msg::PromptChanged(value)))
            .on_submit(Message::Files(Msg::PromptConfirm))
            .size(11)
            .padding([4, 6])
            .width(Length::Fill),
        button(text(confirm_label).size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 10])
            .on_press_maybe(
                (!prompt.value.trim().trim_matches('/').is_empty())
                    .then_some(Message::Files(Msg::PromptConfirm))
            ),
        button(text("Cancel").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 10])
            .on_press(Message::Files(Msg::PromptCancel)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::status_model::SectionId;
    use crate::workspace::{DirEntryInfo, DirListing};

    fn entry(name: &str, path: &str, kind: DirEntryKind) -> DirEntryInfo {
        DirEntryInfo {
            name: name.to_string(),
            path: path.to_string(),
            kind,
            size: None,
        }
    }

    fn listing(dir: &str, entries: Vec<DirEntryInfo>) -> DirListing {
        DirListing {
            path: dir.to_string(),
            entries,
            truncated: false,
        }
    }

    fn status_row(path: &str, index: ChangeKind, worktree: ChangeKind) -> StatusRow {
        StatusRow {
            key: format!("Changes:{path}"),
            section: SectionId::Changes,
            path: path.to_string(),
            old_path: None,
            index,
            worktree,
            kind: worktree,
            dir: String::new(),
            base: String::new(),
            search: String::new(),
        }
    }

    #[test]
    fn flatten_renders_service_order_with_depth() {
        let mut dirs = HashMap::new();
        dirs.insert(
            "".to_string(),
            listing(
                "",
                vec![
                    entry("src", "src", DirEntryKind::Dir),
                    entry("README.md", "README.md", DirEntryKind::File),
                ],
            ),
        );
        dirs.insert(
            "src".to_string(),
            listing(
                "src",
                vec![entry("main.rs", "src/main.rs", DirEntryKind::File)],
            ),
        );

        let expanded: HashSet<String> = HashSet::new();
        let rows = flatten(&dirs, &expanded, &HashMap::new());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "src");
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].is_dir());
        assert_eq!(rows[1].name, "README.md");

        let expanded: HashSet<String> = ["src".to_string()].into_iter().collect();
        let rows = flatten(&dirs, &expanded, &HashMap::new());
        assert_eq!(rows.len(), 3);
        assert!(rows[0].expanded);
        assert_eq!(rows[1].path, "src/main.rs");
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn flatten_without_root_listing_is_empty() {
        let rows = flatten(&HashMap::new(), &HashSet::new(), &HashMap::new());
        assert!(rows.is_empty());
    }

    #[test]
    fn flatten_collapsed_dirs_render_without_children() {
        let mut dirs = HashMap::new();
        dirs.insert(
            "".to_string(),
            listing("", vec![entry("src", "src", DirEntryKind::Dir)]),
        );
        dirs.insert(
            "src".to_string(),
            listing("src", vec![entry("x", "src/x", DirEntryKind::File)]),
        );
        let rows = flatten(&dirs, &HashSet::new(), &HashMap::new());
        assert_eq!(rows.len(), 1, "collapsed: children hidden");
    }

    #[test]
    fn file_states_match_status_visual_language() {
        let rows = vec![
            status_row("new.txt", ChangeKind::Unmodified, ChangeKind::Untracked),
            status_row("staged.txt", ChangeKind::Modified, ChangeKind::Unmodified),
            status_row("dirty.txt", ChangeKind::Modified, ChangeKind::Modified),
            status_row("gone.txt", ChangeKind::Unmodified, ChangeKind::Deleted),
            status_row("clash.txt", ChangeKind::Conflicted, ChangeKind::Modified),
        ];
        let states = file_states(&rows);
        assert_eq!(states["new.txt"], FileState::Untracked);
        assert_eq!(states["staged.txt"], FileState::Staged);
        assert_eq!(states["dirty.txt"], FileState::Staged);
        assert_eq!(states["gone.txt"], FileState::Deleted);
        assert_eq!(states["clash.txt"], FileState::Conflicted);
    }

    #[test]
    fn dir_states_aggregate_strongest_below() {
        let mut files = HashMap::new();
        files.insert("src/old.txt", FileState::Deleted);
        files.insert("src/deep/new.txt", FileState::Untracked);
        files.insert("docs/readme.md", FileState::Modified);
        let files: HashMap<String, FileState> = files
            .into_iter()
            .map(|(path, state)| (path.to_string(), state))
            .collect();
        let dirs = aggregate_dir_states(&files);
        assert_eq!(dirs["src"], FileState::Deleted);
        assert_eq!(dirs["src/deep"], FileState::Untracked);
        assert_eq!(dirs["docs"], FileState::Modified);
    }

    #[test]
    fn states_merge_files_and_dirs_without_collision() {
        let rows = vec![status_row(
            "src/main.rs",
            ChangeKind::Unmodified,
            ChangeKind::Modified,
        )];
        let states = states(&rows);
        assert_eq!(states.get("src/main.rs"), Some(&FileState::Modified));
        assert_eq!(states.get("src"), Some(&FileState::Modified));
    }

    #[test]
    fn join_rel_handles_root_and_nested_dirs() {
        assert_eq!(join_rel("", "a.txt"), "a.txt");
        assert_eq!(join_rel("src", "a.txt"), "src/a.txt");
        assert_eq!(join_rel("src/", "a.txt"), "src/a.txt");
        assert_eq!(join_rel("src", "sub/a.txt"), "src/sub/a.txt");
        assert_eq!(join_rel("src", " / a / "), "src/a");
    }

    #[test]
    fn rename_target_resolves_in_parent() {
        assert_eq!(rename_target("src/a.txt", "b.txt"), "src/b.txt");
        assert_eq!(rename_target("a.txt", "b.txt"), "b.txt");
        assert_eq!(rename_target("src/a.txt", "sub/b.txt"), "src/sub/b.txt");
    }

    #[test]
    fn base_name_takes_last_segment() {
        assert_eq!(base_name("src/main.rs"), "main.rs");
        assert_eq!(base_name("README.md"), "README.md");
    }

    fn panel_with_root(entries: Vec<DirEntryInfo>) -> FilesPanel {
        let mut panel = FilesPanel {
            open: true,
            ..FilesPanel::default()
        };
        panel.accept_dir("", Ok(listing("", entries)));
        panel
    }

    #[test]
    fn toggle_expands_and_requests_missing_listing() {
        let mut panel = panel_with_root(vec![entry("src", "src", DirEntryKind::Dir)]);
        let dir_row = Msg::RowOpen {
            path: "src".to_string(),
            is_dir: true,
        };
        // First toggle: expand + load.
        assert_eq!(
            update(&mut panel, dir_row.clone()),
            Some(Action::LoadDir {
                dir: "src".to_string()
            })
        );
        // Second toggle: collapse, no request.
        assert_eq!(update(&mut panel, dir_row.clone()), None);
        // Expand again while the listing is still missing: another request.
        assert_eq!(
            update(&mut panel, dir_row.clone()),
            Some(Action::LoadDir {
                dir: "src".to_string()
            })
        );
        // Expand once the listing is cached: no request.
        panel.accept_dir("src", Ok(listing("src", vec![])));
        assert_eq!(update(&mut panel, dir_row), None);
    }

    #[test]
    fn toggle_flip_flops_open_state_with_opened_intent() {
        let mut panel = FilesPanel::default();
        assert_eq!(update(&mut panel, Msg::Toggle), Some(Action::Opened));
        assert!(panel.open);
        assert_eq!(update(&mut panel, Msg::Toggle), None);
        assert!(!panel.open);
    }

    #[test]
    fn row_open_routes_dirs_and_files() {
        let mut panel = panel_with_root(vec![]);
        assert_eq!(
            update(
                &mut panel,
                Msg::RowOpen {
                    path: "src".to_string(),
                    is_dir: true
                }
            ),
            Some(Action::LoadDir {
                dir: "src".to_string()
            })
        );
        assert_eq!(
            update(
                &mut panel,
                Msg::RowOpen {
                    path: "main.rs".to_string(),
                    is_dir: false
                }
            ),
            Some(Action::OpenEditor {
                path: "main.rs".to_string()
            })
        );
    }

    #[test]
    fn accept_dir_stores_listing_and_reports_errors() {
        let mut panel = FilesPanel {
            loading: ["src".to_string()].into_iter().collect(),
            ..FilesPanel::default()
        };
        panel.accept_dir("src", Err("boom".to_string()));
        assert!(panel.loading.is_empty(), "in-flight mark cleared");
        assert_eq!(panel.error.as_deref(), Some("boom"));

        panel.accept_dir("src", Ok(listing("src", vec![])));
        assert!(panel.dirs.contains_key("src"));
        assert!(panel.error.is_none());
    }

    #[test]
    fn reload_invalidates_but_keeps_expanded() {
        let mut panel = panel_with_root(vec![entry("src", "src", DirEntryKind::Dir)]);
        panel.expanded.insert("src".to_string());
        assert_eq!(update(&mut panel, Msg::Reload), Some(Action::Reload));
        assert!(panel.dirs.is_empty());
        assert!(panel.expanded.contains("src"));
    }

    #[test]
    fn prompt_confirm_creates_entries() {
        let mut panel = FilesPanel {
            prompt: Some(Prompt {
                kind: PromptKind::NewFile {
                    dir: "src".to_string(),
                },
                value: "lib.rs".to_string(),
            }),
            ..FilesPanel::default()
        };
        assert_eq!(
            update(&mut panel, Msg::PromptConfirm),
            Some(Action::CreateEntry {
                path: "src/lib.rs".to_string(),
                kind: NewEntryKind::File
            })
        );
        assert!(panel.prompt.is_none());
    }

    #[test]
    fn prompt_confirm_renames_within_parent() {
        let mut panel = FilesPanel {
            prompt: Some(Prompt {
                kind: PromptKind::Rename {
                    path: "src/main.rs".to_string(),
                },
                value: "app.rs".to_string(),
            }),
            ..FilesPanel::default()
        };
        assert_eq!(
            update(&mut panel, Msg::PromptConfirm),
            Some(Action::RenameEntry {
                from: "src/main.rs".to_string(),
                to: "src/app.rs".to_string()
            })
        );
    }

    #[test]
    fn prompt_confirm_rejects_empty_or_slashed_names() {
        let mut panel = FilesPanel {
            prompt: Some(Prompt {
                kind: PromptKind::NewFile { dir: String::new() },
                value: "  ".to_string(),
            }),
            ..FilesPanel::default()
        };
        assert_eq!(update(&mut panel, Msg::PromptConfirm), None);
        panel.prompt = Some(Prompt {
            kind: PromptKind::NewFile { dir: String::new() },
            value: "a//b".to_string(),
        });
        assert_eq!(update(&mut panel, Msg::PromptConfirm), None);
        assert!(panel.prompt.is_some(), "invalid input keeps the prompt");
    }

    #[test]
    fn prompt_cancel_clears() {
        let mut panel = FilesPanel {
            prompt: Some(Prompt {
                kind: PromptKind::NewFolder { dir: String::new() },
                value: String::new(),
            }),
            ..FilesPanel::default()
        };
        assert_eq!(update(&mut panel, Msg::PromptCancel), None);
        assert!(panel.prompt.is_none());
    }

    #[test]
    fn prompt_open_prefills_rename_and_clears_creates() {
        let mut panel = FilesPanel::default();
        update(
            &mut panel,
            Msg::PromptOpen(PromptKind::Rename {
                path: "src/main.rs".to_string(),
            }),
        );
        assert_eq!(
            panel.prompt,
            Some(Prompt {
                kind: PromptKind::Rename {
                    path: "src/main.rs".to_string()
                },
                value: "main.rs".to_string(),
            })
        );

        update(&mut panel, Msg::PromptCancel);
        update(
            &mut panel,
            Msg::PromptOpen(PromptKind::NewFile {
                dir: "docs".to_string(),
            }),
        );
        assert_eq!(
            panel.prompt,
            Some(Prompt {
                kind: PromptKind::NewFile {
                    dir: "docs".to_string()
                },
                value: String::new(),
            })
        );
    }
}
