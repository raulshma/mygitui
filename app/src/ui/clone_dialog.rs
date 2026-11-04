//! Clone-repository dialog: a centered card with the remote URL, destination
//! directory (typed or picked natively), and shallow/blobless options.
//!
//! Pure UI + intent module: no service calls, no `Task`. The shell wires it
//! like the other overlays:
//!
//! 1. Own a [`State`]; open it from the Home screen button or the palette
//!    (`repo.clone`) via `Msg::Open`.
//! 2. Route [`Msg`] through [`update`], translating the returned [`Action`]:
//!    `PickFolder` spawns the native folder picker and feeds the result back
//!    as `Msg::FolderPicked`; `Start` runs `service::repo::repo_clone` or
//!    `service::maintenance::clone_blobless` (per the blobless checkbox) and
//!    feeds the result back through the shell's completion message.
//! 3. Layer [`view`] above the workspace while `state.open` is set, the same
//!    way `dialog::confirm` is layered today.
//! 4. Feed `BackendEvent::CloneProgress` events whose `url` matches
//!    [`State::active_url`] into [`State::progress`] — the bar fills by
//!    received objects (the service's own vocabulary).
//!
//! While `running`, the submit button is disabled and the card shows a
//! progress bar (blobless clones run through the git CLI and emit no
//! progress events, so the card shows an indeterminate "Starting clone…"
//! line instead).

use iced::widget::{button, checkbox, column, container, mouse_area, row, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use super::theme::{self, Tokens};

/// Width of the dialog card.
const CARD_WIDTH: f32 = 480.0;

/// One progress snapshot for the in-flight clone (mirrors
/// `service::repo::CloneProgress` minus the url discriminator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// Bytes received so far.
    pub received: usize,
    /// Total objects the remote announced (0 = not yet known).
    pub total: usize,
    /// Objects received so far.
    pub objects: usize,
}

impl Progress {
    /// Fill fraction of the transfer bar (0..=1), or `None` while the remote
    /// has not announced the object count yet.
    pub fn fraction(&self) -> Option<f32> {
        if self.total == 0 {
            return None;
        }
        Some((self.objects as f32 / self.total as f32).clamp(0.0, 1.0))
    }

    /// One-line transfer summary, e.g. `1234 / 5678 objects · 45.2 MB`.
    pub fn summary(&self) -> String {
        match self.total {
            0 => format!("{} objects · {}", self.objects, byte_label(self.received)),
            _ => format!(
                "{} / {} objects · {}",
                self.objects,
                self.total,
                byte_label(self.received)
            ),
        }
    }
}

/// Human-readable byte count (binary units, one decimal below 1 GiB).
fn byte_label(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Dialog state owned by the shell.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub open: bool,
    pub url: String,
    pub destination: String,
    pub shallow: bool,
    /// Depth as typed (parsed on submit; empty/invalid → service default).
    pub depth: String,
    pub blobless: bool,
    /// True between an accepted `Submit` and the completion message.
    pub running: bool,
    /// URL of the in-flight clone; `CloneProgress` events are matched by it.
    pub active_url: String,
    /// Latest progress snapshot for the in-flight clone.
    pub progress: Option<Progress>,
}

impl State {
    /// Opens the dialog with fresh fields (kept between runs otherwise).
    pub fn open(&mut self) {
        self.open = true;
        self.url.clear();
        self.destination.clear();
        self.shallow = false;
        self.depth.clear();
        self.blobless = false;
        self.running = false;
        self.active_url.clear();
        self.progress = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.running = false;
        self.active_url.clear();
        self.progress = None;
    }

    /// Marks a clone as started (`Action::Start` accepted by the shell):
    /// keeps the dialog open, disables submit and resets the progress readout.
    pub fn start(&mut self, url: &str) {
        self.running = true;
        self.active_url = url.to_string();
        self.progress = None;
    }

