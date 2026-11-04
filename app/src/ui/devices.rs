//! SSH device management UI: the Home devices section, the add/edit dialog
//! and the device file browser / repo scanner.
//!
//! Pure UI + intent module (the `clone_dialog` idiom): no service calls, no
//! `Task`. The shell wires it like the other overlays:
//!
//! 1. Own a [`State`]; feed it `device_list` results as
//!    [`Msg::Loaded`] and `BackendEvent::DeviceStatus` transitions as
//!    [`Msg::StatusChanged`].
//! 2. Route [`Msg`] through [`update`], translating the returned [`Action`]
//!    into `remote::commands` calls (`device_add`/`device_update`,
//!    `device_test`, `device_connect`/`device_disconnect`, `device_remove`,
//!    `device_browse_dir`, `device_scan_repos`, `remote_repo_open`) and feed
//!    the results back as the matching `Msg`.
//! 3. Render [`home_section`] inside the Home card, and layer
//!    [`dialog_view`] / [`browse_view`] above the page while their `open`
//!    flags are set; the forget confirmation is a plain `dialog::confirm`
//!    rendered by the shell while [`State::pending_forget`] is set.
//!
//! Secrets never enter this module's state beyond what the user is currently
//! typing into the dialog (they go once into the OS keyring on save and are
//! never read back for display).

use iced::widget::{
    button, checkbox, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{Alignment, Background, Border, Element, Length};

use crate::remote::commands::{DeviceInfo, DeviceInput, DeviceRepoCandidate};
use crate::remote::profiles::{AuthKind, DeviceProfile, DEFAULT_POLL_SECS};
use crate::workspace::{DirEntryKind, DirListing};

use super::app::Message;
use super::theme::{self, Tokens};

/// Width of the add/edit dialog card.
const DIALOG_WIDTH: f32 = 480.0;

/// Width of the device browser card.
const BROWSE_WIDTH: f32 = 560.0;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shell-owned devices UI state (Home section + overlays).
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The Home section is expanded (collapsed shows only the header row).
    pub expanded: bool,
    /// Device profiles + live status, as returned by `device_list`.
    pub rows: Vec<DeviceInfo>,
    /// True once a list round-trip completed (drives the empty state).
    pub loaded: bool,
    /// Last `device_list` failure (rendered in the section with a retry).
    pub load_error: Option<String>,
    /// Device id awaiting the forget confirmation dialog.
    pub pending_forget: Option<String>,
    /// Add/edit device dialog.
    pub dialog: DialogState,
    /// Device file browser / repo scanner overlay.
    pub browse: BrowseState,
}

/// The add/edit device dialog.
#[derive(Debug, Clone)]
pub struct DialogState {
    pub open: bool,
    /// Device being edited (`None` = adding a new one).
    pub editing_id: Option<String>,
    /// Current status of the stored device being edited (`None` = new).
    /// The Test probe dials a throwaway session under the same device id, so
    /// its status events would fight the live one; testing is disabled while
    /// a stored device is connected.
    pub stored_connected: bool,
    pub name: String,
    pub host: String,
    /// Port as typed (parsed on submit; empty = 22).
    pub port: String,
    pub username: String,
    pub kind: AuthKind,
    pub key_path: String,
    /// The key has a passphrase (key auth only).
    pub has_passphrase: bool,
    /// Passphrase as typed (empty = keep the stored one on edit).
    pub passphrase: String,
    /// Password as typed (empty = keep the stored one on edit).
    pub password: String,
    /// Poll interval as typed (parsed on submit; empty = default).
    pub poll_secs: String,
    /// True between an accepted Test and its completion message.
    pub testing: bool,
    /// True between an accepted Submit and its completion message.
    pub saving: bool,
    /// Last probe result (rendered under the Test button).
    pub test_result: Option<Result<String, String>>,
    /// Last save failure (rendered above the actions row).
    pub error: Option<String>,
}

impl Default for DialogState {
    fn default() -> Self {
        Self {
            open: false,
            editing_id: None,
            stored_connected: false,
            name: String::new(),
            host: String::new(),
            port: String::new(),
            username: String::new(),
            // Key auth is the common default (agent or key file).
            kind: AuthKind::KeyFile,
            key_path: String::new(),
            has_passphrase: false,
            passphrase: String::new(),
            password: String::new(),
            poll_secs: String::new(),
            testing: false,
            saving: false,
            test_result: None,
            error: None,
        }
    }
}

impl DialogState {
    /// Opens the dialog for a new device (default auth: key file).
    pub fn open_new() -> Self {
        Self {
            open: true,
            kind: AuthKind::KeyFile,
            ..DialogState::default()
        }
    }

    /// Opens the dialog prefilled from a stored profile. Secret fields start
    /// empty: an empty secret keeps whatever is already in the keyring.
    pub fn for_edit(info: &DeviceInfo) -> Self {
        let profile = &info.profile;
        Self {
            open: true,
            editing_id: Some(profile.id.clone()),
            stored_connected: info.status == "connected",
            name: profile.name.clone(),
            host: profile.host.clone(),
            port: profile.port.to_string(),
            username: profile.username.clone(),
            kind: profile.auth.kind,
            key_path: profile.auth.key_path.clone().unwrap_or_default(),
            has_passphrase: profile.auth.has_passphrase,
            passphrase: String::new(),
            password: String::new(),
            poll_secs: profile.poll_secs.to_string(),
            testing: false,
            saving: false,
            test_result: None,
            error: None,
        }
    }

    pub fn close(&mut self) {
        *self = DialogState::default();
    }
}

/// The device file browser / repo scanner overlay.
#[derive(Debug, Clone, Default)]
pub struct BrowseState {
    pub open: bool,
    pub device_id: String,
    pub device_name: String,
    /// Resolved current directory (absolute POSIX path) once known; `None`
    /// until the home listing arrives (the device resolves `~` itself).
    pub path: Option<String>,
    /// Path field as typed (jump-to on submit; synced to the resolved path).
    pub path_field: String,
    /// Directory request in flight; stale results are dropped by it.
    pub pending: Option<String>,
    pub listing: Option<DirListing>,
    pub loading: bool,
    pub error: Option<String>,
    /// Repositories found by the latest scan (`device_scan_repos`).
    pub scan: Option<Vec<DeviceRepoCandidate>>,
    pub scanning: bool,
    /// Repo open in flight (the shell closes the overlay on success).
    pub opening: Option<String>,
}

impl BrowseState {
    /// Opens the browser for a device; the shell issues the home listing
    /// (the pending request is `""` — the device resolves `~` itself).
    fn open_with(&mut self, device_id: String, device_name: String) {
        *self = Self {
            open: true,
            device_id,
            device_name,
            pending: Some(String::new()),
            loading: true,
            ..BrowseState::default()
        };
    }

    pub fn close(&mut self) {
        *self = BrowseState::default();
    }

    /// Begins navigating to `path` (absolute POSIX, or empty/`~` for the
    /// device home); returns the request for the shell, if any.
    fn request(&mut self, path: &str) -> Option<String> {
        if !self.open {
            return None;
        }
        self.pending = Some(path.to_string());
        self.loading = true;
        self.error = None;
        Some(path.to_string())
    }
}

// ---------------------------------------------------------------------------
// Messages, actions
// ---------------------------------------------------------------------------

