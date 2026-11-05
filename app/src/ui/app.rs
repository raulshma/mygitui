//! Application shell: boot, update, view and the backend-bus subscription.
//!
//! One [`AppServices`] bag is shared through `Arc`; async service bodies run
//! as `Task::perform` futures on the iced (tokio) executor and report back
//! through messages. Push-style signals (watcher batches, op progress,
//! log pages, clones, prompts) arrive over the
//! [`BackendEvent`](crate::service::events) bus subscription.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{button, column, container, row, scrollable, text, text_editor, Space};
use iced::{window, Alignment, Element, Length, Subscription, Task};

use crate::engine::types::{
    BranchInfo, BranchTrashEntry, ChangeKind, CheckpointInfo, CommitOptions, ConflictResolution,
    DiffSide, FetchOptions, FileDiff, LineRange, MergeFavor, MergeOutcome, MergeResult,
    MergetoolInfo, MergetoolResult, NetStats, PreviewInfo, PullOptions, PushOptions, RebaseState,
    RebaseStep, RemoteBranchInfo, RemoteInfo, RepoId, RepoInfo, RepoStatus, ResetKind,
    StageRequest, StageTarget, StashInfo,
};
use crate::remote::prompts::SecretAnswer;
use crate::service::events::BackendEvent;
use crate::service::AppServices;

use super::actions_panel;
use super::ai_settings;
use super::bisect_panel;
use super::blame_view;
use super::bus;
use super::clone_dialog;
use super::compare_view;
use super::config::{self, Config, RecentRepo};
use super::context_menu;
use super::devices;
use super::dialog::{self, ResetRequest};
use super::diff_model;
use super::diff_view::{self, DiffPane, DiffSlot, PaneMode};
use super::editor_view;
use super::file_history_view;
use super::files_panel;
use super::forge_panel;
use super::health_panel;
use super::history_model::{to_log_filter, FilterFields, HistoryFilterField};
use super::home;
use super::output_panel;
use super::palette;
use super::prompts::{self, AuthPromptState, DeviceSecretPromptState, HostKeyPromptState};
use super::quick_open;
use super::quick_switcher;
use super::rebase_planner;
use super::reflog_panel;
use super::remote_panel::net_summary;
use super::repo_view::{self, Load, PendingRebase, PendingRemap, Tab};
use super::stats_panel;
use super::status_model::{count_label, filter_sections, ignore_variants, SectionId, StatusRow};
use super::submodule_panel;
use super::tags_panel;
use super::terminal;
use super::theme::{self, Mode, Tokens};
use super::worktree_panel;

/// Minimum main-window size (also the clamp floor for a restored,
/// hand-edited or zeroed persisted size).
const MIN_WINDOW_SIZE: (f32, f32) = (900.0, 600.0);

/// First-run window settings (the fixed fallbacks; persisted geometry is
/// seeded through [`window_settings`]).
fn main_window_settings() -> window::Settings {
    window_settings(&config::WindowFrame::default())
}

/// Main-window settings seeded from the persisted geometry: size always,
/// position only when the platform reported one (Wayland never does — the
/// platform default is used instead), maximized as observed. Sizes below
/// the minimum clamp up so a degenerate config stays valid.
fn window_settings(frame: &config::WindowFrame) -> window::Settings {
    window::Settings {
        size: iced::Size::new(
            frame.size[0].max(MIN_WINDOW_SIZE.0),
            frame.size[1].max(MIN_WINDOW_SIZE.1),
        ),
        maximized: frame.maximized,
        position: frame
            .position
            .map(|[x, y]| window::Position::Specific(iced::Point::new(x, y)))
            .unwrap_or(window::Position::Default),
        min_size: Some(iced::Size::new(MIN_WINDOW_SIZE.0, MIN_WINDOW_SIZE.1)),
        icon: window::icon::from_file_data(include_bytes!("../../icons/64x64.png"), None).ok(),
        ..window::Settings::default()
    }
}

pub fn run(
    cli_args: Vec<String>,
    instance_listener: crate::instance::SharedListener,
) -> iced::Result {
    // Daemon (multi-window) mode: the primary window is opened from boot so
    // diff popouts can be plain extra windows routed by `window::Id`.
    iced::daemon(
        move || App::boot(cli_args.clone(), instance_listener.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .run()
}

pub struct App {
    services: Arc<AppServices>,
    config: Config,
    /// Sorted copy of `config.recent_repos` kept in sync with the config.
    recent: Vec<RecentRepo>,
    open_in_flight: bool,
    system_mode: iced::theme::Mode,
    modifiers: iced::keyboard::Modifiers,
    /// Monotonic request id for log streams (discards superseded pages).
    log_seq: u64,
    /// Monotonic request id for file-history streams (bus routing).
    file_history_seq: u64,
    tabs: Vec<Tab>,
    active: usize,
    /// Session-restore queue: repo paths still to reopen at boot, chained
    /// one open at a time through `RepoOpened` (empty when there is nothing
    /// to restore or the run was started with an explicit repo argument).
    restore_queue: Vec<String>,
    /// Session tab index to focus once the restore queue drains.
    restore_active: usize,
    /// Debounced window-geometry persistence (main window): the latest
    /// observed frame plus a generation counter collapsing move/resize
    /// bursts into one save (see [`config::WindowSave`]).
    window_save: config::WindowSave,
    show_home: bool,
    notice: Option<Notice>,
    last_op: Option<String>,
    /// Op currently reporting progress, for the footer cancel button.
    active_op: Option<ActiveOp>,
    /// Cancel flag of the running clone (flip to abort the transfer; the
    /// footer shows Cancel while this is set).
    clone_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Single-instance accept loop handle, moved into its task at boot.
    instance_listener: Option<crate::instance::InstanceListener>,
    /// Open AI result dialog (review/explain/branch-name output).
    ai_result: Option<AiResult>,
    /// Open diff popout: its window id and a snapshot clone of the diff
    /// pane (own scroll state; dropped when the window closes).
    popout: Option<(window::Id, DiffPane)>,
    /// Update found by the latest check: (tag, release URL). Enables the
    /// palette's install command until the check is superseded.
    update_ready: Option<(String, String)>,
    /// User-configured custom shell actions (persisted as `actions.json`
    /// next to `config.json`; separate file so schema evolution stays
    /// independent).
    custom_actions: Vec<actions_panel::ActionEntry>,
    auth_prompts: std::collections::VecDeque<AuthPromptState>,
    host_key_prompts: std::collections::VecDeque<HostKeyPromptState>,
    device_secret_prompts: std::collections::VecDeque<DeviceSecretPromptState>,
    /// Command palette overlay state (`ctrl/cmd+shift+p`).
    palette: palette::State,
    /// Right-click context menu state (one menu at a time).
    ctx_menu: context_menu::State,
    /// Repo quick-switcher overlay state (`ctrl/cmd+k`).
    switcher: quick_switcher::State,
    /// Clone-repository dialog state (Home button / `repo.clone` palette).
    clone: clone_dialog::State,
    /// SSH device management (Home section, add/edit dialog, device browser).
    devices: devices::State,
    /// AI settings dialog (draft provider/model/URL fields + keyring edits).
    ai_settings: ai_settings::State,
    /// Last `service::ai::ai_status` probe (header AI chip; refreshed on
    /// boot, dialog open, settings save and the AI toggles — never polled).
    ai_status: Option<crate::service::ai::AiStatus>,
    /// Error text of the last failed AI call (cleared by the next success;
    /// drives the header chip's `AI error: …` state).
    ai_last_error: Option<String>,
    /// The main window's id (focus transitions gate the remote poll pause).
    main_window: Option<window::Id>,
    /// Quick Open overlay state (`ctrl/cmd+P`).
    quick_open: quick_open::State,
    /// Last known cursor position, used to anchor context menus opened via
    /// `mouse_area` right-presses (which do not carry coordinates).
    cursor: iced::Point,
    /// Keyboard-shortcuts help overlay (F1 / palette "Keyboard shortcuts").
    help_open: bool,
}

/// The op whose progress the footer is showing (cancelable via `op_cancel`).
struct ActiveOp {
    repo_id: RepoId,
    op_id: String,
    kind: String,
}

struct Notice {
    text: String,
    error: bool,
    /// Optional URL rendered as an `Open` footer button (release links).
    link: Option<String>,
}

/// In-progress merge/rebase/sequencer control pressed in the repo header.
#[derive(Debug, Clone, Copy)]
pub enum InProgressAction {
    AbortMerge,
    ContinueRebase,
    AbortRebase,
    AbortSequencer,
}

impl InProgressAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::AbortMerge => "Abort merge",
            Self::ContinueRebase => "Continue rebase",
            Self::AbortRebase => "Abort rebase",
            Self::AbortSequencer => "Abort sequencer",
        }
    }
}

impl Notice {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
            link: None,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
            link: None,
        }
    }

    fn with_link(text: impl Into<String>, link: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
            link: Some(link.into()),
        }
    }
}

/// The AI affordances the shell exposes; each maps to one
/// `service::ai` generation.
/// Content of the AI result dialog.
#[derive(Debug, Clone)]
pub struct AiResult {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiAction {
    CommitMessage,
    ReviewStaged,
    StashMessage,
    PrTitleBody,
    BranchName,
}

#[derive(Debug, Clone)]
pub enum Message {
    Backend(BackendEvent),
    SystemTheme(iced::theme::Mode),
    Modifiers(iced::keyboard::Modifiers),
    /// A fixed shell shortcut (`F5`, `Ctrl/Cmd+1..8`, `Ctrl/Cmd+S`,
    /// `Ctrl/Cmd+Enter`, `Ctrl/Cmd+Alt+Left/Right`, `Ctrl/Cmd+W`) mapped by
    /// the global key handler; dispatched with the overlay guard in
    /// [`App::update`].
    Shortcut(ShellShortcut),
    OpenDialog,
    OpenPicked(Option<PathBuf>),
    OpenPath(String),
    RepoOpened {
        path: String,
        result: Result<RepoInfo, String>,
    },
    StatusLoaded {
        repo_id: RepoId,
        result: Result<RepoStatus, String>,
    },
    Refresh(RepoId),
    SelectTab(usize),
    CloseTab(RepoId),
    GoHome,
    CycleTheme,
    CycleAutoFetch,
    AutoFetchTick(RepoId),
    AutoFetched {
        repo_id: RepoId,
        result: Result<Option<NetStats>, String>,
    },
    CancelOp,
    /// Footer cancel for a running clone (flips its transfer flag).
    CloneCancel,
    OpCanceled {
        kind: String,
        result: Result<bool, String>,
    },
    TogglePin(String),
    RemoveRecent(String),
    DismissNotice,

    StatusFilterChanged(String),
    StatusSelect {
        key: String,
    },
    StatusSelectAll,
    StatusSelectNone,
    ToggleSection(SectionId),
    StagePaths {
        paths: Vec<String>,
        unstage: bool,
    },
    StageFile {
        path: String,
        unstage: bool,
    },
    StageHunk {
        slot: DiffSlot,
        file: usize,
        hunk: usize,
    },
    StageSelectedLines {
        slot: DiffSlot,
    },
    ClearLineSelection {
        slot: DiffSlot,
    },
    DiscardPath(String),
    DiscardPaths(Vec<String>),
    MutationDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },

    DiffToggleSplit,
    DiffToggleWrap,
    DiffToggleFile {
        slot: DiffSlot,
        file: usize,
    },
    DiffToggleLine {
        slot: DiffSlot,
        file: usize,
        hunk: usize,
        new_no: u32,
    },
    DiffScrolled {
        slot: DiffSlot,
        y: f32,
        viewport: f32,
    },
    DiffLoaded {
        repo_id: RepoId,
        slot: DiffSlot,
        key: String,
        seq: u64,
        result: Result<Vec<FileDiff>, String>,
    },

    CommitMessageChanged(text_editor::Action),
    CommitAmend(bool),
    CommitNoVerify(bool),
    CommitPressed,
    Committed {
        repo_id: RepoId,
        result: Result<String, String>,
    },

    SelectCommit(String),
    CloseDetail,
    ToggleHistory,
    HistoryScrolled {
        y: f32,
        viewport: f32,
    },
    RefreshLog(RepoId),
    HistoryFilterToggle,
    HistoryFilterChanged {
        field: HistoryFilterField,
        value: String,
    },
    HistoryFilterRegex(bool),
    HistoryFilterFollow(bool),
    HistoryFilterPickaxeRegex(bool),
    HistoryFilterApply,
    HistoryFilterClear,

    ToggleBranchPanel,
    RefreshBranches,
    BranchesLoaded {
        repo_id: RepoId,
        result: Result<Vec<BranchInfo>, String>,
    },
    BranchCreateNameChanged(String),
    BranchCreate,
    BranchSwitch(String),
    BranchDeleteRequested(String),
    BranchDeleteConfirmed,
    BranchDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },

    ToggleStashPanel,
    RefreshStashes,
    StashesLoaded {
        repo_id: RepoId,
        result: Result<Vec<StashInfo>, String>,
    },
    StashMessageChanged(String),
    StashIncludeUntracked(bool),
    StashKeepIndex(bool),
    StashPush,
    StashApply {
        index: u32,
        pop: bool,
    },
    StashDropRequested(u32),
    StashDropConfirmed,
    StashesDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },
    CheckpointsLoaded {
        repo_id: RepoId,
        result: Result<Vec<CheckpointInfo>, String>,
    },
    CheckpointCreate,
    CheckpointRestoreRequested {
        id: String,
        reason: String,
    },
    CheckpointRestoreConfirmed,
    CheckpointsDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },
    /// Palette/stash-panel: delete checkpoints older than 30 days.
    CheckpointGc,
    /// Checkpoint GC finished with the number of deleted checkpoints.
    CheckpointsGcDone {
        repo_id: RepoId,
        result: Result<u32, String>,
    },
    /// Refresh the deleted-branch trash list (stash panel open/refresh).
    RefreshTrash,
    /// Trash list loaded.
    TrashLoaded {
        repo_id: RepoId,
        result: Result<Vec<BranchTrashEntry>, String>,
    },
    /// Restore one trashed branch by id.
    TrashRestore(String),
    /// Trash restore finished with the restored branch name (a name
    /// conflict surfaces as an error here).
    TrashRestoreDone {
        repo_id: RepoId,
        result: Result<String, String>,
    },
    ResetRequested(String),
    ResetKindSelected(ResetKind),
    ResetPreviewLoaded {
        repo_id: RepoId,
        result: Result<PreviewInfo, String>,
    },
    ResetConfirmed,
    ResetDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },

    ToggleRemotePanel,
    RefreshRemotes,
    RemotesLoaded {
        repo_id: RepoId,
        result: Result<Vec<RemoteInfo>, String>,
    },
    RemoteNameChanged(String),
    RemoteUrlChanged(String),
    RemoteAdd,
    FetchRemote(String),
    /// Open the pull options dialog for a remote.
    PullRemote(String),
    PullBranchChanged(String),
    PullRebaseToggled(bool),
    PullFfOnlyToggled(bool),
    PullConfirmed,
    /// Open the push options dialog for a remote.
    PushRemote(String),
    PushForceToggled(bool),
    /// Keystrokes into the force-push typed-confirm field.
    PushConfirmChanged(String),
    PushForceLeaseToggled(bool),
    PushTagsToggled(bool),
    PushConfirmed,
    RemoteRemoveRequested(String),
    RemoteRemoveConfirmed,
    RemotesDone {
        repo_id: RepoId,
        label: String,
        result: Result<String, String>,
    },

    /// Open the merge confirm dialog for a branch (into the current one).
    MergeBranch(String),
    MergeNoFfSelected(bool),
    MergeSquashToggled(bool),
    MergeNoCommitToggled(bool),
    MergeFavorSelected(MergeFavor),
    MergeConfirmed,
    MergeDone {
        repo_id: RepoId,
        label: String,
        result: Result<MergeResult, String>,
    },
    RebaseOntoRequested(String),
    RebasePlanLoaded {
        repo_id: RepoId,
        onto: String,
        result: Result<Vec<RebaseStep>, String>,
    },
    RebaseConfirmed,
    RebaseDone {
        repo_id: RepoId,
        result: Result<RebaseState, String>,
    },
    /// Open the interactive rebase planner for rebasing onto a commit
    /// (commit row context menu).
    RebasePlannerRequested(String),
    /// The planner's initial pick plan (`rebase_plan(onto..HEAD)`) loaded.
    RebasePlannerPlanLoaded {
        repo_id: RepoId,
        onto: String,
        result: Result<Vec<RebaseStep>, String>,
    },
    /// The planner's autosquash rebuild (`autosquash_plan(onto..HEAD)`)
    /// loaded; replaces the authored rows.
    RebasePlannerAutosquashLoaded {
        repo_id: RepoId,
        onto: String,
        result: Result<Vec<RebaseStep>, String>,
    },
    /// Planner dialog interactions (routed into `rebase_planner::Planner`).
    RebasePlanner(rebase_planner::Msg),
    InProgress(InProgressAction),
    ConflictResolve {
        paths: Vec<String>,
        resolution: ConflictResolution,
    },
    RepoActionDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },

    /// Open the "Run mergetool" dialog for the current conflicts (status
    /// panel header + palette `conflicts.mergetool`).
    MergetoolRequested,
    /// The configured-tool read (`mergetool_info`) finished.
    MergetoolInfoLoaded {
        repo_id: RepoId,
        result: Result<MergetoolInfo, String>,
    },
    /// One conflicted path was picked in the mergetool dialog.
    MergetoolPathSelected(usize),
    /// Run `git mergetool` on the selected conflicted path.
    MergetoolRun,
    /// The mergetool run finished; the dialog closes and the repo refreshes
    /// (the conflict list is re-read through the regular status load).
    MergetoolDone {
        repo_id: RepoId,
        path: String,
        result: Result<MergetoolResult, String>,
    },

    AuthUsernameChanged(String),
    AuthPasswordChanged(String),
    AuthStoreToggled(bool),
    AuthPromptSubmit,
    AuthPromptCancel,
    HostKeyAnswer(bool),
    DeviceSecretChanged(String),
    DeviceSecretStoreToggled(bool),
    DeviceSecretSubmit,
    DeviceSecretCancel,
    Noop,

    ConfirmDiscard,
    CancelDialog,

    // Interaction overlays: command palette, context menu, quick-switcher.
    Palette(palette::Msg),
    ContextMenu(context_menu::Msg),
    Switcher(quick_switcher::Msg),
    /// Quick Open overlay (`ctrl/cmd+P`, routed from its own key listener).
    QuickOpen(quick_open::Msg),
    /// Quick Open file index fetched for the active repo.
    QuickOpenIndexLoaded {
        repo_id: RepoId,
        result: Result<crate::workspace::FileIndex, String>,
    },
    /// Clone-repository dialog (fields, folder pick, submit).
    Clone(clone_dialog::Msg),
    /// SSH device management (Home section, add/edit dialog, device browser;
    /// routed through `devices::update` → `devices::Action`).
    Devices(devices::Msg),
    /// A device connect/disconnect finished (footer notice + status events
    /// update the Home rows).
    DeviceConnectDone {
        device_id: String,
        result: Result<String, String>,
    },
    /// A device forget (profile + secrets delete) finished.
    DeviceForgetDone {
        device_id: String,
        result: Result<(), String>,
    },
    /// `remote_repo_open` finished: on success the device repo opens as a
    /// tab (the device browser overlay closes).
    DeviceRepoOpened {
        path: String,
        result: Result<RepoInfo, String>,
    },
    /// A window gained/lost focus; the main window's transitions pause/resume
    /// remote repo change polling (devices keep dialing only while the app
    /// is looked at).
    WindowFocus {
        window: window::Id,
        focused: bool,
    },
    /// Main-window move (window-geometry persistence; popouts ignored).
    WindowMoved {
        window: window::Id,
        position: iced::Point,
    },
    /// Main-window resize (window-geometry persistence; popouts ignored).
    WindowResized {
        window: window::Id,
        size: iced::Size,
    },
    /// A debounced window-geometry save timer fired (generation `seq`).
    WindowSaveTick(u64),
    /// The maximized-state query for a due window-geometry save finished.
    WindowSaveMaximized {
        seq: u64,
        maximized: bool,
    },
    /// Native folder-picker result for the clone destination (None =
    /// canceled).
    ClonePicked(Option<PathBuf>),
    /// Clone finished: on success the destination opens as a repo tab.
    CloneDone {
        destination: String,
        result: Result<String, String>,
    },
    /// Track the cursor so right-press menus can be positioned.
    CursorMoved(iced::Point),
    /// Started dragging a workspace pane divider (mouse press on a strip).
    PaneDragStart(repo_view::PaneDivider),
    /// Toggle the keyboard-shortcuts help overlay (F1 / palette).
    ToggleHelp,
    /// Mouse release after a divider drag: persist the pane sizes.
    PaneDragEnd,
    /// Embedded terminal pane (routed to the active tab).
    Terminal(terminal::Msg),
    /// Show/hide the terminal dock.
    ToggleTerminal,
    /// GitHub PR/CI panel (routed to the active tab).
    Forge(forge_panel::Msg),
    /// Bisect stepper (routed to the active tab).
    Bisect(bisect_panel::Msg),
    /// Run an AI generation for the active tab.
    AiGenerate(AiAction),
    /// Explain one hunk (context text assembled by the diff view).
    AiExplainHunk {
        path: String,
        hunk: String,
    },
    /// AI result dialog closing.
    AiResultClose,
    /// Copy arbitrary text (AI results) to the clipboard.
    CopyText(String),
    /// Palette: query the latest GitHub release.
    CheckUpdates,
    /// Update-check completion.
    UpdateChecked(Result<crate::update_check::Update, String>),
    /// Palette: download, verify and install the update found by a check.
    InstallUpdate,
    /// Install completion (only reached on failure — success restarts
    /// inside the updater).
    UpdateInstallDone(Result<String, String>),
    /// Palette: toggle the startup update check.
    ToggleUpdateCheck,
    /// Open a URL in the OS default handler (update check, docs links).
    OpenUrl(String),
    /// Toggle the AI master switch (palette, AI settings dialog).
    AiToggleEnabled,
    /// Toggle opt-in for the active repo (palette, AI settings dialog).
    AiToggleRepoOptIn,
    /// Open the AI settings dialog (header chip, palette `app.ai_settings`).
    AiOpenSettings,
    /// AI settings dialog (routed through `ai_settings::update` → `Action`).
    AiSettings(ai_settings::Msg),
    /// `service::ai::ai_status` probe result for the header AI chip.
    AiStatusLoaded(crate::service::ai::AiStatus),
    /// Generation completions.
    AiCommitMessageDone(Result<String, String>),
    AiReviewDone(Result<String, String>),
    AiExplainDone {
        path: String,
        result: Result<String, String>,
    },
    AiStashMessageDone(Result<String, String>),
    AiPrTextDone(Result<String, String>),
    AiBranchNameDone(Result<String, String>),
    /// Reflog panel (routed to the active tab).
    Reflog(reflog_panel::Msg),
    /// Repo health/maintenance panel (routed to the active tab).
    Health(health_panel::Msg),
    /// Worktree/submodule/tag panels (routed to the active tab).
    Worktree(worktree_panel::Msg),
    /// Contribution stats panel (routed to the active tab).
    Stats(stats_panel::Msg),
    /// OS accent colour fetched (boot + theme changes).
    AccentLoaded(Option<String>),
    /// Window bookkeeping (daemon mode): any window closed.
    WindowClosed(window::Id),
    /// Diff-viewer messages for the popout window.
    Popout(Box<Message>),
    /// Pop out the active diff pane into its own window.
    PopoutDiff,
    /// Custom actions / output / editor (routed to the active tab).
    Actions(actions_panel::Msg),
    /// A custom action started; feeds the panels and opens the output.
    ActionStarted {
        name: String,
        run_id: String,
    },
    /// Set the footer notice from an arbitrary task.
    Notice(String),
    Output(output_panel::Msg),
    Editor(editor_view::Msg),
    /// Center-pane read views (routed to the active tab).
    Blame(blame_view::Msg),
    /// One compare file's diff finished loading.
    CompareFileDiffLoaded {
        slot: DiffSlot,
        result: Result<Vec<FileDiff>, String>,
    },
    FileHistory(file_history_view::Msg),
    Compare(compare_view::Msg),
    /// Files panel (routed to the active tab).
    Files(files_panel::Msg),
    /// One directory listing finished loading.
    FilesDirLoaded {
        repo_id: RepoId,
        dir: String,
        result: Result<crate::workspace::DirListing, String>,
    },
    /// A files-panel mutation (create/rename/delete) finished.
    FilesEntryDone {
        repo_id: RepoId,
        label: String,
        result: Result<(), String>,
    },
    /// Right-click on a Files panel row (repository-relative path).
    FilesRowMenu {
        path: String,
        is_dir: bool,
    },
    /// Confirmed files-panel delete (dialog).
    FilesDeleteConfirmed,
    Submodule(submodule_panel::Msg),
    Tags(tags_panel::Msg),
    /// Open the ⋯ header menu (worktrees/submodules/tags toggles).
    MoreMenu,
    /// Right-click on a status row (row key).
    StatusRowMenu {
        key: String,
    },
    /// Right-click on a local branch row.
    BranchRowMenu {
        name: String,
    },
    /// Right-click on a history commit row (sha).
    CommitRowMenu {
        sha: String,
    },
    /// Copy a commit SHA to the clipboard (commit context menu).
    CopySha(String),
    /// Cherry-pick a commit onto the current branch.
    CherryPick(String),
    /// Revert a commit on the current branch.
    RevertCommit(String),
    /// One image-diff side finished loading (blob or workdir read).
    ImageLoaded {
        repo_id: RepoId,
        slot: DiffSlot,
        path: String,
        old: bool,
        seq: u64,
        result: Result<Vec<u8>, String>,
    },
    /// Append a path pattern to `.gitignore` (status-row context menu).
    IgnorePath(String),
    /// `.gitignore` append finished.
    Ignored {
        repo_id: RepoId,
        result: Result<(), String>,
    },
    /// Open the gitignore template gallery (status panel + palette).
    IgnoreTemplatesRequested,
    /// Gallery search box changed.
    IgnoreTemplatesFilterChanged(String),
    /// Apply one builtin template by name (gallery row).
    IgnoreTemplateApplied(String),
    /// Template apply finished (the gallery stays open).
    TemplateApplied {
        repo_id: RepoId,
        result: Result<(), String>,
    },
    /// Open the clean-untracked dialog (status panel + palette).
    CleanDialogRequested,
    /// Toggle one path's checkbox in the clean dialog.
    CleanToggled(String),
    /// The clean dialog's typed confirmation changed.
    CleanConfirmChanged(String),
    /// Run the confirmed clean (checkpointed by the service).
    CleanConfirmed,
    /// Clean finished with the number of removed paths.
    CleanDone {
        repo_id: RepoId,
        result: Result<u32, String>,
    },
    /// Open the branch rename dialog (branch row context menu).
    BranchRenameRequested(String),
    /// The rename dialog's target name changed.
    BranchRenameChanged(String),
    /// Run the confirmed rename.
    BranchRenameConfirmed,
    /// Remote-tracking branch list loaded (branch panel).
    RemoteBranchesLoaded {
        repo_id: RepoId,
        result: Result<Vec<RemoteBranchInfo>, String>,
    },
    /// Check out a remote branch into a local one (confirms when the
    /// working tree is dirty).
    RemoteBranchCheckout {
        remote: String,
        name: String,
    },
    /// The dirty-checkout confirm was accepted.
    RemoteBranchCheckoutConfirmed,
    /// Dirty-tree confirm accepted: switch with the changes in place.
    LocalCheckoutConfirmed,
    /// Dirty-tree confirm: stash first, then switch to the pending branch.
    LocalCheckoutStashAndSwitch,
    /// Remote checkout finished with the checked-out local branch.
    RemoteCheckoutDone {
        repo_id: RepoId,
        result: Result<String, String>,
    },
    /// Delete a remote branch (push delete; confirmed by a dialog).
    RemoteBranchDeleteRequested {
        remote: String,
        name: String,
    },
    /// The delete confirm was accepted.
    RemoteBranchDeleteConfirmed,
    /// Remote branch deletion finished.
    RemoteBranchDeleteDone {
        repo_id: RepoId,
        result: Result<NetStats, String>,
    },
    /// Open the "tag here" dialog for a commit (commit row context menu).
    TagAtRequested(String),
    /// Tag-at-commit form fields.
    TagAtNameChanged(String),
    TagAtAnnotatedToggled(bool),
    TagAtMessageChanged(String),
    /// Run the confirmed tag create at the dialog's commit.
    TagAtConfirmed,
    /// Tag creation finished.
    TagAtDone {
        repo_id: RepoId,
        name: String,
        result: Result<(), String>,
    },
    /// Copy the raw text of the diff pane's selected lines.
    CopySelectedLines {
        slot: DiffSlot,
    },
    /// Copy `git describe --tags` for a commit (commit context menu).
    CopyDescribe(String),
    /// Describe finished; on success the result lands on the clipboard.
    DescribeCopied(Result<String, String>),
}

/// Terminal sessions carry their kind in the id: `pty-*` is a local PTY,
/// `rshell-*` an SSH shell over a device session (both emit the same
/// `PtyOutput`/`PtyExit` events the terminal pane consumes).
fn is_remote_shell_session(session: &str) -> bool {
    session.starts_with("rshell-")
}

/// One background auto-fetch cycle for a repo: pick the default remote
/// (`origin`, else the first configured) and fetch with prune. `Ok(None)`
/// means nothing to do (no remotes) and is never surfaced as an error.
async fn auto_fetch_once(
    services: &AppServices,
    repo_id: &RepoId,
) -> Result<Option<NetStats>, String> {
    let remotes = crate::service::network::remotes(services, repo_id.clone()).await?;
    let remote = remotes
        .iter()
        .find(|remote| remote.name == "origin")
        .or_else(|| remotes.first())
        .map(|remote| remote.name.clone());
    let Some(remote) = remote else {
        return Ok(None);
    };
    crate::service::network::fetch(
        services,
        repo_id.clone(),
        FetchOptions {
            remote,
            prune: true,
            ..FetchOptions::default()
        },
    )
    .await
    .map(Some)
}