    /// Applies a natively picked folder: the clone goes into a subdirectory
    /// named after the repository (the picked folder itself must stay
    /// untouched; the service only clones into missing-or-empty paths).
    pub fn set_destination_folder(&mut self, picked: &str) {
        let joined = match repo_name_from_url(&self.url) {
            Some(name) => join_destination(picked, &name),
            None => picked.to_string(),
        };
        if !self.destination.trim().is_empty() && self.destination == joined {
            return;
        }
        self.destination = joined;
    }
}

/// Messages the shell forwards to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Open the dialog (Home button, palette command); resets the fields.
    Open,
    /// Dismiss the dialog (Cancel, ×, scrim press). Ignored while running.
    Close,
    UrlChanged(String),
    DestinationChanged(String),
    /// Open the native folder picker (shell spawns it).
    PickFolder,
    /// Native folder picker result (None = canceled).
    FolderPicked(Option<std::path::PathBuf>),
    ShallowToggled(bool),
    DepthChanged(String),
    BloblessToggled(bool),
    /// Submit: validates the fields and hands off to the shell.
    Submit,
}

/// Intents the shell translates into tasks / shell messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The dialog dismissed itself; `open` is already `false`.
    Close,
    /// Spawn the native folder picker.
    PickFolder,
    /// Run the clone: blobless routes to `maintenance::clone_blobless`, a
    /// plain clone to `repo::repo_clone` (with `depth` when shallow).
    Start {
        url: String,
        destination: String,
        blobless: bool,
        depth: Option<u32>,
    },
}

/// Steps the dialog; returns the intent for the shell to translate, if any.
pub fn update(state: &mut State, msg: Msg) -> Option<Action> {
    match msg {
        Msg::Open => {
            state.open();
            None
        }
        Msg::Close => {
            if state.running {
                return None; // a clone is in flight; let it finish
            }
            state.close();
            Some(Action::Close)
        }
        Msg::UrlChanged(url) => {
            state.url = url;
            None
        }
        Msg::DestinationChanged(destination) => {
            state.destination = destination;
            None
        }
        Msg::PickFolder if !state.running => Some(Action::PickFolder),
        Msg::PickFolder => None,
        Msg::FolderPicked(Some(path)) => {
            if !state.running {
                state.set_destination_folder(&path.to_string_lossy());
            }
            None
        }
        Msg::FolderPicked(None) => None,
        Msg::ShallowToggled(shallow) => {
            state.shallow = shallow;
            None
        }
        Msg::DepthChanged(depth) => {
            state.depth = depth;
            None
        }
        Msg::BloblessToggled(blobless) => {
            state.blobless = blobless;
            None
        }
        Msg::Submit if !state.running && state.open => {
            let url = state.url.trim().to_string();
            let destination = state.destination.trim().to_string();
            if url.is_empty() || destination.is_empty() {
                return None;
            }
            // A depth outside shallow mode is dropped (the checkbox is the
            // only way to ask for one).
            let depth = if state.shallow {
                parse_depth(&state.depth)
            } else {
                None
            };
            state.start(&url);
            Some(Action::Start {
                url,
                destination,
                blobless: state.blobless,
                depth,
            })
        }
        Msg::Submit => None,
    }
}

/// Parses the depth field: trimmed, positive, at most `i32::MAX` (git's
/// `--depth` is a signed int).
pub fn parse_depth(raw: &str) -> Option<u32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let depth: u32 = trimmed.parse().ok()?;
    if depth == 0 {
        return None;
    }
    Some(depth)
}

