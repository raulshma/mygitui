//! Credential broker: bridges git2 credential callbacks to the keyring and,
//! when missing, to the frontend via the `auth-request` event + `auth_respond`
//! command round-trip. Lane C3 owns completing this; netops calls `broker()`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

/// Answer to a pending auth request (from `auth_respond`).
pub struct AuthAnswer {
    pub username: Option<String>,
    pub password: Option<String>,
    pub store: bool,
}

/// Global credential state: cached keyring hits + pending FE requests.
pub struct AuthBroker {
    /// url-host → (user, pass) cache filled from keyring or FE answers.
    cache: Mutex<HashMap<String, (String, String)>>,
    /// op_id → sender woken by `auth_respond`.
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<AuthAnswer>>>,
}

impl AuthBroker {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }
}

static BROKER: OnceLock<Arc<AuthBroker>> = OnceLock::new();

/// Process-wide broker (set up by lib.rs at startup).
pub fn broker() -> &'static Arc<AuthBroker> {
    BROKER.get_or_init(|| Arc::new(AuthBroker::new()))
}

impl AuthBroker {
    /// C3: register a pending FE request; returns the receiver to await.
    pub fn register_pending(
        &self,
        _op_id: String,
    ) -> Option<tokio::sync::oneshot::Receiver<AuthAnswer>> {
        // Completed by C3.
        None
    }

    /// C3: `auth_respond` lands here.
    pub fn answer(&self, _op_id: String, _answer: AuthAnswer) -> bool {
        // Completed by C3.
        false
    }

    /// C3: keyring lookup helper ("https:<host>" entries).
    pub fn keyring_lookup(&self, _host: &str) -> Option<(String, String)> {
        // Completed by C3.
        None
    }

    /// C2 consumes this inside RemoteCallbacks::credentials.
    /// Stub: ssh-agent + plaintext only. C3 adds keyring + FE round-trip.
    pub fn credentials(&self) -> Box<git2::Credentials<'static>> {
        Box::new(
            move |_url: &str,
                  username: Option<&str>,
                  allowed: git2::CredentialType|
                  -> Result<git2::Cred, git2::Error> {
                if allowed.contains(git2::CredentialType::SSH_KEY) {
                    return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
                }
                if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                    let user = username
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| "git".to_string());
                    return git2::Cred::userpass_plaintext(&user, "");
                }
                Err(git2::Error::from_str("no credential source available"))
            },
        )
    }
}