impl App {
    fn boot(
        cli_args: Vec<String>,
        instance_listener: crate::instance::SharedListener,
    ) -> (Self, Task<Message>) {
        crate::auth::init();
        let services = Arc::new(AppServices::new());
        let config = Config::load(&services.config_dir);
        let highlight_enabled = config.highlight;
        let accent_enabled = config.accent;
        let services_boot = services.clone();
        let recent = config.recent_sorted();
        let window_save = config::WindowSave::new(config.window);
        let mut app = Self {
            services,
            config,
            recent,
            open_in_flight: false,
            system_mode: iced::theme::Mode::None,
            modifiers: iced::keyboard::Modifiers::default(),
            log_seq: 0,
            file_history_seq: 0,
            tabs: Vec::new(),
            active: 0,
            restore_queue: Vec::new(),
            restore_active: 0,
            window_save,
            show_home: true,
            notice: None,
            last_op: None,
            active_op: None,
            clone_cancel: None,
            help_open: false,
            instance_listener: None,
            auth_prompts: std::collections::VecDeque::new(),
            host_key_prompts: std::collections::VecDeque::new(),
            device_secret_prompts: std::collections::VecDeque::new(),
            palette: palette::State::default(),
            ctx_menu: context_menu::State::default(),
            switcher: quick_switcher::State::default(),
            clone: clone_dialog::State::default(),
            devices: devices::State::default(),
            ai_settings: ai_settings::State::default(),
            ai_status: None,
            ai_last_error: None,
            main_window: None,
            quick_open: quick_open::State::default(),
            cursor: iced::Point::ORIGIN,
            ai_result: None,
            popout: None,
            update_ready: None,
            custom_actions: Vec::new(),
        };
        app.custom_actions = app.load_custom_actions();

        // Session restore: reopen the persisted tabs unless this launch
        // carries an explicit repo argument (argv / deep link wins for that
        // run). Paths that no longer exist are dropped here; paths that fail
        // to open are dropped silently by the `RepoOpened` arm. Device-backed
        // tabs were never persisted (see `config::session_state`).
        let restore = if first_repo_arg(&cli_args).is_none() && !app.config.session.tabs.is_empty()
        {
            let mut seen = HashSet::new();
            let queue: Vec<String> = app
                .config
                .session
                .tabs
                .iter()
                .filter(|path| std::path::Path::new(path).exists())
                .filter(|path| seen.insert(config::canonical_key(path)))
                .cloned()
                .collect();
            app.restore_active = app.config.session.active.min(queue.len().saturating_sub(1));
            app.restore_queue = queue;
            !app.restore_queue.is_empty()
        } else {
            false
        };

        let instance_listener = crate::instance::take_listener(&instance_listener);
        app.instance_listener = instance_listener;
        // Polish toggles from config (highlight defaults on; the accent
        // colour itself arrives async via AccentLoaded).
        super::diff_view::highlight::set_enabled(highlight_enabled);
        theme::set_accent_enabled(accent_enabled);
        let mut tasks = vec![iced::system::theme().map(Message::SystemTheme)];
        {
            // Geometry from the last run (fixed fallbacks on first run).
            let (main_window, opened) = window::open(window_settings(&app.config.window));
            app.main_window = Some(main_window);
            tasks.push(opened.map(|_| Message::Noop));
        }
        if let Some(path) = first_repo_arg(&cli_args) {
            tasks.push(app.open_repo(path));
        } else if restore {
            // Restore opens chain one at a time: the next open is issued by
            // the `RepoOpened` arm when the previous one settles.
            if let Some(path) = app.restore_queue.first().cloned() {
                tasks.push(app.open_repo(path));
            }
        }
        tasks.push(app.refresh_devices());
        // Header AI chip seed (one-shot probe; refreshed on toggles/save).
        tasks.push(app.refresh_ai_status());
        tasks.push(Task::perform(
            async move {
                let accent = crate::service::misc::os_accent_color(&services_boot).await;
                Message::AccentLoaded(accent.unwrap_or(None))
            },
            std::convert::identity,
        ));
        if app.config.update_check_on_start {
            tasks.push(Task::perform(
                async move { Message::UpdateChecked(crate::update_check::check().await) },
                std::convert::identity,
            ));
        }
        if let Some(listener) = app.instance_listener.take() {
            // Never-ending accept loop; forwarded argv arrives as
            // BackendEvent::CliArgs through the bus subscription.
            tasks.push(Task::perform(
                async move {
                    crate::instance::serve(listener).await;
                    Message::Noop
                },
                std::convert::identity,
            ));
        }
        (app, Task::batch(tasks))
    }