/// Messages the shell forwards to [`update`] (user intents, service results
/// and backend status transitions).
#[derive(Debug, Clone)]
pub enum Msg {
    /// Reload the device list.
    Refresh,
    /// `device_list` result.
    Loaded(Result<Vec<DeviceInfo>, String>),
    /// Collapse/expand the Home section (expanding refreshes the list).
    ToggleExpanded,
    /// `BackendEvent::DeviceStatus` transition for one device.
    StatusChanged {
        device_id: String,
        status: String,
    },
    Connect(String),
    Disconnect(String),
    /// Probe a stored device (row Test button; `device_test` on its profile).
    TestStored(String),
    /// Open the add dialog (Home button, palette `repo.add_device`).
    AddDevice,
    /// Open the edit dialog prefilled for a stored device.
    EditDevice(String),
    /// Ask for the forget confirmation (the shell renders it).
    ForgetRequested(String),
    /// Confirmation accepted; the shell runs `device_remove`.
    ForgetConfirmed,
    Dialog(DialogMsg),
    /// `device_test` result (probe button, dialog and rows).
    TestDone(Result<String, String>),
    /// `device_add` / `device_update` result.
    SaveDone(Result<DeviceInfo, String>),
    Browse(BrowseMsg),
}

/// Add/edit dialog messages.
#[derive(Debug, Clone)]
pub enum DialogMsg {
    NameChanged(String),
    HostChanged(String),
    PortChanged(String),
    UsernameChanged(String),
    AuthSelected(AuthKind),
    KeyPathChanged(String),
    HasPassphraseToggled(bool),
    PassphraseChanged(String),
    PasswordChanged(String),
    PollSecsChanged(String),
    /// Probe the connection as typed (nothing is persisted).
    Test,
    Cancel,
    Submit,
}

/// Device browser messages.
#[derive(Debug, Clone)]
pub enum BrowseMsg {
    /// Open the browser for a device (the shell issues the home listing).
    Open {
        device_id: String,
        name: String,
    },
    Close,
    /// Navigate into an absolute POSIX path (empty/`~` = device home).
    Enter(String),
    /// Go to the parent of the current directory (no-op at the top).
    Up,
    PathChanged(String),
    PathSubmitted,
    /// Scan the current directory for git repositories.
    Scan,
    /// `device_scan_repos` result.
    ScanDone {
        device_id: String,
        result: Result<Vec<DeviceRepoCandidate>, String>,
    },
    /// `device_browse_dir` result (`path` = the requested path).
    DirLoaded {
        device_id: String,
        path: String,
        result: Result<DirListing, String>,
    },
    /// Open a discovered repository as a tab (`remote_repo_open`).
    OpenRepo {
        path: String,
    },
}

/// Intents the shell translates into `remote::commands` calls / tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Reload the device list (`device_list`).
    RefreshList,
    Connect {
        device_id: String,
    },
    Disconnect {
        device_id: String,
    },
    /// Delete the profile (the confirmation was already accepted).
    Forget {
        device_id: String,
    },
    /// Create or update a profile (`device_add` / `device_update`).
    Save {
        input: DeviceInput,
        is_new: bool,
    },
    /// Probe-only connect (`device_test`); nothing is persisted.
    Test {
        input: DeviceInput,
    },
    /// List one device directory (empty path = home).
    BrowseDir {
        device_id: String,
        path: String,
    },
    /// Scan for repositories under `root` (`None` = device home).
    Scan {
        device_id: String,
        root: Option<String>,
    },
    /// Open a device repository as a tab (`remote_repo_open`).
    OpenRepo {
        device_id: String,
        path: String,
    },
}

/// Steps the devices UI; returns the intent for the shell to translate, if
/// any.
pub fn update(state: &mut State, msg: Msg) -> Option<Action> {
    match msg {
        Msg::Refresh => Some(Action::RefreshList),
        Msg::Loaded(result) => {
            state.loaded = true;
            match result {
                Ok(rows) => {
                    state.rows = rows;
                    state.load_error = None;
                    // A forget pending on a since-removed device is moot.
                    let ids: Vec<&str> = state
                        .rows
                        .iter()
                        .map(|row| row.profile.id.as_str())
                        .collect();
                    if let Some(pending) = state.pending_forget.as_ref() {
                        if !ids.contains(&pending.as_str()) {
                            state.pending_forget = None;
                        }
                    }
                }
                Err(err) => state.load_error = Some(err),
            }
            None
        }
        Msg::ToggleExpanded => {
            state.expanded = !state.expanded;
            state.expanded.then_some(Action::RefreshList)
        }
        Msg::StatusChanged { device_id, status } => {
            if let Some(row) = state
                .rows
                .iter_mut()
                .find(|row| row.profile.id == device_id)
            {
                row.status = status;
            }
            None
        }
        Msg::Connect(device_id) => Some(Action::Connect { device_id }),
        Msg::Disconnect(device_id) => Some(Action::Disconnect { device_id }),
        Msg::TestStored(device_id) => state
            .rows
            .iter()
            .find(|row| row.profile.id == device_id)
            .map(|row| Action::Test {
                input: stored_probe_input(&row.profile),
            }),
        Msg::AddDevice => {
            state.dialog = DialogState::open_new();
            None
        }
        Msg::EditDevice(device_id) => {
            if let Some(info) = state.rows.iter().find(|row| row.profile.id == device_id) {
                state.dialog = DialogState::for_edit(info);
            }
            None
        }
        Msg::ForgetRequested(device_id) => {
            if state.rows.iter().any(|row| row.profile.id == device_id) {
                state.pending_forget = Some(device_id);
            }
            None
        }
        Msg::ForgetConfirmed => state
            .pending_forget
            .take()
            .map(|device_id| Action::Forget { device_id }),
        Msg::Dialog(dialog) => dialog_update(state, dialog),
        Msg::TestDone(result) => {
            state.dialog.testing = false;
            state.dialog.test_result = Some(result);
            None
        }
        Msg::SaveDone(result) => {
            state.dialog.saving = false;
            match result {
                Ok(_) => {
                    state.dialog.close();
                    Some(Action::RefreshList)
                }
                Err(err) => {
                    state.dialog.error = Some(err);
                    None
                }
            }
        }
        Msg::Browse(browse) => browse_update(state, browse),
    }
}

fn dialog_update(state: &mut State, msg: DialogMsg) -> Option<Action> {
    if !state.dialog.open && !matches!(msg, DialogMsg::Cancel) {
        return None;
    }
    match msg {
        DialogMsg::NameChanged(value) => state.dialog.name = value,
        DialogMsg::HostChanged(value) => state.dialog.host = value,
        DialogMsg::PortChanged(value) => state.dialog.port = value,
        DialogMsg::UsernameChanged(value) => state.dialog.username = value,
        DialogMsg::AuthSelected(kind) => state.dialog.kind = kind,
        DialogMsg::KeyPathChanged(value) => state.dialog.key_path = value,
        DialogMsg::HasPassphraseToggled(value) => state.dialog.has_passphrase = value,
        DialogMsg::PassphraseChanged(value) => state.dialog.passphrase = value,
        DialogMsg::PasswordChanged(value) => state.dialog.password = value,
        DialogMsg::PollSecsChanged(value) => state.dialog.poll_secs = value,
        DialogMsg::Test => {
            if state.dialog.testing || state.dialog.stored_connected {
                return None;
            }
            let input = dialog_input(&state.dialog).ok()?;
            state.dialog.testing = true;
            state.dialog.test_result = None;
            return Some(Action::Test { input });
        }
        DialogMsg::Cancel => state.dialog.close(),
        DialogMsg::Submit => {
            if state.dialog.saving {
                return None;
            }
            let is_new = state.dialog.editing_id.is_none();
            let input = dialog_input(&state.dialog).ok()?;
            state.dialog.saving = true;
            state.dialog.error = None;
            return Some(Action::Save { input, is_new });
        }
    }
    None
}

