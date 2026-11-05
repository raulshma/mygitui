//! AI settings dialog: a centered card for the persisted `ai` config section
//! (`ui::config::AiSettings`) plus the OpenRouter API key in the OS keyring.
//!
//! Pure UI + intent module, wired like the clone dialog:
//!
//! 1. Own a [`State`] on the shell; open it from the header AI chip or the
//!    palette (`app.ai_settings`) via [`open`] (seeds the drafts from the
//!    current config) and fire the keyring probe for [`Msg::KeyStatusLoaded`].
//! 2. Route [`Msg`] through [`update`], translating the returned [`Action`]:
//!    `ToggleMaster` / `ToggleOptIn` reuse the shell's existing
//!    `AiToggleEnabled` / `AiToggleRepoOptIn` arms (immediate, persisted);
//!    `Save` applies the config fields through the existing atomic config
//!    save and performs the keyring write (`None` field = unset, empty API
//!    key = leave the stored one, "Clear" = delete it).
//! 3. Layer [`view`] above the workspace while `state.open` is set.
//!
//! The master "AI enabled" and "Opt in for this repository" checkboxes are
//! the existing toggles (not drafts): pressing them flips the persisted
//! config immediately — consistent with their palette commands. Everything
//! else (provider, URLs, models, key) is a draft applied on Save; Cancel
//! discards it. The managed-opencode auto-start behavior is untouched: the
//! dialog only edits settings.
//!
//! The header health chip derivation ([`ai_health`]) lives here too: it is
//! derived from the config switch, the last AI call result (tracked by the
//! shell's existing `*Done` arms) and the one-shot `service::ai::ai_status`
//! probe — no extra polling beyond what the service already exposes.

use iced::widget::{button, checkbox, column, row, text, text_input, Space};
use iced::{Alignment, Element, Length};

use crate::service::ai::AiStatus;

use super::config::{AiProviderChoice, AiSettings};
use super::dialog::modal_with_cancel;
use super::theme::{self, Tokens};

/// Width of the dialog card.
const CARD_WIDTH: f32 = 480.0;

/// How the Save action edits the stored OpenRouter key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyEdit {
    /// No change: the field was left empty and Clear was not pressed.
    Keep,
    /// Create/overwrite the stored key with the typed value.
    Set(String),
    /// Delete the stored key ("Clear stored key" pressed).
    Clear,
}

/// Dialog state owned by the shell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub open: bool,
    /// Draft provider (applied on Save).
    pub provider: AiProviderChoice,
    pub opencode_url: String,
    pub opencode_model: String,
    pub openrouter_model: String,
    /// API key field as typed. Empty = keep the stored key (the field never
    /// shows the stored secret — the keyring read only reports its presence).
    pub api_key: String,
    /// Draft flag: the stored key is deleted on Save.
    pub clear_key: bool,
    /// `Some(true/false)` = the stored key exists / does not (keyring probed
    /// when the dialog opened); `None` = probe failed or still in flight.
    pub has_key: Option<bool>,
    /// True between an accepted Save and the completion message.
    pub saving: bool,
    pub error: Option<String>,
}

/// Opens the dialog with drafts seeded from the persisted settings (shell
/// entry point — like [`State::open`] on the clone dialog but config-fed).
pub fn open(state: &mut State, ai: &AiSettings) {
    *state = State {
        open: true,
        provider: ai.provider,
        opencode_url: ai.opencode_url.clone().unwrap_or_default(),
        opencode_model: ai.opencode_model.clone().unwrap_or_default(),
        openrouter_model: ai.openrouter_model.clone().unwrap_or_default(),
        ..State::default()
    };
}

/// Messages the shell forwards to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Dismiss the dialog (Cancel, ×, scrim press). Ignored while saving.
    Close,
    ProviderSelected(AiProviderChoice),
    OpencodeUrlChanged(String),
    OpencodeModelChanged(String),
    OpenrouterModelChanged(String),
    /// Keystrokes into the API key field (un-arms a pending Clear).
    ApiKeyChanged(String),
    /// "Clear stored key": deletes the stored key on Save.
    ClearApiKeyPressed,
    /// Keyring probe result (`None` = probe failed).
    KeyStatusLoaded(Option<bool>),
    /// Master switch checkbox — reuses the shell's `AiToggleEnabled` arm.
    EnabledToggled,
    /// Per-repo checkbox — reuses the shell's `AiToggleRepoOptIn` arm.
    OptInToggled,
    /// Apply the drafts: config fields + keyring edit.
    Save,
    /// Keyring write finished (`Err` keeps the dialog open with the error).
    SaveDone(Result<(), String>),
}