    fn is_dark(&self) -> bool {
        match self.config.theme {
            Mode::Light => false,
            Mode::Dark => true,
            Mode::System => self.system_mode == iced::theme::Mode::Dark,
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens::for_scheme(self.is_dark())
    }

    /// Reads `actions.json` (custom shell actions); missing or malformed
    /// files fall back to an empty list.
    fn load_custom_actions(&self) -> Vec<actions_panel::ActionEntry> {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Entry {
            name: String,
            command: String,
        }
        let path = self.services.config_dir.join("actions.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<Entry>>(&text)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| actions_panel::ActionEntry::new(entry.name, entry.command))
            .collect()
    }

    /// Writes `actions.json` atomically (temp file + rename, matching the
    /// config save discipline).
    fn persist_custom_actions(&self) {
        #[derive(serde::Serialize)]
        struct Entry {
            name: String,
            command: String,
        }
        let entries: Vec<Entry> = self
            .custom_actions
            .iter()
            .map(|action| Entry {
                name: action.name.clone(),
                command: action.command.clone(),
            })
            .collect();
        let path = self.services.config_dir.join("actions.json");
        let Ok(json) = serde_json::to_string_pretty(&entries) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn persist_config(&mut self) {
        if let Err(err) = self.config.save(&self.services.config_dir) {
            self.notice = Some(Notice::error(format!("Could not save settings: {err}")));
        }
    }

    /// Records a main-window geometry change and (re)arms the trailing
    /// save timer; each new generation supersedes the previous tick, so a
    /// drag-resize storm collapses into one config write.
    fn arm_window_save(&mut self, frame: config::WindowFrame) -> Task<Message> {
        if !self.window_save.note_change(frame) {
            return Task::none();
        }
        let seq = self.window_save.seq;
        Task::perform(
            async move {
                tokio::time::sleep(config::WINDOW_SAVE_DEBOUNCE).await;
                Message::WindowSaveTick(seq)
            },
            std::convert::identity,
        )
    }

    /// Persists the debounced geometry (generation `seq`) with the queried
    /// maximized state; stale generations are dropped.
    fn finish_window_save(&mut self, seq: u64, maximized: bool) -> Task<Message> {
        if !self.window_save.due(seq) {
            return Task::none();
        }
        self.window_save.mark_saved(seq);
        let mut frame = self.window_save.frame;
        frame.maximized = maximized;
        self.config.window = frame;
        self.persist_config();
        Task::none()
    }

    fn reindex_recent(&mut self) {
        self.recent = self.config.recent_sorted();
    }

    /// One-shot `service::ai::ai_status` probe for the header AI chip
    /// (boot, dialog open, settings save, AI toggles — no polling).
    fn refresh_ai_status(&self) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move { Message::AiStatusLoaded(crate::service::ai::ai_status(&services).await) },
            std::convert::identity,
        )
    }

    /// Reads the OpenRouter key's presence from the OS keyring for the
    /// settings dialog (`None` when the probe itself failed).
    fn load_ai_key_status(&self) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let has_key = tokio::task::spawn_blocking(move || {
                    crate::keyring_store::secrets_get(
                        &services,
                        crate::service::ai::KEY_OPENROUTER_API.to_string(),
                    )
                    .ok()
                    .flatten()
                    .is_some()
                })
                .await
                .ok();
                Message::AiSettings(ai_settings::Msg::KeyStatusLoaded(has_key))
            },
            std::convert::identity,
        )
    }

    /// Tracks the last AI call outcome for the header chip (error text only
    /// — successes reset it; result payloads are handled by the callers).
    fn note_ai_result(&mut self, result: &Result<String, String>) {
        match result {
            Ok(_) => self.ai_last_error = None,
            Err(error) => self.ai_last_error = Some(error.clone()),
        }
    }

    /// Palette command registry, rebuilt whenever the palette opens so
    /// availability reflects the current shell state.
    fn commands(&self) -> palette::CommandSet {
        let tab = self.tabs.get(self.active);
        let repo_id = tab.map(|tab| tab.repo.repo_id.clone());
        let staged: Vec<String> = tab
            .map(|tab| {
                Self::visible_rows(tab)
                    .into_iter()
                    .filter(|row| row.section == SectionId::Staged)
                    .map(|row| row.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let changes: Vec<String> = tab
            .map(|tab| {
                Self::visible_rows(tab)
                    .into_iter()
                    .filter(|row| row.section == SectionId::Changes)
                    .map(|row| row.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let untracked: Vec<String> = tab
            .map(|tab| {
                Self::visible_rows(tab)
                    .into_iter()
                    .filter(|row| {
                        row.section == SectionId::Changes && row.kind == ChangeKind::Untracked
                    })
                    .map(|row| row.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let head_branch: Option<String> = tab.and_then(|tab| tab.head_branch().map(str::to_string));
        let conflicted: Vec<String> = tab
            .map(|tab| {
                Self::visible_rows(tab)
                    .into_iter()
                    .filter(|row| row.kind == ChangeKind::Conflicted)
                    .map(|row| row.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let remotes: Vec<String> = tab
            .and_then(|tab| tab.remotes.list.as_ref())
            .map(|list| list.iter().map(|remote| remote.name.clone()).collect())
            .unwrap_or_default();

        let mut set = palette::CommandSet::new();
        let mut register =
            |id: &str, label: &str, section: &str, enabled: bool, keybind: Option<&str>| {
                let mut command =
                    palette::Command::new(id.to_string(), label.to_string(), section.to_string());
                if let Some(keybind) = keybind {
                    command = command.keybind(keybind);
                }
                if !enabled {
                    command = command.disabled();
                }
                set.register(command);
            };

        register(
            "repo.open",
            "Open repository…",
            "Repository",
            true,
            Some("Ctrl+O"),
        );
        register("repo.clone", "Clone repository…", "Repository", true, None);
        register(
            "repo.add_device",
            "Add SSH device…",
            "Repository",
            true,
            None,
        );
        register(
            "repo.devices",
            "Manage SSH devices…",
            "Repository",
            true,
            None,
        );
        register(
            "repo.quick_switch",
            "Quick-switch repository…",
            "Repository",
            true,
            Some("Ctrl+K"),
        );
        register(
            "repo.home",
            "Go home",
            "Repository",
            !self.tabs.is_empty(),
            None,
        );
        register(
            "repo.close_tab",
            "Close repository tab",
            "Repository",
            repo_id.is_some(),
            Some("Ctrl+W"),
        );
        register(
            "repo.refresh",
            "Refresh status",
            "Repository",
            repo_id.is_some(),
            Some("F5"),
        );

        register(
            "status.stage_all",
            "Stage all changes",
            "Working copy",
            !changes.is_empty(),
            None,
        );
        register(
            "status.unstage_all",
            "Unstage all changes",
            "Working copy",
            !staged.is_empty(),
            None,
        );
        register(
            "status.select_all",
            "Select all files",
            "Working copy",
            repo_id.is_some(),
            None,
        );
        register(
            "status.select_none",
            "Clear file selection",
            "Working copy",
            repo_id.is_some(),
            None,
        );
        register(
            "status.ignore_templates",
            "Ignore templates…",
            "Working copy",
            repo_id.is_some(),
            None,
        );
        register(
            "status.clean",
            "Clean untracked files…",
            "Working copy",
            !untracked.is_empty(),
            None,
        );
        register(
            "conflicts.mergetool",
            "Run mergetool…",
            "Working copy",
            !conflicted.is_empty(),
            None,
        );

        register(
            "commit.commit",
            "Commit staged changes",
            "Commit",
            repo_id.is_some(),
            Some("Ctrl+Enter"),
        );

        register(
            "diff.toggle_split",
            "Toggle split diff",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "diff.toggle_wrap",
            "Toggle diff line wrap",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "history.toggle",
            "Toggle history panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+4"),
        );
        register(
            "history.filter",
            "Toggle history filters",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "history.refresh",
            "Refresh history",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "branches.toggle",
            "Toggle branches panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+1"),
        );
        register(
            "branches.copy_current",
            "Copy current branch name",
            "Repository",
            head_branch.is_some(),
            None,
        );
        register(
            "stash.toggle",
            "Toggle stashes panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+2"),
        );
        register(
            "remotes.toggle",
            "Toggle remotes panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+3"),
        );
        register(
            "view.toggle_terminal",
            "Toggle terminal panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+8"),
        );
        register(
            "view.toggle_forge",
            "Toggle forge panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+5"),
        );
        register(
            "bisect.toggle",
            "Toggle bisect stepper",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "reflog.toggle",
            "Toggle reflog panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+7"),
        );
        register(
            "health.toggle",
            "Toggle repo health panel",
            "View",
            repo_id.is_some(),
            Some("Ctrl+6"),
        );
        register(
            "files.quick_open",
            "Go to file…",
            "View",
            repo_id.is_some(),
            Some("Ctrl+P"),
        );
        register(
            "view.files",
            "Toggle files panel",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "view.compare",
            "Compare two refs",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "panel.actions",
            "Toggle custom actions panel",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "panel.output",
            "Toggle action output panel",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "panel.stats",
            "Toggle contribution stats",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "view.popout_diff",
            "Pop out diff into a window",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "terminal.restart",
            "Restart terminal",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "app.shortcuts",
            "Keyboard shortcuts",
            "App",
            true,
            Some("F1"),
        );

        register(
            "network.fetch",
            "Fetch all remotes",
            "Network",
            !remotes.is_empty(),
            None,
        );
        register("network.pull", "Pull", "Network", remotes.len() == 1, None);
        register("network.push", "Push", "Network", remotes.len() == 1, None);

        register(
            "stash.push",
            "Stash changes",
            "Safety net",
            repo_id.is_some(),
            None,
        );
        register(
            "checkpoints.create",
            "Create checkpoint",
            "Safety net",
            repo_id.is_some(),
            None,
        );
        register(
            "checkpoints.gc",
            "GC checkpoints (older than 30 days)",
            "Safety net",
            repo_id.is_some(),
            None,
        );
        register(
            "ops.cancel",
            "Cancel running operation",
            "Safety net",
            self.active_op.is_some(),
            None,
        );

        register(
            "ai.commit_message",
            "AI: generate commit message",
            "AI",
            repo_id.is_some(),
            None,
        );
        register(
            "ai.review_staged",
            "AI: review staged changes",
            "AI",
            repo_id.is_some(),
            None,
        );
        register(
            "ai.stash_message",
            "AI: suggest stash message",
            "AI",
            repo_id.is_some(),
            None,
        );
        register(
            "ai.pr_text",
            "AI: PR title & description",
            "AI",
            repo_id.is_some(),
            None,
        );
        register(
            "ai.branch_name",
            "AI: suggest branch name",
            "AI",
            repo_id.is_some(),
            None,
        );
        register(
            "ai.toggle_enabled",
            "AI: toggle master switch",
            "AI",
            true,
            None,
        );
        register(
            "ai.toggle_repo_opt_in",
            "AI: toggle opt-in for this repo",
            "AI",
            repo_id.is_some(),
            None,
        );
        register(
            "app.check_updates",
            "Check for updates",
            "Settings",
            true,
            None,
        );
        register(
            "app.install_update",
            "Install available update",
            "Settings",
            self.update_ready.is_some(),
            None,
        );
        register(
            "app.toggle_update_check",
            "Toggle update check on startup",
            "Settings",
            true,
            None,
        );
        register("app.ai_settings", "AI settings…", "Settings", true, None);
        register("view.cycle_theme", "Cycle theme", "Settings", true, None);
        register(
            "view.cycle_autofetch",
            "Cycle auto-fetch interval",
            "Settings",
            true,
            None,
        );

        set
    }

    /// Quick-switcher entries from the persisted recent/pinned repo list.
    fn switcher_entries(&self) -> Vec<quick_switcher::Entry> {
        self.recent
            .iter()
            .map(|recent| {
                quick_switcher::Entry::new(recent.path.clone())
                    .pinned(recent.pinned)
                    .last_opened((recent.last_opened != 0).then_some(recent.last_opened))
            })
            .collect()
    }

    /// Executes a command by registry id (palette or context-menu origin).
    fn run_command(&mut self, id: &str) -> Task<Message> {
        match id {
            "repo.open" => self.update(Message::OpenDialog),
            "repo.clone" => self.update(Message::Clone(clone_dialog::Msg::Open)),
            "repo.add_device" => self.update(Message::Devices(devices::Msg::AddDevice)),
            "repo.devices" => {
                // Focus the Home devices section: expand it, switch to Home
                // and refresh so the connection statuses are current.
                self.devices.expanded = true;
                Task::batch([self.update(Message::GoHome), self.refresh_devices()])
            }
            "repo.quick_switch" => self.update(Message::Switcher(quick_switcher::Msg::Open)),
            "repo.home" => self.update(Message::GoHome),
            "repo.close_tab" => match self.tabs.get(self.active) {
                Some(tab) => self.update(Message::CloseTab(tab.repo.repo_id.clone())),
                None => Task::none(),
            },
            "repo.refresh" => match self.tabs.get(self.active) {
                Some(tab) => self.update(Message::Refresh(tab.repo.repo_id.clone())),
                None => Task::none(),
            },
            "status.stage_all" => {
                let paths = self
                    .tabs
                    .get(self.active)
                    .map(|tab| {
                        Self::visible_rows(tab)
                            .into_iter()
                            .filter(|row| row.section == SectionId::Changes)
                            .map(|row| row.path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                self.update(Message::StagePaths {
                    paths,
                    unstage: false,
                })
            }
            "status.unstage_all" => {
                let paths = self
                    .tabs
                    .get(self.active)
                    .map(|tab| {
                        Self::visible_rows(tab)
                            .into_iter()
                            .filter(|row| row.section == SectionId::Staged)
                            .map(|row| row.path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                self.update(Message::StagePaths {
                    paths,
                    unstage: true,
                })
            }
            "status.select_all" => self.update(Message::StatusSelectAll),
            "status.select_none" => self.update(Message::StatusSelectNone),
            "status.ignore_templates" => self.update(Message::IgnoreTemplatesRequested),
            "status.clean" => self.update(Message::CleanDialogRequested),
            "conflicts.mergetool" => self.update(Message::MergetoolRequested),
            "commit.commit" => self.update(Message::CommitPressed),
            "diff.toggle_split" => self.update(Message::DiffToggleSplit),
            "diff.toggle_wrap" => self.update(Message::DiffToggleWrap),
            "history.toggle" => self.update(Message::ToggleHistory),
            "history.filter" => self.update(Message::HistoryFilterToggle),
            "history.refresh" => match self.tabs.get(self.active) {
                Some(tab) => self.update(Message::RefreshLog(tab.repo.repo_id.clone())),
                None => Task::none(),
            },
            "branches.toggle" => self.update(Message::ToggleBranchPanel),
            "branches.copy_current" => {
                match self.tabs.get(self.active).and_then(|tab| tab.head_branch()) {
                    Some(name) => self.update(Message::CopyText(name.to_string())),
                    None => Task::none(),
                }
            }
            "stash.toggle" => self.update(Message::ToggleStashPanel),
            "remotes.toggle" => self.update(Message::ToggleRemotePanel),
            "view.toggle_terminal" => self.update(Message::ToggleTerminal),
            "view.toggle_forge" => self.update(Message::Forge(forge_panel::Msg::ToggleOpen)),
            "bisect.toggle" => self.update(Message::Bisect(bisect_panel::Msg::ToggleOpen)),
            "reflog.toggle" => self.update(Message::Reflog(reflog_panel::Msg::ToggleOpen)),
            "health.toggle" => self.update(Message::Health(health_panel::Msg::ToggleOpen)),
            "panel.worktrees" => self.update(Message::Worktree(worktree_panel::Msg::ToggleOpen)),
            "panel.submodules" => self.update(Message::Submodule(submodule_panel::Msg::ToggleOpen)),
            "panel.tags" => self.update(Message::Tags(tags_panel::Msg::ToggleOpen)),
            "panel.actions" => self.update(Message::Actions(actions_panel::Msg::ToggleOpen)),
            "app.shortcuts" => self.update(Message::ToggleHelp),
            "panel.output" => self.update(Message::Output(output_panel::Msg::ToggleOpen)),
            "panel.stats" => self.update(Message::Stats(stats_panel::Msg::ToggleOpen)),
            "view.popout_diff" => self.update(Message::PopoutDiff),
            "files.quick_open" => self.update(Message::QuickOpen(quick_open::Msg::Open)),
            "view.files" => self.update(Message::Files(files_panel::Msg::Toggle)),
            "view.compare" => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.blame.open = false;
                    tab.file_history.open = false;
                    tab.compare.open = true;
                }
                Task::none()
            }
            "terminal.restart" => self.update(Message::Terminal(terminal::Msg::RestartPressed)),
            "network.fetch" => {
                let remotes: Vec<String> = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.remotes.list.as_ref())
                    .map(|list| list.iter().map(|remote| remote.name.clone()).collect())
                    .unwrap_or_default();
                let mut tasks = Vec::new();
                for name in remotes {
                    tasks.push(self.update(Message::FetchRemote(name)));
                }
                Task::batch(tasks)
            }
            "network.pull" => self.first_remote_task(Message::PullRemote),
            "network.push" => self.first_remote_task(Message::PushRemote),
            "stash.push" => self.update(Message::StashPush),
            "checkpoints.create" => self.update(Message::CheckpointCreate),
            "checkpoints.gc" => self.update(Message::CheckpointGc),
            "ops.cancel" => self.update(Message::CancelOp),
            "ai.commit_message" => self.update(Message::AiGenerate(AiAction::CommitMessage)),
            "ai.review_staged" => self.update(Message::AiGenerate(AiAction::ReviewStaged)),
            "ai.stash_message" => self.update(Message::AiGenerate(AiAction::StashMessage)),
            "ai.pr_text" => self.update(Message::AiGenerate(AiAction::PrTitleBody)),
            "ai.branch_name" => self.update(Message::AiGenerate(AiAction::BranchName)),
            "ai.toggle_enabled" => self.update(Message::AiToggleEnabled),
            "ai.toggle_repo_opt_in" => self.update(Message::AiToggleRepoOptIn),
            "app.check_updates" => self.update(Message::CheckUpdates),
            "app.install_update" => self.update(Message::InstallUpdate),
            "app.toggle_update_check" => self.update(Message::ToggleUpdateCheck),
            "app.ai_settings" => self.update(Message::AiOpenSettings),
            "view.cycle_theme" => self.update(Message::CycleTheme),
            "view.cycle_autofetch" => self.update(Message::CycleAutoFetch),
            // Context-menu commands carry their target after a `:` prefix.
            other if other.starts_with("status.stage:") => self.update(Message::StagePaths {
                paths: vec![other["status.stage:".len()..].to_string()],
                unstage: false,
            }),
            other if other.starts_with("status.unstage:") => self.update(Message::StagePaths {
                paths: vec![other["status.unstage:".len()..].to_string()],
                unstage: true,
            }),
            other if other.starts_with("status.discard:") => self.update(Message::DiscardPath(
                other["status.discard:".len()..].to_string(),
            )),
            other if other.starts_with("status.ignore:") => self.update(Message::IgnorePath(
                other["status.ignore:".len()..].to_string(),
            )),
            other if other.starts_with("status.edit:") => {
                let path = other["status.edit:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.blame.open = false;
                    tab.file_history.open = false;
                    tab.compare.open = false;
                }
                self.update(Message::Editor(editor_view::Msg::OpenRequested(path)))
            }
            other if other.starts_with("status.file_history:") => {
                let path = other["status.file_history:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.blame.open = false;
                    tab.compare.open = false;
                }
                self.update(Message::FileHistory(file_history_view::Msg::Opened {
                    path,
                }))
            }
            other if other.starts_with("status.blame:") => {
                let path = other["status.blame:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.file_history.open = false;
                    tab.compare.open = false;
                }
                self.update(Message::Blame(blame_view::Msg::Opened { path }))
            }
            other if other.starts_with("files.open:") => {
                let path = other["files.open:".len()..].to_string();
                match self.tabs.get(self.active) {
                    Some(tab) => self.run_files_action(
                        tab.repo.repo_id.clone(),
                        Some(files_panel::Action::OpenEditor { path }),
                    ),
                    None => Task::none(),
                }
            }
            other if other.starts_with("files.history:") => {
                let path = other["files.history:".len()..].to_string();
                match self.tabs.get(self.active) {
                    Some(tab) => self.run_files_action(
                        tab.repo.repo_id.clone(),
                        Some(files_panel::Action::OpenHistory { path }),
                    ),
                    None => Task::none(),
                }
            }
            other if other.starts_with("files.copy:") => {
                let rel = other["files.copy:".len()..].to_string();
                let abs = self
                    .tabs
                    .get(self.active)
                    .map(|tab| std::path::Path::new(&tab.repo.root).join(&rel));
                match abs {
                    Some(abs) => {
                        self.last_op = Some(format!("Copied path to {rel}"));
                        iced::clipboard::write::<Message>(abs.to_string_lossy().into_owned())
                    }
                    None => Task::none(),
                }
            }
            other if other.starts_with("files.external:") => {
                let path = other["files.external:".len()..].to_string();
                self.open_file_external(&path)
            }
            other if other.starts_with("files.new_file:") => {
                let dir = other["files.new_file:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    files_panel::update(
                        &mut tab.files,
                        files_panel::Msg::PromptOpen(files_panel::PromptKind::NewFile { dir }),
                    );
                }
                Task::none()
            }
            other if other.starts_with("files.new_folder:") => {
                let dir = other["files.new_folder:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    files_panel::update(
                        &mut tab.files,
                        files_panel::Msg::PromptOpen(files_panel::PromptKind::NewFolder { dir }),
                    );
                }
                Task::none()
            }
            other if other.starts_with("files.rename:") => {
                let path = other["files.rename:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    files_panel::update(
                        &mut tab.files,
                        files_panel::Msg::PromptOpen(files_panel::PromptKind::Rename { path }),
                    );
                }
                Task::none()
            }
            other if other.starts_with("files.delete:") => {
                let path = other["files.delete:".len()..].to_string();
                match self.tabs.get(self.active) {
                    Some(tab) => self.run_files_action(
                        tab.repo.repo_id.clone(),
                        Some(files_panel::Action::DeleteEntry { path }),
                    ),
                    None => Task::none(),
                }
            }
            other if other.starts_with("branch.switch:") => self.update(Message::BranchSwitch(
                other["branch.switch:".len()..].to_string(),
            )),
            other if other.starts_with("branch.rename:") => self.update(
                Message::BranchRenameRequested(other["branch.rename:".len()..].to_string()),
            ),
            other if other.starts_with("branch.delete:") => self.update(
                Message::BranchDeleteRequested(other["branch.delete:".len()..].to_string()),
            ),
            other if other.starts_with("commit.copy:") => {
                self.update(Message::CopySha(other["commit.copy:".len()..].to_string()))
            }
            other if other.starts_with("commit.copy_describe:") => self.update(
                Message::CopyDescribe(other["commit.copy_describe:".len()..].to_string()),
            ),
            other if other.starts_with("commit.tag:") => self.update(Message::TagAtRequested(
                other["commit.tag:".len()..].to_string(),
            )),
            other if other.starts_with("commit.cherry_pick:") => self.update(Message::CherryPick(
                other["commit.cherry_pick:".len()..].to_string(),
            )),
            other if other.starts_with("commit.revert:") => self.update(Message::RevertCommit(
                other["commit.revert:".len()..].to_string(),
            )),
            other if other.starts_with("commit.reset:") => self.update(Message::ResetRequested(
                other["commit.reset:".len()..].to_string(),
            )),
            other if other.starts_with("commit.rebase:") => self.update(
                Message::RebasePlannerRequested(other["commit.rebase:".len()..].to_string()),
            ),
            other if other.starts_with("commit.branch:") => {
                let sha = other["commit.branch:".len()..].to_string();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.branches.create_from = Some(sha);
                    tab.branches.open = true;
                }
                Task::none()
            }
            other => {
                self.notice = Some(Notice::error(format!("Unknown command: {other}")));
                Task::none()
            }
        }
    }

    /// Dispatches a per-remote network message for the sole configured
    /// remote (pull/push are single-upstream operations in this UI).
    fn first_remote_task(&mut self, make: impl Fn(String) -> Message) -> Task<Message> {
        match self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.remotes.list.as_ref())
            .and_then(|list| list.first())
            .map(|remote| remote.name.clone())
        {
            Some(name) => self.update(make(name)),
            None => Task::none(),
        }
    }

    /// Executes a terminal pane action against the PTY backend. All PTY
    /// calls are cheap and synchronous; errors mean the session is already
    /// gone and are dropped at debug level.
    fn run_terminal_action(&mut self, action: Option<terminal::Action>) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            terminal::Action::Spawn { repo_root } => self.spawn_terminal(repo_root),
            terminal::Action::Restart {
                old_session,
                repo_root,
            } => {
                if let Some(old) = old_session {
                    if let Err(err) = self.kill_terminal_session(&old) {
                        tracing::debug!(err, "terminal restart: old session kill failed");
                    }
                }
                self.spawn_terminal(repo_root)
            }
            terminal::Action::Write { session, data } => {
                let result = if is_remote_shell_session(&session) {
                    crate::remote::term::rshell_write(&self.services, session, data)
                } else {
                    crate::pty::pty_write(&self.services, session, data)
                };
                if let Err(err) = result {
                    tracing::debug!(err, "pty write failed");
                }
                Task::none()
            }
            terminal::Action::Resize {
                session,
                cols,
                rows,
            } => {
                let result = if is_remote_shell_session(&session) {
                    crate::remote::term::rshell_resize(&self.services, session, rows, cols)
                } else {
                    crate::pty::pty_resize(&self.services, session, rows, cols)
                };
                if let Err(err) = result {
                    tracing::debug!(err, "pty resize failed");
                }
                Task::none()
            }
            terminal::Action::Kill { session } => {
                if let Err(err) = self.kill_terminal_session(&session) {
                    tracing::debug!(err, "pty kill failed");
                }
                Task::none()
            }
            terminal::Action::FollowScroll { id } => {
                terminal::snap_to_bottom(id).map(Message::Terminal)
            }
        }
    }

    /// Kills a terminal session of either kind (local `pty-*` PTY or remote
    /// `rshell-*` SSH shell); errors mean the session is already gone.
    fn kill_terminal_session(&self, session: &str) -> Result<(), String> {
        if is_remote_shell_session(session) {
            crate::remote::term::rshell_kill(&self.services, session.to_string())
        } else {
            crate::pty::pty_kill(&self.services, session.to_string())
        }
    }

    /// Spawns the shell session for the tab whose root matches (the active
    /// tab in practice) and feeds the returned session id back to its pane.
    /// Remote tabs (device repos) spawn an SSH shell over the device session
    /// instead of a local PTY; the pane consumes both unchanged.
    fn spawn_terminal(&mut self, repo_root: PathBuf) -> Task<Message> {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.repo.root == repo_root.to_string_lossy())
        else {
            return Task::none();
        };
        let (cols, rows) = self.tabs[index].terminal.dims();
        let repo_id = self.tabs[index].repo.repo_id.clone();
        let remote = self
            .services
            .repos
            .get(&repo_id)
            .is_some_and(|handle| handle.is_remote());
        let spawned = if remote {
            crate::remote::term::rshell_create(&self.services, repo_id, Some(rows), Some(cols))
        } else {
            crate::pty::pty_create(&self.services, repo_id, Some(rows), Some(cols))
        };
        match spawned {
            Ok(session) => {
                let follow = terminal::update(
                    &mut self.tabs[index].terminal,
                    terminal::Msg::Opened(session),
                );
                self.run_terminal_action(follow)
            }
            Err(err) => {
                self.notice = Some(Notice::error(format!("Could not start terminal: {err}")));
                Task::none()
            }
        }
    }

    /// Releases terminal keyboard focus everywhere (another typing surface
    /// took over: tab switch, commit editor, filter inputs…).
    fn blur_terminals(&mut self) {
        for tab in &mut self.tabs {
            tab.terminal.blur();
        }
    }

    /// Executes a forge panel action against `service::forge`. Results are
    /// reported back through the panel's own `*Loaded` / `ActionDone`
    /// messages; checkout and merge additionally refresh the repo (branch
    /// changed / branch deleted).
    fn run_forge_action(
        &mut self,
        repo_id: RepoId,
        action: Option<forge_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            forge_panel::Action::Refresh { pr_state } => {
                let services = self.services.clone();
                let services2 = self.services.clone();
                let services3 = self.services.clone();
                let repo_id2 = repo_id.clone();
                let repo_id3 = repo_id.clone();
                Task::batch([
                    Task::perform(
                        async move {
                            Message::Forge(forge_panel::Msg::StatusLoaded(
                                crate::service::forge::forge_status(&services).await,
                            ))
                        },
                        std::convert::identity,
                    ),
                    Task::perform(
                        async move {
                            Message::Forge(forge_panel::Msg::ContextLoaded(
                                crate::service::forge::forge_context(&services2, repo_id2).await,
                            ))
                        },
                        std::convert::identity,
                    ),
                    Task::perform(
                        async move {
                            Message::Forge(forge_panel::Msg::PrsLoaded {
                                result: crate::service::forge::pr_list(
                                    &services3, repo_id3, pr_state,
                                )
                                .await,
                            })
                        },
                        std::convert::identity,
                    ),
                ])
            }
            forge_panel::Action::LoadPrDetails {
                number,
                head_branch,
            } => {
                let services = self.services.clone();
                let services2 = self.services.clone();
                let repo_id2 = repo_id.clone();
                let repo_id3 = repo_id.clone();
                Task::batch([
                    Task::perform(
                        async move {
                            Message::Forge(forge_panel::Msg::ChecksLoaded {
                                number,
                                result: crate::service::forge::pr_checks(
                                    &services, repo_id2, number,
                                )
                                .await,
                            })
                        },
                        std::convert::identity,
                    ),
                    Task::perform(
                        async move {
                            let result = crate::service::forge::workflow_runs(
                                &services2,
                                repo_id3,
                                None,
                                (!head_branch.is_empty()).then_some(head_branch),
                                None,
                                None,
                            )
                            .await;
                            Message::Forge(forge_panel::Msg::RunsLoaded { result })
                        },
                        std::convert::identity,
                    ),
                ])
            }
            forge_panel::Action::OpenUrl(url) => Task::perform(
                async move {
                    let _ = tokio::task::spawn_blocking(move || open::that_detached(&url)).await;
                    Message::Noop
                },
                std::convert::identity,
            ),
            forge_panel::Action::CheckoutPr { number } => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                let checkout = Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: "Checkout".to_string(),
                            result: crate::service::forge::pr_checkout(&services, repo_id2, number)
                                .await,
                        })
                    },
                    std::convert::identity,
                );
                let refresh = self.refresh(repo_id);
                checkout.chain(refresh)
            }
            forge_panel::Action::MergePr {
                number,
                method,
                delete_branch,
            } => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                let merge = Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: "Merge".to_string(),
                            result: crate::service::forge::pr_merge(
                                &services,
                                repo_id2,
                                number,
                                method.as_str().to_string(),
                                delete_branch,
                                false,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                );
                let refresh = self.refresh(repo_id);
                merge.chain(refresh)
            }
            forge_panel::Action::ClosePr { number } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: "Close".to_string(),
                            result: crate::service::forge::pr_close(
                                &services, repo_id, number, None,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::ReopenPr { number } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: "Reopen".to_string(),
                            result: crate::service::forge::pr_reopen(
                                &services, repo_id, number, None,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::MarkReady { number, ready } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: "Mark ready".to_string(),
                            result: crate::service::forge::pr_ready(
                                &services, repo_id, number, ready,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::CommentPr { number, body } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: "Comment".to_string(),
                            result: crate::service::forge::pr_comment(
                                &services, repo_id, number, body,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::ReviewPr {
                number,
                action,
                body,
            } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::ActionDone {
                            number: Some(number),
                            label: action.label().to_string(),
                            result: crate::service::forge::pr_review(
                                &services,
                                repo_id,
                                number,
                                action.as_str().to_string(),
                                body,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::OpenRunLogs { run_id } => {
                let services = self.services.clone();
                let services2 = self.services.clone();
                let repo_id2 = repo_id.clone();
                Task::batch([
                    Task::perform(
                        async move {
                            Message::Forge(forge_panel::Msg::RunDetailLoaded {
                                run_id,
                                result: crate::service::forge::workflow_run_detail(
                                    &services, repo_id2, run_id,
                                )
                                .await
                                .map(Box::new),
                            })
                        },
                        std::convert::identity,
                    ),
                    Task::perform(
                        async move {
                            let result = crate::service::forge::workflow_run_logs(
                                &services2, repo_id, run_id, None, false,
                            )
                            .await;
                            Message::Forge(forge_panel::Msg::RunLogsLoaded { run_id, result })
                        },
                        std::convert::identity,
                    ),
                ])
            }
            forge_panel::Action::CancelRun { run_id } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::RunActionDone {
                            run_id,
                            label: "Cancel run".to_string(),
                            result: crate::service::forge::workflow_run_cancel(
                                &services, repo_id, run_id,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::RerunRun {
                run_id,
                failed_only,
            } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::RunActionDone {
                            run_id,
                            label: if failed_only {
                                "Rerun failed".to_string()
                            } else {
                                "Rerun".to_string()
                            },
                            result: crate::service::forge::workflow_run_rerun(
                                &services,
                                repo_id,
                                run_id,
                                failed_only,
                            )
                            .await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::RefreshRuns { branch, reopen } => {
                let services = self.services.clone();
                let runs = Task::perform(
                    async move {
                        let result = crate::service::forge::workflow_runs(
                            &services,
                            repo_id,
                            None,
                            (!branch.is_empty()).then_some(branch),
                            None,
                            None,
                        )
                        .await;
                        Message::Forge(forge_panel::Msg::RunsLoaded { result })
                    },
                    std::convert::identity,
                );
                match reopen {
                    // The open run's detail re-loads right behind the list.
                    Some(run_id) => {
                        let reopen = Task::perform(
                            async move { Message::Forge(forge_panel::Msg::RunLogsPressed(run_id)) },
                            std::convert::identity,
                        );
                        runs.chain(reopen)
                    }
                    None => runs,
                }
            }
            forge_panel::Action::LoadWorkflows => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::WorkflowsLoaded {
                            result: crate::service::forge::workflow_list(&services, repo_id).await,
                        })
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::DispatchWorkflow {
                workflow,
                git_ref,
                inputs,
            } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::DispatchDone(
                            crate::service::forge::workflow_dispatch(
                                &services, repo_id, workflow, git_ref, inputs,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            forge_panel::Action::CreatePr {
                base,
                head,
                title,
                body,
            } => {
                // `gh pr create` always uses the checked-out branch as head;
                // refuse anything else instead of silently ignoring it.
                let current = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| match &tab.status {
                        Load::Ready(status) => status.branch.clone(),
                        _ => None,
                    })
                    .unwrap_or_default();
                if !head.is_empty() && head != current {
                    self.notice = Some(Notice::error(format!(
                        "PR head must be the checked-out branch ({current})"
                    )));
                    return Task::none();
                }
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Forge(forge_panel::Msg::PrCreated(
                            crate::service::forge::pr_create(
                                &services, repo_id, base, title, body, false,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
        }
    }

    /// Fetches the repo's bisect state (cheap JSON read) into the panel.
    fn load_bisect_state(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                Message::Bisect(bisect_panel::Msg::StateLoaded(
                    crate::service::merge::bisect_state(&services, repo_id).await,
                ))
            },
            std::convert::identity,
        )
    }

    /// Fetches the bisect mark-history log into the panel.
    fn load_bisect_log(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                Message::Bisect(bisect_panel::Msg::LogLoaded(
                    crate::service::merge::bisect_log(&services, repo_id)
                        .await
                        .unwrap_or_default(),
                ))
            },
            std::convert::identity,
        )
    }

    /// Executes a bisect panel action against `service::merge`.
    fn run_bisect_action(
        &mut self,
        repo_id: RepoId,
        action: Option<bisect_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            bisect_panel::Action::Refresh => self.load_bisect_state(repo_id),
            bisect_panel::Action::Start { bad, good } => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                let start = Task::perform(
                    async move {
                        Message::Bisect(bisect_panel::Msg::StateLoaded(
                            crate::service::merge::bisect_start(
                                &services,
                                repo_id2,
                                (!bad.trim().is_empty()).then_some(bad),
                                good,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                );
                let log = self.load_bisect_log(repo_id);
                start.chain(log)
            }
            bisect_panel::Action::Mark(mark) => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                let mark = Task::perform(
                    async move {
                        Message::Bisect(bisect_panel::Msg::StateLoaded(
                            crate::service::merge::bisect_mark(&services, repo_id2, mark).await,
                        ))
                    },
                    std::convert::identity,
                );
                let log = self.load_bisect_log(repo_id);
                mark.chain(log)
            }
            bisect_panel::Action::Reset => {
                let services = self.services.clone();
                let services2 = self.services.clone();
                let repo_id2 = repo_id.clone();
                let repo_id3 = repo_id.clone();
                let reset = Task::perform(
                    async move {
                        Message::Bisect(bisect_panel::Msg::StateLoaded(
                            match crate::service::merge::bisect_reset(&services, repo_id2).await {
                                Ok(()) => {
                                    crate::service::merge::bisect_state(&services2, repo_id3).await
                                }
                                Err(err) => Err(err),
                            },
                        ))
                    },
                    std::convert::identity,
                );
                let log = self.load_bisect_log(repo_id);
                reset.chain(log)
            }
            bisect_panel::Action::LoadLog => self.load_bisect_log(repo_id),
        }
    }

    /// Executes a reflog panel action: refresh reads through the service
    /// layer, row intents reuse the shell's copy-sha / reset-confirm /
    /// branch-create flows.
    fn run_reflog_action(
        &mut self,
        repo_id: RepoId,
        action: Option<reflog_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            reflog_panel::Action::Refresh { ref_name } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Reflog(reflog_panel::Msg::EntriesLoaded(
                            crate::service::branches::reflog(&services, repo_id, ref_name).await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            reflog_panel::Action::CopySha(sha) => self.update(Message::CopySha(sha)),
            reflog_panel::Action::ResetTo { sha } => self.update(Message::ResetRequested(sha)),
            reflog_panel::Action::BranchFrom { sha } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.branches.create_from = Some(sha);
                    tab.branches.open = true;
                }
                Task::none()
            }
        }
    }

    /// Executes a health-panel action against `service::maintenance`.
    /// Reads are fire-and-forget loads; mutations report through the
    /// panel's `*Done` messages (the footer shows op progress via the
    /// existing `BackendEvent::OpProgress` handling). Archive opens a
    /// native save dialog first.
    fn run_health_action(
        &mut self,
        repo_id: RepoId,
        action: Option<health_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            health_panel::Action::Refresh => {
                let services = self.services.clone();
                let services2 = self.services.clone();
                let services3 = self.services.clone();
                let repo_id2 = repo_id.clone();
                let repo_id3 = repo_id.clone();
                Task::batch([
                    Task::perform(
                        async move {
                            Message::Health(health_panel::Msg::HealthLoaded(
                                crate::service::maintenance::repo_health(&services, repo_id2).await,
                            ))
                        },
                        std::convert::identity,
                    ),
                    Task::perform(
                        async move {
                            Message::Health(health_panel::Msg::SparseLoaded(
                                crate::service::maintenance::sparse_info(&services2, repo_id3)
                                    .await,
                            ))
                        },
                        std::convert::identity,
                    ),
                    Task::perform(
                        async move {
                            Message::Health(health_panel::Msg::LfsLoaded(
                                crate::service::maintenance::lfs_status(&services3, repo_id).await,
                            ))
                        },
                        std::convert::identity,
                    ),
                ])
            }
            health_panel::Action::RunOp { op, .. } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Health(health_panel::Msg::OpDone(
                            crate::service::maintenance::maintenance_run(&services, repo_id, op)
                                .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            health_panel::Action::Archive { spec, format } => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                Task::perform(
                    async move {
                        // Native save dialog (blocking; off the UI thread).
                        let extension = match format.as_str() {
                            "tar.gz" => "tar.gz",
                            "tar" => "tar",
                            _ => "zip",
                        };
                        let default_name = format!("archive-{spec}.{extension}");
                        let destination = tokio::task::spawn_blocking(move || {
                            rfd::FileDialog::new()
                                .set_file_name(&default_name)
                                .save_file()
                        })
                        .await
                        .ok()
                        .flatten();
                        let Some(destination) = destination else {
                            return Message::Health(health_panel::Msg::ArchiveDone(Err(
                                "archive canceled".to_string(),
                            )));
                        };
                        Message::Health(health_panel::Msg::ArchiveDone(
                            crate::service::maintenance::archive(
                                &services,
                                repo_id2,
                                spec,
                                format.as_str().to_string(),
                                destination.to_string_lossy().into_owned(),
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            health_panel::Action::SparseApply { patterns, add } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Health(health_panel::Msg::SparseDone(
                            crate::service::maintenance::sparse_apply(
                                &services, repo_id, patterns, add,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            health_panel::Action::LfsRun { subcommand } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Health(health_panel::Msg::LfsDone(
                            crate::service::maintenance::lfs_run(&services, repo_id, subcommand)
                                .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
        }
    }

    /// Executes a worktree panel action against the service layer.
    fn run_worktree_action(
        &mut self,
        repo_id: RepoId,
        action: Option<worktree_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        let services = self.services.clone();
        match action {
            worktree_panel::Action::Refresh => Task::perform(
                async move {
                    Message::Worktree(worktree_panel::Msg::ListLoaded(
                        crate::service::worktrees(&services, repo_id).await,
                    ))
                },
                std::convert::identity,
            ),
            worktree_panel::Action::Add { name, path } => Task::perform(
                async move {
                    let result = crate::service::worktree::worktree_add_named(
                        &services,
                        repo_id.clone(),
                        name,
                        path,
                    )
                    .await;
                    let list = match &result {
                        Ok(()) => crate::service::worktrees(&services, repo_id).await,
                        Err(err) => Err(err.clone()),
                    };
                    Message::Worktree(worktree_panel::Msg::ListLoaded(list))
                },
                std::convert::identity,
            ),
            worktree_panel::Action::Remove { name, force } => Task::perform(
                async move {
                    let result = crate::service::worktree::worktree_remove(
                        &services,
                        repo_id.clone(),
                        name,
                        force,
                    )
                    .await;
                    let list = match &result {
                        Ok(()) => crate::service::worktrees(&services, repo_id).await,
                        Err(err) => Err(err.clone()),
                    };
                    Message::Worktree(worktree_panel::Msg::ListLoaded(list))
                },
                std::convert::identity,
            ),
            worktree_panel::Action::Prune => Task::perform(
                async move {
                    let result = crate::service::worktree::worktree_prune(&services, repo_id).await;
                    Message::Worktree(worktree_panel::Msg::PruneDone(result.unwrap_or(0)))
                },
                std::convert::identity,
            ),
        }
    }

    /// Executes a submodule panel action (service::files surface).
    fn run_submodule_action(
        &mut self,
        repo_id: RepoId,
        action: Option<submodule_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        let services = self.services.clone();
        let repo_id2 = repo_id.clone();
        match action {
            submodule_panel::Action::Refresh => Task::perform(
                async move {
                    Message::Submodule(submodule_panel::Msg::ListLoaded(
                        crate::service::files::submodules(&services, repo_id).await,
                    ))
                },
                std::convert::identity,
            ),
            submodule_panel::Action::Update { path } => Task::perform(
                async move {
                    Message::Submodule(submodule_panel::Msg::OpDone(
                        crate::service::files::submodule_update(
                            &services, repo_id2, path, true, true,
                        )
                        .await,
                    ))
                },
                std::convert::identity,
            ),
            submodule_panel::Action::UpdateAll => {
                // The engine updates one submodule per call; fan out over
                // the loaded list (the op queue serializes the mutations).
                let paths: Vec<String> = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.submodule.list.as_ref())
                    .map(|list| list.iter().map(|sub| sub.path.clone()).collect())
                    .unwrap_or_default();
                let mut tasks = Vec::new();
                for path in paths {
                    let services = self.services.clone();
                    let repo_id2 = repo_id.clone();
                    tasks.push(Task::perform(
                        async move {
                            Message::Submodule(submodule_panel::Msg::OpDone(
                                crate::service::files::submodule_update(
                                    &services, repo_id2, path, true, true,
                                )
                                .await,
                            ))
                        },
                        std::convert::identity,
                    ));
                }
                Task::batch(tasks)
            }
            submodule_panel::Action::Sync { path } => Task::perform(
                async move {
                    Message::Submodule(submodule_panel::Msg::OpDone(
                        crate::service::files::submodule_sync(&services, repo_id, path).await,
                    ))
                },
                std::convert::identity,
            ),
        }
    }

    /// Executes a tag panel action (service::tags).
    fn run_tags_action(
        &mut self,
        repo_id: RepoId,
        action: Option<tags_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        let services = self.services.clone();
        let repo_id2 = repo_id.clone();
        match action {
            tags_panel::Action::Refresh => Task::perform(
                async move {
                    Message::Tags(tags_panel::Msg::ListLoaded(
                        crate::service::tags::tag_list(&services, repo_id).await,
                    ))
                },
                std::convert::identity,
            ),
            tags_panel::Action::Create {
                name,
                target,
                message,
                annotated: _,
                signed,
            } => Task::perform(
                async move {
                    Message::Tags(tags_panel::Msg::CreateDone(
                        // `signed` maps to the service's `sign` flag; the
                        // message already carries the annotated-ness.
                        crate::service::tags::tag_create(
                            &services, repo_id2, name, target, message, signed,
                        )
                        .await,
                    ))
                },
                std::convert::identity,
            ),
            tags_panel::Action::Delete { name } => Task::perform(
                async move {
                    Message::Tags(tags_panel::Msg::DeleteDone(
                        crate::service::tags::tag_delete(&services, repo_id, name).await,
                    ))
                },
                std::convert::identity,
            ),
        }
    }

    /// Executes a blame view action (service::repo::repo_blame).
    fn run_blame_action(
        &mut self,
        repo_id: RepoId,
        action: Option<blame_view::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            blame_view::Action::Load { path } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Blame(blame_view::Msg::LinesLoaded(
                            crate::service::repo::repo_blame(
                                &services,
                                repo_id,
                                path.clone(),
                                None,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            blame_view::Action::CopySha(sha) => self.update(Message::CopyText(sha)),
            blame_view::Action::SelectCommit(sha) => self.update(Message::SelectCommit(sha)),
        }
    }

    /// Executes a file-history view action. The stream itself arrives as
    /// `BackendEvent::LogPage`s routed by the per-tab request id.
    fn run_file_history_action(
        &mut self,
        repo_id: RepoId,
        action: Option<file_history_view::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            file_history_view::Action::Load { path } => {
                self.file_history_seq += 1;
                let request_id = self.file_history_seq;
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.file_history_seq = request_id;
                let result = crate::service::log::repo_file_history(
                    &self.services,
                    repo_id,
                    path,
                    request_id,
                );
                if let Err(err) = result {
                    self.notice = Some(Notice::error(format!("File history failed: {err}")));
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.file_history.update(file_history_view::Msg::Failed(err));
                    }
                }
                Task::none()
            }
            // The stream pages itself to completion; LoadMore is a no-op.
            file_history_view::Action::LoadMore => Task::none(),
            file_history_view::Action::SelectCommit(sha) => self.update(Message::SelectCommit(sha)),
            file_history_view::Action::OpenDiff { sha, .. } => {
                self.update(Message::SelectCommit(sha))
            }
        }
    }

    /// Executes a compare view action (two `DiffSide::Commit` sides).
    fn run_compare_action(
        &mut self,
        repo_id: RepoId,
        action: Option<compare_view::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            compare_view::Action::Load { old_ref, new_ref } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Compare(compare_view::Msg::Loaded(
                            crate::service::diff::repo_diff(
                                &services,
                                repo_id,
                                DiffSide::Commit(old_ref.clone()),
                                DiffSide::Commit(new_ref),
                                None,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            compare_view::Action::OpenFileDiff { path } => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let (old_ref, new_ref) = (tab.compare.old_ref.clone(), tab.compare.new_ref.clone());
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::diff::repo_diff(
                            &services,
                            repo_id,
                            DiffSide::Commit(old_ref),
                            DiffSide::Commit(new_ref),
                            Some(vec![path]),
                        )
                        .await;
                        (result, DiffSlot::Commit)
                    },
                    |(result, slot)| Message::CompareFileDiffLoaded { slot, result },
                )
            }
        }
    }

    /// Executes a files-panel action: directory loads (lazy, per level),
    /// editor/history opens (same flow as the status-row commands) and the
    /// worktree mutations (`repo_create_entry` / `repo_rename_entry`;
    /// deletes go through the tab's confirmation dialog first).
    fn run_files_action(
        &mut self,
        repo_id: RepoId,
        action: Option<files_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            files_panel::Action::Opened => {
                // Center-mode exclusivity: opening Files closes the others
                // (the editor stays — it wins the center chain).
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.blame.open = false;
                    tab.file_history.open = false;
                    tab.compare.open = false;
                }
                Task::none()
            }
            files_panel::Action::LoadDir { dir } => self.load_files_dir(repo_id, dir),
            files_panel::Action::Reload => self.refresh_files(repo_id),
            files_panel::Action::OpenEditor { path } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.blame.open = false;
                    tab.file_history.open = false;
                    tab.compare.open = false;
                    tab.files.open = false;
                }
                self.update(Message::Editor(editor_view::Msg::OpenRequested(path)))
            }
            files_panel::Action::OpenHistory { path } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                    tab.blame.open = false;
                    tab.compare.open = false;
                    tab.files.open = false;
                }
                self.update(Message::FileHistory(file_history_view::Msg::Opened {
                    path,
                }))
            }
            files_panel::Action::CreateEntry { path, kind } => {
                let label = match kind {
                    crate::workspace::NewEntryKind::File => "Create file".to_string(),
                    crate::workspace::NewEntryKind::Dir => "Create folder".to_string(),
                };
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::workspace::repo_create_entry(
                            &services,
                            repo_id.clone(),
                            path,
                            kind,
                        )
                        .await;
                        Message::FilesEntryDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            files_panel::Action::RenameEntry { from, to } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::workspace::repo_rename_entry(
                            &services,
                            repo_id.clone(),
                            from,
                            to,
                        )
                        .await;
                        Message::FilesEntryDone {
                            repo_id,
                            label: "Rename".to_string(),
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            files_panel::Action::DeleteEntry { path } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_files_delete = Some(path);
                }
                Task::none()
            }
        }
    }

    /// Issues a lazy load for one files-panel directory ("" = root); cached
    /// listings and in-flight loads are skipped.
    fn load_files_dir(&mut self, repo_id: RepoId, dir: String) -> Task<Message> {
        let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
            return Task::none();
        };
        if !self.tabs[index].files.needs_load(&dir) {
            return Task::none();
        }
        self.tabs[index].files.loading.insert(dir.clone());
        let services = self.services.clone();
        Task::perform(
            async move {
                let result =
                    crate::workspace::repo_list_dir(&services, repo_id.clone(), dir.clone(), false)
                        .await;
                Message::FilesDirLoaded {
                    repo_id,
                    dir,
                    result,
                }
            },
            std::convert::identity,
        )
    }

    /// Reloads the files panel for `repo_id`: root plus every expanded
    /// directory (after `invalidate()` this re-fetches what is visible).
    fn refresh_files(&mut self, repo_id: RepoId) -> Task<Message> {
        let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
            return Task::none();
        };
        if !self.tabs[index].files.open {
            return Task::none();
        }
        let expanded: Vec<String> = self.tabs[index]
            .files
            .expanded
            .iter()
            .filter(|dir| !dir.is_empty())
            .cloned()
            .collect();
        let mut tasks = vec![self.load_files_dir(repo_id.clone(), String::new())];
        for dir in expanded {
            tasks.push(self.load_files_dir(repo_id.clone(), dir));
        }
        Task::batch(tasks)
    }

    /// Opens a worktree file (repository-relative) in the first detected
    /// external editor; a missing editor surfaces as a footer notice.
    fn open_file_external(&mut self, path: &str) -> Task<Message> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Task::none();
        };
        let repo_id = tab.repo.repo_id.clone();
        let apps = crate::service::editor::detect_editors();
        let Some(app) = apps.first() else {
            self.notice = Some(Notice::error("No external editor detected."));
            return Task::none();
        };
        match crate::service::editor::open_in_editor(&self.services, &repo_id, &app.id, path) {
            Ok(()) => {
                self.last_op = Some(format!("Opened {path} in {}", app.name));
                Task::none()
            }
            Err(err) => {
                self.notice = Some(Notice::error(format!("External editor failed: {err}")));
                Task::none()
            }
        }
    }

    /// Executes a custom-actions panel action.
    fn run_actions_action(
        &mut self,
        repo_id: RepoId,
        action: Option<actions_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            actions_panel::Action::Refresh => Task::none(), // set_actions at arm entry
            actions_panel::Action::Add { name, command } => {
                self.custom_actions
                    .push(actions_panel::ActionEntry::new(name, command));
                self.persist_custom_actions();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.actions.update(actions_panel::Msg::Added);
                }
                Task::none()
            }
            actions_panel::Action::Delete { name } => {
                self.custom_actions.retain(|entry| entry.name != name);
                self.persist_custom_actions();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.actions.update(actions_panel::Msg::Deleted);
                }
                Task::none()
            }
            actions_panel::Action::Run { name, command } => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                Task::perform(
                    async move {
                        match crate::service::misc::action_run(
                            &services,
                            repo_id2,
                            name.clone(),
                            command,
                        ) {
                            Ok(run_id) => Message::ActionStarted { name, run_id },
                            Err(err) => Message::Notice(format!("Action failed: {err}")),
                        }
                    },
                    std::convert::identity,
                )
            }
            actions_panel::Action::Cancel { run_id } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let _ = crate::service::misc::action_cancel(&services, run_id);
                        Message::Noop
                    },
                    std::convert::identity,
                )
            }
        }
    }

    /// Executes an editor action: stat+read on open (a write landing in
    /// between fails closed as a conflict, never silently clobbers), the
    /// mtime-guarded write on save, external launch on demand.
    fn run_editor_action(
        &mut self,
        repo_id: RepoId,
        action: Option<editor_view::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        match action {
            editor_view::Action::Load { path } | editor_view::Action::Reload { path } => {
                let services = self.services.clone();
                let repo_id2 = repo_id.clone();
                let path2 = path.clone();
                Task::perform(
                    async move {
                        let meta = crate::service::editor::repo_stat_file(
                            &services,
                            repo_id2.clone(),
                            path2.clone(),
                        )
                        .await;
                        match meta {
                            Ok(meta)
                                if !matches!(
                                    meta.kind,
                                    crate::workspace::FileContentKind::Text
                                ) =>
                            {
                                Message::Editor(editor_view::Msg::Opened {
                                    path: path2,
                                    content: String::new(),
                                    mtime: meta.mtime_ms,
                                })
                            }
                            Ok(meta) => {
                                let content = crate::service::files::repo_read_file(
                                    &services,
                                    repo_id2,
                                    path2.clone(),
                                )
                                .await
                                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                                match content {
                                    Ok(content) => Message::Editor(editor_view::Msg::Opened {
                                        path: path2,
                                        content,
                                        mtime: meta.mtime_ms,
                                    }),
                                    Err(err) => Message::Notice(format!("Open failed: {err}")),
                                }
                            }
                            Err(err) => Message::Notice(format!("Open failed: {err}")),
                        }
                    },
                    std::convert::identity,
                )
            }
            editor_view::Action::Save {
                path,
                content,
                expected_mtime,
            } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::Editor(editor_view::Msg::Saved(
                            crate::service::editor::repo_write_file(
                                &services,
                                repo_id,
                                path,
                                content.into_bytes(),
                                expected_mtime,
                            )
                            .await,
                        ))
                    },
                    std::convert::identity,
                )
            }
            editor_view::Action::OpenExternal { path, app_id } => {
                let result = crate::service::editor::open_in_editor(
                    &self.services,
                    &repo_id,
                    &app_id,
                    &path,
                );
                if let Err(err) = result {
                    self.notice = Some(Notice::error(format!("Open externally failed: {err}")));
                }
                Task::none()
            }
        }
    }

    /// Executes a stats panel action (service::forge stats fns).
    fn run_stats_action(
        &mut self,
        repo_id: RepoId,
        action: Option<stats_panel::Action>,
    ) -> Task<Message> {
        let Some(action) = action else {
            return Task::none();
        };
        let stats_panel::Action::Refresh { window } = action;
        let services = self.services.clone();
        let services2 = self.services.clone();
        let repo_id2 = repo_id.clone();
        Task::batch([
            Task::perform(
                async move {
                    Message::Stats(stats_panel::Msg::ActivityLoaded(
                        crate::service::forge::commit_activity(&services, repo_id, window, None)
                            .await,
                    ))
                },
                std::convert::identity,
            ),
            Task::perform(
                async move {
                    Message::Stats(stats_panel::Msg::ContributorsLoaded(
                        crate::service::forge::contributor_stats(&services2, repo_id2, window)
                            .await,
                    ))
                },
                std::convert::identity,
            ),
        ])
    }

    /// Kicks off blob/workdir reads for every image file of a freshly
    /// loaded diff so [`diff_view::image_row`] can render previews. Slots
    /// are reserved up-front so watcher refreshes don't retrigger loads.
    fn ensure_images(&mut self, index: usize, slot: DiffSlot) -> Task<Message> {
        let Some(tab) = self.tabs.get(index) else {
            return Task::none();
        };
        let pane = match slot {
            DiffSlot::Working => &tab.diff,
            DiffSlot::Commit => &tab.commit_diff,
        };
        let paths: Vec<String> = pane
            .files()
            .iter()
            .filter(|file| file.is_image)
            .map(|file| file.path.clone())
            .filter(|path| !pane.images.contains_key(path))
            .collect();
        if paths.is_empty() {
            return Task::none();
        }
        let repo_id = tab.repo.repo_id.clone();
        let seq = pane.seq;
        let (commit_sha, parent_sha) = match slot {
            DiffSlot::Working => (None, None),
            DiffSlot::Commit => tab
                .history
                .selected_commit()
                .map(|commit| (Some(commit.sha.clone()), commit.parents.first().cloned()))
                .unwrap_or((None, None)),
        };
        {
            let pane = match slot {
                DiffSlot::Working => &mut self.tabs[index].diff,
                DiffSlot::Commit => &mut self.tabs[index].commit_diff,
            };
            for path in &paths {
                pane.images.entry(path.clone()).or_default();
            }
        }

        let mut tasks = Vec::new();
        for path in paths {
            let old_rev = match slot {
                DiffSlot::Working => Some("HEAD".to_string()),
                DiffSlot::Commit => parent_sha.clone(),
            };
            if let Some(revision) = old_rev {
                let services = self.services.clone();
                let repo_id = repo_id.clone();
                let path = path.clone();
                tasks.push(Task::perform(
                    async move {
                        let result = crate::service::files::repo_read_blob_at(
                            &services,
                            repo_id.clone(),
                            revision,
                            path.clone(),
                        )
                        .await;
                        Message::ImageLoaded {
                            repo_id,
                            slot,
                            path,
                            old: true,
                            seq,
                            result,
                        }
                    },
                    std::convert::identity,
                ));
            }
            let services = self.services.clone();
            let repo_id = repo_id.clone();
            let path = path.clone();
            match slot {
                DiffSlot::Working => tasks.push(Task::perform(
                    async move {
                        let result = crate::service::files::repo_read_file(
                            &services,
                            repo_id.clone(),
                            path.clone(),
                        )
                        .await;
                        Message::ImageLoaded {
                            repo_id,
                            slot,
                            path,
                            old: false,
                            seq,
                            result,
                        }
                    },
                    std::convert::identity,
                )),
                DiffSlot::Commit => {
                    if let Some(sha) = commit_sha.clone() {
                        tasks.push(Task::perform(
                            async move {
                                let result = crate::service::files::repo_read_blob_at(
                                    &services,
                                    repo_id.clone(),
                                    sha,
                                    path.clone(),
                                )
                                .await;
                                Message::ImageLoaded {
                                    repo_id,
                                    slot,
                                    path,
                                    old: false,
                                    seq,
                                    result,
                                }
                            },
                            std::convert::identity,
                        ));
                    }
                }
            }
        }
        Task::batch(tasks)
    }

    /// Whether an overlay with its own key listener (command palette, quick
    /// open, quick switcher, context menu) is open: while one is, the fixed
    /// shell shortcuts stay dark so keystrokes can only reach the overlay.
    fn overlay_open(&self) -> bool {
        self.palette.open || self.quick_open.open || self.switcher.open || self.ctx_menu.open
    }

    /// Whether any modal dialog is up — the active tab's confirmation cards,
    /// the global clone/AI-settings/device dialogs, auth prompts or the AI
    /// result window. Shell shortcuts stay dark behind these too: Ctrl+Enter
    /// must not commit while "Discard changes?" is on screen.
    fn dialog_open(&self) -> bool {
        self.clone.open
            || self.ai_settings.open
            || self.devices.dialog.open
            || self.devices.browse.open
            || self.devices.pending_forget.is_some()
            || self.ai_result.is_some()
            || self.help_open
            || !self.auth_prompts.is_empty()
            || !self.host_key_prompts.is_empty()
            || !self.device_secret_prompts.is_empty()
            || self.tabs.get(self.active).is_some_and(Tab::dialog_pending)
    }

    /// Dispatches a fixed shell shortcut (the `Message::Shortcut` arm).
    /// Every target routes through the same arms the buttons/palette use, so
    /// their guards (no repo tab, commit validation, editor save guard)
    /// apply unchanged; safe to fire with no repo tab open.
    fn shortcut(&mut self, shortcut: ShellShortcut) -> Task<Message> {
        match shortcut {
            // Escape claims dialogs before the general dark-out below:
            // while a dialog is up the other shortcuts are dead, but Escape
            // must still reach the dialog's cancel arm. With no dialog
            // open it's a no-op.
            ShellShortcut::CancelDialog => {
                if self.dialog_open() {
                    self.update(Message::CancelDialog)
                } else {
                    Task::none()
                }
            }
            // Tab traversal stays live (focus movement is never harmful),
            // but only away from a focused terminal: the pane owns plain
            // keys and focus jumping out mid-session would be jarring.
            ShellShortcut::FocusNext => {
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.terminal_open && tab.terminal.is_focused())
                {
                    Task::none()
                } else {
                    iced::widget::operation::focus_next::<Message>()
                }
            }
            _ if self.overlay_open() || self.dialog_open() => Task::none(),
            ShellShortcut::Help => {
                self.help_open = !self.help_open;
                Task::none()
            }
            ShellShortcut::RefreshRepo => {
                // A focused terminal owns plain keys (`F5` writes an xterm
                // sequence into the shell); modifier combos are pane-ignored
                // and stay live. Only the non-combo shortcut needs this guard.
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.terminal_open && tab.terminal.is_focused())
                {
                    return Task::none();
                }
                match self.tabs.get(self.active) {
                    Some(tab) => self.update(Message::Refresh(tab.repo.repo_id.clone())),
                    None => Task::none(),
                }
            }
            ShellShortcut::Panel(index) => match panel_toggle_message(index) {
                Some(msg) => self.update(msg),
                None => Task::none(),
            },
            ShellShortcut::SaveEditor => {
                self.update(Message::Editor(editor_view::Msg::SavePressed))
            }
            ShellShortcut::Commit => self.update(Message::CommitPressed),
            ShellShortcut::NextTab | ShellShortcut::PrevTab => {
                if self.tabs.len() < 2 {
                    return Task::none();
                }
                let next = matches!(shortcut, ShellShortcut::NextTab);
                let target = wrapped_tab_index(self.active, self.tabs.len(), next);
                self.update(Message::SelectTab(target))
            }
            ShellShortcut::CloseTab => {
                // A focused terminal should keep Ctrl+W: it's the common
                // readline delete-word binding, and closing the tab kills
                // the PTY session — a data-losing surprise from inside the
                // shell. (The pane yields modifier combos to the shell, so
                // this guard mirrors F5's.)
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.terminal_open && tab.terminal.is_focused())
                {
                    return Task::none();
                }
                match self.tabs.get(self.active) {
                    Some(tab) => self.update(Message::CloseTab(tab.repo.repo_id.clone())),
                    None => Task::none(),
                }
            }
        }
    }

    /// Syncs `config.session` with the open tabs and persists the config
    /// file (called on open / close / activate). Device-backed tabs are
    /// skipped and the focused index is re-based onto the filtered list —
    /// see [`config::session_state`].
    fn persist_session(&mut self) {
        let tabs: Vec<(&str, bool)> = self
            .tabs
            .iter()
            .map(|tab| (tab.repo.root.as_str(), tab.repo.device_id.is_none()))
            .collect();
        let focused = if self.tabs.is_empty() {
            None
        } else {
            Some(self.active)
        };
        self.config.session = config::session_state(&tabs, focused, self.config.session.active);
        self.persist_config();
    }

    /// Continues the boot-time session restore: opens the next queued repo,
    /// or — once the queue drains — focuses the persisted active tab (the
    /// index is best-effort: repos that failed to open shift it, so it is
    /// clamped and silently ignored when out of range).
    fn continue_restore(&mut self) -> Task<Message> {
        if let Some(path) = self.restore_queue.first().cloned() {
            return self.open_repo(path);
        }
        if self.restore_active < self.tabs.len() {
            self.update(Message::SelectTab(self.restore_active))
        } else {
            Task::none()
        }
    }

    fn open_repo(&mut self, path: String) -> Task<Message> {
        if self.open_in_flight {
            return Task::none();
        }
        self.open_in_flight = true;
        self.notice = None;
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::repo::repo_open(&services, path.clone()).await;
                Message::RepoOpened { path, result }
            },
            std::convert::identity,
        )
    }

    /// Reloads the SSH device list for the Home section (`device_list`).
    fn refresh_devices(&self) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::remote::commands::device_list(&services);
                Message::Devices(devices::Msg::Loaded(result))
            },
            std::convert::identity,
        )
    }

    /// Opens (or focuses) the tab for an opened repo and kicks off its
    /// initial loads — shared by the local `RepoOpened` flow and the device
    /// browser's `DeviceRepoOpened` flow.
    fn activate_repo(&mut self, info: RepoInfo) -> Task<Message> {
        let repo_id = info.repo_id.clone();
        match repo_view::tab_index(&self.tabs, &repo_id) {
            Some(index) => self.active = index,
            None => {
                // Seed the tab's diff layout from the saved
                // prefs so split/wrap survive restarts.
                let (diff_split, diff_wrap) = (self.config.diff_split, self.config.diff_wrap);
                let [sidebar, history, terminal_height] = self.config.pane_sizes;
                self.tabs.push(Tab::new(
                    info,
                    diff_split,
                    diff_wrap,
                    repo_view::PaneSizes {
                        sidebar,
                        history,
                        terminal: terminal_height,
                    },
                ));
                self.active = self.tabs.len() - 1;
            }
        }
        self.show_home = false;
        self.persist_session();
        Task::batch([
            self.refresh(repo_id.clone()),
            self.start_log(repo_id.clone()),
            self.load_bisect_state(repo_id),
        ])
    }

    /// Routes a devices message through the pure module, translating any
    /// returned intent into the matching `remote::commands` task.
    fn update_devices(&mut self, msg: devices::Msg) -> Task<Message> {
        // The row-level probe result also reads as a footer op line.
        if let devices::Msg::TestDone(Ok(output)) = &msg {
            self.last_op = Some(format!("Device test ok: {output}"));
        }
        let Some(action) = devices::update(&mut self.devices, msg) else {
            return Task::none();
        };
        match action {
            devices::Action::RefreshList => self.refresh_devices(),
            devices::Action::Connect { device_id } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::remote::commands::device_connect(&services, device_id.clone())
                                .await;
                        Message::DeviceConnectDone { device_id, result }
                    },
                    std::convert::identity,
                )
            }
            devices::Action::Disconnect { device_id } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::remote::commands::device_disconnect(
                            &services,
                            device_id.clone(),
                        )
                        .await
                        .map(|_| "disconnected".to_string());
                        Message::DeviceConnectDone { device_id, result }
                    },
                    std::convert::identity,
                )
            }
            devices::Action::Forget { device_id } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::remote::commands::device_remove(&services, device_id.clone())
                                .await;
                        Message::DeviceForgetDone { device_id, result }
                    },
                    std::convert::identity,
                )
            }
            devices::Action::Save { input, is_new } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = if is_new {
                            crate::remote::commands::device_add(&services, input).await
                        } else {
                            crate::remote::commands::device_update(&services, input).await
                        };
                        Message::Devices(devices::Msg::SaveDone(result))
                    },
                    std::convert::identity,
                )
            }
            devices::Action::Test { input } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::remote::commands::device_test(&services, input).await;
                        Message::Devices(devices::Msg::TestDone(result))
                    },
                    std::convert::identity,
                )
            }
            devices::Action::BrowseDir { device_id, path } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::remote::commands::device_browse_dir(
                            &services,
                            device_id.clone(),
                            path.clone(),
                        )
                        .await;
                        Message::Devices(devices::Msg::Browse(devices::BrowseMsg::DirLoaded {
                            device_id,
                            path,
                            result,
                        }))
                    },
                    std::convert::identity,
                )
            }
            devices::Action::Scan { device_id, root } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::remote::commands::device_scan_repos(
                            &services,
                            device_id.clone(),
                            root,
                        )
                        .await;
                        Message::Devices(devices::Msg::Browse(devices::BrowseMsg::ScanDone {
                            device_id,
                            result,
                        }))
                    },
                    std::convert::identity,
                )
            }
            devices::Action::OpenRepo { device_id, path } => {
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::remote::commands::remote_repo_open(
                            &services,
                            device_id.clone(),
                            path.clone(),
                        )
                        .await;
                        Message::DeviceRepoOpened { path, result }
                    },
                    std::convert::identity,
                )
            }
        }
    }

    fn refresh(&self, repo_id: RepoId) -> Task<Message> {
        if repo_view::tab_index(&self.tabs, &repo_id).is_none() {
            return Task::none();
        }
        let services = self.services.clone();
        let id = repo_id.clone();
        Task::perform(
            async move {
                let result = crate::service::repo::repo_status(&services, repo_id).await;
                Message::StatusLoaded {
                    repo_id: id,
                    result,
                }
            },
            std::convert::identity,
        )
    }

    /// Fetch the working-copy diff for the selected status row when the
    /// pane's identity changed (or a mutation invalidated it).
    fn ensure_working_diff(&mut self, index: usize) -> Task<Message> {
        let Some(tab) = self.tabs.get_mut(index) else {
            return Task::none();
        };
        if !tab.diff_is_stale() {
            return Task::none();
        }
        let Some(row) = tab.selected_status_row() else {
            tab.diff = DiffPane::default();
            return Task::none();
        };
        let Some(key) = tab.working_diff_key() else {
            return Task::none();
        };
        let paths = tab.diff_paths();
        let conflicted = matches!(row.index, ChangeKind::Conflicted)
            || matches!(row.worktree, ChangeKind::Conflicted);
        // Unmerged paths have no index entry, so an index↔worktree diff comes
        // back empty; diffing HEAD→worktree shows the conflict markers.
        let (old, new) = if conflicted {
            (DiffSide::Head, DiffSide::Worktree)
        } else {
            match row.section {
                SectionId::Staged => (DiffSide::Head, DiffSide::Index),
                SectionId::Changes => (DiffSide::Index, DiffSide::Worktree),
            }
        };

        tab.diff_seq += 1;
        let seq = tab.diff_seq;
        tab.diff.set_loading(key.clone());
        tab.diff.seq = seq;
        tab.reload_diff = false;

        let repo_id = tab.repo.repo_id.clone();
        let services = self.services.clone();
        Task::perform(
            async move {
                let result =
                    crate::service::diff::repo_diff(&services, repo_id.clone(), old, new, paths)
                        .await;
                Message::DiffLoaded {
                    repo_id,
                    slot: DiffSlot::Working,
                    key,
                    seq,
                    result,
                }
            },
            std::convert::identity,
        )
    }

    /// Fetch the selected commit's diff against its first parent.
    fn ensure_commit_diff(&mut self, index: usize) -> Task<Message> {
        let Some(tab) = self.tabs.get_mut(index) else {
            return Task::none();
        };
        let (sha, parent) = match tab.history.selected_commit() {
            Some(commit) => (commit.sha.clone(), commit.parents.first().cloned()),
            None => return Task::none(),
        };
        let key = format!("commit:{sha}");
        if tab.commit_diff.key.as_deref() == Some(key.as_str()) && tab.commit_diff.is_ready() {
            return Task::none();
        }

        let Some(parent) = parent else {
            // Root commit: nothing to diff against.
            tab.commit_diff_seq += 1;
            tab.commit_diff = DiffPane::default();
            return Task::none();
        };

        tab.commit_diff_seq += 1;
        let seq = tab.commit_diff_seq;
        tab.commit_diff.set_loading(key.clone());
        tab.commit_diff.seq = seq;

        let repo_id = tab.repo.repo_id.clone();
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::diff::repo_diff(
                    &services,
                    repo_id.clone(),
                    DiffSide::Commit(parent),
                    DiffSide::Commit(sha),
                    None,
                )
                .await;
                Message::DiffLoaded {
                    repo_id,
                    slot: DiffSlot::Commit,
                    key,
                    seq,
                    result,
                }
            },
            std::convert::identity,
        )
    }

    /// Restart the commit-log stream for a repo (cancels the previous one).
    fn start_log(&mut self, repo_id: RepoId) -> Task<Message> {
        let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
            return Task::none();
        };
        self.log_seq += 1;
        let request_id = self.log_seq;
        let tab = &mut self.tabs[index];
        let selected = tab.history.selected.take();
        tab.history.begin(request_id);
        tab.history.selected = selected;

        let filter = to_log_filter(&tab.history_filter);
        let result =
            crate::service::log::repo_log_stream(&self.services, repo_id, filter, request_id);
        if let Err(err) = result {
            self.notice = Some(Notice::error(format!("Could not load history: {err}")));
        }
        Task::none()
    }

    fn load_branches(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::branches::branches(&services, repo_id.clone()).await;
                Message::BranchesLoaded { repo_id, result }
            },
            std::convert::identity,
        )
    }

    fn load_remote_branches(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result =
                    crate::service::branches::remote_branches(&services, repo_id.clone()).await;
                Message::RemoteBranchesLoaded { repo_id, result }
            },
            std::convert::identity,
        )
    }

    /// Runs the remote checkout (the dirty-tree guard already decided this
    /// is safe to go straight in).
    fn checkout_remote_branch(
        &mut self,
        repo_id: RepoId,
        remote: String,
        name: String,
    ) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::branches::branch_checkout_remote(
                    &services,
                    repo_id.clone(),
                    remote,
                    name,
                    None,
                )
                .await;
                Message::RemoteCheckoutDone { repo_id, result }
            },
            std::convert::identity,
        )
    }

    fn load_stashes(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::stash::stash_list(&services, repo_id.clone()).await;
                Message::StashesLoaded { repo_id, result }
            },
            std::convert::identity,
        )
    }

    fn load_checkpoints(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::misc::checkpoints(&services, repo_id.clone()).await;
                Message::CheckpointsLoaded { repo_id, result }
            },
            std::convert::identity,
        )
    }

    fn load_trash(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result =
                    crate::service::branches::branch_trash_list(&services, repo_id.clone()).await;
                Message::TrashLoaded { repo_id, result }
            },
            std::convert::identity,
        )
    }

    fn load_remotes(&self, repo_id: RepoId) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::network::remotes(&services, repo_id.clone()).await;
                Message::RemotesLoaded { repo_id, result }
            },
            std::convert::identity,
        )
    }

    /// Restart the log with the edited filter (or with every field cleared).
    fn apply_history_filter(&mut self, clear: bool) -> Task<Message> {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return Task::none();
        };
        // Backstop for non-button paths (the Apply button already greys
        // out): bad dates must not silently filter everything out.
        if !clear {
            if let Some(error) = tab.history_filter.date_error() {
                self.notice = Some(Notice::error(error));
                return Task::none();
            }
        }
        if clear {
            tab.history_filter = FilterFields::default();
        }
        tab.history.selected = None;
        tab.detail_open = false;
        let repo_id = tab.repo.repo_id.clone();
        self.start_log(repo_id)
    }

    fn spawn_stage(&self, repo_id: RepoId, request: StageRequest, label: String) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result = crate::service::misc::stage(&services, repo_id.clone(), request).await;
                Message::MutationDone {
                    repo_id,
                    label,
                    result,
                }
            },
            std::convert::identity,
        )
    }

    fn spawn_discard(&self, repo_id: RepoId, targets: Vec<StageTarget>) -> Task<Message> {
        let services = self.services.clone();
        let label = count_label("Discard", targets.len());
        Task::perform(
            async move {
                let result =
                    crate::service::misc::discard(&services, repo_id.clone(), targets).await;
                Message::MutationDone {
                    repo_id,
                    label,
                    result,
                }
            },
            std::convert::identity,
        )
    }

    fn spawn_commit(&self, repo_id: RepoId, options: CommitOptions) -> Task<Message> {
        let services = self.services.clone();
        Task::perform(
            async move {
                let result =
                    crate::service::misc::commit(&services, repo_id.clone(), options).await;
                Message::Committed { repo_id, result }
            },
            std::convert::identity,
        )
    }

    fn visible_rows(tab: &Tab) -> Vec<&StatusRow> {
        filter_sections(&tab.status_index, &tab.filter)
            .into_iter()
            .flat_map(|section| section.rows)
            .collect()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // Typing surfaces take the keyboard back from a focused terminal:
        // any of these messages means the user is interacting elsewhere.
        if matches!(
            message,
            Message::SelectTab(_)
                | Message::GoHome
                | Message::CommitMessageChanged(_)
                | Message::StatusFilterChanged(_)
                | Message::BranchCreateNameChanged(_)
                | Message::StashMessageChanged(_)
                | Message::RemoteNameChanged(_)
                | Message::HistoryFilterChanged { .. }
        ) {
            self.blur_terminals();
        }
        // While a modal dialog is up, the palette-family overlays must not
        // open (or toggle closed): they render *beneath* the dialog scrim
        // but their key listeners stay live, so keystrokes would vanish
        // into an invisible overlay. Dialog-openers are blocked for the
        // same stacking reason.
        if self.dialog_open() {
            match message {
                Message::Palette(palette::Msg::Toggle)
                | Message::Switcher(quick_switcher::Msg::Toggle)
                | Message::QuickOpen(quick_open::Msg::Toggle)
                | Message::OpenDialog
                | Message::Clone(clone_dialog::Msg::Open)
                | Message::AiOpenSettings
                | Message::Devices(devices::Msg::AddDevice) => return Task::none(),
                _ => {}
            }
        }
        match message {
            Message::Backend(event) => self.backend(event),
            Message::SystemTheme(mode) => {
                self.system_mode = mode;
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let accent = crate::service::misc::os_accent_color(&services).await;
                        Message::AccentLoaded(accent.unwrap_or(None))
                    },
                    std::convert::identity,
                )
            }
            Message::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::Shortcut(shortcut) => self.shortcut(shortcut),
            Message::OpenDialog => {
                if self.open_in_flight {
                    return Task::none();
                }
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Open repository")
                            .pick_folder()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::OpenPicked,
                )
            }
            Message::OpenPicked(path) => match path {
                Some(path) => self.open_repo(path.to_string_lossy().into_owned()),
                None => Task::none(),
            },
            Message::OpenPath(path) => self.open_repo(path),
            Message::RepoOpened { path, result } => {
                self.open_in_flight = false;
                // A restore open matches the head of the pending queue: it
                // is consumed whether it succeeds or fails, and the chain
                // continues. Failures during restore are silent — a repo
                // that vanished since the last run is not an error.
                let restore_open = self.restore_queue.first().is_some_and(|head| *head == path);
                if restore_open {
                    self.restore_queue.remove(0);
                }
                match result {
                    Ok(info) => {
                        // Record the resolved workdir so recents stay valid
                        // regardless of the cwd used to launch the app, and
                        // skip remote projects (device-backed, not local).
                        if info.device_id.is_none() {
                            self.config.note_recent(&info.root, config::now_millis());
                            self.reindex_recent();
                            self.persist_config();
                        }
                        let mut tasks = vec![self.activate_repo(info)];
                        if restore_open {
                            tasks.push(self.continue_restore());
                        }
                        Task::batch(tasks)
                    }
                    Err(err) => {
                        if restore_open {
                            self.continue_restore()
                        } else {
                            self.notice =
                                Some(Notice::error(format!("Could not open {path}: {err}")));
                            Task::none()
                        }
                    }
                }
            }
            Message::StatusLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(status) => {
                        let tab = &mut self.tabs[index];
                        tab.apply_status(status);
                        if tab.selected_row.is_none() {
                            if let Some(row) = tab.status_index.rows.first() {
                                tab.selected_row = Some(row.key.clone());
                            }
                        }
                    }
                    Err(err) => {
                        self.tabs[index].status = Load::Failed(err);
                        return Task::none();
                    }
                }
                self.ensure_working_diff(index)
            }
            Message::Refresh(repo_id) => {
                let branches_open = repo_view::tab_index(&self.tabs, &repo_id)
                    .is_some_and(|index| self.tabs[index].branches.open);
                let stashes_open = repo_view::tab_index(&self.tabs, &repo_id)
                    .is_some_and(|index| self.tabs[index].stashes.open);
                let remotes_open = repo_view::tab_index(&self.tabs, &repo_id)
                    .is_some_and(|index| self.tabs[index].remotes.open);
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].status = Load::Loading;
                    self.tabs[index].reload_diff = true;
                    if branches_open {
                        self.tabs[index].branches.loading = true;
                    }
                    if stashes_open {
                        self.tabs[index].stashes.loading = true;
                        self.tabs[index].stashes.checkpoints_loading = true;
                    }
                    if remotes_open {
                        self.tabs[index].remotes.loading = true;
                    }
                }
                let mut tasks = vec![
                    self.refresh(repo_id.clone()),
                    self.start_log(repo_id.clone()),
                ];
                if branches_open {
                    tasks.push(self.load_branches(repo_id.clone()));
                }
                if stashes_open {
                    tasks.push(self.load_stashes(repo_id.clone()));
                    tasks.push(self.load_checkpoints(repo_id.clone()));
                }
                if remotes_open {
                    tasks.push(self.load_remotes(repo_id));
                }
                Task::batch(tasks)
            }
            Message::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                    self.show_home = false;
                }
                self.persist_session();
                Task::none()
            }
            Message::CloseTab(repo_id) => {
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    if let Some(session) = self.tabs[index].terminal.session().map(str::to_owned) {
                        if let Err(err) = self.kill_terminal_session(&session) {
                            tracing::debug!(err, "tab close: pty kill failed");
                        }
                    }
                    crate::service::repo::repo_close(&self.services, repo_id);
                    self.tabs.remove(index);
                    if self.tabs.is_empty() {
                        self.active = 0;
                        self.show_home = true;
                    } else {
                        if index < self.active {
                            self.active -= 1;
                        }
                        if self.active >= self.tabs.len() {
                            self.active = self.tabs.len() - 1;
                        }
                    }
                    self.persist_session();
                }
                Task::none()
            }
            Message::GoHome => {
                self.show_home = true;
                // The Home screen shows the SSH devices section; statuses can
                // drift while away, so refresh on the way back.
                self.refresh_devices()
            }
            Message::CycleTheme => {
                self.config.theme = self.config.theme.next();
                self.persist_config();
                Task::none()
            }
            Message::CycleAutoFetch => {
                self.config.auto_fetch_minutes =
                    Config::next_auto_fetch(self.config.auto_fetch_minutes);
                self.persist_config();
                Task::none()
            }
            Message::AutoFetchTick(repo_id) => {
                if self.config.auto_fetch_minutes == 0
                    || repo_view::tab_index(&self.tabs, &repo_id).is_none()
                {
                    return Task::none();
                }
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = auto_fetch_once(&services, &repo_id).await;
                        Message::AutoFetched { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::AutoFetched { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(Some(stats)) => {
                        if !stats.updated_refs.is_empty() {
                            // Silent when up to date (old auto-fetch store
                            // behavior); only moved refs surface.
                            self.last_op = Some(format!(
                                "auto-fetch: {} ref(s) updated",
                                stats.updated_refs.len()
                            ));
                        }
                        if self.tabs[index].branches.open {
                            self.tabs[index].branches.loading = true;
                            return self.load_branches(repo_id);
                        }
                        Task::none()
                    }
                    Ok(None) => Task::none(),
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Auto-fetch failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::CancelOp => {
                let Some(active) = self.active_op.take() else {
                    return Task::none();
                };
                let ActiveOp {
                    repo_id,
                    op_id,
                    kind,
                } = active;
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::service::misc::op_cancel(&services, repo_id, op_id).await;
                        (kind, result)
                    },
                    |(kind, result)| Message::OpCanceled { kind, result },
                )
            }
            Message::CloneCancel => {
                if let Some(cancel) = &self.clone_cancel {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.last_op = Some("cancel requested: clone".to_string());
                }
                Task::none()
            }
            Message::OpCanceled { kind, result } => match result {
                Ok(true) => {
                    self.last_op = Some(format!("cancel requested: {kind}"));
                    Task::none()
                }
                Ok(false) => Task::none(),
                Err(err) => {
                    self.notice = Some(Notice::error(format!("Cancel failed: {err}")));
                    Task::none()
                }
            },
            Message::TogglePin(path) => {
                self.config.toggle_pin(&path);
                self.reindex_recent();
                self.persist_config();
                Task::none()
            }
            Message::RemoveRecent(path) => {
                self.config.remove_recent(&path);
                self.reindex_recent();
                self.persist_config();
                Task::none()
            }
            Message::DismissNotice => {
                self.notice = None;
                Task::none()
            }
            Message::StatusFilterChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.filter = value;
                }
                Task::none()
            }
            Message::StatusSelect { key } => {
                let toggle = self.modifiers.control() || self.modifiers.logo();
                let extend = self.modifiers.shift();
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let rows: Vec<StatusRow> = Self::visible_rows(tab).into_iter().cloned().collect();
                let refs: Vec<&StatusRow> = rows.iter().collect();
                tab.selection.click(&key, &refs, toggle, extend);
                tab.selected_row = Some(key);
                tab.detail_open = false;
                let index = self.active;
                self.ensure_working_diff(index)
            }
            Message::StatusSelectAll => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let rows: Vec<StatusRow> =
                        Self::visible_rows(tab).into_iter().cloned().collect();
                    let refs: Vec<&StatusRow> = rows.iter().collect();
                    tab.selection.select_all(&refs);
                }
                Task::none()
            }
            Message::StatusSelectNone => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.selection.select_none();
                }
                Task::none()
            }
            Message::ToggleSection(section) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.toggle_section(section);
                }
                Task::none()
            }
            Message::StagePaths { paths, unstage } => {
                if paths.is_empty() {
                    return Task::none();
                }
                let label = count_label(if unstage { "Unstage" } else { "Stage" }, paths.len());
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.pending_remap = Some(PendingRemap {
                    paths: paths.iter().cloned().collect::<HashSet<String>>(),
                    unstage,
                });
                tab.reload_diff = true;
                tab.diff.lines.clear();
                let repo_id = tab.repo.repo_id.clone();
                let request = StageRequest {
                    targets: paths
                        .iter()
                        .map(|path| StageTarget::File(path.clone()))
                        .collect(),
                    unstage,
                };
                self.spawn_stage(repo_id, request, label)
            }
            Message::StageFile { path, unstage } => self.update(Message::StagePaths {
                paths: vec![path],
                unstage,
            }),
            Message::StageHunk { slot, file, hunk } => {
                if slot != DiffSlot::Working {
                    return Task::none();
                }
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let mode = tab.working_mode();
                if !mode.line_actions {
                    return Task::none();
                }
                let Some(file_diff) = tab.diff.files().get(file) else {
                    return Task::none();
                };
                let path = file_diff.path.clone();
                let unstage = mode.unstage;
                tab.pending_remap = Some(PendingRemap {
                    paths: HashSet::from([path.clone()]),
                    unstage,
                });
                tab.reload_diff = true;
                tab.diff.lines.clear();
                let repo_id = tab.repo.repo_id.clone();
                let request = StageRequest {
                    targets: vec![StageTarget::Hunk {
                        path,
                        hunk: hunk as u32,
                    }],
                    unstage,
                };
                let label = if unstage {
                    "Unstage hunk"
                } else {
                    "Stage hunk"
                };
                self.spawn_stage(repo_id, request, label.to_string())
            }
            Message::StageSelectedLines { slot } => {
                if slot != DiffSlot::Working {
                    return Task::none();
                }
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let mode = tab.working_mode();
                if !mode.line_actions {
                    return Task::none();
                }
                let mut targets = Vec::new();
                let mut paths = HashSet::new();
                for (file_index, file) in tab.diff.files().iter().enumerate() {
                    for hunk_index in 0..file.hunks.len() {
                        let ranges = tab.diff.lines.ranges(file_index, hunk_index);
                        if ranges.is_empty() {
                            continue;
                        }
                        paths.insert(file.path.clone());
                        targets.push(StageTarget::Lines {
                            path: file.path.clone(),
                            hunk: hunk_index as u32,
                            ranges: ranges
                                .into_iter()
                                .map(|(start, end)| LineRange { start, end })
                                .collect(),
                        });
                    }
                }
                if targets.is_empty() {
                    return Task::none();
                }
                let unstage = mode.unstage;
                tab.pending_remap = Some(PendingRemap { paths, unstage });
                tab.reload_diff = true;
                tab.diff.lines.clear();
                let repo_id = tab.repo.repo_id.clone();
                let label = if unstage {
                    "Unstage lines"
                } else {
                    "Stage lines"
                };
                self.spawn_stage(
                    repo_id,
                    StageRequest { targets, unstage },
                    label.to_string(),
                )
            }
            Message::ClearLineSelection { slot } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    match slot {
                        DiffSlot::Working => tab.diff.lines.clear(),
                        DiffSlot::Commit => tab.commit_diff.lines.clear(),
                    }
                }
                Task::none()
            }
            Message::DiscardPath(path) => self.update(Message::DiscardPaths(vec![path])),
            Message::DiscardPaths(paths) => {
                if paths.is_empty() {
                    return Task::none();
                }
                // Destructive and unrecoverable: confirm before touching disk.
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_discard = Some(paths);
                }
                Task::none()
            }
            Message::ConfirmDiscard => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(paths) = tab.pending_discard.take() else {
                    return Task::none();
                };
                tab.reload_diff = true;
                tab.diff.lines.clear();
                let repo_id = tab.repo.repo_id.clone();
                let targets = paths.into_iter().map(StageTarget::File).collect();
                self.spawn_discard(repo_id, targets)
            }
            Message::CancelDialog => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_discard = None;
                    tab.pending_branch_delete = None;
                    tab.pending_stash_drop = None;
                    tab.pending_checkpoint_restore = None;
                    tab.pending_reset = None;
                    tab.pending_remote_remove = None;
                    tab.pending_files_delete = None;
                    tab.pending_rebase = None;
                    tab.pending_merge = None;
                    tab.pending_pull = None;
                    tab.pending_push = None;
                    tab.pending_ignore_templates = None;
                    tab.pending_clean = None;
                    tab.pending_branch_rename = None;
                    tab.pending_remote_checkout = None;
                    tab.pending_remote_branch_delete = None;
                    tab.pending_tag_at = None;
                    tab.pending_rebase_planner = None;
                    tab.pending_mergetool = None;
                }
                self.devices.pending_forget = None;
                // Global dialogs close on Escape/scrim parity with the
                // per-tab cards; each routes through its own Close so any
                // save/clone guards keep applying.
                if self.clone.open {
                    return self.update(Message::Clone(clone_dialog::Msg::Close));
                }
                if self.ai_settings.open {
                    return self.update(Message::AiSettings(ai_settings::Msg::Close));
                }
                if self.devices.dialog.open {
                    return self.update(Message::Devices(devices::Msg::Dialog(
                        devices::DialogMsg::Cancel,
                    )));
                }
                if self.devices.browse.open {
                    return self.update(Message::Devices(devices::Msg::Browse(
                        devices::BrowseMsg::Close,
                    )));
                }
                self.ai_result = None;
                self.help_open = false;
                Task::none()
            }
            Message::MutationDone {
                repo_id,
                label,
                result,
            } => {
                match &result {
                    // A success supersedes any standing error notice — an
                    // old "discard failed" must not outlive recovery.
                    Ok(()) => {
                        self.last_op = Some(format!("{label} done"));
                        self.notice = None;
                    }
                    Err(err) => {
                        let err = friendly_error(err);
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                    }
                }
                self.refresh(repo_id)
            }
            Message::DiffToggleSplit => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.diff_split = !tab.diff_split;
                    tab.diff.rebuild(tab.diff_split);
                    tab.commit_diff.rebuild(tab.diff_split);
                    self.config.diff_split = tab.diff_split;
                }
                self.persist_config();
                Task::none()
            }
            Message::DiffToggleWrap => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.diff_wrap = !tab.diff_wrap;
                    self.config.diff_wrap = tab.diff_wrap;
                }
                self.persist_config();
                Task::none()
            }
            Message::DiffToggleFile { slot, file } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let split = tab.diff_split;
                    match slot {
                        DiffSlot::Working => tab.diff.toggle_file(file, split),
                        DiffSlot::Commit => tab.commit_diff.toggle_file(file, split),
                    }
                }
                Task::none()
            }
            Message::DiffToggleLine {
                slot,
                file,
                hunk,
                new_no,
            } => {
                let extend = self.modifiers.shift();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    match slot {
                        DiffSlot::Working => tab.diff.lines.toggle(file, hunk, new_no, extend),
                        DiffSlot::Commit => {
                            tab.commit_diff.lines.toggle(file, hunk, new_no, extend)
                        }
                    }
                }
                Task::none()
            }
            Message::DiffScrolled { slot, y, viewport } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let pane = match slot {
                        DiffSlot::Working => &mut tab.diff,
                        DiffSlot::Commit => &mut tab.commit_diff,
                    };
                    pane.scroll = y;
                    pane.viewport = viewport;
                }
                Task::none()
            }
            Message::DiffLoaded {
                repo_id,
                slot,
                key,
                seq,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let tab = &mut self.tabs[index];
                let split = tab.diff_split;
                let pane = match slot {
                    DiffSlot::Working => &mut tab.diff,
                    DiffSlot::Commit => &mut tab.commit_diff,
                };
                if pane.seq != seq || pane.key.as_deref() != Some(key.as_str()) {
                    return Task::none();
                }
                match result {
                    Ok(files) => pane.set_ready(files, split),
                    Err(err) => pane.set_failed(err),
                }
                self.ensure_images(index, slot)
            }
            Message::CommitMessageChanged(action) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.message.perform(action);
                    tab.commit_error = None;
                }
                Task::none()
            }
            Message::CommitAmend(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.amend = value;
                    // `git commit --amend` keeps the previous message:
                    // prefill the editor with HEAD's so the user edits it
                    // instead of retyping from empty (which the engine
                    // refuses). Only prefill an empty box — never clobber
                    // a message the user already typed. HEAD is the log's
                    // first commit when unfiltered; prefer the commit that
                    // carries the HEAD decoration.
                    if value && tab.message.text().trim().is_empty() {
                        let head = tab
                            .history
                            .commits
                            .iter()
                            .find(|commit| commit.refs.iter().any(|r| r.starts_with("HEAD")))
                            .or_else(|| tab.history.commits.first())
                            .map(|commit| commit.message.clone());
                        if let Some(message) = head {
                            tab.message = text_editor::Content::with_text(&message);
                        }
                    }
                }
                Task::none()
            }
            Message::CommitNoVerify(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.no_verify = value;
                }
                Task::none()
            }
            Message::CommitPressed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                if !tab.can_commit() {
                    return Task::none();
                }
                let options = CommitOptions {
                    message: tab.message.text(),
                    amend: tab.amend,
                    no_verify: tab.no_verify,
                    allow_empty: tab.amend,
                    author: None,
                };
                tab.committing = true;
                tab.commit_error = None;
                let repo_id = tab.repo.repo_id.clone();
                self.spawn_commit(repo_id, options)
            }
            Message::Committed { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(sha) => {
                        {
                            let tab = &mut self.tabs[index];
                            tab.committing = false;
                            tab.commit_error = None;
                            tab.message = text_editor::Content::with_text("");
                            tab.amend = false;
                            tab.no_verify = false;
                            tab.reload_diff = true;
                            tab.diff.lines.clear();
                            tab.pending_remap = None;
                        }
                        self.last_op = Some(format!("Committed {}", repo_view::short_sha(&sha)));
                        self.notice = None;
                        Task::batch([self.refresh(repo_id.clone()), self.start_log(repo_id)])
                    }
                    Err(err) => {
                        let tab = &mut self.tabs[index];
                        tab.committing = false;
                        tab.commit_error = Some(err);
                        Task::none()
                    }
                }
            }
            Message::SelectCommit(sha) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.history.selected = Some(sha);
                tab.detail_open = true;
                let index = self.active;
                self.ensure_commit_diff(index)
            }
            Message::CloseDetail => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.detail_open = false;
                }
                let index = self.active;
                self.ensure_working_diff(index)
            }
            Message::ToggleHistory => {
                let mut start = None;
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.history_open = !tab.history_open;
                    if tab.history_open && tab.history.is_empty() && !tab.history.loading {
                        start = Some(tab.repo.repo_id.clone());
                    }
                }
                match start {
                    Some(repo_id) => self.start_log(repo_id),
                    None => Task::none(),
                }
            }
            Message::HistoryScrolled { y, viewport } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.history.scroll = y;
                    tab.history.viewport = viewport;
                }
                Task::none()
            }
            Message::RefreshLog(repo_id) => self.start_log(repo_id),
            Message::HistoryFilterToggle => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.history_filter_open = !tab.history_filter_open;
                }
                Task::none()
            }
            Message::HistoryFilterChanged { field, value } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    match field {
                        HistoryFilterField::Pickaxe => tab.history_filter.set_pickaxe(value),
                        _ => *tab.history_filter.field_mut(field) = value,
                    }
                }
                Task::none()
            }
            Message::HistoryFilterRegex(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.history_filter.regex = value;
                }
                Task::none()
            }
            Message::HistoryFilterFollow(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.history_filter.follow = value;
                }
                Task::none()
            }
            Message::HistoryFilterPickaxeRegex(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.history_filter.set_pickaxe_regex(value);
                }
                Task::none()
            }
            Message::HistoryFilterApply => self.apply_history_filter(false),
            Message::HistoryFilterClear => self.apply_history_filter(true),
            Message::ToggleBranchPanel => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.branches.open = !tab.branches.open;
                if !tab.branches.open {
                    return Task::none();
                }
                tab.stashes.open = false;
                tab.remotes.open = false;
                tab.branches.loading = true;
                tab.branches.error = None;
                tab.branches.remote_loading = true;
                tab.branches.remote_error = None;
                let repo_id = tab.repo.repo_id.clone();
                Task::batch([
                    self.load_branches(repo_id.clone()),
                    self.load_remote_branches(repo_id),
                ])
            }
            Message::RefreshBranches => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.branches.loading = true;
                tab.branches.error = None;
                tab.branches.remote_loading = true;
                tab.branches.remote_error = None;
                let repo_id = tab.repo.repo_id.clone();
                Task::batch([
                    self.load_branches(repo_id.clone()),
                    self.load_remote_branches(repo_id),
                ])
            }
            Message::BranchesLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let branches = &mut self.tabs[index].branches;
                branches.loading = false;
                match result {
                    Ok(list) => {
                        branches.list = Some(list);
                        branches.error = None;
                    }
                    Err(err) => branches.error = Some(err),
                }
                Task::none()
            }
            Message::RemoteBranchesLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let branches = &mut self.tabs[index].branches;
                branches.remote_loading = false;
                match result {
                    Ok(list) => {
                        branches.remote_list = Some(list);
                        branches.remote_error = None;
                    }
                    Err(err) => branches.remote_error = Some(err),
                }
                Task::none()
            }
            Message::BranchCreateNameChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.branches.create_name = value;
                }
                Task::none()
            }
            Message::BranchCreate => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let name = tab.branches.create_name.trim().to_string();
                if name.is_empty() {
                    return Task::none();
                }
                let from = tab.branches.create_from.take();
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Create branch {name}");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::branch_create(
                            &services,
                            repo_id.clone(),
                            name,
                            from,
                            true,
                        )
                        .await;
                        Message::BranchDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RemoteBranchCheckout { remote, name } => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                // Dirty working tree: confirm first (uncommitted changes can
                // block or bleed into the checkout).
                if tab.working_tree_dirty() {
                    tab.pending_remote_checkout = Some((remote, name));
                    return Task::none();
                }
                let repo_id = tab.repo.repo_id.clone();
                self.checkout_remote_branch(repo_id, remote, name)
            }
            Message::RemoteBranchCheckoutConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some((remote, name)) = tab.pending_remote_checkout.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                self.checkout_remote_branch(repo_id, remote, name)
            }
            Message::RemoteCheckoutDone { repo_id, result } => {
                match &result {
                    Ok(branch) => {
                        self.notice = Some(Notice::plain(format!("Checked out {branch}")));
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Remote checkout failed: {err}")));
                    }
                }
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    let tab = &mut self.tabs[index];
                    tab.history.selected = None;
                    tab.detail_open = false;
                    tab.commit_diff = DiffPane::default();
                    tab.reload_diff = true;
                    tab.branches.loading = true;
                }
                Task::batch([
                    self.refresh(repo_id.clone()),
                    self.start_log(repo_id.clone()),
                    self.load_branches(repo_id.clone()),
                    self.load_remote_branches(repo_id),
                ])
            }
            Message::RemoteBranchDeleteRequested { remote, name } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_remote_branch_delete = Some((remote, name));
                }
                Task::none()
            }
            Message::RemoteBranchDeleteConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some((remote, name)) = tab.pending_remote_branch_delete.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        // `PushOptions.delete` with the branch's full ref
                        // name removes it on the remote.
                        let options = PushOptions {
                            remote,
                            refs: vec![format!("refs/heads/{name}")],
                            delete: true,
                            ..PushOptions::default()
                        };
                        let result =
                            crate::service::network::push(&services, repo_id.clone(), options)
                                .await;
                        Message::RemoteBranchDeleteDone { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::RemoteBranchDeleteDone { repo_id, result } => {
                match &result {
                    Ok(_) => self.last_op = Some("Deleted remote branch".to_string()),
                    Err(err) => {
                        self.notice =
                            Some(Notice::error(format!("Remote branch delete failed: {err}")));
                    }
                }
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].branches.remote_loading = true;
                    self.tabs[index].branches.loading = true;
                }
                Task::batch([
                    self.refresh(repo_id.clone()),
                    self.load_branches(repo_id.clone()),
                    self.load_remote_branches(repo_id),
                ])
            }
            Message::TagAtRequested(sha) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_tag_at = Some(dialog::TagAtRequest {
                        sha,
                        ..dialog::TagAtRequest::default()
                    });
                }
                Task::none()
            }
            Message::TagAtNameChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_tag_at {
                        request.name = value;
                    }
                }
                Task::none()
            }
            Message::TagAtAnnotatedToggled(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_tag_at {
                        request.annotated = value;
                    }
                }
                Task::none()
            }
            Message::TagAtMessageChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_tag_at {
                        request.message = value;
                    }
                }
                Task::none()
            }
            Message::TagAtConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_tag_at.take() else {
                    return Task::none();
                };
                let (name, message) = request.service_args();
                if name.is_empty() {
                    return Task::none();
                }
                let repo_id = tab.repo.repo_id.clone();
                let sha = request.sha;
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::tags::tag_create(
                            &services,
                            repo_id.clone(),
                            name.clone(),
                            Some(sha),
                            message,
                            false,
                        )
                        .await;
                        Message::TagAtDone {
                            repo_id,
                            name,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::TagAtDone {
                repo_id,
                name,
                result,
            } => {
                match &result {
                    Ok(()) => self.last_op = Some(format!("Created tag {name}")),
                    Err(err) => self.notice = Some(Notice::error(format!("Tag failed: {err}"))),
                }
                let mut task = self.refresh(repo_id);
                // Keep the tags panel current when it is open.
                if self.tabs.get(self.active).is_some_and(|tab| tab.tags.open) {
                    task = Task::batch([
                        task,
                        self.update(Message::Tags(tags_panel::Msg::RefreshPressed)),
                    ]);
                }
                task
            }
            Message::CopySelectedLines { slot } => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let pane = match slot {
                    DiffSlot::Working => &tab.diff,
                    DiffSlot::Commit => &tab.commit_diff,
                };
                let texts = diff_model::selected_line_texts(pane.files(), &pane.lines);
                if texts.is_empty() {
                    return Task::none();
                }
                let text = texts.join("\n");
                self.update(Message::CopyText(text))
            }
            Message::CopyDescribe(sha) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        Message::DescribeCopied(
                            crate::service::branches::describe(&services, repo_id, sha).await,
                        )
                    },
                    std::convert::identity,
                )
            }
            Message::DescribeCopied(result) => match result {
                Ok(describe) => {
                    self.last_op = Some("Copied describe".to_string());
                    iced::clipboard::write::<Message>(describe)
                }
                Err(err) => {
                    self.notice = Some(Notice::error(format!("Describe failed: {err}")));
                    Task::none()
                }
            },
            Message::BranchSwitch(name) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                // Same dirty-tree guard the remote checkout has: a switch
                // that would conflict surfaces as a confirm (with a stash
                // shortcut) instead of a raw engine error.
                if tab.working_tree_dirty() {
                    tab.pending_local_checkout = Some(name);
                    return Task::none();
                }
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Switch to {name}");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::branch_switch(
                            &services,
                            repo_id.clone(),
                            name,
                            false,
                        )
                        .await;
                        Message::BranchDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::LocalCheckoutConfirmed => {
                let name = match self.tabs.get_mut(self.active) {
                    Some(tab) => tab.pending_local_checkout.take(),
                    None => None,
                };
                match name {
                    Some(name) => self.update(Message::BranchSwitch(name)),
                    None => Task::none(),
                }
            }
            Message::LocalCheckoutStashAndSwitch => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(name) = tab.pending_local_checkout.take() else {
                    return Task::none();
                };
                let include_untracked = tab.stashes.include_untracked;
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                // Stash with a descriptive message; StashesDone chains the
                // switch when the label matches (see that arm).
                Task::perform(
                    async move {
                        let result = crate::service::stash::stash_push(
                            &services,
                            repo_id.clone(),
                            Some(format!("Before switching to {name}")),
                            false,
                            include_untracked,
                        )
                        .await;
                        Message::StashesDone {
                            repo_id,
                            label: format!("Stash and switch:{name}"),
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::BranchDeleteRequested(name) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_branch_delete = Some(name);
                }
                Task::none()
            }
            Message::BranchDeleteConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(name) = tab.pending_branch_delete.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Delete branch {name}");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::branch_delete(
                            &services,
                            repo_id.clone(),
                            name,
                            false,
                        )
                        .await;
                        Message::BranchDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::BranchDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(()) => {
                        self.last_op = Some(format!("{label} done"));
                        self.notice = None;
                        let tab = &mut self.tabs[index];
                        tab.history.selected = None;
                        tab.detail_open = false;
                        tab.commit_diff = DiffPane::default();
                        tab.reload_diff = true;
                        tab.branches.create_name.clear();
                        tab.branches.loading = true;
                        let mut tasks = vec![
                            self.refresh(repo_id.clone()),
                            self.start_log(repo_id.clone()),
                            self.load_branches(repo_id.clone()),
                        ];
                        // A deleted branch just moved into the trash; keep
                        // the stash panel's trash section current.
                        if self.tabs[index].stashes.open {
                            self.tabs[index].stashes.trash_loading = true;
                            tasks.push(self.load_trash(repo_id));
                        }
                        Task::batch(tasks)
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::ToggleStashPanel => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.stashes.open = !tab.stashes.open;
                if !tab.stashes.open {
                    return Task::none();
                }
                tab.branches.open = false;
                tab.remotes.open = false;
                tab.stashes.loading = true;
                tab.stashes.checkpoints_loading = true;
                tab.stashes.trash_loading = true;
                tab.stashes.error = None;
                tab.stashes.checkpoints_error = None;
                tab.stashes.trash_error = None;
                let repo_id = tab.repo.repo_id.clone();
                Task::batch([
                    self.load_stashes(repo_id.clone()),
                    self.load_checkpoints(repo_id.clone()),
                    self.load_trash(repo_id),
                ])
            }
            Message::RefreshStashes => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.stashes.loading = true;
                tab.stashes.checkpoints_loading = true;
                tab.stashes.trash_loading = true;
                tab.stashes.error = None;
                tab.stashes.checkpoints_error = None;
                tab.stashes.trash_error = None;
                let repo_id = tab.repo.repo_id.clone();
                Task::batch([
                    self.load_stashes(repo_id.clone()),
                    self.load_checkpoints(repo_id.clone()),
                    self.load_trash(repo_id),
                ])
            }
            Message::StashesLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let stashes = &mut self.tabs[index].stashes;
                stashes.loading = false;
                match result {
                    Ok(list) => {
                        stashes.list = Some(list);
                        stashes.error = None;
                    }
                    Err(err) => stashes.error = Some(err),
                }
                Task::none()
            }
            Message::CheckpointsLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let stashes = &mut self.tabs[index].stashes;
                stashes.checkpoints_loading = false;
                match result {
                    Ok(list) => {
                        stashes.checkpoints = Some(list);
                        stashes.checkpoints_error = None;
                    }
                    Err(err) => stashes.checkpoints_error = Some(err),
                }
                Task::none()
            }
            Message::RefreshTrash => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.stashes.trash_loading = true;
                tab.stashes.trash_error = None;
                let repo_id = tab.repo.repo_id.clone();
                self.load_trash(repo_id)
            }
            Message::TrashLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let stashes = &mut self.tabs[index].stashes;
                stashes.trash_loading = false;
                match result {
                    Ok(list) => {
                        stashes.trash = Some(list);
                        stashes.trash_error = None;
                    }
                    Err(err) => stashes.trash_error = Some(err),
                }
                Task::none()
            }
            Message::TrashRestore(id) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::branch_trash_restore(
                            &services,
                            repo_id.clone(),
                            id,
                            None,
                        )
                        .await;
                        Message::TrashRestoreDone { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::TrashRestoreDone { repo_id, result } => {
                match &result {
                    Ok(name) => {
                        self.notice = Some(Notice::plain(format!("Restored branch {name}")))
                    }
                    Err(err) => self.notice = Some(Notice::error(format!("Restore failed: {err}"))),
                }
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].stashes.trash_loading = true;
                    self.tabs[index].branches.loading = true;
                }
                Task::batch([
                    self.refresh(repo_id.clone()),
                    self.load_trash(repo_id.clone()),
                    self.load_branches(repo_id),
                ])
            }
            Message::StashMessageChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.stashes.message = value;
                }
                Task::none()
            }
            Message::StashIncludeUntracked(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.stashes.include_untracked = value;
                }
                Task::none()
            }
            Message::StashKeepIndex(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.stashes.keep_index = value;
                }
                Task::none()
            }
            Message::StashPush => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let message = tab.stashes.message.trim().to_string();
                let include_untracked = tab.stashes.include_untracked;
                let keep_index = tab.stashes.keep_index;
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::stash::stash_push(
                            &services,
                            repo_id.clone(),
                            (!message.is_empty()).then_some(message),
                            keep_index,
                            include_untracked,
                        )
                        .await;
                        Message::StashesDone {
                            repo_id,
                            label: "Stash".to_string(),
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::StashApply { index, pop } => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("{} stash@{{{index}}}", if pop { "Pop" } else { "Apply" });
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::stash::stash_apply(
                            &services,
                            repo_id.clone(),
                            index,
                            pop,
                        )
                        .await;
                        Message::StashesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::StashDropRequested(index) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let message = tab
                        .stashes
                        .list
                        .as_ref()
                        .and_then(|list| list.iter().find(|stash| stash.index == index))
                        .map(|stash| stash.message.clone())
                        .unwrap_or_default();
                    tab.pending_stash_drop = Some((index, message));
                }
                Task::none()
            }
            Message::StashDropConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some((index, _)) = tab.pending_stash_drop.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Drop stash@{{{index}}}");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::service::stash::stash_drop(&services, repo_id.clone(), index)
                                .await;
                        Message::StashesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::StashesDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                // "Stash and switch:<branch>" chains the switch after a
                // successful stash (the dirty-checkout confirm's shortcut).
                if result.is_ok() {
                    if let Some(branch) = label.strip_prefix("Stash and switch:") {
                        return self.update(Message::BranchSwitch(branch.to_string()));
                    }
                }
                match result {
                    Ok(()) => {
                        self.last_op = Some(format!("{label} done"));
                        self.notice = None;
                        let tab = &mut self.tabs[index];
                        if label == "Stash" {
                            tab.stashes.message.clear();
                        }
                        tab.stashes.loading = true;
                        tab.reload_diff = true;
                        tab.status = Load::Loading;
                        Task::batch([self.refresh(repo_id.clone()), self.load_stashes(repo_id)])
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::CheckpointCreate => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::misc::checkpoint_create(
                            &services,
                            repo_id.clone(),
                            "manual".to_string(),
                        )
                        .await
                        .map(|_| ());
                        Message::CheckpointsDone {
                            repo_id,
                            label: "Checkpoint".to_string(),
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::CheckpointRestoreRequested { id, reason } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_checkpoint_restore = Some((id, reason));
                }
                Task::none()
            }
            Message::CheckpointRestoreConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some((id, _)) = tab.pending_checkpoint_restore.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = "Restore checkpoint".to_string();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::misc::checkpoint_restore(
                            &services,
                            repo_id.clone(),
                            id,
                        )
                        .await;
                        Message::CheckpointsDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::CheckpointsDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(()) => {
                        self.last_op = Some(format!("{label} done"));
                        self.notice = None;
                        self.tabs[index].stashes.checkpoints_loading = true;
                        self.tabs[index].reload_diff = true;
                        Task::batch([
                            self.refresh(repo_id.clone()),
                            self.load_checkpoints(repo_id),
                        ])
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::CheckpointGc => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::service::misc::checkpoint_gc(&services, repo_id.clone(), 30)
                                .await;
                        Message::CheckpointsGcDone { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::CheckpointsGcDone { repo_id, result } => {
                match result {
                    Ok(freed) => {
                        self.notice = Some(Notice::plain(format!(
                            "GC freed {freed} checkpoint{} (older than 30 days)",
                            if freed == 1 { "" } else { "s" }
                        )));
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Checkpoint GC failed: {err}")));
                    }
                }
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].stashes.checkpoints_loading = true;
                }
                self.load_checkpoints(repo_id)
            }
            Message::ResetRequested(sha) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let summary = tab
                    .history
                    .selected_commit()
                    .map(|commit| commit.summary.clone())
                    .unwrap_or_default();
                tab.pending_reset = Some(ResetRequest {
                    sha,
                    summary,
                    kind: ResetKind::Mixed,
                    preview: None,
                });
                Task::none()
            }
            Message::ResetKindSelected(kind) => {
                let mut task = Task::none();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_reset {
                        request.kind = kind;
                        request.preview = None;
                        if dialog::reset_kind_discards_worktree(kind) {
                            let repo_id = tab.repo.repo_id.clone();
                            let sha = request.sha.clone();
                            let services = self.services.clone();
                            task = Task::perform(
                                async move {
                                    // Both param spellings: the libgit2
                                    // engine reads "to", the remote engine
                                    // "target".
                                    let params = serde_json::json!({ "to": sha, "target": sha });
                                    let result = crate::service::misc::ops_preview(
                                        &services,
                                        repo_id.clone(),
                                        "reset_hard".to_string(),
                                        params,
                                    )
                                    .await;
                                    Message::ResetPreviewLoaded { repo_id, result }
                                },
                                std::convert::identity,
                            );
                        }
                    }
                }
                task
            }
            Message::ResetPreviewLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                // Advisory only: a failed preview just leaves the note out.
                if let (Ok(preview), Some(request)) = (result, &mut self.tabs[index].pending_reset)
                {
                    request.preview = Some(preview);
                }
                Task::none()
            }
            Message::ResetConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_reset.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!(
                    "Reset ({}) to {}",
                    dialog::reset_kind_label(request.kind),
                    repo_view::short_sha(&request.sha)
                );
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let guarded = crate::service::misc::guard_checkpoint(
                            &services,
                            repo_id.clone(),
                            "pre-reset".to_string(),
                        )
                        .await;
                        let result = match guarded {
                            Ok(_) => {
                                crate::service::merge::reset(
                                    &services,
                                    repo_id.clone(),
                                    request.kind,
                                    request.sha,
                                )
                                .await
                            }
                            Err(err) => Err(err),
                        };
                        Message::ResetDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::ResetDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(()) => {
                        self.last_op = Some(format!("{label} done"));
                        self.notice = None;
                        let tab = &mut self.tabs[index];
                        tab.history.selected = None;
                        tab.detail_open = false;
                        tab.commit_diff = DiffPane::default();
                        tab.reload_diff = true;
                        Task::batch([self.refresh(repo_id.clone()), self.start_log(repo_id)])
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::ToggleRemotePanel => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.remotes.open = !tab.remotes.open;
                if !tab.remotes.open {
                    return Task::none();
                }
                tab.branches.open = false;
                tab.stashes.open = false;
                tab.remotes.loading = true;
                tab.remotes.error = None;
                let repo_id = tab.repo.repo_id.clone();
                self.load_remotes(repo_id)
            }
            Message::RefreshRemotes => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.remotes.loading = true;
                tab.remotes.error = None;
                let repo_id = tab.repo.repo_id.clone();
                self.load_remotes(repo_id)
            }
            Message::RemotesLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let remotes = &mut self.tabs[index].remotes;
                remotes.loading = false;
                match result {
                    Ok(list) => {
                        remotes.list = Some(list);
                        remotes.error = None;
                    }
                    Err(err) => remotes.error = Some(err),
                }
                Task::none()
            }
            Message::RemoteNameChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.remotes.add_name = value;
                }
                Task::none()
            }
            Message::RemoteUrlChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.remotes.add_url = value;
                }
                Task::none()
            }
            Message::RemoteAdd => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let name = tab.remotes.add_name.trim().to_string();
                let url = tab.remotes.add_url.trim().to_string();
                if name.is_empty() || url.is_empty() {
                    return Task::none();
                }
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Add remote {name}");
                let summary = format!("{label} done");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::remote_add(
                            &services,
                            repo_id.clone(),
                            name,
                            url,
                        )
                        .await
                        .map(|_| summary);
                        Message::RemotesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::FetchRemote(remote) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                // The row's buttons render disabled ("…") while busy, so a
                // double-click can't enqueue duplicate fetches.
                tab.remotes.busy = Some(remote.clone());
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("fetch {remote}");
                let summary_label = label.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::fetch(
                            &services,
                            repo_id.clone(),
                            FetchOptions {
                                remote,
                                prune: false,
                                refs: Vec::new(),
                                depth: None,
                            },
                        )
                        .await
                        .map(|stats| net_summary(&summary_label, &stats));
                        Message::RemotesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::PullRemote(remote) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_pull = Some(dialog::PullRequest {
                        remote,
                        ..Default::default()
                    });
                }
                Task::none()
            }
            Message::PullBranchChanged(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_pull.as_mut())
                {
                    request.branch = value;
                }
                Task::none()
            }
            Message::PullRebaseToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_pull.as_mut())
                {
                    request.rebase = value;
                }
                Task::none()
            }
            Message::PullFfOnlyToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_pull.as_mut())
                {
                    request.ff_only = value;
                }
                Task::none()
            }
            Message::PullConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_pull.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                tab.remotes.busy = Some(request.remote.clone());
                let label = format!("pull {}", request.remote);
                let summary_label = label.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::pull(
                            &services,
                            repo_id.clone(),
                            PullOptions {
                                remote: request.remote,
                                branch: request.branch.trim().to_string(),
                                ff_only: request.ff_only,
                                rebase: request.rebase,
                            },
                        )
                        .await
                        .map(|stats| net_summary(&summary_label, &stats));
                        Message::RemotesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::PushRemote(remote) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_push = Some(dialog::PushRequest {
                        remote,
                        ..Default::default()
                    });
                }
                Task::none()
            }
            Message::PushForceToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_push.as_mut())
                {
                    request.force = value;
                    // Arming (or disarming) force always resets the typed
                    // confirmation — it must be re-typed for this attempt.
                    request.confirm_typed.clear();
                    if value {
                        // The two force modes are mutually exclusive.
                        request.force_with_lease = false;
                    }
                }
                Task::none()
            }
            Message::PushConfirmChanged(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_push.as_mut())
                {
                    request.confirm_typed = value;
                }
                Task::none()
            }
            Message::PushForceLeaseToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_push.as_mut())
                {
                    request.force_with_lease = value;
                    if value {
                        request.force = false;
                    }
                }
                Task::none()
            }
            Message::PushTagsToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_push.as_mut())
                {
                    request.tags = value;
                }
                Task::none()
            }
            Message::PushConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_push.take() else {
                    return Task::none();
                };
                // Backstop for the view-level gate: plain force must have
                // been typed-confirmed with the remote name.
                if request.force_needs_confirm() {
                    return Task::none();
                }
                let repo_id = tab.repo.repo_id.clone();
                tab.remotes.busy = Some(request.remote.clone());
                let label = format!("push {}", request.remote);
                let summary_label = label.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::push(
                            &services,
                            repo_id.clone(),
                            PushOptions {
                                remote: request.remote,
                                branch: String::new(),
                                force: request.force,
                                force_with_lease: request.force_with_lease,
                                set_upstream: true,
                                refs: Vec::new(),
                                tags: request.tags,
                                delete: false,
                            },
                        )
                        .await
                        .map(|stats| net_summary(&summary_label, &stats));
                        Message::RemotesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RemoteRemoveRequested(name) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_remote_remove = Some(name);
                }
                Task::none()
            }
            Message::RemoteRemoveConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(name) = tab.pending_remote_remove.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Remove remote {name}");
                let summary = format!("{label} done");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::remote_remove(
                            &services,
                            repo_id.clone(),
                            name,
                        )
                        .await
                        .map(|_| summary);
                        Message::RemotesDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RemotesDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(summary) => {
                        self.last_op = Some(summary);
                        self.notice = None;
                        let tab = &mut self.tabs[index];
                        tab.remotes.busy = None;
                        if label.starts_with("Add remote") {
                            tab.remotes.add_name.clear();
                            tab.remotes.add_url.clear();
                        }
                        tab.remotes.loading = true;
                        Task::batch([
                            self.refresh(repo_id.clone()),
                            self.start_log(repo_id.clone()),
                            self.load_remotes(repo_id),
                        ])
                    }
                    Err(err) => {
                        self.tabs[index].remotes.busy = None;
                        self.notice = Some(Notice::error(format!(
                            "{label} failed: {}",
                            friendly_error(&err)
                        )));
                        Task::none()
                    }
                }
            }
            Message::MergeBranch(name) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_merge = Some(dialog::MergeRequest::new(name));
                }
                Task::none()
            }
            Message::MergeNoFfSelected(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_merge.as_mut())
                {
                    request.no_ff = value;
                }
                Task::none()
            }
            Message::MergeSquashToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_merge.as_mut())
                {
                    request.squash = value;
                }
                Task::none()
            }
            Message::MergeNoCommitToggled(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_merge.as_mut())
                {
                    request.no_commit = value;
                }
                Task::none()
            }
            Message::MergeFavorSelected(value) => {
                if let Some(request) = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|tab| tab.pending_merge.as_mut())
                {
                    request.favor = value;
                }
                Task::none()
            }
            Message::MergeConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_merge.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Merge {}", request.name);
                let options = request.options();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::merge::merge_branch(
                            &services,
                            repo_id.clone(),
                            request.name,
                            options,
                        )
                        .await;
                        Message::MergeDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::MergeDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(merge) => {
                        let detail = match merge.outcome {
                            MergeOutcome::FastForward => merge
                                .new_head
                                .as_deref()
                                .map(|sha| {
                                    format!("fast-forwarded to {}", repo_view::short_sha(sha))
                                })
                                .unwrap_or_else(|| "fast-forwarded".to_string()),
                            MergeOutcome::Merged => "merge commit created".to_string(),
                            MergeOutcome::UpToDate => "already up to date".to_string(),
                            MergeOutcome::Conflicted => format!(
                                "{} conflicts — resolve them, then commit",
                                merge.conflicts.len()
                            ),
                            MergeOutcome::Squashed => "squashed into the index".to_string(),
                            MergeOutcome::NoCommit => "merged, not committed".to_string(),
                        };
                        self.last_op = Some(format!("{label}: {detail}"));
                        self.notice = None;
                        let branches_open = self.tabs[index].branches.open;
                        self.tabs[index].reload_diff = true;
                        let mut tasks = vec![
                            self.refresh(repo_id.clone()),
                            self.start_log(repo_id.clone()),
                        ];
                        if branches_open {
                            self.tabs[index].branches.loading = true;
                            tasks.push(self.load_branches(repo_id));
                        }
                        Task::batch(tasks)
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!(
                            "{label} failed: {}",
                            friendly_error(&err)
                        )));
                        Task::none()
                    }
                }
            }
            Message::RebaseOntoRequested(onto) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::merge::rebase_plan(
                            &services,
                            repo_id.clone(),
                            onto.clone(),
                        )
                        .await;
                        Message::RebasePlanLoaded {
                            repo_id,
                            onto,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RebasePlanLoaded {
                repo_id,
                onto,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(plan) if plan.is_empty() => {
                        self.notice = Some(Notice::error(format!(
                            "Nothing to rebase: the current branch has no commits past {onto}"
                        )));
                        Task::none()
                    }
                    Ok(plan) => {
                        self.tabs[index].pending_rebase = Some(PendingRebase { onto, plan });
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!(
                            "Could not plan rebase onto {onto}: {err}"
                        )));
                        Task::none()
                    }
                }
            }
            Message::RebaseConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(rebase) = tab.pending_rebase.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::merge::rebase_start(
                            &services,
                            repo_id.clone(),
                            rebase.plan,
                            Some(rebase.onto),
                        )
                        .await;
                        Message::RebaseDone { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::RebaseDone { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(state) => {
                        let detail = if state.active {
                            format!(
                                "paused at step {} — resolve conflicts, then continue",
                                state.current + 1
                            )
                        } else {
                            "done".to_string()
                        };
                        self.last_op = Some(format!("Rebase {detail}"));
                        self.notice = None;
                        let branches_open = self.tabs[index].branches.open;
                        {
                            let tab = &mut self.tabs[index];
                            tab.history.selected = None;
                            tab.detail_open = false;
                            tab.commit_diff = DiffPane::default();
                            tab.reload_diff = true;
                            tab.status = Load::Loading;
                            if branches_open {
                                tab.branches.loading = true;
                            }
                        }
                        let mut tasks = vec![
                            self.refresh(repo_id.clone()),
                            self.start_log(repo_id.clone()),
                        ];
                        if branches_open {
                            tasks.push(self.load_branches(repo_id));
                        }
                        Task::batch(tasks)
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Rebase failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::RebasePlannerRequested(onto) => {
                // A running rebase owns the worktree; planning a second one
                // would race it.
                let rebasing = self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.status.ready().is_some_and(|status| status.rebasing));
                if rebasing {
                    self.notice = Some(Notice::error(
                        "A rebase is already in progress — continue or abort it first".to_string(),
                    ));
                    return Task::none();
                }
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let onto_summary = tab
                    .history
                    .commits
                    .iter()
                    .find(|commit| commit.sha == onto)
                    .map(|commit| commit.summary.clone())
                    .unwrap_or_default();
                tab.pending_rebase_planner =
                    Some(rebase_planner::Planner::new(onto.clone(), onto_summary));
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::merge::rebase_plan(
                            &services,
                            repo_id.clone(),
                            onto.clone(),
                        )
                        .await;
                        Message::RebasePlannerPlanLoaded {
                            repo_id,
                            onto,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RebasePlannerPlanLoaded {
                repo_id,
                onto,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                // Stale load (the planner was closed or reopened on another
                // commit): drop.
                let Some(planner) = self.tabs[index].pending_rebase_planner.as_mut() else {
                    return Task::none();
                };
                if planner.onto != onto {
                    return Task::none();
                }
                match result {
                    Ok(plan) if plan.is_empty() => {
                        self.tabs[index].pending_rebase_planner = None;
                        self.notice = Some(Notice::error(format!(
                            "Nothing to rebase: the current branch has no commits past {onto}"
                        )));
                    }
                    Ok(plan) => {
                        load_planner_plan(&mut self.tabs[index], &onto, plan);
                    }
                    Err(err) => {
                        let Some(planner) = self.tabs[index].pending_rebase_planner.as_mut() else {
                            return Task::none();
                        };
                        if planner.onto == onto {
                            planner.loading = false;
                            planner.error = Some(err);
                        }
                    }
                }
                Task::none()
            }
            Message::RebasePlannerAutosquashLoaded {
                repo_id,
                onto,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(plan) => {
                        load_planner_plan(&mut self.tabs[index], &onto, plan);
                    }
                    Err(err) => {
                        // Keep the authored rows; surface why the rebuild
                        // failed.
                        let Some(planner) = self.tabs[index].pending_rebase_planner.as_mut() else {
                            return Task::none();
                        };
                        if planner.onto == onto {
                            planner.loading = false;
                            planner.error = Some(err);
                        }
                    }
                }
                Task::none()
            }
            Message::RebasePlanner(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(action) = tab
                    .pending_rebase_planner
                    .as_mut()
                    .and_then(|planner| planner.update(msg))
                else {
                    return Task::none();
                };
                match action {
                    rebase_planner::Action::Autosquash => {
                        let Some(onto) = tab
                            .pending_rebase_planner
                            .as_ref()
                            .map(|planner| planner.onto.clone())
                        else {
                            return Task::none();
                        };
                        let repo_id = tab.repo.repo_id.clone();
                        let services = self.services.clone();
                        Task::perform(
                            async move {
                                let result = crate::service::branches::autosquash_plan(
                                    &services,
                                    repo_id.clone(),
                                    onto.clone(),
                                )
                                .await;
                                Message::RebasePlannerAutosquashLoaded {
                                    repo_id,
                                    onto,
                                    result,
                                }
                            },
                            std::convert::identity,
                        )
                    }
                    rebase_planner::Action::Start { onto, steps } => {
                        // The dialog disables Start while the tree is dirty;
                        // the guard here keeps a stale press from running.
                        if tab.working_tree_dirty() {
                            return Task::none();
                        }
                        tab.pending_rebase_planner = None;
                        let repo_id = tab.repo.repo_id.clone();
                        let services = self.services.clone();
                        Task::perform(
                            async move {
                                let result = crate::service::merge::rebase_start(
                                    &services,
                                    repo_id.clone(),
                                    steps,
                                    Some(onto),
                                )
                                .await;
                                Message::RebaseDone { repo_id, result }
                            },
                            std::convert::identity,
                        )
                    }
                }
            }
            Message::InProgress(action) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = action.label().to_string();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = match action {
                            InProgressAction::AbortMerge => {
                                crate::service::merge::merge_abort(&services, repo_id.clone()).await
                            }
                            InProgressAction::ContinueRebase => {
                                crate::service::merge::rebase_continue(&services, repo_id.clone())
                                    .await
                                    .map(|_| ())
                            }
                            InProgressAction::AbortRebase => {
                                crate::service::merge::rebase_abort(&services, repo_id.clone())
                                    .await
                            }
                            InProgressAction::AbortSequencer => {
                                crate::service::merge::sequencer_abort(&services, repo_id.clone())
                                    .await
                            }
                        };
                        Message::RepoActionDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::ConflictResolve { paths, resolution } => {
                if paths.is_empty() {
                    return Task::none();
                }
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let mode = match resolution {
                    ConflictResolution::Ours => "ours",
                    ConflictResolution::Theirs => "theirs",
                    ConflictResolution::Both => "both",
                };
                let label = format!("Resolve {} conflicts ({mode})", paths.len());
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let mut result = Ok(());
                        for path in paths {
                            if let Err(err) = crate::service::merge::conflict_resolve(
                                &services,
                                repo_id.clone(),
                                path,
                                resolution,
                                None,
                            )
                            .await
                            {
                                result = Err(err);
                                break;
                            }
                        }
                        Message::RepoActionDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RepoActionDone {
                repo_id,
                label,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                match result {
                    Ok(()) => {
                        self.last_op = Some(format!("{label} done"));
                        self.notice = None;
                        let branches_open = self.tabs[index].branches.open;
                        {
                            let tab = &mut self.tabs[index];
                            tab.reload_diff = true;
                            tab.status = Load::Loading;
                            if branches_open {
                                tab.branches.loading = true;
                            }
                        }
                        let mut tasks = vec![
                            self.refresh(repo_id.clone()),
                            self.start_log(repo_id.clone()),
                        ];
                        if branches_open {
                            tasks.push(self.load_branches(repo_id));
                        }
                        Task::batch(tasks)
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::MergetoolRequested => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                // Conflicted paths from the loaded status (unfiltered — the
                // dialog picks among all conflicts, not the visible rows).
                let paths: Vec<String> = tab
                    .status_index
                    .rows
                    .iter()
                    .filter(|row| row.kind == ChangeKind::Conflicted)
                    .map(|row| row.path.clone())
                    .collect();
                if paths.is_empty() {
                    return Task::none();
                }
                tab.pending_mergetool = Some(dialog::MergetoolRequest::new(paths));
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::service::branches::mergetool_info(&services, repo_id.clone())
                                .await;
                        Message::MergetoolInfoLoaded { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::MergetoolInfoLoaded { repo_id, result } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                if let Some(request) = &mut self.tabs[index].pending_mergetool {
                    match result {
                        Ok(info) => request.info = Some(info),
                        Err(err) => request.error = Some(err),
                    }
                }
                Task::none()
            }
            Message::MergetoolPathSelected(index) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_mergetool {
                        request.selected = index;
                    }
                }
                Task::none()
            }
            Message::MergetoolRun => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = &mut tab.pending_mergetool else {
                    return Task::none();
                };
                let Some(path) = request.selected_path().map(str::to_string) else {
                    return Task::none();
                };
                // Deliberately off the op queue: the external tool runs
                // interactively for minutes (service/branches.rs).
                let tool = request.effective_tool().map(str::to_string);
                request.running = true;
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::mergetool_run(
                            &services,
                            repo_id.clone(),
                            path.clone(),
                            tool,
                        )
                        .await;
                        Message::MergetoolDone {
                            repo_id,
                            path,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::MergetoolDone {
                repo_id,
                path,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                // The dialog closes either way; the status refresh re-reads
                // the conflict list so resolved files leave it.
                self.tabs[index].pending_mergetool = None;
                match result {
                    Ok(run) if run.success => {
                        self.last_op = Some(format!("Mergetool finished for {path}"));
                        self.refresh(repo_id)
                    }
                    Ok(run) if !run.output.trim().is_empty() => {
                        self.notice =
                            Some(Notice::error(format!("Mergetool failed: {}", run.output)));
                        self.refresh(repo_id)
                    }
                    Ok(_) => {
                        self.notice = Some(Notice::error(format!("Mergetool failed for {path}")));
                        self.refresh(repo_id)
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Mergetool failed: {err}")));
                        self.refresh(repo_id)
                    }
                }
            }
            Message::AuthUsernameChanged(value) => {
                if let Some(prompt) = self.auth_prompts.front_mut() {
                    prompt.username = value;
                }
                Task::none()
            }
            Message::AuthPasswordChanged(value) => {
                if let Some(prompt) = self.auth_prompts.front_mut() {
                    prompt.password = value;
                }
                Task::none()
            }
            Message::AuthStoreToggled(value) => {
                if let Some(prompt) = self.auth_prompts.front_mut() {
                    prompt.store = value;
                }
                Task::none()
            }
            Message::AuthPromptSubmit => {
                let Some(prompt) = self.auth_prompts.front() else {
                    return Task::none();
                };
                if !prompt.can_submit() {
                    return Task::none();
                }
                let prompt = self.auth_prompts.pop_front().expect("front checked above");
                let username = prompt.username.trim();
                let username = (!username.is_empty()).then(|| username.to_string());
                let password = (!prompt.password.is_empty()).then(|| prompt.password.clone());
                if let Err(err) = crate::service::network::auth_respond(
                    &self.services,
                    prompt.request.op_id,
                    username,
                    password,
                    prompt.store,
                ) {
                    self.notice = Some(Notice::error(format!("Could not answer prompt: {err}")));
                }
                Task::none()
            }
            Message::AuthPromptCancel => {
                let Some(prompt) = self.auth_prompts.pop_front() else {
                    return Task::none();
                };
                let _ = crate::service::network::auth_respond(
                    &self.services,
                    prompt.request.op_id,
                    None,
                    None,
                    false,
                );
                Task::none()
            }
            Message::HostKeyAnswer(accept) => {
                let Some(prompt) = self.host_key_prompts.pop_front() else {
                    return Task::none();
                };
                if !crate::remote::prompts::respond_host_key(&prompt.request.request_id, accept) {
                    self.notice = Some(Notice::error("The host key prompt expired."));
                }
                Task::none()
            }
            Message::DeviceSecretChanged(value) => {
                if let Some(prompt) = self.device_secret_prompts.front_mut() {
                    prompt.password = value;
                }
                Task::none()
            }
            Message::DeviceSecretStoreToggled(value) => {
                if let Some(prompt) = self.device_secret_prompts.front_mut() {
                    prompt.store = value;
                }
                Task::none()
            }
            Message::DeviceSecretSubmit => {
                let Some(prompt) = self.device_secret_prompts.pop_front() else {
                    return Task::none();
                };
                if !crate::remote::prompts::respond_secret(
                    &prompt.request.request_id,
                    Some(SecretAnswer {
                        password: prompt.password,
                        store: prompt.store,
                    }),
                ) {
                    self.notice = Some(Notice::error("The secret prompt expired."));
                }
                Task::none()
            }
            Message::DeviceSecretCancel => {
                let Some(prompt) = self.device_secret_prompts.pop_front() else {
                    return Task::none();
                };
                let _ = crate::remote::prompts::respond_secret(&prompt.request.request_id, None);
                Task::none()
            }
            // --- interaction overlays -------------------------------------
            Message::Palette(msg) => {
                let opening = matches!(msg, palette::Msg::Open)
                    || matches!(msg, palette::Msg::Toggle) && !self.palette.open;
                if opening {
                    self.palette.commands = self.commands();
                }
                match palette::update(&mut self.palette, msg) {
                    Some(palette::Action::RunCommand(id)) => return self.run_command(&id),
                    Some(palette::Action::Close) | None => {}
                }
                Task::none()
            }
            Message::ContextMenu(msg) => match context_menu::update(&mut self.ctx_menu, msg) {
                Some(context_menu::Action::Command(id)) => self.run_command(&id),
                _ => Task::none(),
            },
            Message::Switcher(msg) => {
                let opening = matches!(msg, quick_switcher::Msg::Open)
                    || matches!(msg, quick_switcher::Msg::Toggle) && !self.switcher.open;
                if opening {
                    self.switcher.entries = self.switcher_entries();
                }
                match quick_switcher::update(&mut self.switcher, msg) {
                    Some(quick_switcher::Action::Open(path)) => {
                        self.switcher.close();
                        return self.open_repo(path.to_string_lossy().into_owned());
                    }
                    Some(quick_switcher::Action::Close) | None => {}
                }
                Task::none()
            }
            Message::Clone(msg) => {
                let opening = matches!(msg, clone_dialog::Msg::Open);
                match clone_dialog::update(&mut self.clone, msg) {
                    Some(clone_dialog::Action::Close) | None => {}
                    Some(clone_dialog::Action::CancelClone) => {
                        // Abandon the in-flight clone and dismiss the
                        // dialog; CloneDone for the cancelled transfer is
                        // a no-op (already closed, flag flipped).
                        if let Some(cancel) = &self.clone_cancel {
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        self.clone_cancel = None;
                        self.clone.close();
                        self.clone.running = false;
                        self.last_op = Some("Clone cancelled".to_string());
                        return Task::none();
                    }
                    Some(clone_dialog::Action::PickFolder) => {
                        return Task::perform(
                            async {
                                rfd::AsyncFileDialog::new()
                                    .set_title("Clone into")
                                    .pick_folder()
                                    .await
                                    .map(|handle| handle.path().to_path_buf())
                            },
                            Message::ClonePicked,
                        );
                    }
                    Some(clone_dialog::Action::Start {
                        url,
                        destination,
                        blobless,
                        depth,
                    }) => {
                        // The dialog's own update already flipped it into the
                        // running state (active_url set for CloneProgress).
                        // The flag lives on the shell so the footer Cancel
                        // can abort the transfer mid-run.
                        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        self.clone_cancel = Some(cancel.clone());
                        let services = self.services.clone();
                        return Task::perform(
                            async move {
                                let result = if blobless {
                                    crate::service::maintenance::clone_blobless(
                                        &services,
                                        url.clone(),
                                        destination.clone(),
                                        depth,
                                        cancel,
                                    )
                                    .await
                                } else {
                                    crate::service::repo::repo_clone(
                                        &services,
                                        url.clone(),
                                        destination.clone(),
                                        depth,
                                        cancel,
                                    )
                                    .await
                                };
                                Message::CloneDone {
                                    destination,
                                    result,
                                }
                            },
                            std::convert::identity,
                        );
                    }
                }
                // Fresh dialog: put the keyboard in the URL field so the
                // user can paste and hit Enter without a click.
                if opening {
                    return iced::widget::operation::focus::<Message>(iced::widget::Id::new(
                        "clone-url",
                    ));
                }
                Task::none()
            }
            Message::ClonePicked(path) => {
                self.update(Message::Clone(clone_dialog::Msg::FolderPicked(path)))
            }
            Message::QuickOpen(msg) => {
                let opening = matches!(msg, quick_open::Msg::Open)
                    || matches!(msg, quick_open::Msg::Toggle) && !self.quick_open.open;
                if opening {
                    if self.tabs.is_empty() {
                        // No repository open: the hotkey is inert.
                        return Task::none();
                    }
                    self.quick_open.open();
                    // Fetch/build the index on every open (cheap, capped,
                    // and keeps up with watcher churn).
                    let repo_id = self.tabs[self.active].repo.repo_id.clone();
                    let services = self.services.clone();
                    return Task::perform(
                        async move {
                            let result =
                                crate::workspace::repo_file_index(&services, repo_id.clone()).await;
                            Message::QuickOpenIndexLoaded { repo_id, result }
                        },
                        std::convert::identity,
                    );
                }
                match quick_open::update(&mut self.quick_open, msg) {
                    Some(quick_open::Action::Open(path)) => {
                        self.quick_open.close();
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.detail_open = false;
                            tab.blame.open = false;
                            tab.file_history.open = false;
                            tab.compare.open = false;
                            tab.files.open = false;
                        }
                        return self.update(Message::Editor(editor_view::Msg::OpenRequested(path)));
                    }
                    Some(quick_open::Action::Close) | None => {}
                }
                Task::none()
            }
            Message::QuickOpenIndexLoaded { repo_id, result } => {
                // Stale (closed or another tab became active): drop.
                let matches_active = self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.repo.repo_id == repo_id)
                    && self.quick_open.open;
                if !matches_active {
                    return Task::none();
                }
                match result {
                    Ok(index) => {
                        let entries = index
                            .entries
                            .into_iter()
                            .filter(|entry| !entry.is_dir)
                            .map(|entry| quick_open::Entry { path: entry.path })
                            .collect();
                        self.quick_open.set_entries(entries);
                    }
                    Err(err) => {
                        self.quick_open.loading = false;
                        self.notice = Some(Notice::error(format!("Quick Open failed: {err}")));
                        self.quick_open.close();
                    }
                }
                Task::none()
            }
            Message::CloneDone {
                destination,
                result,
            } => {
                self.clone.running = false;
                self.clone.progress = None;
                self.clone_cancel = None;
                match result {
                    Ok(path) => {
                        self.clone.close();
                        self.last_op = Some(format!("Cloned into {path}"));
                        self.open_repo(path)
                    }
                    Err(err) if err.contains("cancelled") => {
                        // User-initiated footer cancel: not an error.
                        self.last_op = Some("Clone cancelled".to_string());
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!(
                            "Clone of {destination} failed: {err}"
                        )));
                        Task::none()
                    }
                }
            }
            Message::Devices(msg) => self.update_devices(msg),
            Message::DeviceConnectDone { device_id, result } => {
                match result {
                    Ok(status) => {
                        let name = self
                            .devices
                            .rows
                            .iter()
                            .find(|row| row.profile.id == device_id)
                            .map(|row| row.profile.name.as_str())
                            .unwrap_or(device_id.as_str());
                        self.last_op = Some(format!("{name}: {status}"));
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Device connect failed: {err}")));
                    }
                }
                Task::none()
            }
            Message::DeviceForgetDone { device_id, result } => match result {
                Ok(()) => {
                    let name = self
                        .devices
                        .rows
                        .iter()
                        .find(|row| row.profile.id == device_id)
                        .map(|row| row.profile.name.clone())
                        .unwrap_or_else(|| device_id.clone());
                    self.last_op = Some(format!("Forgot device {name}"));
                    self.refresh_devices()
                }
                Err(err) => {
                    self.notice = Some(Notice::error(format!("Could not forget device: {err}")));
                    Task::none()
                }
            },
            Message::DeviceRepoOpened { path, result } => match result {
                Ok(info) => {
                    self.devices.browse.close();
                    self.activate_repo(info)
                }
                Err(err) => {
                    self.devices.browse.opening = None;
                    self.notice = Some(Notice::error(format!("Could not open {path}: {err}")));
                    Task::none()
                }
            },
            Message::WindowFocus { window, focused } => {
                if self.main_window == Some(window) {
                    // Remote repos poll their device over SSH on a timer;
                    // pause while the app window is not focused and resume
                    // when it comes back (the transport reconnects lazily).
                    let _ =
                        crate::remote::commands::remote_poll_set_paused(&self.services, !focused);
                }
                Task::none()
            }
            Message::WindowMoved { window, position } => {
                if self.main_window == Some(window) {
                    let mut frame = self.window_save.frame;
                    frame.position = Some([position.x, position.y]);
                    return self.arm_window_save(frame);
                }
                Task::none()
            }
            Message::WindowResized { window, size } => {
                if self.main_window == Some(window) {
                    let mut frame = self.window_save.frame;
                    frame.size = [size.width, size.height];
                    return self.arm_window_save(frame);
                }
                Task::none()
            }
            Message::WindowSaveTick(seq) => {
                if !self.window_save.due(seq) {
                    return Task::none();
                }
                // Resolve the maximized state once the storm has settled;
                // a change arriving mid-query supersedes this generation.
                match self.main_window {
                    Some(id) => iced::window::is_maximized(id)
                        .map(move |maximized| Message::WindowSaveMaximized { seq, maximized }),
                    None => self.finish_window_save(seq, false),
                }
            }
            Message::WindowSaveMaximized { seq, maximized } => {
                self.finish_window_save(seq, maximized)
            }
            Message::CursorMoved(position) => {
                // Divider drags reuse the globally tracked cursor: the delta
                // since the last move event resizes the dragged pane (no
                // window-size math needed). The cursor is updated first so
                // the delta is correct on the next event.
                let previous = self.cursor;
                self.cursor = position;
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let Some(divider) = tab.pane_drag else {
                        return Task::none();
                    };
                    let dx = position.x - previous.x;
                    let dy = position.y - previous.y;
                    match divider {
                        repo_view::PaneDivider::Sidebar => {
                            tab.sidebar_width = (tab.sidebar_width + dx)
                                .clamp(repo_view::SIDEBAR_MIN_WIDTH, repo_view::SIDEBAR_MAX_WIDTH);
                        }
                        repo_view::PaneDivider::History => {
                            tab.history_width = (tab.history_width - dx)
                                .clamp(repo_view::HISTORY_MIN_WIDTH, repo_view::HISTORY_MAX_WIDTH);
                        }
                        repo_view::PaneDivider::Terminal => {
                            tab.terminal_height = (tab.terminal_height - dy).clamp(
                                repo_view::TERMINAL_MIN_HEIGHT,
                                repo_view::TERMINAL_MAX_HEIGHT,
                            );
                        }
                    }
                }
                Task::none()
            }
            Message::ToggleHelp => {
                self.help_open = !self.help_open;
                Task::none()
            }
            Message::PaneDragStart(divider) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pane_drag = Some(divider);
                }
                Task::none()
            }
            Message::PaneDragEnd => {
                // Persist the sizes as the new defaults (config + live tabs
                // keep their own copies; new tabs read the config).
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.pane_drag.take().is_some() {
                        self.config.pane_sizes =
                            [tab.sidebar_width, tab.history_width, tab.terminal_height];
                        self.persist_config();
                    }
                }
                Task::none()
            }
            Message::ToggleTerminal => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.terminal_open = !tab.terminal_open;
                if tab.terminal_open && !tab.terminal.has_session() {
                    let action = terminal::update(&mut tab.terminal, terminal::Msg::SpawnRequested);
                    self.run_terminal_action(action)
                } else {
                    if !tab.terminal_open {
                        tab.terminal.blur();
                    }
                    Task::none()
                }
            }
            Message::Terminal(msg) => {
                // The pane ignores modifier combos, so intercept paste here:
                // read the clipboard and deliver it as Msg::Paste.
                if let terminal::Msg::Key(iced::keyboard::Key::Character(c), modifiers) = &msg {
                    if c.eq_ignore_ascii_case("v") && (modifiers.command() || modifiers.control()) {
                        return iced::clipboard::read().map(|text| {
                            Message::Terminal(terminal::Msg::Paste(text.unwrap_or_default()))
                        });
                    }
                }
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = terminal::update(&mut tab.terminal, msg);
                self.run_terminal_action(action)
            }
            Message::Forge(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                // Mirror the per-frame inputs the pure update half cannot
                // see (its own state copy + the persistent view context).
                if let Load::Ready(status) = &tab.status {
                    if let Some(branch) = &status.branch {
                        tab.forge.current_branch = branch.clone();
                    }
                    tab.forge_ctx.branch = status.branch.clone().unwrap_or_default();
                    tab.forge_ctx.head_sha = status.head.clone().unwrap_or_default();
                }
                if let Some(list) = &tab.branches.list {
                    let names: Vec<String> = list.iter().map(|b| b.name.clone()).collect();
                    tab.forge.base_branches = names.clone();
                    tab.forge_ctx.base_branches = names;
                }
                // Action outcomes land in the global footer notice too: the
                // panel is a dropdown that may not be open when they arrive.
                // (The panel banner keeps its own copy for in-panel reading.)
                let outcome = match &msg {
                    forge_panel::Msg::ActionDone { label, result, .. }
                    | forge_panel::Msg::RunActionDone { label, result, .. } => {
                        Some((label.clone(), result.as_ref().map_err(Clone::clone)))
                    }
                    forge_panel::Msg::DispatchDone(result) => Some((
                        "Dispatch".to_string(),
                        result.as_ref().map_err(Clone::clone),
                    )),
                    _ => None,
                };
                if let Some((label, result)) = outcome {
                    match result {
                        Ok(output) => {
                            self.notice = Some(Notice::plain(if output.trim().is_empty() {
                                format!("{label} ok")
                            } else {
                                format!("{label}: {output}")
                            }));
                        }
                        Err(err) => {
                            self.notice = Some(Notice::error(format!(
                                "{label} failed: {}",
                                forge_panel::forge_error_text(&err)
                            )));
                        }
                    }
                }
                let action = forge_panel::update(&mut tab.forge, msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_forge_action(repo_id, action)
            }
            Message::Bisect(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.bisect.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_bisect_action(repo_id, action)
            }
            Message::Reflog(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.reflog.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_reflog_action(repo_id, action)
            }
            Message::AiGenerate(action) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                let task = match action {
                    AiAction::CommitMessage => Task::perform(
                        async move {
                            Message::AiCommitMessageDone(
                                crate::service::ai::ai_commit_message(&services, repo_id).await,
                            )
                        },
                        std::convert::identity,
                    ),
                    AiAction::ReviewStaged => Task::perform(
                        async move {
                            Message::AiReviewDone(
                                crate::service::ai::ai_review_staged(&services, repo_id).await,
                            )
                        },
                        std::convert::identity,
                    ),
                    AiAction::StashMessage => Task::perform(
                        async move {
                            Message::AiStashMessageDone(
                                crate::service::ai::ai_stash_message(&services, repo_id).await,
                            )
                        },
                        std::convert::identity,
                    ),
                    AiAction::PrTitleBody => Task::perform(
                        async move {
                            Message::AiPrTextDone(
                                crate::service::ai::ai_pr_title_body(&services, repo_id).await,
                            )
                        },
                        std::convert::identity,
                    ),
                    AiAction::BranchName => {
                        let description = tab.message.text().trim().to_string();
                        if description.is_empty() {
                            self.notice = Some(Notice::error(
                                "Type a description first, then ask for a branch name",
                            ));
                            return Task::none();
                        }
                        Task::perform(
                            async move {
                                Message::AiBranchNameDone(
                                    crate::service::ai::ai_branch_name(
                                        &services,
                                        repo_id,
                                        description,
                                    )
                                    .await,
                                )
                            },
                            std::convert::identity,
                        )
                    }
                };
                self.last_op = Some("AI: generating…".to_string());
                task
            }
            Message::AiExplainHunk { path, hunk } => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                self.last_op = Some("AI: generating…".to_string());
                Task::perform(
                    async move {
                        let result = crate::service::ai::ai_explain_hunk(
                            &services,
                            repo_id,
                            path.clone(),
                            hunk,
                        )
                        .await;
                        Message::AiExplainDone { path, result }
                    },
                    std::convert::identity,
                )
            }
            Message::CheckUpdates => {
                self.last_op = Some("Checking for updates…".to_string());
                Task::perform(
                    async move { Message::UpdateChecked(crate::update_check::check().await) },
                    std::convert::identity,
                )
            }
            Message::UpdateChecked(result) => match result {
                Ok(update) => {
                    self.notice = Some(match update {
                        crate::update_check::Update::UpToDate(version) => {
                            self.update_ready = None;
                            Notice::plain(format!("Up to date (v{version})"))
                        }
                        crate::update_check::Update::Available(tag, url) => {
                            self.update_ready = Some((tag.clone(), url.clone()));
                            Notice::with_link(
                                format!("New release {tag} available — install via the palette"),
                                url,
                            )
                        }
                    });
                    Task::none()
                }
                Err(err) => {
                    self.notice = Some(Notice::error(format!("Update check failed: {err}")));
                    Task::none()
                }
            },
            Message::InstallUpdate => {
                let Some((tag, _)) = self.update_ready.clone() else {
                    self.notice = Some(Notice::plain(
                        "No update staged — run `Check for updates` first",
                    ));
                    return Task::none();
                };
                self.last_op = Some(format!("Installing update {tag}…"));
                Task::perform(
                    async move { Message::UpdateInstallDone(crate::updater::install(&tag).await) },
                    std::convert::identity,
                )
            }
            // Success never reaches here: the updater restarts/exits the
            // process before returning.
            Message::UpdateInstallDone(result) => {
                self.notice = Some(match result {
                    Ok(_) => Notice::plain("Update installed".to_string()),
                    Err(err) => Notice::error(format!("Update install failed: {err}")),
                });
                Task::none()
            }
            Message::ToggleUpdateCheck => {
                self.config.update_check_on_start = !self.config.update_check_on_start;
                let on = self.config.update_check_on_start;
                self.persist_config();
                self.notice = Some(Notice::plain(format!(
                    "Startup update check {}",
                    if on { "enabled" } else { "disabled" }
                )));
                Task::none()
            }
            Message::OpenUrl(url) => Task::perform(
                async move {
                    let _ = tokio::task::spawn_blocking(move || open::that_detached(&url)).await;
                    Message::Noop
                },
                std::convert::identity,
            ),
            Message::AiResultClose => {
                self.ai_result = None;
                Task::none()
            }
            Message::CopyText(text) => {
                self.last_op = Some("Copied".to_string());
                iced::clipboard::write::<Message>(text)
            }
            Message::AiToggleEnabled => {
                self.config.ai.enabled = !self.config.ai.enabled;
                let enabled = self.config.ai.enabled;
                self.persist_config();
                self.notice = Some(Notice::plain(format!(
                    "AI assist {}",
                    if enabled { "enabled" } else { "disabled" }
                )));
                // The header chip derives from the master switch — refresh.
                self.refresh_ai_status()
            }
            Message::AiToggleRepoOptIn => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let path = tab.repo.root.clone();
                let enabled = !self.config.ai_enabled_for(&path);
                self.config.set_ai_opt_in(&path, enabled);
                self.persist_config();
                self.notice = Some(Notice::plain(format!(
                    "AI assist {} for this repo",
                    if enabled { "opted in" } else { "disabled" }
                )));
                self.refresh_ai_status()
            }
            Message::AiOpenSettings => {
                ai_settings::open(&mut self.ai_settings, &self.config.ai);
                Task::batch([self.load_ai_key_status(), self.refresh_ai_status()])
            }
            Message::AiSettings(msg) => {
                let action = ai_settings::update(&mut self.ai_settings, msg);
                match action {
                    Some(ai_settings::Action::Close) | None => Task::none(),
                    // The dialog's checkboxes reuse the palette toggles
                    // (immediate, persisted, footer-noticed).
                    Some(ai_settings::Action::ToggleMaster) => {
                        self.update(Message::AiToggleEnabled)
                    }
                    Some(ai_settings::Action::ToggleOptIn) => {
                        self.update(Message::AiToggleRepoOptIn)
                    }
                    Some(ai_settings::Action::Save {
                        provider,
                        opencode_url,
                        opencode_model,
                        openrouter_model,
                        api_key,
                    }) => {
                        self.config.ai.provider = provider;
                        self.config.ai.opencode_url = opencode_url;
                        self.config.ai.opencode_model = opencode_model;
                        self.config.ai.openrouter_model = openrouter_model;
                        self.persist_config();
                        // Keyring writes are blocking — off the UI thread;
                        // the chip refresh runs after the write lands.
                        let services = self.services.clone();
                        let keyring = Task::perform(
                            async move {
                                let result = tokio::task::spawn_blocking(move || match api_key {
                                    ai_settings::ApiKeyEdit::Keep => Ok(()),
                                    ai_settings::ApiKeyEdit::Set(key) => {
                                        crate::keyring_store::secrets_set(
                                            &services,
                                            crate::service::ai::KEY_OPENROUTER_API.to_string(),
                                            key,
                                        )
                                    }
                                    ai_settings::ApiKeyEdit::Clear => {
                                        crate::keyring_store::secrets_delete(
                                            &services,
                                            crate::service::ai::KEY_OPENROUTER_API.to_string(),
                                        )
                                    }
                                })
                                .await
                                .unwrap_or_else(|error| {
                                    Err(format!("keyring task failed: {error}"))
                                });
                                Message::AiSettings(ai_settings::Msg::SaveDone(result))
                            },
                            std::convert::identity,
                        );
                        keyring.chain(self.refresh_ai_status())
                    }
                }
            }
            Message::AiStatusLoaded(status) => {
                self.ai_status = Some(status);
                Task::none()
            }
            Message::AiCommitMessageDone(result) => {
                self.note_ai_result(&result);
                match result {
                    Ok(message) => {
                        let Some(tab) = self.tabs.get_mut(self.active) else {
                            return Task::none();
                        };
                        tab.message = text_editor::Content::with_text(&message);
                        self.last_op = Some("AI: commit message ready".to_string());
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("AI: {err}")));
                        Task::none()
                    }
                }
            }
            Message::AiReviewDone(result) => {
                self.note_ai_result(&result);
                match result {
                    Ok(body) => {
                        self.ai_result = Some(AiResult {
                            title: "AI review of staged changes".to_string(),
                            body,
                        });
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("AI: {err}")));
                        Task::none()
                    }
                }
            }
            Message::AiExplainDone { path, result } => {
                self.note_ai_result(&result);
                match result {
                    Ok(body) => {
                        self.ai_result = Some(AiResult {
                            title: format!("AI explanation — {path}"),
                            body,
                        });
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("AI: {err}")));
                        Task::none()
                    }
                }
            }
            Message::AiStashMessageDone(result) => {
                self.note_ai_result(&result);
                match result {
                    Ok(message) => {
                        let Some(tab) = self.tabs.get_mut(self.active) else {
                            return Task::none();
                        };
                        tab.stashes.message = message;
                        self.last_op = Some("AI: stash message ready".to_string());
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("AI: {err}")));
                        Task::none()
                    }
                }
            }
            Message::AiPrTextDone(result) => {
                self.note_ai_result(&result);
                match result {
                    Ok(text) => {
                        // The service returns "title\n\nbody" (parse rules in
                        // ai::prompts); feed the forge create form.
                        let (title, body) = match text.split_once("\n\n") {
                            Some((title, body)) => (title.to_string(), body.to_string()),
                            None => (text.clone(), text),
                        };
                        self.ai_result = Some(AiResult {
                            title: "AI PR title & description".to_string(),
                            body: format!("{title}\n\n{body}"),
                        });
                        Task::batch([
                            self.update(Message::Forge(forge_panel::Msg::CreateTitleChanged(
                                title,
                            ))),
                            self.update(Message::Forge(forge_panel::Msg::CreateBodySet(body))),
                        ])
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("AI: {err}")));
                        Task::none()
                    }
                }
            }
            Message::AiBranchNameDone(result) => {
                self.note_ai_result(&result);
                match result {
                    Ok(name) => {
                        self.ai_result = Some(AiResult {
                            title: "AI branch name suggestion".to_string(),
                            body: name,
                        });
                        Task::none()
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("AI: {err}")));
                        Task::none()
                    }
                }
            }
            Message::Health(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.health.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_health_action(repo_id, action)
            }
            Message::Worktree(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.worktree.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_worktree_action(repo_id, action)
            }
            Message::Submodule(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.submodule.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_submodule_action(repo_id, action)
            }
            Message::Tags(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.tags.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_tags_action(repo_id, action)
            }
            Message::MoreMenu => {
                let menu = context_menu::Menu::new()
                    .header("Manage")
                    .command("view.files", "Files…")
                    .command("panel.worktrees", "Worktrees…")
                    .command("panel.submodules", "Submodules…")
                    .command("panel.tags", "Tags…")
                    .command("panel.stats", "Contribution stats…")
                    .command("view.popout_diff", "Pop out diff");
                self.ctx_menu.open_at(self.cursor, menu);
                Task::none()
            }
            Message::Blame(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.blame.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_blame_action(repo_id, action)
            }
            Message::FileHistory(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.file_history.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_file_history_action(repo_id, action)
            }
            Message::Compare(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.compare.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_compare_action(repo_id, action)
            }
            Message::Files(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = files_panel::update(&mut tab.files, msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_files_action(repo_id, action)
            }
            Message::FilesDirLoaded {
                repo_id,
                dir,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                self.tabs[index].files.accept_dir(&dir, result);
                Task::none()
            }
            Message::FilesEntryDone {
                repo_id,
                label,
                result,
            } => {
                match &result {
                    Ok(()) => self.last_op = Some(format!("{label} done")),
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                    }
                }
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].files.invalidate();
                }
                Task::batch([self.refresh(repo_id.clone()), self.refresh_files(repo_id)])
            }
            Message::FilesRowMenu { path, is_dir } => {
                self.ctx_menu
                    .open_at(self.cursor, files_row_menu(&path, is_dir));
                Task::none()
            }
            Message::FilesDeleteConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(path) = tab.pending_files_delete.take() else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::workspace::repo_delete_entry(&services, repo_id.clone(), path)
                                .await;
                        Message::FilesEntryDone {
                            repo_id,
                            label: "Delete".to_string(),
                            result: result.map(|_| ()),
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::CompareFileDiffLoaded { slot, result } => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let pane = &mut tab.compare_diff;
                pane.seq += 1;
                match result {
                    Ok(files) => pane.set_ready(files, false),
                    Err(err) => pane.set_failed(err),
                }
                let _ = slot;
                Task::none()
            }
            Message::Actions(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                if matches!(
                    msg,
                    actions_panel::Msg::ToggleOpen | actions_panel::Msg::RefreshPressed
                ) {
                    let actions = self.custom_actions.clone();
                    tab.actions.set_actions(actions);
                }
                let action = tab.actions.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_actions_action(repo_id, action)
            }
            Message::Output(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = output_panel::update(&mut tab.output, msg);
                match action {
                    Some(output_panel::Action::Cancel { run_id }) => {
                        let services = self.services.clone();
                        Task::perform(
                            async move {
                                let _ = crate::service::misc::action_cancel(&services, run_id);
                                Message::Noop
                            },
                            std::convert::identity,
                        )
                    }
                    Some(output_panel::Action::FollowScroll { id }) => {
                        output_panel::snap_to_bottom(id).map(Message::Output)
                    }
                    None => Task::none(),
                }
            }
            Message::Editor(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                if matches!(msg, editor_view::Msg::OpenRequested(_)) {
                    let apps = crate::service::editor::detect_editors();
                    tab.editor.update(editor_view::Msg::AppsLoaded(apps));
                }
                let action = tab.editor.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_editor_action(repo_id, action)
            }
            Message::ActionStarted { name, run_id } => {
                self.last_op = Some(format!("Action {name} started"));
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.actions.update(actions_panel::Msg::RunStarted {
                        name: name.clone(),
                        run_id: run_id.clone(),
                    });
                    output_panel::update(
                        &mut tab.output,
                        output_panel::Msg::RunOpened { run_id, name },
                    );
                    tab.output.open = true;
                }
                Task::none()
            }
            Message::AccentLoaded(hex) => {
                match hex.as_deref().map(theme::set_os_accent_hex) {
                    Some(true) | None => {}
                    Some(false) => {
                        tracing::debug!("OS accent colour unparsable; ignoring");
                    }
                }
                Task::none()
            }
            Message::WindowClosed(id) => {
                if self
                    .popout
                    .as_ref()
                    .is_some_and(|(popout, _)| *popout == id)
                {
                    self.popout = None;
                }
                Task::none()
            }
            Message::PopoutDiff => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                if !tab.diff.is_ready() {
                    return Task::none();
                }
                let pane = tab.diff.clone();
                let settings = window::Settings {
                    size: iced::Size::new(900.0, 640.0),
                    ..main_window_settings()
                };
                let (id, opened) = window::open(settings);
                self.popout = Some((id, pane));
                opened.map(|_| Message::Noop)
            }
            Message::Popout(msg) => {
                // Diff-viewer messages for the popout pane (scroll etc.);
                // unrelated messages are ignored there.
                if let Some((_, pane)) = self.popout.as_mut() {
                    if let Message::DiffScrolled { y, viewport, .. } = *msg {
                        pane.scroll = y;
                        pane.viewport = viewport;
                    }
                }
                Task::none()
            }
            Message::Notice(text) => {
                self.notice = Some(Notice::plain(text));
                Task::none()
            }
            Message::Stats(msg) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let action = tab.stats.update(msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_stats_action(repo_id, action)
            }
            Message::StatusRowMenu { key } => {
                let menu = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.status_index.row(&key))
                    .map(status_row_menu);
                if let Some(menu) = menu {
                    self.ctx_menu.open_at(self.cursor, menu);
                }
                Task::none()
            }
            Message::BranchRowMenu { name } => {
                let known = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.branches.list.as_ref())
                    .is_some_and(|list| list.iter().any(|b| b.name == name));
                if known {
                    self.ctx_menu.open_at(self.cursor, branch_row_menu(&name));
                }
                Task::none()
            }
            Message::CommitRowMenu { sha } => {
                let known = self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.history.commits.iter().any(|c| c.sha == sha));
                if known {
                    self.ctx_menu.open_at(self.cursor, commit_row_menu(&sha));
                }
                Task::none()
            }
            Message::CopySha(sha) => {
                let short = repo_view::short_sha(&sha);
                self.last_op = Some(format!("Copied {short}"));
                iced::clipboard::write::<Message>(sha)
            }
            Message::CherryPick(sha) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::merge::cherry_pick(
                            &services,
                            repo_id.clone(),
                            vec![sha],
                        )
                        .await;
                        Message::MergeDone {
                            repo_id,
                            label: "Cherry-pick".to_string(),
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::RevertCommit(sha) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result =
                            crate::service::merge::revert(&services, repo_id.clone(), vec![sha])
                                .await;
                        Message::MergeDone {
                            repo_id,
                            label: "Revert".to_string(),
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::ImageLoaded {
                repo_id,
                slot,
                path,
                old,
                seq,
                result,
            } => {
                let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) else {
                    return Task::none();
                };
                let tab = &mut self.tabs[index];
                let pane = match slot {
                    DiffSlot::Working => &mut tab.diff,
                    DiffSlot::Commit => &mut tab.commit_diff,
                };
                if pane.seq != seq {
                    return Task::none();
                }
                if let Ok(bytes) = result {
                    if !bytes.is_empty() {
                        let handle = iced::widget::image::Handle::from_bytes(bytes);
                        let entry = pane.images.entry(path).or_default();
                        if old {
                            entry.old = Some(handle);
                        } else {
                            entry.new = Some(handle);
                        }
                    }
                }
                Task::none()
            }
            Message::IgnorePath(pattern) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                let pattern_copy = pattern.clone();
                Task::perform(
                    async move {
                        let result = crate::service::files::gitignore_add(
                            &services,
                            repo_id.clone(),
                            pattern_copy,
                        )
                        .await;
                        Message::Ignored { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::Ignored { repo_id, result } => {
                match &result {
                    Ok(()) => self.last_op = Some("Ignore pattern done".to_string()),
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Ignore pattern failed: {err}")));
                    }
                }
                self.refresh(repo_id)
            }
            Message::IgnoreTemplatesRequested => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_ignore_templates = Some(dialog::IgnoreTemplates::default());
                }
                Task::none()
            }
            Message::IgnoreTemplatesFilterChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(state) = &mut tab.pending_ignore_templates {
                        state.filter = value;
                    }
                }
                Task::none()
            }
            Message::IgnoreTemplateApplied(name) => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::files::gitignore_apply_template(
                            &services,
                            repo_id.clone(),
                            name,
                        )
                        .await;
                        Message::TemplateApplied { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::TemplateApplied { repo_id, result } => {
                match &result {
                    Ok(()) => self.last_op = Some("Ignore template applied".to_string()),
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Apply template failed: {err}")));
                    }
                }
                self.refresh(repo_id)
            }
            Message::CleanDialogRequested => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let paths: Vec<String> = tab
                    .status_index
                    .rows
                    .iter()
                    .filter(|row| {
                        row.section == SectionId::Changes && row.kind == ChangeKind::Untracked
                    })
                    .map(|row| row.path.clone())
                    .collect();
                if paths.is_empty() {
                    return Task::none();
                }
                tab.pending_clean = Some(dialog::CleanRequest::new(paths));
                Task::none()
            }
            Message::CleanToggled(path) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_clean {
                        request.toggle(&path);
                    }
                }
                Task::none()
            }
            Message::CleanConfirmChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_clean {
                        request.confirm_text = value;
                    }
                }
                Task::none()
            }
            Message::CleanConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_clean.take() else {
                    return Task::none();
                };
                let paths = request.selected();
                if paths.is_empty() {
                    return Task::none();
                }
                tab.reload_diff = true;
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        // repo_clean validates, snapshots a "pre-clean"
                        // checkpoint, then deletes — the safety chain is the
                        // service's, the dialog only gates it.
                        let result = crate::service::maintenance::repo_clean(
                            &services,
                            repo_id.clone(),
                            paths,
                            "pre-clean".to_string(),
                        )
                        .await;
                        Message::CleanDone { repo_id, result }
                    },
                    std::convert::identity,
                )
            }
            Message::CleanDone { repo_id, result } => {
                match result {
                    Ok(removed) => {
                        self.notice = Some(Notice::plain(format!(
                            "Cleaned {removed} untracked path{}",
                            if removed == 1 { "" } else { "s" }
                        )))
                    }
                    Err(err) => self.notice = Some(Notice::error(format!("Clean failed: {err}"))),
                }
                self.refresh(repo_id)
            }
            Message::BranchRenameRequested(name) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.pending_branch_rename = Some(dialog::RenameRequest {
                        old: name.clone(),
                        new: name,
                    });
                }
                Task::none()
            }
            Message::BranchRenameChanged(value) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(request) = &mut tab.pending_branch_rename {
                        request.new = value;
                    }
                }
                Task::none()
            }
            Message::BranchRenameConfirmed => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let Some(request) = tab.pending_branch_rename.take() else {
                    return Task::none();
                };
                let new = request.new.trim().to_string();
                if new.is_empty() || new == request.old {
                    return Task::none();
                }
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Rename {} to {new}", request.old);
                let old = request.old;
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::branch_rename(
                            &services,
                            repo_id.clone(),
                            old,
                            new,
                        )
                        .await;
                        Message::BranchDone {
                            repo_id,
                            label,
                            result,
                        }
                    },
                    std::convert::identity,
                )
            }
            Message::Noop => Task::none(),
        }
    }

    fn backend(&mut self, event: BackendEvent) -> Task<Message> {
        match event {
            BackendEvent::CliArgs(args) => {
                // Second-launch forwarding: open whatever path was passed
                // (flags ignored, same rule as first launch).
                match first_repo_arg(&args) {
                    Some(path) => self.open_repo(path),
                    None => Task::none(),
                }
            }
            BackendEvent::RepoChanged(changed) => {
                let branches_open = repo_view::tab_index(&self.tabs, &changed.repo_id)
                    .is_some_and(|index| self.tabs[index].branches.open);
                let stashes_open = repo_view::tab_index(&self.tabs, &changed.repo_id)
                    .is_some_and(|index| self.tabs[index].stashes.open);
                let files_open = repo_view::tab_index(&self.tabs, &changed.repo_id)
                    .is_some_and(|index| self.tabs[index].files.open);
                if let Some(index) = repo_view::tab_index(&self.tabs, &changed.repo_id) {
                    self.tabs[index].reload_diff = true;
                    if branches_open && (changed.head_moved || changed.full) {
                        self.tabs[index].branches.loading = true;
                    }
                    if stashes_open && changed.full {
                        self.tabs[index].stashes.loading = true;
                        self.tabs[index].stashes.checkpoints_loading = true;
                        self.tabs[index].stashes.trash_loading = true;
                    }
                    if files_open {
                        // The tree cache is per-listing; drop it and let the
                        // refresh below re-fetch root + expanded directories.
                        self.tabs[index].files.invalidate();
                    }
                }
                let mut tasks = vec![self.refresh(changed.repo_id.clone())];
                if files_open {
                    tasks.push(self.refresh_files(changed.repo_id.clone()));
                }
                if changed.head_moved || changed.full {
                    tasks.push(self.start_log(changed.repo_id.clone()));
                    if branches_open {
                        tasks.push(self.load_branches(changed.repo_id.clone()));
                    }
                    if stashes_open {
                        tasks.push(self.load_stashes(changed.repo_id.clone()));
                        tasks.push(self.load_checkpoints(changed.repo_id.clone()));
                        tasks.push(self.load_trash(changed.repo_id.clone()));
                    }
                }
                Task::batch(tasks)
            }
            BackendEvent::LogPage { request_id, page } => {
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.history.request_id == request_id)
                {
                    tab.history.accept(page);
                } else if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.file_history.open && tab.file_history_seq == request_id)
                {
                    let _ = tab.file_history.update(file_history_view::Msg::PageLoaded {
                        commits: page.commits,
                        rows: page.rows,
                        next_cursor: page.next_cursor,
                    });
                }
                Task::none()
            }
            BackendEvent::ActionOutput(payload) => {
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.repo.repo_id.0 == payload.repo_id)
                {
                    if payload.done {
                        output_panel::update(
                            &mut tab.output,
                            output_panel::Msg::Exit {
                                run_id: payload.run_id.clone(),
                                code: payload.exit_code,
                            },
                        );
                        tab.actions.update(actions_panel::Msg::RunFinished {
                            run_id: payload.run_id,
                        });
                    } else {
                        output_panel::update(
                            &mut tab.output,
                            output_panel::Msg::Output {
                                run_id: payload.run_id,
                                line: payload.line,
                            },
                        );
                    }
                }
                Task::none()
            }
            BackendEvent::OpProgress(progress) => {
                let pct = progress
                    .pct
                    .map(|pct| format!(" · {pct:.0}%"))
                    .unwrap_or_default();
                self.last_op = Some(format!("{}: {}{pct}", progress.kind, progress.message));
                if progress.done {
                    self.active_op = None;
                    if let Some(err) = progress.error.as_deref() {
                        self.notice =
                            Some(Notice::error(format!("{} failed: {err}", progress.kind)));
                    }
                    return self.refresh(RepoId(progress.repo_id));
                }
                self.active_op = Some(ActiveOp {
                    repo_id: RepoId(progress.repo_id),
                    op_id: progress.op_id,
                    kind: progress.kind,
                });
                Task::none()
            }
            BackendEvent::OpLog(line) => {
                self.last_op = Some(line.line);
                Task::none()
            }
            BackendEvent::AuthRequest(request) => {
                self.auth_prompts.push_back(AuthPromptState::new(request));
                Task::none()
            }
            BackendEvent::HostKeyRequest(request) => {
                self.host_key_prompts
                    .push_back(HostKeyPromptState { request });
                Task::none()
            }
            BackendEvent::DeviceAuthRequest(request) => {
                self.device_secret_prompts
                    .push_back(DeviceSecretPromptState {
                        request,
                        password: String::new(),
                        store: false,
                    });
                Task::none()
            }
            BackendEvent::DeviceStatus(event) => {
                self.update(Message::Devices(devices::Msg::StatusChanged {
                    device_id: event.device_id,
                    status: event.status,
                }))
            }
            BackendEvent::CloneProgress(progress) => {
                // Streamed into the clone dialog's progress bar while its own
                // clone is running; unrelated clones only tick the footer.
                if self.clone.open && self.clone.running && self.clone.active_url == progress.url {
                    self.clone.progress = Some(clone_dialog::Progress {
                        received: progress.received,
                        total: progress.total,
                        objects: progress.objects,
                    });
                }
                self.last_op = Some(format!(
                    "clone: {} / {} objects",
                    progress.objects, progress.total
                ));
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(bus::stream).map(Message::Backend),
            iced::system::theme_changes().map(Message::SystemTheme),
            iced::event::listen_with(modifiers_changed),
            iced::event::listen_with(window_focus_changed),
            iced::event::listen_with(window_geometry_changed),
            // Mouse release ends a pane-divider drag (the press started it).
            iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) => Some(Message::PaneDragEnd),
                _ => None,
            }),
            iced::window::close_events().map(Message::WindowClosed),
            // Overlay components own their key/backdrop handling; each mapper
            // is a no-op while its overlay is closed.
            iced::event::listen_with(|event, status, window| {
                palette::event_to_msg(event, status, window).map(Message::Palette)
            }),
            iced::event::listen_with(|event, status, window| {
                context_menu::event_to_msg(event, status, window).map(Message::ContextMenu)
            }),
            iced::event::listen_with(|event, status, window| {
                quick_switcher::event_to_msg(event, status, window).map(Message::Switcher)
            }),
            iced::event::listen_with(|event, status, window| {
                quick_open::event_to_msg(event, status, window).map(Message::QuickOpen)
            }),
            self.auto_fetch_subscription(),
        ];
        // Terminal panes: PTY output per live session (every tab, so
        // background panes keep accumulating output).
        for tab in &self.tabs {
            if let Some(session) = tab.terminal.session() {
                subs.push(terminal::subscription(session).map(Message::Terminal));
            }
        }
        // While the focused terminal holds keyboard focus, route key presses
        // to it; returning Some consumes them so other widgets never see a
        // keystroke meant for the shell. Shortcut combos (ctrl/cmd) are
        // ignored by the pane itself, so shell hotkeys keep working.
        if let Some(tab) = self.tabs.get(self.active) {
            if tab.terminal_open && tab.terminal.is_focused() {
                subs.push(iced::event::listen_with(
                    |event, _status, _window| match event {
                        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                            key,
                            modifiers,
                            ..
                        }) => Some(Message::Terminal(terminal::Msg::Key(key, modifiers))),
                        _ => None,
                    },
                ));
            }
        }
        Subscription::batch(subs)
    }

    /// Per-repo auto-fetch timer: fires [`Message::AutoFetchTick`] on the
    /// configured cadence while a tab is open. Keyed by (repo, cadence) so
    /// changing the cadence restarts the clock; `0` disables it entirely.
    fn auto_fetch_subscription(&self) -> Subscription<Message> {
        let minutes = self.config.auto_fetch_minutes;
        if minutes == 0 {
            return Subscription::none();
        }
        Subscription::batch(self.tabs.iter().map(|tab| {
            let repo_id = tab.repo.repo_id.clone();
            Subscription::run_with((repo_id.0.clone(), minutes), |(_, minutes)| {
                let interval = Duration::from_secs(60 * u64::from(*minutes));
                futures_util::stream::unfold((), move |_| async move {
                    tokio::time::sleep(interval).await;
                    Some(((), ()))
                })
            })
            // `with` carries the repo id into the stream identity and items;
            // `map` closures must be non-capturing in iced 0.14.
            .with(repo_id)
            .map(|(repo_id, _tick)| Message::AutoFetchTick(repo_id))
        }))
    }

    fn view(&self, id: window::Id) -> Element<'_, Message> {
        if let Some((_, pane)) = self.popout.as_ref().filter(|(popout, _)| *popout == id) {
            return diff_view::view(
                pane,
                DiffSlot::Commit,
                self.tokens(),
                false,
                false,
                PaneMode::default(),
            )
            .map(|msg| Message::Popout(Box::new(msg)));
        }
        let _ = id;
        let tokens = self.tokens();
        let body: Element<'_, Message> = if self.show_home || self.tabs.is_empty() {
            home::view(
                tokens,
                &self.recent,
                self.open_in_flight,
                config::now_millis(),
                &self.devices,
            )
        } else {
            repo_view::view(
                &self.tabs,
                self.active,
                tokens,
                self.window_save.frame.size[0],
            )
        };

        let page: Element<'_, Message> = column![self.header(tokens), body, self.footer(tokens)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // Overlays stack above the page; prompts stay topmost.
        let mut layers: Vec<Element<'_, Message>> = vec![page];
        if self.palette.open {
            layers.push(palette::view(&self.palette, &tokens).map(Message::Palette));
        }
        if self.switcher.open {
            layers.push(quick_switcher::view(&self.switcher, &tokens).map(Message::Switcher));
        }
        if self.quick_open.open {
            layers.push(quick_open::view(&self.quick_open, &tokens).map(Message::QuickOpen));
        }
        if self.clone.open {
            layers.push(clone_dialog::view(&self.clone, &tokens).map(Message::Clone));
        }
        // AI settings: a global overlay (the ai section of config.json is
        // app-wide; the per-repo checkbox reads the active tab).
        if self.ai_settings.open {
            let repo_opt_in = self
                .tabs
                .get(self.active)
                .map(|tab| self.config.ai_enabled_for(&tab.repo.root));
            layers.push(
                ai_settings::view(
                    &self.ai_settings,
                    &tokens,
                    self.config.ai.enabled,
                    repo_opt_in,
                )
                .map(Message::AiSettings),
            );
        }
        // SSH devices: the add/edit dialog and the device file browser are
        // global overlays (reachable from Home and the palette); the forget
        // confirmation renders while a device id is pending.
        if self.devices.dialog.open {
            layers.push(devices::dialog_view(&self.devices, &tokens).map(Message::Devices));
        }
        if self.devices.browse.open {
            layers.push(devices::browse_view(&self.devices, &tokens).map(Message::Devices));
        }
        if let Some(id) = self.devices.pending_forget.clone() {
            let name = self
                .devices
                .rows
                .iter()
                .find(|row| row.profile.id == id)
                .map(|row| row.profile.name.clone())
                .unwrap_or_else(|| id.clone());
            let spec = dialog::Confirm {
                title: "Forget device".to_string(),
                body: format!(
                    "Forget “{name}”? The profile and its stored secrets (OS keyring) are removed and the device is disconnected."
                ),
                detail: None,
                confirm_label: "Forget".to_string(),
                danger: true,
            };
            layers.push(dialog::confirm(
                tokens,
                &spec,
                Message::CancelDialog,
                Message::Devices(devices::Msg::ForgetConfirmed),
            ));
        }
        if self.ctx_menu.open {
            layers.push(context_menu::view(&self.ctx_menu, &tokens).map(Message::ContextMenu));
        }
        if let Some(result) = &self.ai_result {
            layers.push(ai_result_overlay(result, tokens));
        }
        if self.help_open {
            layers.push(help_overlay(tokens));
        }
        if let Some(overlay) = prompts::overlay(
            tokens,
            self.auth_prompts.front(),
            self.host_key_prompts.front(),
            self.device_secret_prompts.front(),
        ) {
            layers.push(overlay);
        }
        iced::widget::stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn header(&self, tokens: Tokens) -> Element<'_, Message> {
        let mut actions = row![].spacing(4).align_y(Alignment::Center);

        if !self.tabs.is_empty() {
            actions = actions.push(
                button(text("Home").size(12))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([5, 10])
                    .on_press(Message::GoHome),
            );
        }

        let open_label = if self.open_in_flight {
            "Opening…"
        } else {
            "Open…"
        };
        let mut open = button(text(open_label).size(12))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([5, 10]);
        if !self.open_in_flight {
            open = open.on_press(Message::OpenDialog);
        }
        actions = actions.push(open);

        actions = actions.push(
            button(text(format!("Theme: {}", self.config.theme.label())).size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([5, 10])
                .on_press(Message::CycleTheme),
        );

        actions = actions.push(
            button(text(Config::auto_fetch_label(self.config.auto_fetch_minutes)).size(12))
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([5, 10])
                .on_press(Message::CycleAutoFetch),
        );

        // AI health chip (derived, click opens the AI settings dialog).
        let ai_health = ai_settings::ai_health(
            self.config.ai.enabled,
            self.ai_status.as_ref(),
            self.ai_last_error.as_deref(),
        );
        let ai_tone = match ai_settings::ai_health_tone(&ai_health) {
            ai_settings::AiHealthTone::Muted => tokens.muted_foreground,
            ai_settings::AiHealthTone::Good => tokens.added,
            ai_settings::AiHealthTone::Warn => tokens.modified,
            ai_settings::AiHealthTone::Bad => tokens.deleted,
        };
        actions = actions.push(
            button(
                text(ai_settings::ai_health_label(&ai_health))
                    .size(12)
                    .color(ai_tone),
            )
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([5, 10])
            .on_press(Message::AiOpenSettings),
        );

        container(
            row![
                text("mygitui")
                    .size(14)
                    .font(theme::semibold())
                    .color(tokens.foreground),
                Space::new().width(Length::Fill),
                actions,
            ]
            .align_y(Alignment::Center)
            .padding([6, 10]),
        )
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(tokens.card)),
            border: iced::Border {
                color: tokens.border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
    }

    fn footer(&self, tokens: Tokens) -> Element<'_, Message> {
        let (label, color) = match &self.notice {
            Some(notice) => (
                notice.text.clone(),
                if notice.error {
                    tokens.deleted
                } else {
                    tokens.foreground
                },
            ),
            None => (
                self.last_op.clone().unwrap_or_else(|| "Ready".to_string()),
                tokens.muted_foreground,
            ),
        };

        let mut bar = row![
            text(label).size(11).color(color),
            Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center)
        .padding([4, 10]);

        if self.active_op.is_some() {
            bar = bar.push(
                button(text("Cancel").size(11))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Message::CancelOp),
            );
        } else if self.clone_cancel.is_some() && self.clone.open && self.clone.running {
            // Clones aren't repo ops (no `op_cancel` id); the flag aborts
            // the transfer. Available from the moment the clone starts.
            bar = bar.push(
                button(text("Cancel").size(11))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Message::CloneCancel),
            );
        }

        if let Some(link) = self.notice.as_ref().and_then(|notice| notice.link.clone()) {
            bar = bar.push(
                button(text("Open").size(11))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Message::OpenUrl(link)),
            );
        }
        if self.notice.is_some() {
            bar = bar.push(
                button(text("Dismiss").size(11))
                    .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                    .padding([2, 8])
                    .on_press(Message::DismissNotice),
            );
        }

        container(bar)
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(iced::Background::Color(tokens.card)),
                border: iced::Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    fn theme(&self, _id: window::Id) -> iced::Theme {
        theme::app_theme(self.tokens())
    }

    fn title(&self, id: window::Id) -> String {
        if self
            .popout
            .as_ref()
            .is_some_and(|(popout, _)| *popout == id)
        {
            return "mygitui — diff".to_string();
        }
        "mygitui".to_string()
    }
}