fn browse_update(state: &mut State, msg: BrowseMsg) -> Option<Action> {
    match msg {
        BrowseMsg::Open { device_id, name } => {
            state.browse.open_with(device_id.clone(), name);
            Some(Action::BrowseDir {
                device_id,
                path: String::new(),
            })
        }
        BrowseMsg::Close => {
            state.browse.close();
            None
        }
        BrowseMsg::Enter(path) => {
            let device_id = state.browse.device_id.clone();
            state
                .browse
                .request(&path)
                .map(|path| Action::BrowseDir { device_id, path })
        }
        BrowseMsg::Up => {
            let parent = state
                .browse
                .listing
                .as_ref()
                .and_then(|listing| parent_path(&listing.path))?;
            let device_id = state.browse.device_id.clone();
            state
                .browse
                .request(&parent)
                .map(|path| Action::BrowseDir { device_id, path })
        }
        BrowseMsg::PathChanged(value) => {
            state.browse.path_field = value;
            None
        }
        BrowseMsg::PathSubmitted => {
            let path = state.browse.path_field.trim().to_string();
            let device_id = state.browse.device_id.clone();
            state
                .browse
                .request(&path)
                .map(|path| Action::BrowseDir { device_id, path })
        }
        BrowseMsg::Scan => {
            if state.browse.scanning {
                return None;
            }
            let root = state
                .browse
                .listing
                .as_ref()
                .map(|listing| listing.path.clone())
                .filter(|path| !path.is_empty());
            state.browse.scanning = true;
            state.browse.scan = None;
            Some(Action::Scan {
                device_id: state.browse.device_id.clone(),
                root,
            })
        }
        BrowseMsg::ScanDone { device_id, result } => {
            if !state.browse.open || state.browse.device_id != device_id {
                return None;
            }
            state.browse.scanning = false;
            match result {
                Ok(candidates) => state.browse.scan = Some(candidates),
                Err(err) => state.browse.error = Some(err),
            }
            None
        }
        BrowseMsg::DirLoaded {
            device_id,
            path,
            result,
        } => {
            if !state.browse.open || state.browse.device_id != device_id {
                return None;
            }
            if state.browse.pending.as_deref() != Some(path.as_str()) {
                return None; // stale result (superseded navigation)
            }
            state.browse.pending = None;
            state.browse.loading = false;
            match result {
                Ok(listing) => {
                    // The backend resolves the requested path ("" / "~" →
                    // home) and returns absolute entry paths.
                    let resolved = listing.path.clone();
                    state.browse.path = Some(resolved.clone());
                    state.browse.path_field = resolved;
                    state.browse.listing = Some(listing);
                    state.browse.error = None;
                }
                Err(err) => state.browse.error = Some(err),
            }
            None
        }
        BrowseMsg::OpenRepo { path } => {
            if state.browse.opening.is_some() {
                return None;
            }
            state.browse.opening = Some(path.clone());
            Some(Action::OpenRepo {
                device_id: state.browse.device_id.clone(),
                path,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Parent of an absolute POSIX path (`None` at the top). Trailing slashes
/// are normalized; `/` has no parent (the browser treats the device home as
/// the top anyway — entering `/` resolves back to home).
pub fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(pos) => Some(trimmed[..pos].to_string()),
        None => None,
    }
}

/// Parses the port field: trimmed, empty = 22 (the backend also defaults 0).
pub fn parse_port(raw: &str) -> Result<u16, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(22);
    }
    trimmed
        .parse::<u16>()
        .map_err(|_| format!("Port must be a number between 1 and 65535, got “{trimmed}”."))
}

/// Parses the poll-interval field: trimmed, empty = the backend default.
pub fn parse_poll_secs(raw: &str) -> Result<Option<u32>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u32>()
        .map(Some)
        .map_err(|_| format!("Poll interval must be a number of seconds, got “{trimmed}”."))
}

/// Builds the backend input from the dialog fields, validating the
/// user-facing requirements (host, username, key path for key auth). Empty
/// secret fields mean "keep whatever is stored in the keyring".
pub fn dialog_input(dialog: &DialogState) -> Result<DeviceInput, String> {
    let host = dialog.host.trim();
    if host.is_empty() {
        return Err("Host is required.".to_string());
    }
    let username = dialog.username.trim();
    if username.is_empty() {
        return Err("Username is required.".to_string());
    }
    if dialog.kind == AuthKind::KeyFile && dialog.key_path.trim().is_empty() {
        return Err("A private key path is required for key auth.".to_string());
    }
    Ok(DeviceInput {
        id: dialog.editing_id.clone(),
        name: dialog.name.trim().to_string(),
        host: host.to_string(),
        port: parse_port(&dialog.port)?,
        username: username.to_string(),
        kind: dialog.kind,
        key_path: (dialog.kind == AuthKind::KeyFile).then(|| dialog.key_path.trim().to_string()),
        has_passphrase: dialog.has_passphrase,
        // Secrets are not trimmed: any character may be part of one.
        password: (dialog.kind == AuthKind::Password && !dialog.password.is_empty())
            .then(|| dialog.password.clone()),
        passphrase: (dialog.kind == AuthKind::KeyFile
            && dialog.has_passphrase
            && !dialog.passphrase.is_empty())
        .then(|| dialog.passphrase.clone()),
        poll_secs: parse_poll_secs(&dialog.poll_secs)?,
    })
}

/// `device_test` input for a stored profile: reuses its id so the keyring
/// secrets apply, sends no new secrets (they are already stored).
pub fn stored_probe_input(profile: &DeviceProfile) -> DeviceInput {
    DeviceInput {
        id: Some(profile.id.clone()),
        name: profile.name.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        kind: profile.auth.kind,
        key_path: profile.auth.key_path.clone(),
        has_passphrase: profile.auth.has_passphrase,
        password: None,
        passphrase: None,
        poll_secs: Some(profile.poll_secs),
    }
}