/// Intents the shell translates into tasks / shell messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The dialog dismissed itself (Cancel/×; `open` is already false).
    Close,
    /// Flip the master switch (shell reuses its `AiToggleEnabled` arm).
    ToggleMaster,
    /// Flip the active repo's opt-in (shell reuses `AiToggleRepoOptIn`).
    ToggleOptIn,
    Save {
        provider: AiProviderChoice,
        /// `None` = managed opencode (no attach URL).
        opencode_url: Option<String>,
        /// `None` = provider default.
        opencode_model: Option<String>,
        /// `None` = backend default.
        openrouter_model: Option<String>,
        api_key: ApiKeyEdit,
    },
}

/// Steps the dialog; returns the intent for the shell to translate, if any.
pub fn update(state: &mut State, msg: Msg) -> Option<Action> {
    match msg {
        Msg::Close => {
            if state.saving {
                return None; // a save is in flight; let it finish
            }
            state.open = false;
            Some(Action::Close)
        }
        Msg::ProviderSelected(provider) => {
            state.provider = provider;
            None
        }
        Msg::OpencodeUrlChanged(url) => {
            state.opencode_url = url;
            None
        }
        Msg::OpencodeModelChanged(model) => {
            state.opencode_model = model;
            None
        }
        Msg::OpenrouterModelChanged(model) => {
            state.openrouter_model = model;
            None
        }
        Msg::ApiKeyChanged(key) => {
            state.api_key = key;
            state.clear_key = false;
            None
        }
        Msg::ClearApiKeyPressed => {
            state.clear_key = true;
            state.api_key.clear();
            None
        }
        Msg::KeyStatusLoaded(has_key) => {
            state.has_key = has_key;
            None
        }
        Msg::EnabledToggled => Some(Action::ToggleMaster),
        Msg::OptInToggled => Some(Action::ToggleOptIn),
        Msg::Save if !state.saving => {
            state.saving = true;
            state.error = None;
            let api_key = if state.clear_key {
                ApiKeyEdit::Clear
            } else {
                match state.api_key.trim() {
                    "" => ApiKeyEdit::Keep,
                    key => ApiKeyEdit::Set(key.to_string()),
                }
            };
            Some(Action::Save {
                provider: state.provider,
                opencode_url: trim_some(&state.opencode_url),
                opencode_model: trim_some(&state.opencode_model),
                openrouter_model: trim_some(&state.openrouter_model),
                api_key,
            })
        }
        Msg::Save => None,
        Msg::SaveDone(Ok(())) => {
            state.saving = false;
            state.open = false;
            state.api_key.clear();
            state.clear_key = false;
            None
        }
        Msg::SaveDone(Err(error)) => {
            state.saving = false;
            state.error = Some(error);
            None
        }
    }
}

/// `Some(trimmed)` for non-blank text, `None` for blank (persisted `None`
/// means "use the default").
fn trim_some(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

// ---------------------------------------------------------------------------
// Header health chip (derived; no polling beyond service::ai::ai_status)
// ---------------------------------------------------------------------------

/// Honest, coarse AI health for the header chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiHealth {
    /// Master switch off (an old error does not override this).
    Off,
    /// Enabled and the provider has what it needs.
    Ready,
    /// Enabled but the provider lacks its key/binary/URL.
    Unconfigured,
    /// The status probe has not landed yet (brief boot/save window).
    Unknown,
    /// The last AI call failed (text clipped by [`ai_health_label`]).
    Error(String),
}

/// Derives the chip state: master switch first, then the last AI call
/// result, then the provider's configuration (from the one-shot
/// `ai_status` probe — `None` = not probed yet).
pub fn ai_health(enabled: bool, status: Option<&AiStatus>, last_error: Option<&str>) -> AiHealth {
    if !enabled {
        return AiHealth::Off;
    }
    if let Some(error) = last_error.map(str::trim).filter(|error| !error.is_empty()) {
        return AiHealth::Error(error.to_string());
    }
    let Some(status) = status else {
        return AiHealth::Unknown;
    };
    match status.provider {
        crate::ai::AiProvider::Opencode => match &status.opencode.detection {
            Some(detection) if !detection.installed => AiHealth::Unconfigured,
            // Installed (or not probed): the managed server auto-starts on
            // first use and attach URLs need nothing else.
            _ => AiHealth::Ready,
        },
        crate::ai::AiProvider::OpenRouter => {
            if status.openrouter.has_key {
                AiHealth::Ready
            } else {
                AiHealth::Unconfigured
            }
        }
    }
}

