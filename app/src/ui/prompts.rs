//! Global prompt modals: HTTPS/SSH credentials, SSH host-key verification
//! and remote-device secrets.
//!
//! Backend ops block inside these prompts (auth broker / prompt broker), so
//! the modal must be answerable from anywhere in the app. Secrets are only
//! forwarded to the broker (optionally persisted to the OS keyring by the
//! service layer); the UI never writes them to disk.

use iced::widget::{button, checkbox, column, container, row, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::auth::AuthRequestEvent;
use crate::remote::prompts::{DeviceAuthRequestEvent, HostKeyRequestEvent};

use super::app::Message;
use super::dialog::modal_with_cancel;
use super::theme::{self, Tokens};

/// Width of the prompt card (matches the settings/clone dialog cards).
const CARD_WIDTH: f32 = 480.0;

/// FE state of an HTTP(S)/SSH credential request.
#[derive(Debug)]
pub struct AuthPromptState {
    pub request: AuthRequestEvent,
    pub username: String,
    pub password: String,
    pub store: bool,
}

impl AuthPromptState {
    pub fn new(request: AuthRequestEvent) -> Self {
        Self {
            request,
            username: String::new(),
            password: String::new(),
            store: false,
        }
    }

    pub fn wants_username(&self) -> bool {
        self.request.kind != "ssh-passphrase"
    }

    pub fn wants_password(&self) -> bool {
        self.request.kind != "https-user"
    }

    pub fn can_submit(&self) -> bool {
        if self.wants_username() && self.username.trim().is_empty() {
            return false;
        }
        if self.wants_password() && self.password.is_empty() {
            return false;
        }
        true
    }
}

/// FE state of an SSH host-key confirmation.
#[derive(Debug)]
pub struct HostKeyPromptState {
    pub request: HostKeyRequestEvent,
}

/// FE state of a remote-device secret prompt (key passphrase / password).
#[derive(Debug)]
pub struct DeviceSecretPromptState {
    pub request: DeviceAuthRequestEvent,
    pub password: String,
    pub store: bool,
}

/// The prompt modal currently waiting for an answer, if any.
pub fn overlay<'a>(
    tokens: Tokens,
    auth: Option<&AuthPromptState>,
    host_key: Option<&HostKeyPromptState>,
    device: Option<&DeviceSecretPromptState>,
) -> Option<Element<'a, Message>> {
    if let Some(state) = auth {
        return Some(auth_prompt(tokens, state));
    }
    if let Some(state) = host_key {
        return Some(host_key_prompt(tokens, state));
    }
    device.map(|state| device_prompt(tokens, state))
}

fn auth_prompt<'a>(tokens: Tokens, state: &AuthPromptState) -> Element<'a, Message> {
    let mut body = column![
        text("Authentication required")
            .size(14)
            .font(theme::semibold()),
        Space::new().height(4),
        text(clip(&state.request.url, 72))
            .size(theme::TEXT_SMALL)
            .font(theme::mono())
            .color(tokens.muted_foreground),
        text(state.request.prompt.clone())
            .size(theme::TEXT_SMALL)
            .color(tokens.foreground),
    ]
    .spacing(3);

    if state.wants_username() {
        body = body.push(
            text_input("Username", state.username.as_str())
                .on_input(Message::AuthUsernameChanged)
                .on_submit(Message::AuthPromptSubmit)
                .size(theme::TEXT_SMALL)
                .padding([5, 8]),
        );
    }
    if state.wants_password() {
        body = body.push(
            text_input("Password / passphrase", state.password.as_str())
                .secure(true)
                .on_input(Message::AuthPasswordChanged)
                .on_submit(Message::AuthPromptSubmit)
                .size(theme::TEXT_SMALL)
                .padding([5, 8]),
        );
        body = body.push(
            checkbox(state.store)
                .label("Remember in the OS keyring")
                .size(theme::TEXT_BODY)
                .text_size(theme::TEXT_META)
                .spacing(4)
                .on_toggle(Message::AuthStoreToggled),
        );
    }

    let mut confirm = button(text("Continue").size(theme::TEXT_SMALL))
        .style(theme::button_style(tokens, theme::ButtonKind::Primary))
        .padding([4, 12]);
    if state.can_submit() {
        confirm = confirm.on_press(Message::AuthPromptSubmit);
    }

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(theme::TEXT_SMALL))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(Message::AuthPromptCancel),
        confirm,
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    // Shared modal scaffold. A scrim press answers the prompt with its
    // cancel — the blocked op unblocks instead of hanging on the auth
    // timeout that a silently vanished prompt used to incur.
    modal_with_cancel(
        tokens,
        CARD_WIDTH,
        body.into(),
        actions.into(),
        Message::AuthPromptCancel,
    )
}

