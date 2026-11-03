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

use iced::widget::{button, column, container, row, text, text_editor, Space};
use iced::{window, Alignment, Element, Length, Subscription, Task};

use crate::engine::types::{
    BranchInfo, ChangeKind, CheckpointInfo, CommitOptions, ConflictResolution, DiffSide,
    FetchOptions, FileDiff, LineRange, MergeOptions, MergeOutcome, MergeResult, NetStats,
    PreviewInfo, PullOptions, PushOptions, RebaseState, RebaseStep, RemoteInfo, RepoId, RepoInfo,
    RepoStatus, ResetKind, StageRequest, StageTarget, StashInfo,
};
use crate::remote::prompts::SecretAnswer;
use crate::service::events::BackendEvent;
use crate::service::AppServices;

use super::bus;
use super::config::{self, Config, RecentRepo};
use super::context_menu;
use super::dialog::{self, ResetRequest};
use super::diff_view::{DiffPane, DiffSlot};
use super::forge_panel;
use super::history_model::{to_log_filter, FilterFields, HistoryFilterField};
use super::home;
use super::palette;
use super::prompts::{self, AuthPromptState, DeviceSecretPromptState, HostKeyPromptState};
use super::quick_switcher;
use super::remote_panel::net_summary;
use super::repo_view::{self, Load, PendingRebase, PendingRemap, Tab};
use super::status_model::{count_label, filter_sections, SectionId, StatusRow};
use super::terminal;
use super::theme::{self, Mode, Tokens};

