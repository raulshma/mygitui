//! AI provider layer: one generation surface over two backends.
//!
//! Ported from the old Svelte webview's `src/lib/ai/*` (the TS is the spec):
//!   - `opencode`   — a local opencode server (1.x "v1" sync prompt round
//!     trip with per-repo sessions, 2.x "v2" temp-session generation with an
//!     `/api/event` wait). Health probing strictly identifies a server by
//!     decoding the body — a 200 alone does not make an opencode server.
//!   - `openrouter` — direct OpenRouter chat/completions with the user's API
//!     key (OS keyring), non-streaming.
//!
//! Secrets (`AiConfig::opencode_password`, `AiConfig::openrouter_api_key`)
//! live in memory only, are filled in per call by [`crate::service::ai`]
//! from the OS keyring / serve registry, and never appear in `Debug` or
//! error strings. Non-secret settings persist in `config.json` (see
//! [`crate::ui::config::AiSettings`]).
//!
//! Retries: [`retry::with_transport_retries`] is the single retry owner —
//! it retries ONLY transport-class failures with jittered exponential
//! backoff. A failed generation surfaces to the caller; nothing replays
//! user-visible mutations. (A retried generation repeats the prompt
//! server-side — an extra session message or an orphaned temp session at
//! worst — and never touches repository state, which is why replaying it
//! is safe; do not reuse the helper for genuinely mutating calls.)

mod budget;
mod opencode;
mod openrouter;
mod prompts;
mod retry;
mod sse;

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use budget::{budgeted_diff_text, is_generated_path, DiffBudget, MAX_DIFF_CHARS};
pub use opencode::{
    basic_auth_header, decode_health_v1, decode_info_v2, discover, is_json_content_type,
    normalize_directory, parse_model_slug, parts_to_text, probe, reset_v1_sessions, Discovery,
    Health, HealthKind, DEFAULT_OPENCODE_URL, HEALTH_TIMEOUT, SESSION_TITLE, V2_GENERATION_TIMEOUT,
};
pub use openrouter::{DEFAULT_OPENROUTER_MODEL, OPENROUTER_API_BASE};
pub use prompts::{
    cap_diff, diff_summary_text, format_file_diff, parse_branch_name, parse_commit_message,
    prompts, staged_diff_text, CommitRef, PromptContext,
};
pub use retry::{
    backoff_delay, with_transport_retries, DEFAULT_RETRY_ATTEMPTS, RETRY_BASE, RETRY_CAP,
};
pub use sse::{decode_opencode_event, parse_sse, parse_sse_frame, SseDecoder, SSE_DONE};

/// Which backend a generation goes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    /// A local opencode server (managed by the serve registry or attached).
    #[default]
    Opencode,
    /// Direct OpenRouter API access with the user's API key.
    OpenRouter,
}

/// The AI features mygitui can run (the six from the old UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiFeature {
    /// Conventional-commits message from the staged diff.
    CommitMessage,
    /// Pull request title + description from branch commits + diff stats.
    PrTitleBody,
    /// Explanation of one diff hunk.
    ExplainHunk,
    /// Review of the whole staged diff.
    ReviewStaged,
    /// Short stash message for parked work-in-progress.
    StashMessage,
    /// Kebab-case branch name suggestion.
    BranchName,
}

/// Runtime configuration for one generation round trip. Built per call by
/// the service layer; secrets are in-memory only (never persisted, never
/// logged — see the module docs).
#[derive(Clone, Default)]
pub struct AiConfig {
    /// Backend the request goes to.
    pub provider: AiProvider,
    /// opencode server base URL (already resolved by the service layer:
    /// managed registry URL or the manual attach URL). `None` probes
    /// [`DEFAULT_OPENCODE_URL`].
    pub opencode_url: Option<String>,
    /// opencode basic-auth password (`Authorization: Basic
    /// base64("opencode:" + password)`), from the keyring or the managed
    /// serve registry. In-memory only.
    pub opencode_password: Option<String>,
    /// OpenRouter API key (keyring). In-memory only.
    pub openrouter_api_key: Option<String>,
    /// Model override: opencode `provider/model` slug or OpenRouter slug.
    /// `None` uses the backend default.
    pub model: Option<String>,
    /// Repository workspace directory handed to opencode (normalized to
    /// forward slashes — 1.x mis-parses backslash paths). In-memory only.
    pub directory: Option<String>,
}

impl fmt::Debug for AiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Secrets are redacted: a dumped config must never carry them.
        f.debug_struct("AiConfig")
            .field("provider", &self.provider)
            .field("opencode_url", &self.opencode_url)
            .field(
                "opencode_password",
                &self.opencode_password.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "openrouter_api_key",
                &self.openrouter_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field("model", &self.model)
            .field("directory", &self.directory)
            .finish()
    }
}

/// One generation request (prompt builders own the text; this owns routing).
#[derive(Debug, Clone)]
pub struct GenRequest {
    /// Which feature is running (labels errors; no behavior of its own).
    pub feature: AiFeature,
    /// System prompt (instructions); omitted on the wire when empty.
    pub system: String,
    /// The user prompt (data + task).
    pub prompt: String,
    /// Output token cap (OpenRouter `max_tokens` only; opencode has none).
    pub max_tokens: Option<u32>,
    /// Session affinity key (the repo id): opencode v1 reuses one server
    /// session per key; OpenRouter and v2 temp sessions ignore it.
    pub session_key: String,
}

