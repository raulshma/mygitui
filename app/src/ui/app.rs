//! Application shell: boot, update, view and the backend-bus subscription.
//!
//! One [`AppServices`] bag is shared through `Arc`; async service bodies run
//! as `Task::perform` futures on the iced (tokio) executor and report back
//! through messages. Push-style signals (watcher batches, op progress,
//! clones, prompts) arrive over the [`BackendEvent`](crate::service::events)
//! bus subscription.

use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::{button, column, container, row, text, Space};
use iced::{window, Alignment, Element, Length, Subscription, Task};

use crate::engine::types::{RepoId, RepoInfo, RepoStatus};
use crate::service::events::BackendEvent;
use crate::service::AppServices;

use super::bus;
use super::config::{self, Config, RecentRepo};
use super::home;
use super::repo_view::{self, Load, Tab};
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
    SelectFile(String),
    GoHome,
    CycleTheme,
    TogglePin(String),
    RemoveRecent(String),
    DismissNotice,
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

    fn set_status(&mut self, repo_id: &RepoId, result: Result<RepoStatus, String>) {
        let Some(index) = repo_view::tab_index(&self.tabs, repo_id) else {
            return;
        };
        self.tabs[index].status = match result {
            Ok(status) => Load::Ready(status),
            Err(err) => Load::Failed(err),
        };
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Backend(event) => self.backend(event),
            Message::SystemTheme(mode) => {
                self.system_mode = mode;
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
                        self.refresh(repo_id)
                    }
                    Err(err) => {
                        self.notice = Some(Notice::error(format!("Could not open {path}: {err}")));
                        Task::none()
                    }
                }
            }
            Message::StatusLoaded { repo_id, result } => {
                self.set_status(&repo_id, result);
                Task::none()
            }
            Message::Refresh(repo_id) => {
                if let Some(index) = repo_view::tab_index(&self.tabs, &repo_id) {
                    self.tabs[index].status = Load::Loading;
                }
                self.refresh(repo_id)
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
            Message::SelectFile(path) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.selected = Some(path);
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
        }
    }

    fn backend(&mut self, event: BackendEvent) -> Task<Message> {
        match event {
            BackendEvent::RepoChanged(changed) => self.refresh(changed.repo_id),
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