/// Window focus transitions (main window) pause/resume remote repo change
/// polling: devices keep dialing only while the app is looked at. The
/// update arm filters to [`App::main_window`] (popouts are irrelevant).
fn window_focus_changed(
    event: iced::Event,
    _status: iced::event::Status,
    window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocus {
            window,
            focused: true,
        }),
        iced::Event::Window(window::Event::Unfocused) => Some(Message::WindowFocus {
            window,
            focused: false,
        }),
        _ => None,
    }
}

/// Window move/resize events feeding the debounced geometry persistence.
/// Like the focus listener, the update arm filters to the main window
/// (popout geometry is irrelevant).
fn window_geometry_changed(
    event: iced::Event,
    _status: iced::event::Status,
    window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Moved(position)) => {
            Some(Message::WindowMoved { window, position })
        }
        iced::Event::Window(window::Event::Resized(size)) => {
            Some(Message::WindowResized { window, size })
        }
        _ => None,
    }
}

/// Appends an actionable hint to raw engine/git error strings for the
/// failure classes users actually hit. Matching is on the engine's stable
/// phrasings (lowercased substring); anything unmatched passes through
/// unchanged — never worse than the raw string.
fn friendly_error(err: &str) -> String {
    let lower = err.to_lowercase();
    let hint = if lower.contains("auth")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("permission denied")
        || lower.contains("publickey")
        || lower.contains("credentials")
    {
        "Check your credentials or SSH key, then retry — the app prompts when a remote asks."
    } else if lower.contains("tls")
        || lower.contains("ssl")
        || lower.contains("could not resolve")
        || lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("network")
    {
        "Check your network connection and the remote URL."
    } else if lower.contains("non-fast-forward")
        || lower.contains("fetch first")
        || lower.contains("rejected")
    {
        "The remote has newer commits — pull first, then push (or force-push deliberately)."
    } else if lower.contains("index.lock")
        || lower.contains("lock file")
        || lower.contains("already locked")
    {
        "Another git process may still be running — wait a moment and retry."
    } else if lower.contains("conflict")
        || lower.contains("local changes")
        || lower.contains("would be overwritten")
    {
        "Commit or stash your changes first, then retry."
    } else {
        return err.to_string();
    };
    format!("{err} — {hint}")
}

