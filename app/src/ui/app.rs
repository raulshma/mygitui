//! Application shell: boot, update, view.
//!
//! The process has one [`AppServices`] bag (repo manager, action/pty/opencode
//! registries, device pool) shared by reference through `Arc`. Backend work
//! is spawned onto the shared tokio runtime (`crate::rt`) and reports back
//! through [`BackendEvent`](crate::service::events::BackendEvent); the iced
//! subscription bridging that bus lands with the first real panels.

use std::sync::Arc;

use iced::widget::{center, text};
use iced::{window, Element, Size, Task};

use crate::service::AppServices;

pub fn run(cli_args: Vec<String>) -> iced::Result {
    iced::application(move || App::boot(cli_args.clone()), App::update, App::view)
        .title("mygitui")
        .theme(App::theme)
        .window(window::Settings {
            size: Size::new(1400.0, 900.0),
            min_size: Some(Size::new(900.0, 600.0)),
            ..window::Settings::default()
        })
        .run()
}

pub struct App {
    services: Arc<AppServices>,
    cli_args: crate::CliArgs,
    theme: iced::Theme,
}

#[derive(Debug, Clone)]
pub enum Message {}

impl App {
    fn boot(cli_args: Vec<String>) -> (Self, Task<Message>) {
        crate::auth::init();
        (
            Self {
                services: Arc::new(AppServices::new()),
                cli_args: crate::CliArgs(std::sync::Mutex::new(cli_args)),
                theme: iced::Theme::Dark,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        center(text("mygitui").size(24)).into()
    }

    fn theme(&self) -> iced::Theme {
        self.theme.clone()
    }
}