/// Derives the repository directory name from a remote URL: the last path
/// segment minus a `.git` suffix. Handles https/ssh URLs, `git@host:path`
/// scp-like forms and plain local paths; `None` when nothing sensible
/// remains (empty, root, only a host).
pub fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    // scp-like: `git@github.com:user/repo.git` — take the part after ':'.
    let path = match trimmed.split_once(':') {
        // A drive letter (`C:\repo`) is a path, not a scheme separator.
        Some((before, after))
            if !before.is_empty()
                && !after.starts_with('/')
                && !before.contains('/')
                && before.len() > 1 =>
        {
            after
        }
        _ => trimmed,
    };
    let name = path
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())?;
    if name == ".git" || name == ".." {
        return None;
    }
    let name = name.strip_suffix(".git").unwrap_or(name);
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// Joins a picked folder and a repository name into the clone destination.
/// Reuses the host's separator so the display matches the platform.
pub fn join_destination(picked: &str, name: &str) -> String {
    if picked.is_empty() {
        return name.to_string();
    }
    let separator = if picked.ends_with('/') || picked.ends_with('\\') {
        ""
    } else if picked.contains('\\') && !picked.contains('/') {
        "\\"
    } else {
        "/"
    };
    format!("{picked}{separator}{name}")
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

    let title = row![
        text("Clone repository").size(14).font(theme::semibold()),
        Space::new().width(Length::Fill),
        button(text("×").size(12))
            .style(theme::button_style(tokens, theme::ButtonKind::Ghost))
            .padding([3, 8])
            .on_press(Msg::Close),
    ]
    .align_y(Alignment::Center);

    let url = labeled_field(
        tokens,
        "Repository URL",
        text_input("https://github.com/user/repo.git", state.url.as_str())
            .on_input(Msg::UrlChanged)
            .size(12)
            .padding([5, 8]),
    );

    let destination = labeled_field(
        tokens,
        "Destination directory",
        row![
            text_input("/path/to/clone-into", state.destination.as_str())
                .on_input(Msg::DestinationChanged)
                .size(12)
                .padding([5, 8])
                .width(Length::Fill),
            button(text("Browse…").size(11))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([5, 10])
                .on_press_maybe((!state.running).then_some(Msg::PickFolder)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    );

    let depth_field: Element<'static, Msg> = if state.shallow {
        labeled_field(
            tokens,
            "Depth (commits)",
            text_input("1", state.depth.as_str())
                .on_input(Msg::DepthChanged)
                .size(12)
                .padding([5, 8])
                .width(Length::Fixed(90.0)),
        )
    } else {
        Space::new().height(0).into()
    };

    let mut options = column![
        checkbox(state.shallow)
            .label("Shallow clone")
            .text_size(11)
            .size(14)
            .spacing(5)
            .on_toggle(Msg::ShallowToggled),
        checkbox(state.blobless)
            .label("Blobless (no file contents)")
            .text_size(11)
            .size(14)
            .spacing(5)
            .on_toggle(Msg::BloblessToggled),
    ]
    .spacing(6);
    options = options.push(Space::new().height(2));
    options = options.push(depth_field);

    let progress_area: Element<'static, Msg> = if state.running {
        match &state.progress {
            Some(progress) => {
                let mut area = column![].spacing(4);
                if let Some(fraction) = progress.fraction() {
                    area = area.push(progress_bar(tokens, fraction));
                }
                area = area.push(
                    text(progress.summary())
                        .size(10)
                        .color(tokens.muted_foreground),
                );
                area.into()
            }
            None => text(if state.blobless {
                "Cloning (blobless)…"
            } else {
                "Starting clone…"
            })
            .size(10)
            .color(tokens.muted_foreground)
            .into(),
        }
    } else {
        Space::new().height(0).into()
    };

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press_maybe((!state.running).then_some(Msg::Close)),
        button(text(if state.running { "Cloning…" } else { "Clone" }).size(11))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 12])
            .on_press_maybe((!state.running).then_some(Msg::Submit)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let card = container(
        column![title, url, destination, options, progress_area, actions]
            .spacing(10)
            .width(Length::Fill),
    )
    .width(Length::Fixed(CARD_WIDTH))
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

    // Like dialog.rs: presses on the scrim cancel the dialog (unless a
    // clone is running — the widget handles that), inputs consume theirs.
    mouse_area(scrim).on_press(Msg::Close).into()
}

/// Label above a field, following the dialog spacing vocabulary.
fn labeled_field<'a>(
    tokens: Tokens,
    label: &'a str,
    field: impl Into<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    column![
        text(label).size(11).color(tokens.muted_foreground),
        field.into(),
    ]
    .spacing(3)
    .width(Length::Fill)
    .into()
}