/// Fixed shell shortcuts handled by the global key handler (fixed defaults,
/// not user-configurable). The palette mirrors these in its hint fields and
/// `docs/contracts.md` lists the final mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellShortcut {
    /// `F5` — refresh the active repo's status.
    RefreshRepo,
    /// `Ctrl/Cmd+1..8` — toggle the panel at this fixed index (see
    /// [`panel_toggle_message`]).
    Panel(usize),
    /// `Ctrl/Cmd+S` — save the in-app editor file (no-op when it is closed).
    SaveEditor,
    /// `Ctrl/Cmd+Enter` — commit staged changes (same arm as the Commit
    /// button, so its validation applies).
    Commit,
    /// `Ctrl/Cmd+Alt+Right` — next repo tab (wraps).
    NextTab,
    /// `Ctrl/Cmd+Alt+Left` — previous repo tab (wraps).
    PrevTab,
    /// `Ctrl/Cmd+W` — close the current repo tab.
    CloseTab,
    /// `Escape` — dismiss the top-most open modal dialog (no-op when none
    /// is; the palette-family overlays close themselves via their own key
    /// listeners before this ever fires).
    CancelDialog,
    /// `F1` — toggle the keyboard-shortcuts help overlay.
    Help,
    /// `Tab` — move keyboard focus to the next focusable widget (best-
    /// effort traversal; widgets without focus state are skipped).
    FocusNext,
}