/// Header chip label for the state.
pub fn ai_health_label(health: &AiHealth) -> String {
    match health {
        AiHealth::Off => "AI off".to_string(),
        AiHealth::Ready => "AI ready".to_string(),
        AiHealth::Unconfigured => "AI not configured".to_string(),
        AiHealth::Unknown => "AI status…".to_string(),
        AiHealth::Error(error) => format!("AI error: {}", clip(error, 48)),
    }
}

/// Header chip colour bucket (theme token name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiHealthTone {
    Muted,
    Good,
    Warn,
    Bad,
}

/// Tone for the chip label (the shell maps it to theme tokens).
pub fn ai_health_tone(health: &AiHealth) -> AiHealthTone {
    match health {
        AiHealth::Off | AiHealth::Unknown => AiHealthTone::Muted,
        AiHealth::Ready => AiHealthTone::Good,
        AiHealth::Unconfigured => AiHealthTone::Warn,
        AiHealth::Error(_) => AiHealthTone::Bad,
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Overlay for the current state; layer above the workspace while
/// `state.open` is set (a zero-sized element when closed).
///
/// `enabled` / `repo_opt_in` are the live values behind the two reused
/// checkboxes (`None` = no repository tab open — the checkbox hides).
pub fn view(
    state: &State,
    tokens: &Tokens,
    enabled: bool,
    repo_opt_in: Option<bool>,
) -> Element<'static, Msg> {
    if !state.open {
        return Space::new().width(0).height(0).into();
    }
    // Own the tokens so style closures capture a `Copy` value instead of the
    // reference (keeping the element `'static`).
    let tokens = *tokens;

    let title = row![
        text("AI settings").size(14).font(theme::semibold()),
        Space::new().width(Length::Fill),
        theme::mini_button(
            tokens,
            theme::icon::CLOSE,
            "Close dialog",
            (!state.saving).then_some(Msg::Close),
        ),
    ]
    .align_y(Alignment::Center);

    let mut switches = column![checkbox(enabled)
        .label("Enable AI assist")
        .size(theme::TEXT_BODY)
        .text_size(theme::TEXT_SMALL)
        .spacing(4)
        .on_toggle(|_| Msg::EnabledToggled),]
    .spacing(6);
    if let Some(opt_in) = repo_opt_in {
        switches = switches.push(
            checkbox(opt_in)
                .label("Opt in for this repository")
                .size(theme::TEXT_BODY)
                .text_size(theme::TEXT_SMALL)
                .spacing(4)
                .on_toggle(|_| Msg::OptInToggled),
        );
    }

    let mut provider = column![text("Provider")
        .size(theme::TEXT_SMALL)
        .color(tokens.muted_foreground)]
    .spacing(3)
    .width(Length::Fill);
    for (choice, label, description) in [
        (
            AiProviderChoice::Opencode,
            "opencode",
            "local server — managed by the app or attached by URL",
        ),
        (
            AiProviderChoice::OpenRouter,
            "openrouter",
            "direct API access with your key (stored in the OS keyring)",
        ),
    ] {
        provider = provider.push(option_line(
            tokens,
            state.provider == choice,
            label,
            description,
            Msg::ProviderSelected(choice),
        ));
    }

    let provider_fields: Element<'static, Msg> = match state.provider {
        AiProviderChoice::Opencode => column![
            labeled_field(
                tokens,
                "opencode server URL (empty = managed by the app)",
                text_input("http://127.0.0.1:4096", state.opencode_url.as_str())
                    .on_input(Msg::OpencodeUrlChanged)
                    .size(theme::TEXT_BODY)
                    .padding([5, 8]),
            ),
            labeled_field(
                tokens,
                "opencode model (empty = server default)",
                text_input("provider/model", state.opencode_model.as_str())
                    .on_input(Msg::OpencodeModelChanged)
                    .size(theme::TEXT_BODY)
                    .padding([5, 8]),
            ),
        ]
        .spacing(8)
        .into(),
        AiProviderChoice::OpenRouter => {
            let mut section = column![
                labeled_field(
                    tokens,
                    "OpenRouter model (empty = backend default)",
                    text_input("vendor/model", state.openrouter_model.as_str())
                        .on_input(Msg::OpenrouterModelChanged)
                        .size(theme::TEXT_BODY)
                        .padding([5, 8]),
                ),
                labeled_field(
                    tokens,
                    "API key (stored in the OS keyring)",
                    text_input(
                        "paste a new key — empty keeps the stored one",
                        state.api_key.as_str()
                    )
                    .secure(true)
                    .on_input(Msg::ApiKeyChanged)
                    .size(theme::TEXT_BODY)
                    .padding([5, 8]),
                ),
            ]
            .spacing(8);
            section = section.push(key_status_row(state, tokens));
            section.into()
        }
    };

    let mut body = column![title, switches, provider, provider_fields]
        .spacing(10)
        .width(Length::Fill);

    if let Some(error) = &state.error {
        body = body.push(
            text(error.clone())
                .size(theme::TEXT_SMALL)
                .color(tokens.deleted),
        );
    }

    let actions = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(theme::TEXT_SMALL))
            .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
            .padding([4, 12])
            .on_press_maybe((!state.saving).then_some(Msg::Close)),
        button(text(if state.saving { "Saving…" } else { "Save" }).size(theme::TEXT_SMALL))
            .style(theme::button_style(tokens, theme::ButtonKind::Primary))
            .padding([4, 12])
            .on_press_maybe((!state.saving).then_some(Msg::Save)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    // Shared modal scaffold: scrim press closes (the update arm already
    // swallows Close while a save is in flight), inputs consume theirs.
    modal_with_cancel(tokens, CARD_WIDTH, body.into(), actions.into(), Msg::Close)
}