/// Hand-rolled transfer bar (two containers over tokens.primary): the
/// default `progress_bar` widget styles from the iced theme palette, which
/// this app does not use.
fn progress_bar(tokens: Tokens, fraction: f32) -> Element<'static, Msg> {
    container(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(4.0))
            .style(move |_theme| container::Style {
                background: Some(Background::Color(theme::mix(
                    tokens.primary,
                    tokens.card,
                    fraction.clamp(0.0, 1.0),
                ))),
                border: Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..container::Style::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fixed(6.0))
    .style(move |_theme| container::Style {
        background: Some(Background::Color(tokens.muted)),
        border: Border {
            color: tokens.border,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// The platform path separator (`Path` is authoritative but tests need the
/// unix form; windows uses `\`).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_depth_accepts_positive_ints() {
        assert_eq!(parse_depth("1"), Some(1));
        assert_eq!(parse_depth(" 50 "), Some(50));
        assert_eq!(parse_depth("0"), None);
        assert_eq!(parse_depth(""), None);
        assert_eq!(parse_depth("abc"), None);
        assert_eq!(parse_depth("-3"), None);
    }

    #[test]
    fn repo_name_from_url_variants() {
        assert_eq!(
            repo_name_from_url("https://github.com/user/repo.git"),
            Some("repo".to_string())
        );
        assert_eq!(
            repo_name_from_url("https://github.com/user/repo"),
            Some("repo".to_string())
        );
        assert_eq!(
            repo_name_from_url("git@github.com:user/repo.git"),
            Some("repo".to_string())
        );
        assert_eq!(
            repo_name_from_url("/home/dev/project.git/"),
            Some("project".to_string())
        );
        assert_eq!(
            repo_name_from_url("https://github.com/user/repo.git/"),
            Some("repo".to_string())
        );
        assert_eq!(repo_name_from_url("https://github.com/"), None);
        assert_eq!(repo_name_from_url(""), None);
    }

    #[test]
    fn repo_name_keeps_windows_paths() {
        assert_eq!(
            repo_name_from_url("C:\\dev\\myrepo.git"),
            Some("myrepo".to_string())
        );
    }

    #[test]
    fn join_destination_appends_name() {
        assert_eq!(
            join_destination("/home/dev", "repo"),
            "/home/dev/repo".to_string()
        );
        assert_eq!(join_destination("", "repo"), "repo".to_string());
        assert_eq!(
            join_destination("/home/dev/", "repo"),
            "/home/dev/repo".to_string()
        );
    }

    #[test]
    fn join_destination_uses_backslash_for_windows() {
        assert_eq!(
            join_destination("C:\\dev", "repo"),
            "C:\\dev\\repo".to_string()
        );
    }

    #[test]
    fn progress_fraction_fills_by_objects() {
        let progress = Progress {
            received: 4096,
            total: 100,
            objects: 50,
        };
        assert_eq!(progress.fraction(), Some(0.5));
        assert_eq!(progress.summary(), "50 / 100 objects · 4.0 KiB".to_string());
        assert_eq!(
            Progress::default().fraction(),
            None,
            "no total announced yet"
        );
        // Overshoot clamps (remote counts can wobble mid-transfer).
        let progress = Progress {
            received: 0,
            total: 10,
            objects: 20,
        };
        assert_eq!(progress.fraction(), Some(1.0));
    }

    #[test]
    fn progress_summary_without_total() {
        let progress = Progress {
            received: 2048,
            total: 0,
            objects: 3,
        };
        assert_eq!(progress.summary(), "3 objects · 2.0 KiB");
    }

    #[test]
    fn byte_label_units() {
        assert_eq!(byte_label(0), "0 B");
        assert_eq!(byte_label(999), "999 B");
        assert_eq!(byte_label(1024), "1.0 KiB");
        assert_eq!(byte_label(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn set_destination_folder_joins_repo_name() {
        let mut state = State {
            url: "https://github.com/user/widget.git".to_string(),
            ..State::default()
        };
        state.set_destination_folder("/tmp");
        assert_eq!(state.destination, "/tmp/widget");
    }

    #[test]
    fn open_resets_fields() {
        let mut state = State {
            url: "leftover".to_string(),
            running: true,
            ..State::default()
        };
        state.open();
        assert!(state.open);
        assert!(!state.running);
        assert_eq!(state.url, "");
        assert_eq!(state.destination, "");
    }

    #[test]
    fn submit_validates_and_starts() {
        let mut state = State::default();
        state.open();
        // Missing url: no action.
        assert_eq!(update(&mut state, Msg::Submit), None);
        state.url = "https://example.com/r.git".to_string();
        // Missing destination: still no action.
        assert_eq!(update(&mut state, Msg::Submit), None);
        state.destination = "/tmp/r".to_string();
        assert_eq!(
            update(&mut state, Msg::Submit),
            Some(Action::Start {
                url: "https://example.com/r.git".to_string(),
                destination: "/tmp/r".to_string(),
                blobless: false,
                depth: None,
            })
        );
        assert!(state.running);
        assert_eq!(state.active_url, "https://example.com/r.git");
        // While running, submit is a no-op.
        assert_eq!(update(&mut state, Msg::Submit), None);
    }

    #[test]
    fn submit_shallow_uses_parsed_depth() {
        let mut state = State::default();
        state.open();
        state.url = "https://example.com/r.git".to_string();
        state.destination = "/tmp/r".to_string();
        state.shallow = true;
        state.depth = "10".to_string();
        let action = update(&mut state, Msg::Submit);
        assert_eq!(
            action,
            Some(Action::Start {
                url: "https://example.com/r.git".to_string(),
                destination: "/tmp/r".to_string(),
                blobless: false,
                depth: Some(10),
            })
        );
    }

    #[test]
    fn submit_not_shallow_drops_depth() {
        let mut state = State::default();
        state.open();
        state.url = "https://example.com/r.git".to_string();
        state.destination = "/tmp/r".to_string();
        state.depth = "10".to_string();
        match update(&mut state, Msg::Submit) {
            Some(Action::Start { depth, .. }) => assert_eq!(depth, None),
            other => panic!("expected Start, got {other:?}"),
        }
    }

    #[test]
    fn submit_carries_blobless_flag() {
        let mut state = State::default();
        state.open();
        state.url = "https://example.com/r.git".to_string();
        state.destination = "/tmp/r".to_string();
        state.blobless = true;
        match update(&mut state, Msg::Submit) {
            Some(Action::Start { blobless, .. }) => assert!(blobless),
            other => panic!("expected Start, got {other:?}"),
        }
    }

    #[test]
    fn close_while_running_is_ignored() {
        let mut state = State::default();
        state.open();
        state.url = "https://example.com/r.git".to_string();
        state.destination = "/tmp/r".to_string();
        let _ = update(&mut state, Msg::Submit);
        assert!(state.running);
        // Close is swallowed while a clone is in flight.
        assert_eq!(update(&mut state, Msg::Close), None);
        assert!(state.open);
    }

    #[test]
    fn close_after_finish_dismisses() {
        let mut state = State::default();
        state.open();
        state.running = true;
        state.close();
        assert!(!state.open);
        // A fresh close now yields the intent (shell clears overlay).
        state.open();
        assert_eq!(update(&mut state, Msg::Close), Some(Action::Close));
        assert!(!state.open);
    }

    #[test]
    fn folder_pick_while_running_ignored() {
        let mut state = State::default();
        state.open();
        state.running = true;
        assert_eq!(
            update(&mut state, Msg::FolderPicked(Some("/tmp".into()))),
            None
        );
        assert_eq!(state.destination, "");
        assert_eq!(update(&mut state, Msg::PickFolder), None);
    }

    #[test]
    fn folder_pick_without_url_uses_folder_as_is() {
        let mut state = State::default();
        state.open();
        assert_eq!(
            update(&mut state, Msg::FolderPicked(Some("/tmp/empty".into()))),
            None
        );
        assert_eq!(state.destination, "/tmp/empty");
    }
}