/// The `Ctrl/Cmd+1..8` panel mapping, in the fixed shortcut order that
/// mirrors the repo header toggle buttons left→right: 1 Branches,
/// 2 Stashes, 3 Remotes, 4 History, 5 Forge, 6 Repo health, 7 Reflog,
/// 8 Terminal.
fn panel_toggle_message(index: usize) -> Option<Message> {
    match index {
        0 => Some(Message::ToggleBranchPanel),
        1 => Some(Message::ToggleStashPanel),
        2 => Some(Message::ToggleRemotePanel),
        3 => Some(Message::ToggleHistory),
        4 => Some(Message::Forge(forge_panel::Msg::ToggleOpen)),
        5 => Some(Message::Health(health_panel::Msg::ToggleOpen)),
        6 => Some(Message::Reflog(reflog_panel::Msg::ToggleOpen)),
        7 => Some(Message::ToggleTerminal),
        _ => None,
    }
}

/// Wrapped tab step for the `Ctrl/Cmd+Alt+Left/Right` shortcuts (`next` is
/// +1 / -1). A single tab stays put; `len == 0` is a no-op for the caller.
fn wrapped_tab_index(active: usize, len: usize, next: bool) -> usize {
    if len <= 1 {
        return active.min(len.saturating_sub(1));
    }
    if next {
        (active + 1) % len
    } else {
        active.checked_sub(1).unwrap_or(len - 1)
    }
}