pub fn run(cli_args: Vec<String>) -> iced::Result {
    iced::application(move || App::boot(cli_args.clone()), App::update, App::view)
        .title("mygitui")
        .theme(App::theme)
        .subscription(App::subscription)
        .window(window::Settings {
            size: iced::Size::new(1152.0, 720.0),
            min_size: Some(iced::Size::new(900.0, 600.0)),
            icon: window::icon::from_file_data(include_bytes!("../../icons/64x64.png"), None).ok(),
            ..window::Settings::default()
        })
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
    tabs: Vec<Tab>,
    active: usize,
    show_home: bool,
    notice: Option<Notice>,
    last_op: Option<String>,
    /// Op currently reporting progress, for the footer cancel button.
    active_op: Option<ActiveOp>,
    auth_prompts: std::collections::VecDeque<AuthPromptState>,
    host_key_prompts: std::collections::VecDeque<HostKeyPromptState>,
    device_secret_prompts: std::collections::VecDeque<DeviceSecretPromptState>,
    /// Command palette overlay state (`ctrl/cmd+shift+p`).
    palette: palette::State,
    /// Right-click context menu state (one menu at a time).
    ctx_menu: context_menu::State,
    /// Repo quick-switcher overlay state (`ctrl/cmd+k`).
    switcher: quick_switcher::State,
    /// Last known cursor position, used to anchor context menus opened via
    /// `mouse_area` right-presses (which do not carry coordinates).
    cursor: iced::Point,
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
    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Backend(BackendEvent),
    SystemTheme(iced::theme::Mode),
    Modifiers(iced::keyboard::Modifiers),
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
    PullRemote(String),
    PushRemote(String),
    RemoteRemoveRequested(String),
    RemoteRemoveConfirmed,
    RemotesDone {
        repo_id: RepoId,
        label: String,
        result: Result<String, String>,
    },

    MergeBranch(String),
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
    /// Track the cursor so right-press menus can be positioned.
    CursorMoved(iced::Point),
    /// Embedded terminal pane (routed to the active tab).
    Terminal(terminal::Msg),
    /// Show/hide the terminal dock.
    ToggleTerminal,
    /// GitHub PR/CI panel (routed to the active tab).
    Forge(forge_panel::Msg),
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
    fn boot(cli_args: Vec<String>) -> (Self, Task<Message>) {
        crate::auth::init();
        let services = Arc::new(AppServices::new());
        let config = Config::load(&services.config_dir);
        let recent = config.recent_sorted();
        let mut app = Self {
            services,
            config,
            recent,
            open_in_flight: false,
            system_mode: iced::theme::Mode::None,
            modifiers: iced::keyboard::Modifiers::default(),
            log_seq: 0,
            tabs: Vec::new(),
            active: 0,
            show_home: true,
            notice: None,
            last_op: None,
            active_op: None,
            auth_prompts: std::collections::VecDeque::new(),
            host_key_prompts: std::collections::VecDeque::new(),
            device_secret_prompts: std::collections::VecDeque::new(),
            palette: palette::State::default(),
            ctx_menu: context_menu::State::default(),
            switcher: quick_switcher::State::default(),
            cursor: iced::Point::ORIGIN,
        };

        let mut tasks = vec![iced::system::theme().map(Message::SystemTheme)];
        if let Some(path) = first_repo_arg(&cli_args) {
            tasks.push(app.open_repo(path));
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

    fn persist_config(&mut self) {
        if let Err(err) = self.config.save(&self.services.config_dir) {
            self.notice = Some(Notice::error(format!("Could not save settings: {err}")));
        }
    }

    fn reindex_recent(&mut self) {
        self.recent = self.config.recent_sorted();
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
            None,
        );
        register(
            "repo.refresh",
            "Refresh status",
            "Repository",
            repo_id.is_some(),
            None,
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
            "commit.commit",
            "Commit staged changes",
            "Commit",
            repo_id.is_some(),
            None,
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
            None,
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
            None,
        );
        register(
            "stash.toggle",
            "Toggle stashes panel",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "remotes.toggle",
            "Toggle remotes panel",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "view.toggle_terminal",
            "Toggle terminal panel",
            "View",
            repo_id.is_some(),
            None,
        );
        register(
            "view.toggle_forge",
            "Toggle forge panel",
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
            "ops.cancel",
            "Cancel running operation",
            "Safety net",
            self.active_op.is_some(),
            None,
        );

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
            "stash.toggle" => self.update(Message::ToggleStashPanel),
            "remotes.toggle" => self.update(Message::ToggleRemotePanel),
            "view.toggle_terminal" => self.update(Message::ToggleTerminal),
            "view.toggle_forge" => self.update(Message::Forge(forge_panel::Msg::ToggleOpen)),
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
            "ops.cancel" => self.update(Message::CancelOp),
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
            other if other.starts_with("branch.switch:") => self.update(Message::BranchSwitch(
                other["branch.switch:".len()..].to_string(),
            )),
            other if other.starts_with("branch.delete:") => self.update(
                Message::BranchDeleteRequested(other["branch.delete:".len()..].to_string()),
            ),
            other if other.starts_with("commit.copy:") => {
                self.update(Message::CopySha(other["commit.copy:".len()..].to_string()))
            }
            other if other.starts_with("commit.cherry_pick:") => self.update(Message::CherryPick(
                other["commit.cherry_pick:".len()..].to_string(),
            )),
            other if other.starts_with("commit.revert:") => self.update(Message::RevertCommit(
                other["commit.revert:".len()..].to_string(),
            )),
            other if other.starts_with("commit.reset:") => self.update(Message::ResetRequested(
                other["commit.reset:".len()..].to_string(),
            )),
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
                    if let Err(err) = crate::pty::pty_kill(&self.services, old) {
                        tracing::debug!(err, "terminal restart: old session kill failed");
                    }
                }
                self.spawn_terminal(repo_root)
            }
            terminal::Action::Write { session, data } => {
                if let Err(err) = crate::pty::pty_write(&self.services, session, data) {
                    tracing::debug!(err, "pty write failed");
                }
                Task::none()
            }
            terminal::Action::Resize {
                session,
                cols,
                rows,
            } => {
                if let Err(err) = crate::pty::pty_resize(&self.services, session, rows, cols) {
                    tracing::debug!(err, "pty resize failed");
                }
                Task::none()
            }
            terminal::Action::Kill { session } => {
                if let Err(err) = crate::pty::pty_kill(&self.services, session) {
                    tracing::debug!(err, "pty kill failed");
                }
                Task::none()
            }
            terminal::Action::FollowScroll { id } => {
                terminal::snap_to_bottom(id).map(Message::Terminal)
            }
        }
    }

    /// Spawns the shell session for the tab whose root matches (the active
    /// tab in practice) and feeds the returned session id back to its pane.
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
        match crate::pty::pty_create(&self.services, repo_id, Some(rows), Some(cols)) {
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
        match message {
            Message::Backend(event) => self.backend(event),
            Message::SystemTheme(mode) => {
                self.system_mode = mode;
                Task::none()
            }
            Message::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
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
                        let repo_id = info.repo_id.clone();
                        match repo_view::tab_index(&self.tabs, &repo_id) {
                            Some(index) => self.active = index,
                            None => {
                                self.tabs.push(Tab::new(info));
                                self.active = self.tabs.len() - 1;
                            }
                        }
                        self.show_home = false;
                        Task::batch([self.refresh(repo_id.clone()), self.start_log(repo_id)])
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Could not open {path}: {err}")));
                        Task::none()
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
                Task::none()
            }
            Message::CloseTab(repo_id) => {
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    if let Some(session) = self.tabs[index].terminal.session().map(str::to_owned) {
                        if let Err(err) = crate::pty::pty_kill(&self.services, session) {
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
                }
                Task::none()
            }
            Message::GoHome => {
                self.show_home = true;
                Task::none()
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
                    tab.pending_rebase = None;
                }
                Task::none()
            }
            Message::MutationDone {
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
                self.refresh(repo_id)
            }
            Message::DiffToggleSplit => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.diff_split = !tab.diff_split;
                    tab.diff.rebuild(tab.diff_split);
                    tab.commit_diff.rebuild(tab.diff_split);
                }
                Task::none()
            }
            Message::DiffToggleWrap => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.diff_wrap = !tab.diff_wrap;
                }
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
                let repo_id = tab.repo.repo_id.clone();
                self.load_branches(repo_id)
            }
            Message::RefreshBranches => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.branches.loading = true;
                tab.branches.error = None;
                let repo_id = tab.repo.repo_id.clone();
                self.load_branches(repo_id)
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
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Create branch {name}");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::branches::branch_create(
                            &services,
                            repo_id.clone(),
                            name,
                            None,
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
            Message::BranchSwitch(name) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
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
                        Task::batch([
                            self.refresh(repo_id.clone()),
                            self.start_log(repo_id.clone()),
                            self.load_branches(repo_id),
                        ])
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
                tab.stashes.error = None;
                tab.stashes.checkpoints_error = None;
                let repo_id = tab.repo.repo_id.clone();
                Task::batch([
                    self.load_stashes(repo_id.clone()),
                    self.load_checkpoints(repo_id),
                ])
            }
            Message::RefreshStashes => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                tab.stashes.loading = true;
                tab.stashes.checkpoints_loading = true;
                tab.stashes.error = None;
                tab.stashes.checkpoints_error = None;
                let repo_id = tab.repo.repo_id.clone();
                Task::batch([
                    self.load_stashes(repo_id.clone()),
                    self.load_checkpoints(repo_id),
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
            Message::StashPush => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let message = tab.stashes.message.trim().to_string();
                let include_untracked = tab.stashes.include_untracked;
                let repo_id = tab.repo.repo_id.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::stash::stash_push(
                            &services,
                            repo_id.clone(),
                            (!message.is_empty()).then_some(message),
                            false,
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
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("pull {remote}");
                let summary_label = label.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::pull(
                            &services,
                            repo_id.clone(),
                            PullOptions {
                                remote,
                                branch: String::new(),
                                ff_only: false,
                                rebase: false,
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
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("push {remote}");
                let summary_label = label.clone();
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::network::push(
                            &services,
                            repo_id.clone(),
                            PushOptions {
                                remote,
                                branch: String::new(),
                                force: false,
                                force_with_lease: false,
                                set_upstream: true,
                                refs: Vec::new(),
                                tags: false,
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
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
                        Task::none()
                    }
                }
            }
            Message::MergeBranch(name) => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return Task::none();
                };
                let repo_id = tab.repo.repo_id.clone();
                let label = format!("Merge {name}");
                let services = self.services.clone();
                Task::perform(
                    async move {
                        let result = crate::service::merge::merge_branch(
                            &services,
                            repo_id.clone(),
                            name,
                            MergeOptions::default(),
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
                        self.notice = Some(Notice::error(format!("{label} failed: {err}")));
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
            Message::CursorMoved(position) => {
                self.cursor = position;
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
                let action = forge_panel::update(&mut tab.forge, msg);
                let repo_id = tab.repo.repo_id.clone();
                self.run_forge_action(repo_id, action)
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
            Message::Noop => Task::none(),
        }
    }

    fn backend(&mut self, event: BackendEvent) -> Task<Message> {
        match event {
            BackendEvent::RepoChanged(changed) => {
                let branches_open = repo_view::tab_index(&self.tabs, &changed.repo_id)
                    .is_some_and(|index| self.tabs[index].branches.open);
                let stashes_open = repo_view::tab_index(&self.tabs, &changed.repo_id)
                    .is_some_and(|index| self.tabs[index].stashes.open);
                if let Some(index) = repo_view::tab_index(&self.tabs, &changed.repo_id) {
                    self.tabs[index].reload_diff = true;
                    if branches_open && (changed.head_moved || changed.full) {
                        self.tabs[index].branches.loading = true;
                    }
                    if stashes_open && changed.full {
                        self.tabs[index].stashes.loading = true;
                        self.tabs[index].stashes.checkpoints_loading = true;
                    }
                }
                let mut tasks = vec![self.refresh(changed.repo_id.clone())];
                if changed.head_moved || changed.full {
                    tasks.push(self.start_log(changed.repo_id.clone()));
                    if branches_open {
                        tasks.push(self.load_branches(changed.repo_id.clone()));
                    }
                    if stashes_open {
                        tasks.push(self.load_stashes(changed.repo_id.clone()));
                        tasks.push(self.load_checkpoints(changed.repo_id.clone()));
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
            _ => Task::none(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(bus::stream).map(Message::Backend),
            iced::system::theme_changes().map(Message::SystemTheme),
            iced::event::listen_with(modifiers_changed),
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

    fn view(&self) -> Element<'_, Message> {
        let tokens = self.tokens();
        let body: Element<'_, Message> = if self.show_home || self.tabs.is_empty() {
            home::view(
                tokens,
                &self.recent,
                self.open_in_flight,
                config::now_millis(),
            )
        } else {
            repo_view::view(&self.tabs, self.active, tokens)
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
        if self.ctx_menu.open {
            layers.push(context_menu::view(&self.ctx_menu, &tokens).map(Message::ContextMenu));
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

    fn theme(&self) -> iced::Theme {
        theme::app_theme(self.tokens())
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
                iced::keyboard::Key::Character(c) if cmd && c == "k" => {
                    Some(Message::Switcher(quick_switcher::Msg::Toggle))
                }
                _ => None,
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
        menu = menu.command(
            format!("status.ignore:{}", item.path),
            format!("Ignore “{}”", item.base),
        );
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
    menu = menu.command(format!("branch.delete:{name}"), format!("Delete “{name}”"));
    menu
}

/// Context menu for a history commit row (right-click).
fn commit_row_menu(sha: &str) -> context_menu::Menu {
    let short = repo_view::short_sha(sha);
    context_menu::Menu::new()
        .command(format!("commit.copy:{sha}"), format!("Copy SHA ({short})"))
        .separator()
        .command(format!("commit.cherry_pick:{sha}"), "Cherry-pick onto HEAD")
        .command(format!("commit.revert:{sha}"), "Revert")
        .command(format!("commit.reset:{sha}"), "Reset branch to here…")
}

/// First non-flag argument; used for `mygitui <path>` first launch.
fn first_repo_arg(args: &[String]) -> Option<String> {
    args.iter().find(|arg| !arg.starts_with('-')).cloned()
}

#[cfg(test)]
mod tests {
    use super::first_repo_arg;

    #[test]
    fn first_repo_arg_skips_flags() {
        let args = vec!["--debug".to_string(), "C:\\repo".to_string()];
        assert_eq!(first_repo_arg(&args).as_deref(), Some("C:\\repo"));
        assert_eq!(first_repo_arg(&[]), None);
        assert_eq!(first_repo_arg(&["-v".to_string()]), None);
    }
}