/// Uniform failure for anything AI: kind + a safe message (never secret
/// material). The service layer flattens this to its `String` convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    /// Backend not reachable / not running / timed out.
    Unavailable(String),
    /// Credentials missing or rejected (never names the secret).
    Unauthenticated(String),
    /// Backend answered but unusably (parse error, empty, non-2xx).
    BadResponse(String),
    /// The request is missing input the caller must provide.
    InvalidInput(String),
    /// AI is disabled or the repository has not opted in.
    OptIn(String),
    /// The request was cancelled.
    Cancelled,
}

impl AiError {
    /// The safe, secret-free message.
    pub fn message(&self) -> &str {
        match self {
            AiError::Unavailable(m)
            | AiError::Unauthenticated(m)
            | AiError::BadResponse(m)
            | AiError::InvalidInput(m)
            | AiError::OptIn(m) => m,
            AiError::Cancelled => "AI request cancelled",
        }
    }

    /// True for transport-class failures (backend unreachable/timed out) —
    /// the only class the single retry owner may retry.
    pub fn is_transport(&self) -> bool {
        matches!(self, AiError::Unavailable(_))
    }
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Hand-written Display (repo convention for typed errors avoids the
        // derive here): kind prefix + safe message.
        let kind = match self {
            AiError::Unavailable(_) => "unavailable",
            AiError::Unauthenticated(_) => "unauthenticated",
            AiError::BadResponse(_) => "bad-response",
            AiError::InvalidInput(_) => "invalid-input",
            AiError::OptIn(_) => "opt-in",
            AiError::Cancelled => "cancelled",
        };
        write!(f, "{kind}: {}", self.message())
    }
}

impl std::error::Error for AiError {}

/// Routes one generation through the configured backend.
pub async fn generate(req: GenRequest, cfg: &AiConfig) -> Result<String, AiError> {
    match cfg.provider {
        AiProvider::Opencode => opencode::generate(req, cfg).await,
        AiProvider::OpenRouter => openrouter::generate(req, cfg).await,
    }
}

/// Shared HTTP client (connection pooling; per-request timeouts are set by
/// the callers — the probe is 2s, generation minutes).
fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Default uniform [0, 1) jitter source for [`retry::backoff_delay`]
/// (dependency-free: 53 bits from a random v4 UUID).
pub fn default_jitter() -> f64 {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let hi = u64::from_be_bytes(bytes[..8].try_into().expect("8 bytes"));
    (hi >> 11) as f64 / (1u64 << 53) as f64
}

/// Convenience wrapper so callers do not re-declare the sleep generic for
/// the production sleep (tokio timer).
pub async fn sleep_real(delay: Duration) {
    tokio::time::sleep(delay).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_error_display_is_kind_plus_safe_message() {
        let err = AiError::Unavailable("server down".into());
        assert_eq!(err.to_string(), "unavailable: server down");
        assert_eq!(
            AiError::Cancelled.to_string(),
            "cancelled: AI request cancelled"
        );
        assert_eq!(AiError::BadResponse("bad".into()).message(), "bad");
    }

    #[test]
    fn transport_classification_gates_retries() {
        assert!(AiError::Unavailable("x".into()).is_transport());
        assert!(!AiError::Unauthenticated("x".into()).is_transport());
        assert!(!AiError::BadResponse("x".into()).is_transport());
        assert!(!AiError::InvalidInput("x".into()).is_transport());
        assert!(!AiError::OptIn("x".into()).is_transport());
        assert!(!AiError::Cancelled.is_transport());
    }

    #[test]
    fn ai_config_debug_redacts_secrets() {
        let cfg = AiConfig {
            provider: AiProvider::Opencode,
            opencode_password: Some("hunter2".into()),
            openrouter_api_key: Some("sk-or-123".into()),
            opencode_url: Some("http://127.0.0.1:4096".into()),
            ..Default::default()
        };
        let dump = format!("{cfg:?}");
        assert!(!dump.contains("hunter2"), "password leaked: {dump}");
        assert!(!dump.contains("sk-or-123"), "key leaked: {dump}");
        assert!(dump.contains("[redacted]"));
    }

    #[test]
    fn default_jitter_is_uniform_in_unit_range() {
        for _ in 0..32 {
            let j = default_jitter();
            assert!((0.0..1.0).contains(&j), "jitter out of range: {j}");
        }
        assert_ne!(default_jitter(), default_jitter(), "must not be constant");
    }

    #[test]
    fn feature_and_provider_serde_names_stay_stable() {
        assert_eq!(
            serde_json::to_string(&AiFeature::CommitMessage).unwrap(),
            "\"commit-message\""
        );
        assert_eq!(
            serde_json::to_string(&AiFeature::PrTitleBody).unwrap(),
            "\"pr-title-body\""
        );
        assert_eq!(
            serde_json::to_string(&AiProvider::Opencode).unwrap(),
            "\"opencode\""
        );
        assert_eq!(
            serde_json::from_str::<AiProvider>("\"openrouter\"").unwrap(),
            AiProvider::OpenRouter
        );
    }
}