/// Maps a key press to a [`ShellShortcut`]. `cmd` mirrors the existing
/// global bindings (`⌘` on macOS, `Ctrl` elsewhere). Every binding is a
/// modifier combo except `F5` — plain keystrokes belong to typing surfaces
/// (the focused terminal pane writes `F5` into the shell itself).
fn shell_shortcut(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<ShellShortcut> {
    use iced::keyboard::key::Named;
    let cmd = modifiers.logo() || (modifiers.control() && !cfg!(target_os = "macos"));
    match key {
        iced::keyboard::Key::Named(Named::F5) => Some(ShellShortcut::RefreshRepo),
        // Escape is a plain key, but it only ever acts on a dialog; typing
        // surfaces (text inputs, the terminal) don't lose it — when no
        // dialog is open the arm below is a no-op.
        iced::keyboard::Key::Named(Named::Escape) => Some(ShellShortcut::CancelDialog),
        iced::keyboard::Key::Named(Named::F1) => Some(ShellShortcut::Help),
        iced::keyboard::Key::Named(Named::Tab) => Some(ShellShortcut::FocusNext),
        iced::keyboard::Key::Named(Named::Enter) if cmd => Some(ShellShortcut::Commit),
        iced::keyboard::Key::Named(Named::ArrowRight) if cmd && modifiers.alt() => {
            Some(ShellShortcut::NextTab)
        }
        iced::keyboard::Key::Named(Named::ArrowLeft) if cmd && modifiers.alt() => {
            Some(ShellShortcut::PrevTab)
        }
        iced::keyboard::Key::Character(c) if cmd => match c.as_str() {
            "1" => Some(ShellShortcut::Panel(0)),
            "2" => Some(ShellShortcut::Panel(1)),
            "3" => Some(ShellShortcut::Panel(2)),
            "4" => Some(ShellShortcut::Panel(3)),
            "5" => Some(ShellShortcut::Panel(4)),
            "6" => Some(ShellShortcut::Panel(5)),
            "7" => Some(ShellShortcut::Panel(6)),
            "8" => Some(ShellShortcut::Panel(7)),
            "s" => Some(ShellShortcut::SaveEditor),
            "w" => Some(ShellShortcut::CloseTab),
            _ => None,
        },
        _ => None,
    }
}

fn modifiers_changed(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::Modifiers(modifiers))
        }
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::CursorMoved(position))
        }
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            let cmd = modifiers.logo() || (modifiers.control() && !cfg!(target_os = "macos"));
            match &key {
                iced::keyboard::Key::Character(c) if cmd && modifiers.shift() && c == "P" => {
                    Some(Message::Palette(palette::Msg::Toggle))
                }
                iced::keyboard::Key::Character(c) if cmd && c == "o" => Some(Message::OpenDialog),
                iced::keyboard::Key::Character(c) if cmd && c == "k" => {
                    Some(Message::Switcher(quick_switcher::Msg::Toggle))
                }
                iced::keyboard::Key::Character(c) if cmd && c == "p" => {
                    Some(Message::QuickOpen(quick_open::Msg::Toggle))
                }
                // Everything else in the fixed shortcut suite routes through
                // Message::Shortcut, which drops it while an overlay with its
                // own key listener is open.
                _ => shell_shortcut(&key, modifiers).map(Message::Shortcut),
            }
        }
        _ => None,
    }
}