/// Key presence indicator + clear button (openrouter only). The stored key
/// itself is never displayed.
fn key_status_row(state: &State, tokens: Tokens) -> Element<'static, Msg> {
    let status = match state.has_key {
        None => "checking keyring…".to_string(),
        Some(true) if state.clear_key => "stored (clears on Save)".to_string(),
        Some(true) => "stored".to_string(),
        Some(false) => "not set".to_string(),
    };
    let mut row = row![
        text(format!("Keyring: {status}"))
            .size(theme::TEXT_META)
            .color(tokens.muted_foreground),
        Space::new().width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if state.has_key == Some(true) && !state.clear_key {
        row = row.push(
            button(text("Clear stored key").size(theme::TEXT_META))
                .style(theme::button_style(tokens, theme::ButtonKind::Secondary))
                .padding([2, 8])
                .on_press_maybe((!state.saving).then_some(Msg::ClearApiKeyPressed)),
        );
    }
    row.into()
}

/// One radio-style line of the provider picker (mirrors dialog.rs's
/// option_line, owned here to keep the module's message type).
fn option_line<'a>(
    tokens: Tokens,
    selected: bool,
    label: &'a str,
    description: &'a str,
    on_press: Msg,
) -> Element<'static, Msg> {
    let label = label.to_string();
    let description = description.to_string();
    button(
        row![
            text(if selected { "●" } else { "○" })
                .size(theme::TEXT_META)
                .color(if selected {
                    tokens.primary
                } else {
                    tokens.muted_foreground
                }),
            text(label).size(theme::TEXT_SMALL).color(tokens.foreground),
            text(description)
                .size(theme::TEXT_META)
                .color(tokens.muted_foreground),
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
    .on_press(on_press)
    .into()
}

/// Label above a field, following the dialog spacing vocabulary.
fn labeled_field(
    tokens: Tokens,
    label: &str,
    field: impl Into<Element<'static, Msg>>,
) -> Element<'static, Msg> {
    let label = label.to_string();
    column![
        text(label)
            .size(theme::TEXT_SMALL)
            .color(tokens.muted_foreground),
        field.into(),
    ]
    .spacing(3)
    .width(Length::Fill)
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
    use super::*;

    fn state_with(fields: impl FnOnce(&mut State)) -> State {
        let mut state = State::default();
        fields(&mut state);
        state
    }

    fn settings(provider: AiProviderChoice) -> AiSettings {
        AiSettings {
            enabled: true,
            provider,
            opencode_url: Some("http://127.0.0.1:4096".into()),
            opencode_model: Some("anthropic/claude".into()),
            openrouter_model: Some("vendor/model".into()),
            ..AiSettings::default()
        }
    }

    #[test]
    fn open_seeds_drafts_from_settings_and_resets_secrets() {
        let mut state = state_with(|s| {
            s.api_key = "leftover".into();
            s.clear_key = true;
            s.error = Some("stale".into());
            s.saving = true;
        });
        open(&mut state, &settings(AiProviderChoice::OpenRouter));
        assert!(state.open);
        assert_eq!(state.provider, AiProviderChoice::OpenRouter);
        assert_eq!(state.opencode_url, "http://127.0.0.1:4096");
        assert_eq!(state.opencode_model, "anthropic/claude");
        assert_eq!(state.openrouter_model, "vendor/model");
        assert_eq!(state.api_key, "", "the key field never carries secrets");
        assert!(!state.clear_key);
        assert_eq!(state.error, None);
        assert!(!state.saving);
        // Unset fields read back as empty strings (draft → None on Save).
        open(&mut state, &AiSettings::default());
        assert_eq!(state.provider, AiProviderChoice::Opencode);
        assert_eq!(state.opencode_url, "");
    }

    #[test]
    fn close_is_ignored_while_saving_and_works_afterwards() {
        let mut state = state_with(|s| {
            s.open = true;
            s.saving = true;
        });
        assert_eq!(update(&mut state, Msg::Close), None);
        assert!(state.open);

        state.saving = false;
        assert_eq!(update(&mut state, Msg::Close), Some(Action::Close));
        assert!(!state.open);
    }

    #[test]
    fn save_carries_trimmed_fields_and_blank_become_none() {
        let mut state = state_with(|s| {
            s.provider = AiProviderChoice::OpenRouter;
            s.opencode_url = "  http://host:4096  ".into();
            s.opencode_model = "   ".into();
            s.openrouter_model = " vendor/model ".into();
        });
        assert_eq!(
            update(&mut state, Msg::Save),
            Some(Action::Save {
                provider: AiProviderChoice::OpenRouter,
                opencode_url: Some("http://host:4096".into()),
                opencode_model: None,
                openrouter_model: Some("vendor/model".into()),
                api_key: ApiKeyEdit::Keep,
            })
        );
        assert!(state.saving);
        // While saving, Save is a no-op.
        assert_eq!(update(&mut state, Msg::Save), None);
    }

    #[test]
    fn save_maps_api_key_edits() {
        // Empty field + no clear pressed: keep the stored key.
        let mut state = State::default();
        assert!(matches!(
            update(&mut state, Msg::Save),
            Some(Action::Save {
                api_key: ApiKeyEdit::Keep,
                ..
            })
        ));
        // Typed value: Set (trimmed).
        let mut state = state_with(|s| s.api_key = "  sk-or-1  ".into());
        assert!(matches!(
            update(&mut state, Msg::Save),
            Some(Action::Save {
                api_key: ApiKeyEdit::Set(key),
                ..
            }) if key == "sk-or-1"
        ));
        // Clear pressed: Clear wins, and typing again un-arms it.
        let mut state = state_with(|s| {
            s.api_key = "typed".into();
            s.clear_key = true;
        });
        assert!(matches!(
            update(&mut state, Msg::Save),
            Some(Action::Save {
                api_key: ApiKeyEdit::Clear,
                ..
            })
        ));
        assert_eq!(
            update(&mut state, Msg::ApiKeyChanged("n".into())),
            None,
            "typing is not an action"
        );
        assert!(!state.clear_key);
        // A fresh dialog (the first Save set `saving`): typing "n" submits
        // as Set again.
        let mut state = state_with(|s| s.api_key = "n".into());
        assert!(matches!(
            update(&mut state, Msg::Save),
            Some(Action::Save {
                api_key: ApiKeyEdit::Set(key),
                ..
            }) if key == "n"
        ));
    }

    #[test]
    fn clear_key_press_arms_and_key_status_lands() {
        let mut state = state_with(|s| s.has_key = Some(true));
        assert_eq!(update(&mut state, Msg::ClearApiKeyPressed), None);
        assert!(state.clear_key);
        assert_eq!(state.api_key, "");
        assert_eq!(update(&mut state, Msg::KeyStatusLoaded(Some(false))), None);
        assert_eq!(state.has_key, Some(false));
    }

    #[test]
    fn toggles_are_intents_not_draft_writes() {
        let mut state = State::default();
        assert_eq!(
            update(&mut state, Msg::EnabledToggled),
            Some(Action::ToggleMaster)
        );
        assert_eq!(
            update(&mut state, Msg::OptInToggled),
            Some(Action::ToggleOptIn)
        );
    }

    #[test]
    fn save_done_success_closes_and_failure_stays_open() {
        let mut state = state_with(|s| {
            s.saving = true;
            s.open = true;
        });
        assert_eq!(update(&mut state, Msg::SaveDone(Ok(()))), None);
        assert!(!state.saving);
        assert!(!state.open);

        let mut state = state_with(|s| {
            s.saving = true;
            s.open = true;
        });
        assert_eq!(
            update(&mut state, Msg::SaveDone(Err("keyring unavailable".into()))),
            None
        );
        assert!(!state.saving);
        assert!(state.open, "failed saves keep the dialog for a retry");
        assert_eq!(state.error.as_deref(), Some("keyring unavailable"));
    }

    // -- health chip derivation ------------------------------------------------

    fn ai_status(provider: crate::ai::AiProvider, detection_installed: Option<bool>) -> AiStatus {
        let detection = detection_installed.map(|installed| crate::opencode::OpencodeDetection {
            installed,
            path: None,
            version: None,
            major: None,
        });
        AiStatus {
            enabled: true,
            provider,
            model: None,
            opencode: crate::service::ai::AiOpencodeStatus {
                detection,
                managed: crate::opencode::OpencodeServeState::default(),
                health: None,
                url: None,
            },
            openrouter: crate::service::ai::AiOpenrouterStatus { has_key: true },
        }
    }

    #[test]
    fn health_off_wins_over_everything() {
        let status = ai_status(crate::ai::AiProvider::OpenRouter, None);
        assert_eq!(ai_health(false, Some(&status), Some("boom")), AiHealth::Off);
        assert_eq!(ai_health(false, None, None), AiHealth::Off);
    }

    #[test]
    fn health_error_carries_the_last_failure_text() {
        assert_eq!(
            ai_health(true, None, Some("  quota exceeded ")),
            AiHealth::Error("quota exceeded".into())
        );
        // Blank errors do not count as a failure.
        assert_eq!(ai_health(true, None, Some("   ")), AiHealth::Unknown);
    }

    #[test]
    fn health_needs_the_status_probe_before_claiming_ready() {
        assert_eq!(ai_health(true, None, None), AiHealth::Unknown);
    }

    #[test]
    fn health_openrouter_needs_the_keyring_key() {
        let with_key = ai_status(crate::ai::AiProvider::OpenRouter, None);
        assert_eq!(ai_health(true, Some(&with_key), None), AiHealth::Ready);
        let mut without_key = ai_status(crate::ai::AiProvider::OpenRouter, None);
        without_key.openrouter.has_key = false;
        assert_eq!(
            ai_health(true, Some(&without_key), None),
            AiHealth::Unconfigured
        );
    }

    #[test]
    fn health_opencode_needs_the_binary_but_not_a_running_server() {
        let installed = ai_status(crate::ai::AiProvider::Opencode, Some(true));
        assert_eq!(ai_health(true, Some(&installed), None), AiHealth::Ready);
        let missing = ai_status(crate::ai::AiProvider::Opencode, Some(false));
        assert_eq!(
            ai_health(true, Some(&missing), None),
            AiHealth::Unconfigured
        );
        // Not probed yet: assume ready (the managed server auto-starts).
        let unprobed = ai_status(crate::ai::AiProvider::Opencode, None);
        assert_eq!(ai_health(true, Some(&unprobed), None), AiHealth::Ready);
    }

    #[test]
    fn health_labels_and_tones() {
        assert_eq!(ai_health_label(&AiHealth::Off), "AI off");
        assert_eq!(ai_health_label(&AiHealth::Ready), "AI ready");
        assert_eq!(
            ai_health_label(&AiHealth::Unconfigured),
            "AI not configured"
        );
        assert_eq!(ai_health_label(&AiHealth::Unknown), "AI status…");
        assert_eq!(
            ai_health_label(&AiHealth::Error("quota exceeded".into())),
            "AI error: quota exceeded"
        );
        assert_eq!(ai_health_tone(&AiHealth::Off), AiHealthTone::Muted);
        assert_eq!(ai_health_tone(&AiHealth::Ready), AiHealthTone::Good);
        assert_eq!(ai_health_tone(&AiHealth::Unconfigured), AiHealthTone::Warn);
        assert_eq!(
            ai_health_tone(&AiHealth::Error("x".into())),
            AiHealthTone::Bad
        );
    }

    #[test]
    fn health_error_label_clips_long_text() {
        let long = "x".repeat(120);
        let label = ai_health_label(&AiHealth::Error(long));
        assert!(label.starts_with("AI error: "));
        assert!(label.chars().count() < 70);
        assert!(label.ends_with('…'));
    }
}