/// One-line summary of a profile (`user@host:port · key file · poll 5s`).
pub fn profile_summary(profile: &DeviceProfile) -> String {
    let auth = match (profile.auth.kind, profile.auth.has_passphrase) {
        (AuthKind::Password, _) => "password",
        (AuthKind::KeyFile, false) => "key file",
        (AuthKind::KeyFile, true) => "key file + passphrase",
    };
    format!(
        "{}@{}:{} · {} · poll {}s",
        profile.username, profile.host, profile.port, auth, profile.poll_secs
    )
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// The SSH devices section rendered inside the Home card, below the recent
/// repositories: a collapsible header row plus one row per device.
pub fn home_section<'a>(state: &'a State, tokens: Tokens) -> Element<'a, Message> {
    let mut body = column![].spacing(4).width(Length::Fill);

    let mut header = row![]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    header = header.push(
        button(
            text(if state.expanded {
                "▾ SSH devices"
            } else {
                "▸ SSH devices"
            })
            .size(12)
            .font(theme::semibold()),
        )
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([4, 8])
        .on_press(Message::Devices(Msg::ToggleExpanded)),
    );
    let caption = section_caption(state);
    if !caption.is_empty() {
        header = header.push(text(caption).size(11).color(tokens.muted_foreground));
    }
    if state.expanded {
        header = header.push(Space::new().width(Length::Fill));
        header = header.push(
            button(text("Add device…").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([4, 10])
                .on_press(Message::Devices(Msg::AddDevice)),
        );
    }
    body = body.push(header);

    if state.expanded {
        if let Some(err) = &state.load_error {
            body = body.push(
                row![
                    text(err.clone()).size(11).color(tokens.deleted),
                    Space::new().width(Length::Fill),
                    button(text("Retry").size(11))
                        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                        .padding([4, 10])
                        .on_press(Message::Devices(Msg::Refresh)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        } else if !state.loaded {
            body = body.push(
                text("Loading devices…")
                    .size(11)
                    .color(tokens.muted_foreground),
            );
        } else if state.rows.is_empty() {
            body = body.push(
                text("No devices yet. Add one to open repositories stored on it over SSH.")
                    .size(11)
                    .color(tokens.muted_foreground),
            );
        } else {
            for device in &state.rows {
                body = body.push(device_row(tokens, device));
            }
        }
    }

    body.into()
}

/// Caption next to the section header (empty while not loaded).
fn section_caption(state: &State) -> String {
    if !state.loaded {
        return String::new();
    }
    let connected = state
        .rows
        .iter()
        .filter(|row| row.status == "connected")
        .count();
    match (state.rows.len(), connected) {
        (0, _) => "none yet".to_string(),
        (n, 0) => format!("{n} device{}", if n == 1 { "" } else { "s" }),
        (n, c) => format!(
            "{n} device{} · {c} connected",
            if n == 1 { "" } else { "s" }
        ),
    }
}

fn device_row<'a>(tokens: Tokens, device: &'a DeviceInfo) -> Element<'a, Message> {
    let status = Status::parse(&device.status);
    let id = device.profile.id.clone();

    let label = container(
        column![
            text(device.profile.name.clone())
                .size(13)
                .color(tokens.foreground),
            text(profile_summary(&device.profile))
                .size(11)
                .color(tokens.muted_foreground),
        ]
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .align_x(Alignment::Start);

    let mut actions = row![]
        .spacing(4)
        .align_y(Alignment::Center)
        .push(status_dot(tokens, status));

    match status {
        Status::Connected => {
            actions = actions.push(row_button(
                tokens,
                "Disconnect",
                Some(Message::Devices(Msg::Disconnect(id.clone()))),
            ));
        }
        Status::Connecting => {
            actions = actions.push(row_button(
                tokens,
                "Cancel",
                Some(Message::Devices(Msg::Disconnect(id.clone()))),
            ));
        }
        Status::Offline => {
            actions = actions.push(row_button(
                tokens,
                "Connect",
                Some(Message::Devices(Msg::Connect(id.clone()))),
            ));
        }
    }

    // Browsing needs the live session. Probing is for offline devices: a
    // probe on a connected device dials a throwaway session under the same
    // device id, and its status events would fight the live one's.
    actions = actions.push(row_button(
        tokens,
        "Browse…",
        (status == Status::Connected).then(|| {
            Message::Devices(Msg::Browse(BrowseMsg::Open {
                device_id: id.clone(),
                name: device.profile.name.clone(),
            }))
        }),
    ));
    actions = actions.push(row_button(
        tokens,
        "Test",
        (status == Status::Offline).then(|| Message::Devices(Msg::TestStored(id.clone()))),
    ));
    actions = actions.push(row_button(
        tokens,
        "Edit…",
        Some(Message::Devices(Msg::EditDevice(id.clone()))),
    ));
    actions = actions.push(row_button(
        tokens,
        "Forget",
        Some(Message::Devices(Msg::ForgetRequested(id))),
    ));

    row![label, actions]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// Small ghost button for a device row (disabled when `on_press` is `None`).
fn row_button<'a>(
    tokens: Tokens,
    label: &'a str,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let b = button(text(label).size(11))
        .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
        .padding([4, 8]);
    if let Some(message) = on_press {
        b.on_press(message).into()
    } else {
        b.into()
    }
}

/// Colored status dot for a device row (green connected, yellow connecting,
/// grey offline).
fn status_dot(tokens: Tokens, status: Status) -> Element<'static, Message> {
    let color = match status {
        Status::Connected => tokens.added,
        Status::Connecting => tokens.modified,
        Status::Offline => tokens.muted_foreground,
    };
    container(text("●").size(10).color(color))
        .width(Length::Fixed(16.0))
        .align_x(Alignment::Center)
        .into()
}

/// The add/edit device dialog (layered above the page while open). Fields:
/// name, host, port, username, auth kind (key file / password) with the
/// matching secret fields, poll interval and a Test connection probe.
pub fn dialog_view(state: &State, tokens: &Tokens) -> Element<'static, Msg> {
    if !state.dialog.open {
        return Space::new().width(0).height(0).into();
    }
    let tokens = *tokens;
    let dialog = &state.dialog;
    let msg = |m: DialogMsg| Msg::Dialog(m);

    let title = row![
        text(if dialog.editing_id.is_some() {
            "Edit device"
        } else {
            "Add SSH device"
        })
        .size(14)
        .font(theme::semibold()),
        Space::new().width(Length::Fill),
        button(text("×").size(12))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(msg(DialogMsg::Cancel)),
    ]
    .align_y(Alignment::Center);

    let name = labeled_field(
        tokens,
        "Name (empty = user@host)",
        text_input("raspberry-pi", dialog.name.as_str())
            .on_input(move |v| msg(DialogMsg::NameChanged(v)))
            .size(12)
            .padding([5, 8]),
    );
    let host = labeled_field(
        tokens,
        "Host",
        text_input("192.168.1.42", dialog.host.as_str())
            .on_input(move |v| msg(DialogMsg::HostChanged(v)))
            .size(12)
            .padding([5, 8]),
    );
    let port_user = row![
        labeled_field(
            tokens,
            "Port",
            text_input("22", dialog.port.as_str())
                .on_input(move |v| msg(DialogMsg::PortChanged(v)))
                .size(12)
                .padding([5, 8])
                .width(Length::Fixed(90.0)),
        ),
        labeled_field(
            tokens,
            "Username",
            text_input("pi", dialog.username.as_str())
                .on_input(move |v| msg(DialogMsg::UsernameChanged(v)))
                .size(12)
                .padding([5, 8])
                .width(Length::Fill),
        ),
    ]
    .spacing(10)
    .width(Length::Fill);

    let mut auth = column![text("Authentication")
        .size(11)
        .color(tokens.muted_foreground)]
    .spacing(4)
    .width(Length::Fill);
    for (kind, label, description) in [
        (AuthKind::KeyFile, "SSH key file", "key on this machine"),
        (
            AuthKind::Password,
            "Password",
            "typed once, stored in the keyring",
        ),
    ] {
        let selected = dialog.kind == kind;
        auth = auth.push(
            button(
                row![
                    text(if selected { "●" } else { "○" })
                        .size(10)
                        .color(if selected {
                            tokens.primary
                        } else {
                            tokens.muted_foreground
                        }),
                    text(label).size(11).color(tokens.foreground),
                    text(description).size(10).color(tokens.muted_foreground),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .style(theme::button_style(
                tokens,
                if selected {
                    theme::ButtonKind::Secondary
                } else {
                    theme::ButtonKind::Ghost
                },
            ))
            .padding([4, 8])
            .width(Length::Fill)
            .on_press(msg(DialogMsg::AuthSelected(kind))),
        );
    }

    let mut kind_fields = column![].spacing(6).width(Length::Fill);
    if dialog.kind == AuthKind::KeyFile {
        kind_fields = kind_fields.push(labeled_field(
            tokens,
            "Private key path",
            text_input("~/.ssh/id_ed25519", dialog.key_path.as_str())
                .on_input(move |v| msg(DialogMsg::KeyPathChanged(v)))
                .size(12)
                .padding([5, 8]),
        ));
        kind_fields = kind_fields.push(
            checkbox(dialog.has_passphrase)
                .label("The key has a passphrase")
                .size(12)
                .text_size(11)
                .spacing(5)
                .on_toggle(move |v| msg(DialogMsg::HasPassphraseToggled(v))),
        );
        if dialog.has_passphrase {
            let label = if dialog.editing_id.is_some() {
                "Key passphrase (empty = keep stored)"
            } else {
                "Key passphrase"
            };
            kind_fields = kind_fields.push(labeled_field(
                tokens,
                label,
                text_input("Passphrase", dialog.passphrase.as_str())
                    .secure(true)
                    .on_input(move |v| msg(DialogMsg::PassphraseChanged(v)))
                    .size(12)
                    .padding([5, 8]),
            ));
        }
    } else {
        let label = if dialog.editing_id.is_some() {
            "Password (empty = keep stored)"
        } else {
            "Password"
        };
        kind_fields = kind_fields.push(labeled_field(
            tokens,
            label,
            text_input("Password", dialog.password.as_str())
                .secure(true)
                .on_input(move |v| msg(DialogMsg::PasswordChanged(v)))
                .size(12)
                .padding([5, 8]),
        ));
    }

    let poll = labeled_field(
        tokens,
        format!("Poll interval seconds (empty = {DEFAULT_POLL_SECS})"),
        text_input("5", dialog.poll_secs.as_str())
            .on_input(move |v| msg(DialogMsg::PollSecsChanged(v)))
            .size(12)
            .padding([5, 8])
            .width(Length::Fixed(90.0)),
    );

    // Probe area: Test connection button + the last result. Testing is
    // disabled while a stored device is connected (the throwaway probe
    // session emits status transitions for the same device id).
    let test_enabled = !dialog.testing && !dialog.stored_connected && dialog_input(dialog).is_ok();
    let mut test_button = button(
        text(if dialog.testing {
            "Testing…"
        } else {
            "Test connection"
        })
        .size(11),
    )
    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
    .padding([4, 10]);
    if test_enabled {
        test_button = test_button.on_press(msg(DialogMsg::Test));
    }
    let mut test_area = column![row![
        test_button,
        text(if dialog.stored_connected {
            "Device is connected — disconnect before testing new credentials."
        } else {
            "Dials the device without saving anything."
        })
        .size(10)
        .color(tokens.muted_foreground),
    ]
    .spacing(8)
    .align_y(Alignment::Center),]
    .spacing(4)
    .width(Length::Fill);
    match &dialog.test_result {
        Some(Ok(output)) => {
            test_area = test_area.push(text(format!("ok: {output}")).size(10).color(tokens.added));
        }
        Some(Err(err)) => {
            test_area = test_area.push(text(err.clone()).size(10).color(tokens.deleted));
        }
        None => {}
    }

    let mut body = column![
        title,
        name,
        host,
        port_user,
        auth,
        kind_fields,
        poll,
        test_area
    ]
    .spacing(10)
    .width(Length::Fill);
    if let Some(err) = &dialog.error {
        body = body.push(text(err.clone()).size(11).color(tokens.deleted));
    }

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press_maybe((!dialog.saving).then_some(msg(DialogMsg::Cancel))),
        button(
            text(if dialog.saving {
                "Saving…"
            } else if dialog.editing_id.is_some() {
                "Save"
            } else {
                "Add device"
            })
            .size(11)
        )
        .style(theme::button_style(tokens, theme::ButtonKind::Primary))
        .padding([4, 12])
        .on_press_maybe(
            (!dialog.saving && dialog_input(dialog).is_ok()).then_some(msg(DialogMsg::Submit)),
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let card = container(column![body, Space::new().height(8), actions].spacing(6))
        .width(Length::Fixed(DIALOG_WIDTH))
        .padding([16, 18])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.card)),
            text_color: Some(tokens.card_foreground),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
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

    mouse_area(scrim)
        .on_press(Msg::Dialog(DialogMsg::Cancel))
        .into()
}

/// The device file browser / repo scanner overlay (layered above the page
/// while open): toolbar (Up, path field, Scan for repositories), the
/// directory listing and the discovered repositories with Open buttons.
pub fn browse_view(state: &State, tokens: &Tokens) -> Element<'static, Msg> {
    if !state.browse.open {
        return Space::new().width(0).height(0).into();
    }
    let tokens = *tokens;
    let browse = &state.browse;

    let title = row![
        text(format!("Browsing {}", browse.device_name))
            .size(14)
            .font(theme::semibold()),
        Space::new().width(Length::Fill),
        button(text("×").size(12))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(BrowseMsg::Close),
    ]
    .align_y(Alignment::Center);

    let mut up = button(text("Up").size(11))
        .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
        .padding([4, 10]);
    if !browse.loading && browse.listing.is_some() {
        up = up.on_press(BrowseMsg::Up);
    }

    let path_field = text_input("/home/pi/projects", browse.path_field.as_str())
        .on_input(BrowseMsg::PathChanged)
        .on_submit(BrowseMsg::PathSubmitted)
        .size(11)
        .padding([4, 8])
        .width(Length::Fill);

    let mut scan = button(
        text(if browse.scanning {
            "Scanning…"
        } else {
            "Scan for repositories"
        })
        .size(11),
    )
    .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
    .padding([4, 10]);
    if !browse.loading && !browse.scanning {
        scan = scan.on_press(BrowseMsg::Scan);
    }

    let mut body = column![
        title,
        row![up, path_field, scan]
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::Fill),
    ]
    .spacing(10)
    .width(Length::Fill);

    if let Some(err) = &browse.error {
        body = body.push(text(err.clone()).size(11).color(tokens.deleted));
    }

    // Directory listing (directories navigate; files are inert labels).
    let listing_area: Element<'static, BrowseMsg> = if browse.loading {
        text("Loading…")
            .size(11)
            .color(tokens.muted_foreground)
            .into()
    } else if let Some(listing) = &browse.listing {
        let mut rows = column![].spacing(2).width(Length::Fill);
        if listing.entries.is_empty() {
            rows = rows.push(
                text("Empty directory")
                    .size(11)
                    .color(tokens.muted_foreground),
            );
        }
        for entry in &listing.entries {
            let is_dir = matches!(entry.kind, DirEntryKind::Dir | DirEntryKind::Submodule);
            let row_element: Element<'static, BrowseMsg> = if is_dir {
                button(
                    row![
                        text("▸").size(10).color(tokens.muted_foreground),
                        text(entry.name.clone()).size(12).color(tokens.foreground),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
                .padding([3, 8])
                .width(Length::Fill)
                .on_press(BrowseMsg::Enter(entry.path.clone()))
                .into()
            } else {
                row![
                    Space::new().width(10.0),
                    text(entry.name.clone())
                        .size(12)
                        .color(tokens.muted_foreground),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
            };
            rows = rows.push(row_element);
        }
        let mut listing_col = column![scrollable(rows)
            .height(Length::Fixed(260.0))
            .width(Length::Fill)]
        .spacing(2)
        .width(Length::Fill);
        if listing.truncated {
            listing_col = listing_col.push(
                text("Listing truncated.")
                    .size(10)
                    .color(tokens.muted_foreground),
            );
        }
        listing_col.into()
    } else {
        text("Enter a path or press Scan.")
            .size(11)
            .color(tokens.muted_foreground)
            .into()
    };
    body = body.push(listing_area);

    // Scan results with Open buttons (repo paths on the device).
    let mut scan_area = column![].spacing(4).width(Length::Fill);
    if browse.scanning {
        scan_area = scan_area.push(
            text("Scanning for git repositories…")
                .size(11)
                .color(tokens.muted_foreground),
        );
    } else if let Some(candidates) = &browse.scan {
        scan_area = scan_area.push(
            text(format!("Repositories found: {}", candidates.len()))
                .size(11)
                .font(theme::semibold()),
        );
        if candidates.is_empty() {
            scan_area = scan_area.push(
                text("No repositories under the current directory (depth 4).")
                    .size(11)
                    .color(tokens.muted_foreground),
            );
        }
        for candidate in candidates {
            scan_area = scan_area.push(
                row![
                    text(candidate.path.clone())
                        .size(11)
                        .font(theme::mono())
                        .color(tokens.foreground),
                    button(text("Open").size(10))
                        .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                        .padding([2, 8])
                        .on_press_maybe((browse.opening.is_none()).then_some(
                            BrowseMsg::OpenRepo {
                                path: candidate.path.clone(),
                            }
                        ),),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
        }
    }
    body = body.push(scan_area);

    if let Some(path) = &browse.opening {
        body = body.push(
            text(format!("Opening {path}…"))
                .size(11)
                .color(tokens.muted_foreground),
        );
    }

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Close").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(BrowseMsg::Close),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let card = container(column![body, Space::new().height(8), actions].spacing(6))
        .width(Length::Fixed(BROWSE_WIDTH))
        .padding([16, 18])
        .style(move |_theme| container::Style {
            background: Some(Background::Color(tokens.card)),
            text_color: Some(tokens.card_foreground),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
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

    let content: Element<'static, BrowseMsg> = mouse_area(scrim).on_press(BrowseMsg::Close).into();
    content.map(Msg::Browse)
}

/// Label above a field, following the clone-dialog spacing vocabulary.
/// Labels may be owned strings (they render as `Cow::Owned` text).
fn labeled_field<'a>(
    tokens: Tokens,
    label: impl Into<String>,
    field: impl Into<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    column![
        text(label.into()).size(11).color(tokens.muted_foreground),
        field.into(),
    ]
    .spacing(3)
    .width(Length::Fill)
    .into()
}

/// Connection status of one device row (mirrors the backend vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Connected,
    Connecting,
    #[default]
    Offline,
}

impl Status {
    /// Parses a `device-status` string (`"connected"` / `"connecting"` /
    /// `"disconnected"`; anything else is offline).
    pub fn parse(raw: &str) -> Self {
        match raw {
            "connected" => Self::Connected,
            "connecting" => Self::Connecting,
            _ => Self::Offline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, name: &str, status: &str) -> DeviceInfo {
        DeviceInfo {
            profile: DeviceProfile {
                id: id.to_string(),
                name: name.to_string(),
                host: "10.0.0.5".to_string(),
                port: 22,
                username: "pi".to_string(),
                auth: crate::remote::profiles::DeviceAuth {
                    kind: AuthKind::KeyFile,
                    key_path: Some("/home/me/.ssh/id_ed25519".to_string()),
                    has_passphrase: false,
                },
                poll_secs: DEFAULT_POLL_SECS,
            },
            status: status.to_string(),
        }
    }

    fn state_with_rows() -> State {
        State {
            loaded: true,
            expanded: true,
            rows: vec![
                info("dev-1", "pi", "connected"),
                info("dev-2", "nuc", "offline"),
            ],
            ..State::default()
        }
    }

    #[test]
    fn status_parse_maps_backend_vocabulary() {
        assert_eq!(Status::parse("connected"), Status::Connected);
        assert_eq!(Status::parse("connecting"), Status::Connecting);
        assert_eq!(Status::parse("disconnected"), Status::Offline);
        assert_eq!(Status::parse(""), Status::Offline);
        assert_eq!(Status::parse("bogus"), Status::Offline);
    }

    #[test]
    fn status_changed_updates_matching_row_only() {
        let mut state = state_with_rows();
        assert_eq!(
            update(
                &mut state,
                Msg::StatusChanged {
                    device_id: "dev-2".to_string(),
                    status: "connecting".to_string(),
                }
            ),
            None
        );
        assert_eq!(state.rows[0].status, "connected");
        assert_eq!(state.rows[1].status, "connecting");
        // Unknown device: ignored.
        assert_eq!(
            update(
                &mut state,
                Msg::StatusChanged {
                    device_id: "nope".to_string(),
                    status: "connected".to_string(),
                }
            ),
            None
        );
        assert_eq!(state.rows[1].status, "connecting");
    }

    #[test]
    fn toggle_expand_refreshes_when_expanding() {
        let mut state = State::default();
        assert_eq!(
            update(&mut state, Msg::ToggleExpanded),
            Some(Action::RefreshList)
        );
        assert!(state.expanded);
        assert_eq!(update(&mut state, Msg::ToggleExpanded), None);
        assert!(!state.expanded);
    }

    #[test]
    fn loaded_replaces_rows_and_drops_stale_forget() {
        let mut state = state_with_rows();
        state.pending_forget = Some("dev-gone".to_string());
        assert_eq!(
            update(
                &mut state,
                Msg::Loaded(Ok(vec![info("dev-1", "pi", "offline")]))
            ),
            None
        );
        assert!(state.loaded);
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.pending_forget, None);
        assert_eq!(state.load_error, None);
    }

    #[test]
    fn loaded_error_is_rendered_with_retry() {
        let mut state = State::default();
        assert_eq!(
            update(&mut state, Msg::Loaded(Err("boom".to_string()))),
            None
        );
        assert!(state.loaded);
        assert_eq!(state.load_error.as_deref(), Some("boom"));
    }

    #[test]
    fn forget_requires_confirmation_and_rows() {
        let mut state = State::default();
        // Unknown device: no pending confirm.
        assert_eq!(
            update(&mut state, Msg::ForgetRequested("dev-9".to_string())),
            None
        );
        assert_eq!(state.pending_forget, None);

        let mut state = state_with_rows();
        assert_eq!(
            update(&mut state, Msg::ForgetRequested("dev-1".to_string())),
            None
        );
        assert_eq!(state.pending_forget.as_deref(), Some("dev-1"));
        assert_eq!(
            update(&mut state, Msg::ForgetConfirmed),
            Some(Action::Forget {
                device_id: "dev-1".to_string(),
            })
        );
        // Consumed: a second confirm has nothing to act on.
        assert_eq!(update(&mut state, Msg::ForgetConfirmed), None);
    }

    #[test]
    fn connect_and_disconnect_pass_device_ids() {
        let mut state = state_with_rows();
        assert_eq!(
            update(&mut state, Msg::Connect("dev-2".to_string())),
            Some(Action::Connect {
                device_id: "dev-2".to_string(),
            })
        );
        assert_eq!(
            update(&mut state, Msg::Disconnect("dev-1".to_string())),
            Some(Action::Disconnect {
                device_id: "dev-1".to_string(),
            })
        );
    }

    #[test]
    fn parent_path_walks_up_to_root_and_stops() {
        assert_eq!(
            parent_path("/home/pi/projects"),
            Some("/home/pi".to_string())
        );
        assert_eq!(parent_path("/home"), Some("/".to_string()));
        assert_eq!(parent_path("/"), None);
        assert_eq!(parent_path(""), None);
        assert_eq!(parent_path("/home/pi/"), Some("/home".to_string()));
    }

    #[test]
    fn parse_port_defaults_and_validates() {
        assert_eq!(parse_port(""), Ok(22));
        assert_eq!(parse_port(" 2222 "), Ok(2222));
        assert_eq!(parse_port("0"), Ok(0), "backend sanitizes 0 to 22");
        assert!(parse_port("abc").is_err());
        assert!(parse_port("99999").is_err());
    }

    #[test]
    fn parse_poll_secs_defaults_and_validates() {
        assert_eq!(parse_poll_secs(""), Ok(None));
        assert_eq!(parse_poll_secs(" 30 "), Ok(Some(30)));
        assert!(parse_poll_secs("abc").is_err());
        assert!(parse_poll_secs("-1").is_err());
    }

    fn dialog() -> DialogState {
        DialogState {
            open: true,
            host: "10.0.0.5".to_string(),
            username: "pi".to_string(),
            kind: AuthKind::KeyFile,
            key_path: "~/.ssh/id_ed25519".to_string(),
            ..DialogState::default()
        }
    }

    #[test]
    fn dialog_input_validates_required_fields() {
        let mut d = dialog();
        let input = dialog_input(&d).expect("valid fields");
        assert_eq!(input.host, "10.0.0.5");
        assert_eq!(input.port, 22);
        assert_eq!(input.kind, AuthKind::KeyFile);
        assert_eq!(input.key_path.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(input.password, None);
        assert_eq!(input.passphrase, None);
        assert_eq!(input.poll_secs, None);
        assert_eq!(input.id, None);

        d.host = "  ".to_string();
        assert_eq!(
            dialog_input(&d).unwrap_err(),
            "Host is required.".to_string()
        );
        d.host = "h".to_string();
        d.username = "".to_string();
        assert_eq!(
            dialog_input(&d).unwrap_err(),
            "Username is required.".to_string()
        );
        d.username = "pi".to_string();
        d.key_path = " ".to_string();
        assert!(dialog_input(&d)
            .unwrap_err()
            .contains("private key path is required"));
    }

    #[test]
    fn dialog_input_secrets_only_for_the_matching_kind() {
        let mut d = dialog();
        d.has_passphrase = true;
        d.passphrase = "secret".to_string();
        d.password = "also-secret".to_string();
        let input = dialog_input(&d).expect("valid");
        // Key auth: the passphrase is stored, the password field is dropped.
        assert_eq!(input.passphrase.as_deref(), Some("secret"));
        assert_eq!(input.password, None);

        d.kind = AuthKind::Password;
        d.key_path = "".to_string();
        let input = dialog_input(&d).expect("valid");
        // Password auth: the password is stored, the passphrase field is dropped.
        assert_eq!(input.password.as_deref(), Some("also-secret"));
        assert_eq!(input.passphrase, None);
        assert_eq!(input.key_path, None);
    }

    #[test]
    fn dialog_input_empty_secret_keeps_stored_one() {
        let mut d = dialog();
        d.editing_id = Some("dev-1".to_string());
        d.kind = AuthKind::Password;
        let input = dialog_input(&d).expect("valid");
        assert_eq!(input.id.as_deref(), Some("dev-1"));
        assert_eq!(input.password, None, "empty password keeps the keyring one");
    }

    #[test]
    fn dialog_input_carries_poll_secs() {
        let mut d = dialog();
        d.poll_secs = "15".to_string();
        let input = dialog_input(&d).expect("valid");
        assert_eq!(input.poll_secs, Some(15));
    }

    #[test]
    fn dialog_submit_validates_and_locks() {
        let mut state = State::default();
        // Closed dialog: inert.
        assert_eq!(update(&mut state, Msg::Dialog(DialogMsg::Submit)), None);

        state.dialog.open = true;
        // Missing host: no action, not locked.
        assert_eq!(update(&mut state, Msg::Dialog(DialogMsg::Submit)), None);
        assert!(!state.dialog.saving);

        state.dialog.host = "h".to_string();
        state.dialog.username = "u".to_string();
        state.dialog.key_path = "k".to_string();
        assert_eq!(
            update(&mut state, Msg::Dialog(DialogMsg::Submit)),
            Some(Action::Save {
                input: dialog_input(&state.dialog).expect("valid"),
                is_new: true,
            })
        );
        assert!(state.dialog.saving);
        // While saving, submit is a no-op.
        assert_eq!(update(&mut state, Msg::Dialog(DialogMsg::Submit)), None);
    }

    #[test]
    fn dialog_save_done_closes_on_success_and_refreshes() {
        let mut state = State {
            dialog: DialogState {
                open: true,
                saving: true,
                ..DialogState::default()
            },
            ..State::default()
        };
        assert_eq!(
            update(
                &mut state,
                Msg::SaveDone(Ok(info("dev-new", "pi", "offline")))
            ),
            Some(Action::RefreshList)
        );
        assert!(!state.dialog.open);
    }

    #[test]
    fn dialog_save_done_keeps_open_on_failure() {
        let mut state = State {
            dialog: DialogState {
                open: true,
                saving: true,
                ..DialogState::default()
            },
            ..State::default()
        };
        assert_eq!(
            update(&mut state, Msg::SaveDone(Err("nope".to_string()))),
            None
        );
        assert!(state.dialog.open);
        assert!(!state.dialog.saving);
        assert_eq!(state.dialog.error.as_deref(), Some("nope"));
    }

    #[test]
    fn dialog_test_requires_valid_fields_and_unlocks_on_result() {
        let mut state = State {
            dialog: DialogState::open_new(),
            ..State::default()
        };
        state.dialog.host = "h".to_string();
        state.dialog.username = "u".to_string();
        state.dialog.key_path = "k".to_string();
        assert_eq!(
            update(&mut state, Msg::Dialog(DialogMsg::Test)),
            Some(Action::Test {
                input: dialog_input(&state.dialog).expect("valid"),
            })
        );
        assert!(state.dialog.testing);
        // Result unlocks and is rendered.
        assert_eq!(
            update(&mut state, Msg::TestDone(Ok("Linux".to_string()))),
            None
        );
        assert!(!state.dialog.testing);
        assert_eq!(state.dialog.test_result, Some(Ok("Linux".to_string())));
    }

    #[test]
    fn dialog_test_disabled_for_connected_stored_device() {
        let mut state = State {
            dialog: DialogState::for_edit(&info("dev-1", "pi", "connected")),
            ..State::default()
        };
        assert!(state.dialog.stored_connected);
        assert_eq!(update(&mut state, Msg::Dialog(DialogMsg::Test)), None);
        assert!(!state.dialog.testing);
    }

    #[test]
    fn dialog_for_edit_prefills_from_profile() {
        let d = DialogState::for_edit(&info("dev-1", "pi", "offline"));
        assert_eq!(d.editing_id.as_deref(), Some("dev-1"));
        assert!(!d.stored_connected);
        assert_eq!(d.name, "pi");
        assert_eq!(d.host, "10.0.0.5");
        assert_eq!(d.port, "22");
        assert_eq!(d.username, "pi");
        assert_eq!(d.kind, AuthKind::KeyFile);
        assert_eq!(d.key_path, "/home/me/.ssh/id_ed25519");
        assert_eq!(d.poll_secs, format!("{DEFAULT_POLL_SECS}"));
        assert!(d.password.is_empty() && d.passphrase.is_empty());
    }

    #[test]
    fn dialog_cancel_closes_even_while_saving() {
        let mut state = State {
            dialog: DialogState {
                open: true,
                saving: true,
                ..DialogState::default()
            },
            ..State::default()
        };
        assert_eq!(update(&mut state, Msg::Dialog(DialogMsg::Cancel)), None);
        assert!(!state.dialog.open);
    }

    #[test]
    fn stored_probe_input_reuses_the_profile_secrets() {
        let input = stored_probe_input(&info("dev-1", "pi", "offline").profile);
        assert_eq!(input.id.as_deref(), Some("dev-1"));
        assert_eq!(input.password, None);
        assert_eq!(input.passphrase, None);
        assert_eq!(input.poll_secs, Some(DEFAULT_POLL_SECS));
        assert_eq!(input.host, "10.0.0.5");
    }

    #[test]
    fn profile_summary_mentions_auth_and_poll() {
        let mut i = info("dev-1", "pi", "offline");
        assert!(profile_summary(&i.profile).contains("pi@10.0.0.5:22"));
        assert!(profile_summary(&i.profile).contains("key file"));
        assert!(profile_summary(&i.profile).contains("poll 5s"));
        i.profile.auth.has_passphrase = true;
        assert!(profile_summary(&i.profile).contains("key file + passphrase"));
        i.profile.auth.kind = AuthKind::Password;
        assert!(profile_summary(&i.profile).contains("password"));
    }

    #[test]
    fn browse_open_requests_home_listing() {
        let mut state = State::default();
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::Open {
                    device_id: "dev-1".to_string(),
                    name: "pi".to_string(),
                })
            ),
            Some(Action::BrowseDir {
                device_id: "dev-1".to_string(),
                path: String::new(),
            })
        );
        assert!(state.browse.open);
        assert!(state.browse.loading);
        assert_eq!(state.browse.device_name, "pi");
    }

    #[test]
    fn browse_dir_loaded_resolves_path_and_drops_stale() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );

        // Stale result (different device): dropped.
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::DirLoaded {
                    device_id: "dev-9".to_string(),
                    path: String::new(),
                    result: Ok(DirListing {
                        path: "/home/pi".to_string(),
                        entries: vec![],
                        truncated: false,
                    })
                })
            ),
            None
        );
        assert!(state.browse.pending.is_some());

        let listing = DirListing {
            path: "/home/pi".to_string(),
            entries: vec![crate::workspace::DirEntryInfo {
                name: "projects".to_string(),
                path: "/home/pi/projects".to_string(),
                kind: DirEntryKind::Dir,
                size: None,
            }],
            truncated: false,
        };
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::DirLoaded {
                    device_id: "dev-1".to_string(),
                    path: String::new(),
                    result: Ok(listing)
                })
            ),
            None
        );
        assert!(!state.browse.loading);
        assert_eq!(state.browse.pending, None);
        assert_eq!(state.browse.path.as_deref(), Some("/home/pi"));
        assert_eq!(state.browse.path_field, "/home/pi");
        assert_eq!(state.browse.listing.as_ref().unwrap().entries.len(), 1);
    }

    #[test]
    fn browse_dir_loaded_error_stays_on_current_dir() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::DirLoaded {
                    device_id: "dev-1".to_string(),
                    path: String::new(),
                    result: Err("sftp failed".to_string()),
                })
            ),
            None
        );
        assert!(!state.browse.loading);
        assert_eq!(state.browse.path, None);
        assert_eq!(state.browse.error.as_deref(), Some("sftp failed"));
    }

    #[test]
    fn browse_enter_requests_absolute_path() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::Enter("/home/pi".to_string()))
            ),
            Some(Action::BrowseDir {
                device_id: "dev-1".to_string(),
                path: "/home/pi".to_string(),
            })
        );
        assert_eq!(state.browse.pending.as_deref(), Some("/home/pi"));
        // Closed browser: navigation is inert.
        state.browse.close();
        assert_eq!(
            update(&mut state, Msg::Browse(BrowseMsg::Enter("/x".to_string()))),
            None
        );
    }

    #[test]
    fn browse_up_walks_parents_from_the_resolved_listing() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        // No listing yet: Up is inert.
        assert_eq!(update(&mut state, Msg::Browse(BrowseMsg::Up)), None);

        state.browse.listing = Some(DirListing {
            path: "/home/pi/projects".to_string(),
            entries: vec![],
            truncated: false,
        });
        assert_eq!(
            update(&mut state, Msg::Browse(BrowseMsg::Up)),
            Some(Action::BrowseDir {
                device_id: "dev-1".to_string(),
                path: "/home/pi".to_string(),
            })
        );
        // At the top: Up is inert.
        state.browse.listing = Some(DirListing {
            path: "/".to_string(),
            entries: vec![],
            truncated: false,
        });
        assert_eq!(update(&mut state, Msg::Browse(BrowseMsg::Up)), None);
    }

    #[test]
    fn browse_path_submit_trims_and_requests() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        state.browse.path_field = "  /srv/git  ".to_string();
        assert_eq!(
            update(&mut state, Msg::Browse(BrowseMsg::PathSubmitted)),
            Some(Action::BrowseDir {
                device_id: "dev-1".to_string(),
                path: "/srv/git".to_string(),
            })
        );
    }

    #[test]
    fn browse_scan_uses_the_current_directory() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        // No listing yet: scans the device home (None root).
        assert_eq!(
            update(&mut state, Msg::Browse(BrowseMsg::Scan)),
            Some(Action::Scan {
                device_id: "dev-1".to_string(),
                root: None,
            })
        );
        assert!(state.browse.scanning);
        assert!(state.browse.scan.is_none());
        // While scanning, further scans are ignored.
        assert_eq!(update(&mut state, Msg::Browse(BrowseMsg::Scan)), None);

        let candidates = vec![DeviceRepoCandidate {
            path: "/home/pi/myrepo".to_string(),
            name: "myrepo".to_string(),
        }];
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::ScanDone {
                    device_id: "dev-1".to_string(),
                    result: Ok(candidates),
                })
            ),
            None
        );
        assert!(!state.browse.scanning);
        assert_eq!(state.browse.scan.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn browse_scan_error_surfaces_and_unlocks() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        let _ = update(&mut state, Msg::Browse(BrowseMsg::Scan));
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::ScanDone {
                    device_id: "dev-1".to_string(),
                    result: Err("scan timed out".to_string()),
                })
            ),
            None
        );
        assert!(!state.browse.scanning);
        assert_eq!(state.browse.error.as_deref(), Some("scan timed out"));
    }

    #[test]
    fn browse_open_repo_locks_until_finished() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::OpenRepo {
                    path: "/home/pi/myrepo".to_string(),
                })
            ),
            Some(Action::OpenRepo {
                device_id: "dev-1".to_string(),
                path: "/home/pi/myrepo".to_string(),
            })
        );
        assert_eq!(state.browse.opening.as_deref(), Some("/home/pi/myrepo"));
        // While an open is in flight, further opens are ignored.
        assert_eq!(
            update(
                &mut state,
                Msg::Browse(BrowseMsg::OpenRepo {
                    path: "/other".to_string(),
                })
            ),
            None
        );
    }

    #[test]
    fn browse_close_resets_everything() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            Msg::Browse(BrowseMsg::Open {
                device_id: "dev-1".to_string(),
                name: "pi".to_string(),
            }),
        );
        state.browse.scan = Some(vec![]);
        state.browse.opening = Some("/x".to_string());
        assert_eq!(update(&mut state, Msg::Browse(BrowseMsg::Close)), None);
        assert!(!state.browse.open);
        assert!(state.browse.scan.is_none());
        assert_eq!(state.browse.opening, None);
    }

    #[test]
    fn test_stored_probe_uses_the_row_profile() {
        let mut state = state_with_rows();
        assert_eq!(
            update(&mut state, Msg::TestStored("dev-2".to_string())),
            Some(Action::Test {
                input: stored_probe_input(&state.rows[1].profile),
            })
        );
        // Unknown device: inert.
        assert_eq!(
            update(&mut state, Msg::TestStored("dev-9".to_string())),
            None
        );
    }
}
