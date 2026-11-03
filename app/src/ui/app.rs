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

use iced::widget::{button, column, container, row, text, text_editor, Space};
use iced::{window, Alignment, Element, Length, Subscription, Task};

use crate::engine::types::{
    BranchInfo, CommitOptions, DiffSide, FileDiff, LineRange, RepoId, RepoInfo, RepoStatus,
    StageRequest, StageTarget,
};
use crate::service::events::BackendEvent;
use crate::service::AppServices;

use super::bus;
use super::config::{self, Config, RecentRepo};
use super::diff_view::{DiffPane, DiffSlot};
use super::history_model::{to_log_filter, FilterFields, HistoryFilterField};
use super::home;
use super::repo_view::{self, Load, PendingRemap, Tab};
use super::status_model::{count_label, filter_sections, SectionId, StatusRow};
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
}

struct Notice {
    text: String,
    error: bool,
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

    ConfirmDiscard,
    CancelDialog,
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
        let (old, new) = match row.section {
            SectionId::Staged => (DiffSide::Head, DiffSide::Index),
            SectionId::Changes => (DiffSide::Index, DiffSide::Worktree),
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

    fn visible_rows<'a>(tab: &'a Tab) -> Vec<&'a StatusRow> {
        filter_sections(&tab.status_index, &tab.filter)
            .into_iter()
            .flat_map(|section| section.rows)
            .collect()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
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
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].status = Load::Loading;
                    self.tabs[index].reload_diff = true;
                    if branches_open {
                        self.tabs[index].branches.loading = true;
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
            Message::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                    self.show_home = false;
                }
                Task::none()
            }
            Message::CloseTab(repo_id) => {
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
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
                Task::none()
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
        }
    }

    fn backend(&mut self, event: BackendEvent) -> Task<Message> {
        match event {
            BackendEvent::RepoChanged(changed) => {
                let branches_open = repo_view::tab_index(&self.tabs, &changed.repo_id)
                    .is_some_and(|index| self.tabs[index].branches.open);
                if let Some(index) = repo_view::tab_index(&self.tabs, &changed.repo_id) {
                    self.tabs[index].reload_diff = true;
                    if branches_open && (changed.head_moved || changed.full) {
                        self.tabs[index].branches.loading = true;
                    }
                }
                let mut tasks = vec![self.refresh(changed.repo_id.clone())];
                if changed.head_moved || changed.full {
                    tasks.push(self.start_log(changed.repo_id.clone()));
                    if branches_open {
                        tasks.push(self.load_branches(changed.repo_id.clone()));
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
                if !progress.done {
                    return Task::none();
                }
                if let Some(err) = progress.error.as_deref() {
                    self.notice = Some(Notice::error(format!("{} failed: {err}", progress.kind)));
                }
                self.refresh(RepoId(progress.repo_id))
            }
            BackendEvent::OpLog(line) => {
                self.last_op = Some(line.line);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(bus::stream).map(Message::Backend),
            iced::system::theme_changes().map(Message::SystemTheme),
            iced::event::listen_with(modifiers_changed),
        ])
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

        column![self.header(tokens), body, self.footer(tokens)]
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
        _ => None,
    }
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