/// Context menu for a status row (right-click).
fn status_row_menu(item: &StatusRow) -> context_menu::Menu {
    let mut menu = context_menu::Menu::new();
    match item.section {
        SectionId::Staged => {
            menu = menu.command(
                format!("status.unstage:{}", item.path),
                format!("Unstage “{}”", item.base),
            );
        }
        SectionId::Changes => {
            menu = menu.command(
                format!("status.stage:{}", item.path),
                format!("Stage “{}”", item.base),
            );
            menu = menu.command(
                format!("status.discard:{}", item.path),
                format!("Discard changes in “{}”", item.base),
            );
        }
    }
    if item.index != ChangeKind::Conflicted && item.worktree != ChangeKind::Conflicted {
        menu = menu.separator();
        let variants = ignore_variants(&item.path);
        menu = menu.command(
            format!("status.ignore:{}", variants.exact),
            format!("Ignore “{}”", item.base),
        );
        if let Some(glob) = &variants.extension {
            menu = menu.command(format!("status.ignore:{glob}"), format!("Ignore “{glob}”"));
        }
        if let Some(dir) = &variants.directory {
            menu = menu.command(format!("status.ignore:{dir}"), format!("Ignore “{dir}”"));
        }
    }
    menu = menu.separator();
    menu = menu.command(format!("status.file_history:{}", item.path), "File history");
    menu = menu.command(format!("status.blame:{}", item.path), "Blame");
    menu
}

/// Context menu for a Files panel row (right-click). `path` is
/// repository-relative with forward slashes; directories (and the worktree
/// root, via the header buttons) get create/rename/delete entries, files get
/// open/history/copy/external-editor ones.
fn files_row_menu(path: &str, is_dir: bool) -> context_menu::Menu {
    let mut menu = context_menu::Menu::new();
    if path.is_empty() {
        menu = menu.header("Working tree");
    } else {
        menu = menu.header(files_panel::base_name(path).to_string());
    }
    if is_dir {
        menu = menu
            .command(format!("files.new_file:{path}"), "New file…")
            .command(format!("files.new_folder:{path}"), "New folder…");
        if !path.is_empty() {
            menu = menu
                .separator()
                .command(format!("files.rename:{path}"), "Rename…")
                .command(format!("files.delete:{path}"), "Delete");
        }
    } else {
        menu = menu
            .command(format!("files.open:{path}"), "Open")
            .command(format!("files.history:{path}"), "File history")
            .separator()
            .command(format!("files.copy:{path}"), "Copy path")
            .command(format!("files.external:{path}"), "Open in external editor")
            .separator()
            .command(format!("files.rename:{path}"), "Rename…")
            .command(format!("files.delete:{path}"), "Delete");
    }
    menu
}

/// Context menu for a local branch row (right-click).
fn branch_row_menu(name: &str) -> context_menu::Menu {
    let mut menu = context_menu::Menu::new();
    menu = menu.command(
        format!("branch.switch:{name}"),
        format!("Switch to “{name}”"),
    );
    menu = menu.separator();
    menu = menu.command(format!("branch.rename:{name}"), "Rename…");
    menu = menu.command(format!("branch.delete:{name}"), format!("Delete “{name}”"));
    menu
}

/// Keyboard-shortcuts help overlay (F1 / palette "Keyboard shortcuts").
/// The fixed shell suite plus the overlay hotkeys, in discovery order.
fn help_overlay<'a>(tokens: Tokens) -> Element<'a, Message> {
    const ROWS: [(&str, &str); 13] = [
        ("Refresh repository", "F5"),
        ("Toggle panels", "Ctrl+1 … 8"),
        ("Save editor file", "Ctrl+S"),
        ("Commit staged changes", "Ctrl+Enter"),
        ("Next / previous tab", "Ctrl+Alt+→ / ←"),
        ("Close tab", "Ctrl+W"),
        ("Close dialog", "Esc"),
        ("Cycle keyboard focus", "Tab"),
        ("Command palette", "Ctrl+Shift+P"),
        ("Repo quick-switcher", "Ctrl+K"),
        ("Quick open file", "Ctrl+P"),
        ("Open repository", "Ctrl+O"),
        ("Toggle this help", "F1"),
    ];
    let mut body = column![
        text("Keyboard shortcuts")
            .size(theme::TEXT_TITLE)
            .font(theme::semibold()),
        Space::new().height(6),
    ]
    .spacing(2);
    for (action, key) in ROWS {
        body = body.push(
            row![
                text(action).size(theme::TEXT_SMALL).width(Length::Fill),
                text(key)
                    .size(theme::TEXT_SMALL)
                    .font(theme::mono())
                    .color(tokens.muted_foreground),
            ]
            .align_y(Alignment::Center),
        );
    }
    body = body.push(text("The command palette (Ctrl+Shift+P) lists every command; most appear here as they ship."))
        .push(Space::new().height(4));

    super::dialog::modal_with_cancel(
        tokens,
        520.0,
        body.into(),
        Space::new().width(0).into(),
        Message::ToggleHelp,
    )
}

/// Centered modal showing an AI generation (review/explain/branch name)
/// with copy + close; the body scrolls for long reviews.
fn ai_result_overlay<'a>(result: &'a AiResult, tokens: Tokens) -> Element<'a, Message> {
    let title = row![
        text(result.title.as_str())
            .size(12)
            .font(theme::semibold())
            .width(Length::Fill),
        button(text("Copy").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(Message::CopyText(result.body.clone())),
        Space::new().width(6),
        button(text("×").size(12))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(Message::AiResultClose),
    ]
    .align_y(Alignment::Center);

    let body = column![
        title,
        Space::new().height(8),
        scrollable(
            container(
                text(result.body.as_str())
                    .size(theme::TEXT_SMALL)
                    .font(theme::mono())
            )
            .width(Length::Fill)
        )
        .height(Length::Fixed(380.0)),
    ]
    .spacing(4)
    .width(Length::Fill);

    // Shared modal scaffold: proper dimming (BLACK 0.4) like every other
    // dialog — the old muted-tint scrim was near-invisible in dark theme.
    super::dialog::modal_with_cancel(
        tokens,
        640.0,
        body.into(),
        Space::new().width(0).into(),
        Message::AiResultClose,
    )
}

/// Feeds a loaded plan (the planner's initial pick list or an autosquash
/// rebuild) into the tab's open planner, enriching the rows with commit
/// subjects from the streamed history. A stale load (planner closed, or
/// reopened on another commit) is dropped.
fn load_planner_plan(tab: &mut Tab, onto: &str, plan: Vec<RebaseStep>) {
    // Snapshot the subjects first: the planner and the history both live on
    // the tab, so the two borrows cannot overlap.
    let summaries: Vec<(String, Option<String>)> = {
        let commits = &tab.history.commits;
        plan.iter()
            .map(|step| {
                let summary = commits
                    .iter()
                    .find(|commit| commit.sha == step.sha)
                    .map(|commit| commit.summary.clone());
                (step.sha.clone(), summary)
            })
            .collect()
    };
    let Some(planner) = tab.pending_rebase_planner.as_mut() else {
        return;
    };
    if planner.onto != onto {
        return;
    }
    planner.plan_loaded(plan);
    rebase_planner::fill_summaries(&mut planner.rows, |sha| {
        summaries
            .iter()
            .find(|(key, _)| key == sha)
            .and_then(|(_, summary)| summary.clone())
    });
}

/// Context menu for a history commit row (right-click).
fn commit_row_menu(sha: &str) -> context_menu::Menu {
    let short = repo_view::short_sha(sha);
    context_menu::Menu::new()
        .command(format!("commit.copy:{sha}"), format!("Copy SHA ({short})"))
        .command(format!("commit.copy_describe:{sha}"), "Copy describe")
        .separator()
        .command(format!("commit.tag:{sha}"), "Tag here…")
        .command(format!("commit.cherry_pick:{sha}"), "Cherry-pick onto HEAD")
        .command(format!("commit.revert:{sha}"), "Revert")
        .command(format!("commit.reset:{sha}"), "Reset branch to here…")
        .command(format!("commit.rebase:{sha}"), "Rebase from here…")
        .command(
            format!("commit.branch:{sha}"),
            format!("Create branch here (from {short})"),
        )
}

/// First non-flag argument; used for `mygitui <path>` first launch.
/// First usable launch argument: a plain repository path (non-flag), or a
/// `mygitui://open?path=…` deep link. Flags are skipped; unparseable deep
/// links are too (later args still get a chance).
fn first_repo_arg(args: &[String]) -> Option<String> {
    args.iter()
        .filter(|arg| !arg.starts_with('-'))
        .find_map(|arg| {
            if let Some(rest) = arg.strip_prefix("mygitui://open") {
                let query = rest.trim_start_matches('/');
                let query = query.strip_prefix('?').unwrap_or(query);
                query.split('&').find_map(|pair| {
                    pair.strip_prefix("path=")
                        .map(percent_decode)
                        .filter(|path| !path.is_empty())
                })
            } else {
                Some(arg.clone())
            }
        })
}

/// Minimal percent-decoding for deep-link query values (`%XX` and `+`).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{first_repo_arg, panel_toggle_message, shell_shortcut, wrapped_tab_index};
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Modifiers};

    #[test]
    fn first_repo_arg_skips_flags() {
        let args = vec!["--debug".to_string(), "C:\\repo".to_string()];
        assert_eq!(first_repo_arg(&args).as_deref(), Some("C:\\repo"));
        assert_eq!(first_repo_arg(&[]), None);
        assert_eq!(first_repo_arg(&["-v".to_string()]), None);
    }

    #[test]
    fn first_repo_arg_decodes_deep_links() {
        assert_eq!(
            first_repo_arg(&["mygitui://open?path=%2Fhome%2Fme%2Fmy+repo".to_string()]),
            Some("/home/me/my repo".to_string())
        );
        assert_eq!(
            first_repo_arg(&["mygitui://open/?path=/repos/demo".to_string()]),
            Some("/repos/demo".to_string())
        );
        assert_eq!(
            first_repo_arg(&["--flag".to_string(), "mygitui://open".to_string()]),
            None
        );
        assert_eq!(
            first_repo_arg(&["mygitui://bogus".to_string()]),
            Some("mygitui://bogus".to_string())
        );
    }

    /// The platform shortcut modifier (`Ctrl` everywhere, `⌘` on macOS).
    fn cmd() -> Modifiers {
        if cfg!(target_os = "macos") {
            Modifiers::LOGO
        } else {
            Modifiers::CTRL
        }
    }

    #[test]
    fn f5_refreshes_without_modifiers() {
        assert_eq!(
            shell_shortcut(&Key::Named(Named::F5), Modifiers::empty()),
            Some(super::ShellShortcut::RefreshRepo)
        );
        // Ctrl+F5 stays the same refresh; plain digits/letters do nothing.
        assert_eq!(
            shell_shortcut(&Key::Named(Named::F5), cmd()),
            Some(super::ShellShortcut::RefreshRepo)
        );
        assert_eq!(
            shell_shortcut(&Key::Character("1".into()), Modifiers::empty()),
            None
        );
        assert_eq!(
            shell_shortcut(&Key::Character("s".into()), Modifiers::empty()),
            None
        );
    }

    #[test]
    fn ctrl_digits_map_to_the_fixed_panel_order() {
        for (index, digit) in ["1", "2", "3", "4", "5", "6", "7", "8"].iter().enumerate() {
            assert_eq!(
                shell_shortcut(&Key::Character((*digit).into()), cmd()),
                Some(super::ShellShortcut::Panel(index)),
                "Ctrl+{digit} maps to panel index {index}"
            );
        }
        assert_eq!(
            shell_shortcut(&Key::Character("9".into()), cmd()),
            None,
            "only 1..8 are bound"
        );
        // The panel mapping mirrors the repo header toggles left→right:
        // 1 Branches, 2 Stashes, 3 Remotes, 4 History, 5 Forge, 6 Health,
        // 7 Reflog, 8 Terminal.
        assert!(matches!(
            panel_toggle_message(0),
            Some(super::Message::ToggleBranchPanel)
        ));
        assert!(matches!(
            panel_toggle_message(1),
            Some(super::Message::ToggleStashPanel)
        ));
        assert!(matches!(
            panel_toggle_message(2),
            Some(super::Message::ToggleRemotePanel)
        ));
        assert!(matches!(
            panel_toggle_message(3),
            Some(super::Message::ToggleHistory)
        ));
        assert!(matches!(
            panel_toggle_message(4),
            Some(super::Message::Forge(super::forge_panel::Msg::ToggleOpen))
        ));
        assert!(matches!(
            panel_toggle_message(5),
            Some(super::Message::Health(super::health_panel::Msg::ToggleOpen))
        ));
        assert!(matches!(
            panel_toggle_message(6),
            Some(super::Message::Reflog(super::reflog_panel::Msg::ToggleOpen))
        ));
        assert!(matches!(
            panel_toggle_message(7),
            Some(super::Message::ToggleTerminal)
        ));
        assert!(panel_toggle_message(8).is_none());
    }

    #[test]
    fn combo_shortcuts_map_to_their_actions() {
        assert_eq!(
            shell_shortcut(&Key::Named(Named::Enter), cmd()),
            Some(super::ShellShortcut::Commit)
        );
        assert_eq!(
            shell_shortcut(&Key::Character("s".into()), cmd()),
            Some(super::ShellShortcut::SaveEditor)
        );
        assert_eq!(
            shell_shortcut(&Key::Character("w".into()), cmd()),
            Some(super::ShellShortcut::CloseTab)
        );
        // Enter without modifiers stays a typing keystroke.
        assert_eq!(
            shell_shortcut(&Key::Named(Named::Enter), Modifiers::empty()),
            None
        );
        // Tab cycling needs cmd+alt on the arrows.
        assert_eq!(
            shell_shortcut(&Key::Named(Named::ArrowRight), cmd() | Modifiers::ALT),
            Some(super::ShellShortcut::NextTab)
        );
        assert_eq!(
            shell_shortcut(&Key::Named(Named::ArrowLeft), cmd() | Modifiers::ALT),
            Some(super::ShellShortcut::PrevTab)
        );
        assert_eq!(
            shell_shortcut(&Key::Named(Named::ArrowRight), cmd()),
            None,
            "alt is required for tab cycling"
        );
        assert_eq!(
            shell_shortcut(&Key::Named(Named::ArrowRight), Modifiers::ALT),
            None
        );
    }

    #[test]
    fn wrapped_tab_index_steps_and_wraps() {
        assert_eq!(wrapped_tab_index(0, 3, true), 1);
        assert_eq!(wrapped_tab_index(2, 3, true), 0, "next wraps at the end");
        assert_eq!(wrapped_tab_index(2, 3, false), 1);
        assert_eq!(
            wrapped_tab_index(0, 3, false),
            2,
            "previous wraps at the start"
        );
        // Degenerate cases: no tabs or a single tab never moves.
        assert_eq!(wrapped_tab_index(0, 0, true), 0);
        assert_eq!(wrapped_tab_index(0, 1, true), 0);
        assert_eq!(wrapped_tab_index(0, 1, false), 0);
    }

    #[test]
    fn window_settings_clamp_restored_size_and_keep_position() {
        let frame = super::config::WindowFrame {
            size: [1.0, 1.0],
            position: Some([12.0, 24.0]),
            maximized: true,
        };
        let settings = super::window_settings(&frame);
        assert_eq!(settings.size.width, super::MIN_WINDOW_SIZE.0);
        assert_eq!(settings.size.height, super::MIN_WINDOW_SIZE.1);
        assert!(settings.maximized);
        assert!(matches!(
            settings.position,
            iced::window::Position::Specific(point) if point.x == 12.0 && point.y == 24.0
        ));

        // No persisted position: the platform default applies (Wayland).
        let settings = super::window_settings(&super::config::WindowFrame::default());
        assert!(matches!(settings.position, iced::window::Position::Default));
        assert_eq!(settings.size.width, 1152.0);
        assert_eq!(settings.size.height, 720.0);
        assert!(!settings.maximized);
    }
}