fn host_key_prompt<'a>(tokens: Tokens, state: &HostKeyPromptState) -> Element<'a, Message> {
    let request = &state.request;
    let changed = request.status == "changed";

    let mut body = column![
        text(if changed {
            "SSH host key changed"
        } else {
            "Verify SSH host key"
        })
        .size(14)
        .font(theme::semibold()),
        Space::new().height(4),
        text(format!(
            "{}:{} offers a {} key.",
            request.host, request.port, request.algo
        ))
        .size(theme::TEXT_SMALL)
        .color(tokens.foreground),
        Space::new().height(4),
        mono_box(tokens, &request.fingerprint),
    ]
    .spacing(3);

    if changed {
        body = body.push(
            text("This differs from the pinned key. Accepting can expose the connection to a man-in-the-middle.")
                .size(theme::TEXT_SMALL)
                .color(tokens.deleted),
        );
        if let Some(pinned) = request.pinned_fingerprint.as_deref() {
            body = body.push(
                text(format!("Pinned: {pinned}"))
                    .size(theme::TEXT_META)
                    .font(theme::mono())
                    .color(tokens.muted_foreground),
            );
        }
    }

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Reject").size(theme::TEXT_SMALL))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(Message::HostKeyAnswer(false)),
        button(
            text(if changed {
                "Accept anyway"
            } else {
                "Accept & pin"
            })
            .size(theme::TEXT_SMALL)
        )
        .style(theme::button_style(
            tokens,
            if changed {
                theme::ButtonKind::Danger
            } else {
                theme::ButtonKind::Primary
            },
        ))
        .padding([4, 12])
        .on_press(Message::HostKeyAnswer(true)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    // Scrim press = Reject (answers the broker, so the blocked op proceeds).
    modal_with_cancel(
        tokens,
        CARD_WIDTH,
        body.into(),
        actions.into(),
        Message::HostKeyAnswer(false),
    )
}

fn device_prompt<'a>(tokens: Tokens, state: &DeviceSecretPromptState) -> Element<'a, Message> {
    let title = if state.request.kind == "ssh-passphrase" {
        "SSH key passphrase"
    } else {
        "Device password"
    };

    let mut confirm = button(text("Unlock").size(theme::TEXT_SMALL))
        .style(theme::button_style(tokens, theme::ButtonKind::Primary))
        .padding([4, 12]);
    if !state.password.is_empty() {
        confirm = confirm.on_press(Message::DeviceSecretSubmit);
    }

    let body = column![
        text(title).size(14).font(theme::semibold()),
        Space::new().height(4),
        text(state.request.prompt.clone())
            .size(theme::TEXT_SMALL)
            .color(tokens.foreground),
        text_input("Secret", state.password.as_str())
            .secure(true)
            .on_input(Message::DeviceSecretChanged)
            .on_submit(Message::DeviceSecretSubmit)
            .size(theme::TEXT_SMALL)
            .padding([5, 8]),
        checkbox(state.store)
            .label("Remember in the OS keyring")
            .size(theme::TEXT_BODY)
            .text_size(theme::TEXT_META)
            .spacing(4)
            .on_toggle(Message::DeviceSecretStoreToggled),
    ]
    .spacing(3);

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(theme::TEXT_SMALL))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press(Message::DeviceSecretCancel),
        confirm,
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    // Scrim press cancels the unlock (answers the broker).
    modal_with_cancel(
        tokens,
        CARD_WIDTH,
        body.into(),
        actions.into(),
        Message::DeviceSecretCancel,
    )
}

fn mono_box<'a>(tokens: Tokens, value: &str) -> Element<'a, Message> {
    container(
        text(value.to_string())
            .size(theme::TEXT_SMALL)
            .font(theme::mono())
            .color(tokens.foreground),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(tokens.muted)),
        border: Border {
            color: tokens.border,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn clip(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let head: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::AuthPromptState;
    use crate::auth::AuthRequestEvent;

    fn state(kind: &str, username: &str, password: &str) -> AuthPromptState {
        AuthPromptState {
            request: AuthRequestEvent {
                op_id: "op".into(),
                repo_id: "repo".into(),
                url: "https://example.com/x.git".into(),
                kind: kind.into(),
                prompt: "prompt".into(),
            },
            username: username.into(),
            password: password.into(),
            store: false,
        }
    }

    #[test]
    fn https_user_requires_username_only() {
        assert!(!state("https-user", "", "secret").can_submit());
        assert!(state("https-user", "raul", "").can_submit());
        assert!(!state("https-user", " ", "secret").can_submit());
    }

    #[test]
    fn ssh_passphrase_requires_password_only() {
        assert!(!state("ssh-passphrase", "", "").can_submit());
        assert!(state("ssh-passphrase", "", "hunter2").can_submit());
        assert!(state("ssh-passphrase", "ignored", "hunter2").can_submit());
    }

    #[test]
    fn https_pass_requires_password() {
        assert!(!state("https-pass", "raul", "").can_submit());
        assert!(state("https-pass", "raul", "token").can_submit());
    }
}
