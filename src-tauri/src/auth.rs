//! Credential broker: bridges git2 credential callbacks to the keyring and,
//! when missing, to the frontend via the `auth-request` event +
//! `auth_respond` command round-trip. Lane C3 owns this file; netops calls
//! `broker().credentials()`.
//!
//! Callback chain per git2 invocation (per-URL attempt counter):
//!
//! 1. **ssh-agent** — `Cred::ssh_key_from_agent`, never interactive.
//! 2. **HTTPS cache/keyring** — in-process per-host cache, filled from the
//!    OS keyring (`https:<host>:user` / `https:<host>:pass` entries under the
//!    `mygitui` service, read through the keyring crate directly — not the
//!    `secrets_*` IPC commands).
//! 3. **git credential helpers** — `git credential fill` against whatever
//!    `credential.helper` the user configured (Git Credential Manager, …);
//!    non-interactive, so a miss falls through. Rejections call `git
//!    credential reject`; "remember me" answers also call `approve` so cmd
//!    git shares the credential. See `credential_helper.rs`.
//! 4. **FE round-trip** — emit `auth-request` `{op_id, repo_id, url, kind,
//!    prompt}` (contracts.md M2) and block on a tokio oneshot with a 60s
//!    timeout. `store: true` answers persist back to the keyring + cache
//!    (+ helpers via approve).
//!
//! libgit2 retries the callback after a rejected credential: attempt 2
//! clears the host's cache and re-prompts once; attempt 3 fails the op.
//!
//! Interactive prompting sits behind the [`Prompter`] seam: production uses
//! [`FePrompter`] (emit + block), unit tests inject a fake. Besides
//! testability this keeps tauri's `Emitter for AppHandle<Wry>`
//! monomorphization — and with it the tao/muda window code whose
//! `TaskDialogIndirect` import needs the comctl32 v6 SxS manifest that app
//! binaries embed but bare test exes lack (STATUS_ENTRYPOINT_NOT_FOUND on
//! Windows) — out of the test binary.
//!
//! The credentials callback runs synchronously on a blocking thread
//! (net-op worker). The FE wait bridges the tokio oneshot through a bare
//! std thread + `recv_timeout` so it works from any blocking context.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::credential_helper;
use crate::engine::types::RepoId;

/// `auth-request` event name (contracts.md, Events M2).
pub const AUTH_REQUEST_EVENT: &str = "auth-request";

/// How long the credentials callback waits for `auth_respond` before
/// giving up (the op fails with a timeout error; nothing stays wedged).
const AUTH_TIMEOUT: Duration = Duration::from_secs(60);

/// Keyring service, same value as `keyring_store::SERVICE` (kept private
/// there, duplicated here — entries are read via the keyring crate
/// directly, not through the `secrets_*` commands).
const SERVICE: &str = "mygitui";

/// Prompt kinds (contracts.md, Events M2 — fixed strings).
pub const KIND_HTTPS_USER: &str = "https-user";
pub const KIND_HTTPS_PASS: &str = "https-pass";
pub const KIND_SSH_PASSPHRASE: &str = "ssh-passphrase";

/// Max credential attempts per URL before the op fails: one fresh try plus
/// one re-prompt after a rejection.
const MAX_ATTEMPTS: u32 = 2;

/// Answer to a pending auth request (from `auth_respond`).
pub struct AuthAnswer {
    pub username: Option<String>,
    pub password: Option<String>,
    pub store: bool,
}

/// `auth-request` event payload (contracts.md M2).
#[derive(Debug, Clone, Serialize)]
pub struct AuthRequestEvent {
    pub op_id: String,
    pub repo_id: String,
    pub url: String,
    /// "https-user" | "https-pass" | "ssh-passphrase".
    pub kind: String,
    pub prompt: String,
}

/// Interactive prompting seam (see module docs). `ask` may block the
/// calling (blocking) thread for up to [`AUTH_TIMEOUT`].
trait Prompter {
    fn ask(&self, url: &str, kind: &str, prompt: String) -> Result<AuthAnswer, String>;
}

/// Late-bound FE ask function (see [`FePrompter`]).
type AskFn = Box<dyn Fn(&str, &str, String) -> Result<AuthAnswer, String> + Send + Sync>;

/// Credential-helper lookup seam: production shells out to `git credential
/// fill` ([`credential_helper::fill`]); tests inject fakes to stay
/// hermetic (no git spawn, no host credential manager).
type HelperFillFn =
    Box<dyn Fn(&credential_helper::Query<'_>) -> Option<(String, String)> + Send + Sync>;

/// The production prompter, late-bound: [`init`] installs the closure that
/// performs the actual `auth-request` round-trip (`ask_fe`). Referencing
/// `ask_fe` only from inside that closure — installed at app startup, never
/// from tests — keeps tauri's `Emitter for AppHandle<Wry>` monomorphization
/// (and the tao/muda window code it links, whose `TaskDialogIndirect`
/// import needs the comctl32 v6 SxS manifest that bare test exes lack)
/// garbage-collected out of test binaries.
static FE_ASK: OnceLock<AskFn> = OnceLock::new();

/// Production prompter: delegates to the [`FE_ASK`] hook.
struct FePrompter;

impl Prompter for FePrompter {
    fn ask(&self, url: &str, kind: &str, prompt: String) -> Result<AuthAnswer, String> {
        match FE_ASK.get() {
            Some(ask) => ask(url, kind, prompt),
            None => {
                Err("interactive authentication unavailable: auth::init not called".to_string())
            }
        }
    }
}

/// Global credential state: cached keyring hits + pending FE requests.
pub struct AuthBroker {
    /// url-host → (user, pass) cache filled from keyring or FE answers.
    cache: Mutex<HashMap<String, (String, String)>>,
    /// Hosts whose most recently served credential came from git's helper
    /// store (fill hit or approve): only those get `git credential reject`
    /// when the remote refuses the credential.
    helper_served: Mutex<HashSet<String>>,
    /// op_id → sender woken by `auth_respond`.
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<AuthAnswer>>>,
    /// Set by `init` from lib.rs setup; `None` in unit tests / headless.
    app: RwLock<Option<tauri::AppHandle>>,
    counter: AtomicU64,
    /// `git credential fill` seam (see [`HelperFillFn`]).
    helper_fill: HelperFillFn,
}

impl AuthBroker {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            helper_served: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashMap::new()),
            app: RwLock::new(None),
            counter: AtomicU64::new(0),
            helper_fill: Box::new(credential_helper::fill),
        }
    }
}

static BROKER: OnceLock<Arc<AuthBroker>> = OnceLock::new();

/// Process-wide broker (set up by lib.rs at startup).
pub fn broker() -> &'static Arc<AuthBroker> {
    BROKER.get_or_init(|| Arc::new(AuthBroker::new()))
}

/// Give the broker the app handle used to emit `auth-request` events and
/// install the FE prompting hook. Called from lib.rs `.setup(...)`; never
/// from tests/headless (prompting then fails gracefully with "auth::init
/// not called").
pub fn init(app: tauri::AppHandle) {
    *broker().app.write() = Some(app);
    let _ = FE_ASK.set(Box::new(|url: &str, kind: &str, prompt: String| {
        broker().ask_fe(url, kind, prompt)
    }));
}

thread_local! {
    /// Repo whose net op is running on this thread; fills the `repo_id`
    /// field of auth-request events (libgit2 invokes credential callbacks
    /// inline on the op's thread). Set by `with_netop_context`.
    static NETOP_REPO: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with `repo_id` registered as this thread's net-op context so
/// credentials callbacks triggered inside `f` attribute their auth-request
/// events to the right repo. Reentrant (nested calls restore correctly):
/// the borrow is released around `f` because the RefCell cannot be
/// mutably borrowed twice.
pub fn with_netop_context<R>(repo_id: &RepoId, f: impl FnOnce() -> R) -> R {
    let previous = NETOP_REPO.with_borrow_mut(|slot| slot.replace(repo_id.0.clone()));
    let result = f();
    NETOP_REPO.with_borrow_mut(|slot| *slot = previous);
    result
}

fn current_repo_id() -> String {
    NETOP_REPO.with_borrow(|slot| slot.clone().unwrap_or_default())
}

impl AuthBroker {
    /// Register a pending FE request; returns the receiver to await.
    /// A duplicate `op_id` replaces (and cancels) the previous waiter.
    pub fn register_pending(
        &self,
        op_id: String,
    ) -> Option<tokio::sync::oneshot::Receiver<AuthAnswer>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().insert(op_id, tx);
        Some(rx)
    }

    /// `auth_respond` lands here. Returns `true` when a waiter was woken
    /// with the answer.
    pub fn answer(&self, op_id: String, answer: AuthAnswer) -> bool {
        match self.pending.lock().remove(&op_id) {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }

    /// Drop a pending request (timeout / emit failure); wakes its bridge
    /// thread with a cancellation.
    fn cancel_pending(&self, op_id: &str) {
        self.pending.lock().remove(op_id);
    }

    /// Keyring lookup ("https:<host>" entries). A hit also warms the
    /// in-process cache. Misses (no entry / no backend) return `None`.
    pub fn keyring_lookup(&self, host: &str) -> Option<(String, String)> {
        let pair = keyring_https_pair(host)?;
        self.cache.lock().insert(host.to_string(), pair.clone());
        Some(pair)
    }

    /// Credentials callback factory consumed by netops inside
    /// `RemoteCallbacks::credentials`. Tracks per-URL attempt counts so a
    /// rejected credential clears the cache and re-prompts once; a second
    /// rejection fails the operation.
    pub fn credentials(&self) -> Box<git2::Credentials<'static>> {
        let broker: Arc<AuthBroker> = broker().clone();
        let attempts: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
        Box::new(
            move |url: &str,
                  username: Option<&str>,
                  allowed: git2::CredentialType|
                  -> Result<git2::Cred, git2::Error> {
                let attempt_index = {
                    let mut seen = attempts.lock();
                    let count = seen.entry(url.to_string()).or_insert(0);
                    *count += 1;
                    *count
                };
                broker.attempt(&FePrompter, url, username, allowed, attempt_index)
            },
        )
    }

    /// One callback invocation (see module docs for the chain). The
    /// `prompter` seam carries the interactive step; unit tests drive this
    /// method directly with a fake.
    fn attempt(
        &self,
        prompter: &dyn Prompter,
        url: &str,
        username: Option<&str>,
        allowed: git2::CredentialType,
        attempt_index: u32,
    ) -> Result<git2::Cred, git2::Error> {
        let (protocol, host, path) = credential_helper::split_url(url);
        if attempt_index > MAX_ATTEMPTS {
            return Err(git2::Error::from_str(&format!(
                "authentication failed for {host} ({MAX_ATTEMPTS} attempts)"
            )));
        }
        let known_user = username.map(str::to_string);
        if attempt_index > 1 {
            // The previous credential was rejected: anything cached for
            // this host is stale. Drop it so we re-prompt, not loop. If it
            // came from git's helper store, erase it there too (what git
            // itself does on a rejected credential) so a dead token cannot
            // be served again.
            self.cache.lock().remove(&host);
            if self.helper_served.lock().remove(&host) {
                credential_helper::reject(&credential_helper::Query {
                    protocol: &protocol,
                    host: &host,
                    path: path.as_deref(),
                    username: known_user.as_deref(),
                });
            }
        }

        // Some servers ask for just a username (no password scheme).
        if allowed.contains(git2::CredentialType::USERNAME) && username.is_none() {
            return git2::Cred::username("git");
        }

        // 1. ssh-agent: non-interactive, always first for SSH.
        if allowed.contains(git2::CredentialType::SSH_KEY) && attempt_index == 1 {
            return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }

        // 2./3./4. HTTPS chain: in-process cache → keyring → git credential
        // helpers → FE round-trip.
        if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            if let Some((user, pass)) = self.cache.lock().get(&host).cloned() {
                return git2::Cred::userpass_plaintext(&user, &pass);
            }
            if attempt_index == 1 {
                // Keyring only on the first try: a rejection means the
                // stored pair is wrong, don't serve it twice.
                if let Some((user, pass)) = self.keyring_lookup(&host) {
                    return git2::Cred::userpass_plaintext(&user, &pass);
                }
                // Same for the user's git credential helpers (GCM & co) —
                // what cmd git would use. Non-interactive by design: a
                // miss falls through to the FE prompt.
                let query = credential_helper::Query {
                    protocol: &protocol,
                    host: &host,
                    path: path.as_deref(),
                    username: known_user.as_deref(),
                };
                if let Some((user, pass)) = (self.helper_fill)(&query) {
                    self.helper_served.lock().insert(host.clone());
                    self.cache
                        .lock()
                        .insert(host.clone(), (user.clone(), pass.clone()));
                    return git2::Cred::userpass_plaintext(&user, &pass);
                }
            }
            let kind = https_prompt_kind(known_user.is_some());
            let prompt = build_prompt(kind, &host, known_user.as_deref());
            let answer = prompter
                .ask(url, kind, prompt)
                .map_err(|reason| git2::Error::from_str(&reason))?;
            let (user, pass) = resolve_https_answer(&answer, known_user.as_deref())?;
            self.remember(&protocol, &host, &user, &pass, answer.store);
            return git2::Cred::userpass_plaintext(&user, &pass);
        }

        // Agent key rejected but the server takes interactive auth: prompt
        // for the default key's passphrase.
        if allowed.contains(git2::CredentialType::SSH_INTERACTIVE) {
            let prompt = build_prompt(KIND_SSH_PASSPHRASE, &host, Some(username.unwrap_or("git")));
            let answer = prompter
                .ask(url, KIND_SSH_PASSPHRASE, prompt)
                .map_err(|reason| git2::Error::from_str(&reason))?;
            let passphrase = answer
                .password
                .filter(|p| !p.is_empty())
                .ok_or_else(|| git2::Error::from_str("ssh passphrase not provided"))?;
            let key = default_ssh_key().ok_or_else(|| {
                git2::Error::from_str(
                    "no default ssh key found (tried ~/.ssh/id_ed25519, id_ecdsa, id_rsa)",
                )
            })?;
            // Passphrases are never persisted; session-only by design.
            return git2::Cred::ssh_key(username.unwrap_or("git"), None, &key, Some(&passphrase));
        }

        Err(git2::Error::from_str(&format!(
            "no credential source available for {host}"
        )))
    }

    /// Emit `auth-request` and block for `auth_respond`
    /// (≤ [`AUTH_TIMEOUT`]). Referenced only by the [`FE_ASK`] closure that
    /// [`init`] installs — never from unit tests (see module docs on
    /// linkage).
    fn ask_fe(&self, url: &str, kind: &str, prompt: String) -> Result<AuthAnswer, String> {
        let app = self.app.read().clone();
        let Some(app) = app else {
            return Err(
                "interactive authentication unavailable: no app handle (auth::init not called)"
                    .to_string(),
            );
        };
        use tauri::Emitter;

        let op_id = format!(
            "auth-{:x}-{:x}",
            std::process::id(),
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        // Register before emitting: an instant FE answer must find a waiter.
        let rx = self
            .register_pending(op_id.clone())
            .expect("register_pending always returns a receiver");
        let payload = AuthRequestEvent {
            op_id: op_id.clone(),
            repo_id: current_repo_id(),
            url: url.to_string(),
            kind: kind.to_string(),
            prompt,
        };
        if let Err(err) = app.emit(AUTH_REQUEST_EVENT, &payload) {
            self.cancel_pending(&op_id);
            return Err(format!("failed to emit auth-request: {err}"));
        }
        self.wait_for_answer(&op_id, rx)
    }

    /// Block the (by definition blocking) caller for up to
    /// [`AUTH_TIMEOUT`]. The tokio oneshot is drained in a bare std thread:
    /// `blocking_recv` panics inside async contexts, and this code may run
    /// on either the op-queue worker or a `spawn_blocking` thread.
    fn wait_for_answer(
        &self,
        op_id: &str,
        rx: tokio::sync::oneshot::Receiver<AuthAnswer>,
    ) -> Result<AuthAnswer, String> {
        let (tx, done) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(format!("auth-wait-{op_id}"))
            .spawn(move || {
                let _ = tx.send(rx.blocking_recv());
            })
            .map_err(|err| format!("failed to spawn auth wait thread: {err}"))?;
        match done.recv_timeout(AUTH_TIMEOUT) {
            Ok(Ok(answer)) => Ok(answer),
            // Waiter canceled (replaced/cleaned up).
            Ok(Err(_)) => Err("auth request canceled".to_string()),
            Err(_) => {
                // Drop the pending sender so the bridge thread unblocks.
                self.cancel_pending(op_id);
                Err(format!(
                    "timed out after {}s waiting for auth_respond",
                    AUTH_TIMEOUT.as_secs()
                ))
            }
        }
    }

    /// Store an answer: always the session cache, plus both keyring
    /// entries when the user asked to persist. Persisted credentials also
    /// go through `git credential approve`, so the user's own git (cmd git,
    /// IDEs) shares them.
    fn remember(&self, protocol: &str, host: &str, user: &str, pass: &str, store: bool) {
        self.cache
            .lock()
            .insert(host.to_string(), (user.to_string(), pass.to_string()));
        if !store {
            return;
        }
        for (key, value) in [(https_key_user(host), user), (https_key_pass(host), pass)] {
            let result = keyring::Entry::new(SERVICE, &key).and_then(|e| e.set_password(value));
            if let Err(err) = result {
                tracing::warn!(host, key = %key, error = %err, "storing credential in keyring failed");
            }
        }
        credential_helper::approve(
            &credential_helper::Query {
                protocol,
                host,
                path: None,
                username: Some(user),
            },
            pass,
        );
        self.helper_served.lock().insert(host.to_string());
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Keyring entry keys for one HTTPS host.
pub(crate) fn https_key_user(host: &str) -> String {
    format!("https:{host}:user")
}

pub(crate) fn https_key_pass(host: &str) -> String {
    format!("https:{host}:pass")
}

/// Read the `(user, pass)` pair for `host` from the keyring; `None` unless
/// both entries exist. Storage failures count as misses (headless Linux CI
/// has no Secret Service).
fn keyring_https_pair(host: &str) -> Option<(String, String)> {
    let user = keyring_get(&https_key_user(host))?;
    let pass = keyring_get(&https_key_pass(host))?;
    Some((user, pass))
}

fn keyring_get(key: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE, key)
        .map_err(|err| {
            tracing::debug!(key, error = %err, "opening keyring entry failed; treating as miss");
            err
        })
        .ok()?;
    match entry.get_password() {
        Ok(value) => Some(value),
        Err(keyring::Error::NoEntry) => None,
        Err(err) => {
            tracing::debug!(key, error = %err, "keyring read failed; treating as miss");
            None
        }
    }
}

/// Which dialog the FE should show: username+password when the username is
/// unknown, password-only when it came from the URL/cache.
pub(crate) fn https_prompt_kind(username_known: bool) -> &'static str {
    if username_known {
        KIND_HTTPS_PASS
    } else {
        KIND_HTTPS_USER
    }
}

/// Human-facing prompt text for each kind.
pub(crate) fn build_prompt(kind: &str, host: &str, username: Option<&str>) -> String {
    match kind {
        KIND_HTTPS_USER => format!("Sign in to {host}: username and password"),
        KIND_HTTPS_PASS => format!("Password for {} at {host}", username.unwrap_or("git")),
        KIND_SSH_PASSPHRASE => format!("SSH key passphrase for {host}"),
        _ => format!("Credentials for {host}"),
    }
}

/// Normalize an FE answer into `(user, pass)`. An entirely empty answer
/// means the user dismissed the dialog. A password-only answer keeps the
/// known username.
pub(crate) fn resolve_https_answer(
    answer: &AuthAnswer,
    known_user: Option<&str>,
) -> Result<(String, String), git2::Error> {
    let user = answer
        .username
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(|| known_user.map(str::to_string));
    let pass = answer.password.clone().filter(|p| !p.is_empty());
    match (user, pass) {
        (None, None) | (None, Some(_)) => Err(git2::Error::from_str(
            "authentication declined (no username)",
        )),
        (Some(_), None) => Err(git2::Error::from_str(
            "authentication declined (empty password)",
        )),
        (Some(user), Some(pass)) => Ok((user, pass)),
    }
}

/// First existing default ssh key, for the passphrase prompt path.
fn default_ssh_key() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let home = std::path::PathBuf::from(home);
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn broker() -> AuthBroker {
        let mut broker = AuthBroker::new();
        // Hermetic default: the credential-helper seam never hits (no git
        // spawn, no host credential manager). Helper-focused tests override
        // `helper_fill` directly.
        broker.helper_fill = Box::new(|_| None);
        broker
    }

    /// `expect_err` for `Result<Cred, _>` (`Cred` is not `Debug`).
    fn cred_err(result: Result<git2::Cred, git2::Error>) -> git2::Error {
        match result {
            Ok(_) => panic!("expected the attempt to fail"),
            Err(err) => err,
        }
    }

    /// Fake FE prompter: hands out canned answers, records what was asked.
    /// Keeping tests off [`FePrompter`] keeps tauri's Emitter-for-Wry
    /// monomorphization (and the windowing stack it links) out of the
    /// test binary — see module docs.
    struct FakePrompter {
        canned: Mutex<VecDeque<Result<AuthAnswer, String>>>,
        asked: Mutex<Vec<(String, String, String)>>,
    }

    impl FakePrompter {
        fn with(answers: Vec<Result<AuthAnswer, String>>) -> Self {
            Self {
                canned: Mutex::new(answers.into_iter().collect()),
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asks(&self) -> Vec<(String, String, String)> {
            self.asked.lock().clone()
        }
    }

    impl Prompter for FakePrompter {
        fn ask(&self, url: &str, kind: &str, prompt: String) -> Result<AuthAnswer, String> {
            self.asked
                .lock()
                .push((url.to_string(), kind.to_string(), prompt));
            self.canned
                .lock()
                .pop_front()
                .unwrap_or_else(|| Err("no more answers".to_string()))
        }
    }

    fn answer(user: Option<&str>, pass: Option<&str>, store: bool) -> Result<AuthAnswer, String> {
        Ok(AuthAnswer {
            username: user.map(str::to_string),
            password: pass.map(str::to_string),
            store,
        })
    }

    #[test]
    fn register_pending_answer_roundtrip() {
        let broker = broker();
        let rx = broker
            .register_pending("op-1".to_string())
            .expect("receiver");
        assert!(broker.answer(
            "op-1".to_string(),
            AuthAnswer {
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                store: false,
            }
        ));
        let answered = rx.blocking_recv().expect("answered");
        assert_eq!(answered.username.as_deref(), Some("user"));
        assert_eq!(answered.password.as_deref(), Some("pass"));
        assert!(!answered.store);
    }

    #[test]
    fn answer_unknown_or_consumed_op_id_fails() {
        let broker = broker();
        assert!(!broker.answer(
            "nope".to_string(),
            AuthAnswer {
                username: None,
                password: None,
                store: false
            }
        ));
        let rx = broker
            .register_pending("op-2".to_string())
            .expect("receiver");
        assert!(broker.answer(
            "op-2".to_string(),
            AuthAnswer {
                username: None,
                password: Some("x".to_string()),
                store: false
            }
        ));
        // Already consumed by the first answer.
        assert!(!broker.answer(
            "op-2".to_string(),
            AuthAnswer {
                username: None,
                password: Some("y".to_string()),
                store: false
            }
        ));
        drop(rx);
    }

    #[test]
    fn duplicate_registration_cancels_previous_waiter() {
        let broker = broker();
        let first = broker
            .register_pending("dup".to_string())
            .expect("receiver");
        let _second = broker
            .register_pending("dup".to_string())
            .expect("receiver");
        assert!(
            first.blocking_recv().is_err(),
            "first waiter must be canceled by re-registration"
        );
    }

    #[test]
    fn https_chain_prompts_then_fills_cache() {
        let broker = broker();
        let url = "https://fresh-never-seen.example.net/repo.git";
        let prompter = FakePrompter::with(vec![answer(Some("ada"), Some("pw1"), false)]);

        // Attempt 1: no cache, no keyring entry for this host → FE prompt.
        broker
            .attempt(
                &prompter,
                url,
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .expect("prompted credential accepted");
        assert_eq!(prompter.asks().len(), 1);
        let (asked_url, kind, prompt) = &prompter.asks()[0];
        assert_eq!(asked_url, url);
        assert_eq!(kind, KIND_HTTPS_USER, "username unknown → user+pass kind");
        assert!(prompt.contains("fresh-never-seen.example.net"));

        // The answer warmed the session cache: a fresh attempt 1 must NOT
        // prompt again.
        broker
            .attempt(
                &prompter,
                url,
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .expect("cache hit serves without prompting");
        assert_eq!(prompter.asks().len(), 1, "no second prompt for cache hit");
        assert_eq!(
            broker
                .cache
                .lock()
                .get("fresh-never-seen.example.net")
                .map(|(u, p)| (u.as_str(), p.as_str())),
            Some(("ada", "pw1"))
        );
    }

    /// A `git credential fill` hit (GCM & co) serves the credential without
    /// prompting and warms the session cache.
    #[test]
    fn helper_hit_serves_without_prompting() {
        let mut broker = broker();
        broker.helper_fill = Box::new(|q| {
            assert_eq!(q.protocol, "https");
            assert_eq!(q.host, "gcm.example.net");
            assert_eq!(q.path, Some("o/r.git"));
            Some(("gcm-user".to_string(), "gcm-pass".to_string()))
        });
        let prompter = FakePrompter::with(vec![]);

        broker
            .attempt(
                &prompter,
                "https://gcm.example.net/o/r.git",
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .expect("helper credential served");
        assert!(prompter.asks().is_empty(), "helper hit never prompts");
        assert_eq!(
            broker
                .cache
                .lock()
                .get("gcm.example.net")
                .map(|(u, p)| (u.as_str(), p.as_str())),
            Some(("gcm-user", "gcm-pass"))
        );
        assert!(broker.helper_served.lock().contains("gcm.example.net"));
    }

    /// When the remote refuses a credential that came from the helper
    /// store, the broker erases it there (`git credential reject`) so the
    /// dead token cannot be served again.
    #[test]
    fn rejected_helper_credential_is_rejected_in_the_store() {
        let mut broker = broker();
        broker.helper_fill = Box::new(|_| Some(("stale".to_string(), "token".to_string())));
        let prompter = FakePrompter::with(vec![]);

        broker
            .attempt(
                &prompter,
                "https://stale.example.net/r.git",
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .expect("helper hit");
        assert!(broker.helper_served.lock().contains("stale.example.net"));

        // Attempt 2 = the remote refused it: the marker is consumed (and
        // the store erase fired), then the attempt fails closed on the
        // answer-less prompter.
        assert!(broker
            .attempt(
                &prompter,
                "https://stale.example.net/r.git",
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                2,
            )
            .is_err());
        assert!(
            !broker.helper_served.lock().contains("stale.example.net"),
            "the erase must consume the marker"
        );
    }

    /// `remember` with `store: true` also routes the credential through
    /// `git credential approve` (tracked via helper_served so a later
    /// rejection can erase it); `store: false` must not.
    #[test]
    fn remember_with_store_marks_helper_store() {
        let broker = broker();
        let remember_host = "mygitui-remember.example.invalid";
        let user_entry = keyring::Entry::new(SERVICE, &https_key_user(remember_host)).unwrap();
        let pass_entry = keyring::Entry::new(SERVICE, &https_key_pass(remember_host)).unwrap();
        let _ = user_entry.delete_credential();
        let _ = pass_entry.delete_credential();

        broker.remember("https", remember_host, "ada", "pw", true);
        assert!(broker.helper_served.lock().contains(remember_host));

        broker.remember(
            "https",
            "mygitui-nostore.example.invalid",
            "ada",
            "pw",
            false,
        );
        assert!(!broker
            .helper_served
            .lock()
            .contains("mygitui-nostore.example.invalid"));

        let _ = user_entry.delete_credential();
        let _ = pass_entry.delete_credential();
    }

    #[test]
    fn rejected_attempt_clears_cache_and_reprompts_once() {
        let broker = broker();
        let url = "https://stale.example.net/repo.git";
        let prompter = FakePrompter::with(vec![
            answer(Some("first"), Some("bad"), false),
            answer(Some("second"), Some("good"), false),
        ]);
        broker
            .attempt(
                &prompter,
                url,
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .expect("first prompt");

        // Attempt 2 = libgit2 rejected attempt 1: cache cleared, re-prompt.
        broker
            .attempt(
                &prompter,
                url,
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                2,
            )
            .expect("re-prompt");
        assert_eq!(prompter.asks().len(), 2);

        // Attempt 3 = second rejection: fail closed, no third prompt.
        let err = cred_err(broker.attempt(
            &prompter,
            url,
            None,
            git2::CredentialType::USER_PASS_PLAINTEXT,
            MAX_ATTEMPTS + 1,
        ));
        assert!(
            err.message().contains("authentication failed"),
            "got: {err:?}"
        );
        assert_eq!(prompter.asks().len(), 2, "no prompt after the cap");
    }

    #[test]
    fn declined_answer_fails_the_attempt() {
        let broker = broker();
        let prompter = FakePrompter::with(vec![answer(None, None, false)]);
        let err = cred_err(broker.attempt(
            &prompter,
            "https://declined.example.net/r.git",
            None,
            git2::CredentialType::USER_PASS_PLAINTEXT,
            1,
        ));
        assert!(err.message().contains("declined"), "got: {err:?}");
    }

    #[test]
    fn prompter_error_surfaces_as_git_error() {
        let broker = broker();
        let prompter = FakePrompter::with(vec![Err("dialog closed".to_string())]);
        let err = cred_err(broker.attempt(
            &prompter,
            "https://closed.example.net/r.git",
            None,
            git2::CredentialType::USER_PASS_PLAINTEXT,
            1,
        ));
        assert_eq!(err.message(), "dialog closed");
    }

    #[test]
    fn known_username_prompts_password_only() {
        let broker = broker();
        let prompter = FakePrompter::with(vec![answer(None, Some("pw"), false)]);
        broker
            .attempt(
                &prompter,
                "https://user@known.example.net/r.git",
                Some("user"),
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .expect("password-only answer accepted (user from url)");
        let (_, kind, prompt) = &prompter.asks()[0];
        assert_eq!(kind, KIND_HTTPS_PASS);
        assert!(
            prompt.contains("user") && prompt.contains("known.example.net"),
            "prompt names the user and host: {prompt}"
        );
    }

    #[test]
    fn cache_hit_and_clear() {
        let broker = broker();
        {
            let mut cache = broker.cache.lock();
            cache.insert(
                "example.com".to_string(),
                ("u".to_string(), "p".to_string()),
            );
        }
        let prompter = FakePrompter::with(vec![]);
        assert!(broker
            .attempt(
                &prompter,
                "https://example.com/repo.git",
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                1,
            )
            .is_ok());
        assert!(prompter.asks().is_empty(), "cache hit never prompts");
        // A second attempt (rejection) clears the cache before anything
        // else.
        assert!(broker
            .attempt(
                &prompter,
                "https://example.com/repo.git",
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT,
                2,
            )
            .is_err());
        assert!(
            broker.cache.lock().is_empty(),
            "rejected attempt must clear the host cache"
        );
    }

    #[test]
    fn attempt_over_limit_fails_closed() {
        let broker = broker();
        let prompter = FakePrompter::with(vec![]);
        let err = cred_err(broker.attempt(
            &prompter,
            "https://example.com/repo.git",
            None,
            git2::CredentialType::USER_PASS_PLAINTEXT,
            MAX_ATTEMPTS + 1,
        ));
        assert!(
            err.message().contains("authentication failed"),
            "got: {err:?}"
        );
    }

    #[test]
    fn ssh_agent_first_on_initial_ssh_attempt() {
        let broker = broker();
        let prompter = FakePrompter::with(vec![]);
        // Without a running agent this errors; with one it succeeds —
        // either way the branch is taken and the ladder terminates without
        // touching prompts.
        let _ = broker.attempt(
            &prompter,
            "git@ssh.example.com:owner/repo.git",
            Some("git"),
            git2::CredentialType::SSH_KEY,
            1,
        );
        assert!(prompter.asks().is_empty());
    }

    #[test]
    fn netop_context_sets_and_restores_repo_id() {
        assert_eq!(current_repo_id(), "");
        let result = with_netop_context(&RepoId("r1".to_string()), || {
            assert_eq!(current_repo_id(), "r1");
            with_netop_context(&RepoId("r2".to_string()), || {
                assert_eq!(current_repo_id(), "r2");
                "inner"
            })
        });
        assert_eq!(result, "inner");
        assert_eq!(current_repo_id(), "", "context restored after the op");
    }

    #[test]
    fn https_prompt_kind_selection() {
        assert_eq!(https_prompt_kind(true), KIND_HTTPS_PASS);
        assert_eq!(https_prompt_kind(false), KIND_HTTPS_USER);
    }

    #[test]
    fn prompt_texts() {
        assert_eq!(
            build_prompt(KIND_HTTPS_USER, "github.com", None),
            "Sign in to github.com: username and password"
        );
        assert_eq!(
            build_prompt(KIND_HTTPS_PASS, "github.com", Some("raul")),
            "Password for raul at github.com"
        );
        assert_eq!(
            build_prompt(KIND_SSH_PASSPHRASE, "github.com", Some("git")),
            "SSH key passphrase for github.com"
        );
    }

    #[test]
    fn resolve_https_answer_variants() {
        let declined = AuthAnswer {
            username: None,
            password: None,
            store: false,
        };
        assert!(resolve_https_answer(&declined, None).is_err());

        let password_only = AuthAnswer {
            username: None,
            password: Some("token".to_string()),
            store: false,
        };
        let (user, pass) = resolve_https_answer(&password_only, Some("known")).unwrap();
        assert_eq!((user.as_str(), pass.as_str()), ("known", "token"));

        let full = AuthAnswer {
            username: Some("override".to_string()),
            password: Some("pw".to_string()),
            store: true,
        };
        let (user, pass) = resolve_https_answer(&full, Some("known")).unwrap();
        assert_eq!((user.as_str(), pass.as_str()), ("override", "pw"));
    }

    #[test]
    fn keyring_entry_keys() {
        assert_eq!(https_key_user("github.com"), "https:github.com:user");
        assert_eq!(https_key_pass("github.com"), "https:github.com:pass");
    }

    #[test]
    fn auth_request_event_serializes_with_contract_fields() {
        let event = AuthRequestEvent {
            op_id: "auth-1-2".to_string(),
            repo_id: "repo".to_string(),
            url: "https://github.com/o/r.git".to_string(),
            kind: KIND_HTTPS_USER.to_string(),
            prompt: "Sign in".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        for key in ["op_id", "repo_id", "url", "kind", "prompt"] {
            assert!(json.get(key).is_some(), "missing {key} in {json}");
        }
        assert_eq!(json["kind"], "https-user");
    }

    /// Real-keyring roundtrip for the `https:<host>` entry pair (skipped on
    /// backends without a usable store, mirroring keyring_store tests).
    #[test]
    fn keyring_https_pair_roundtrip() {
        if !keyring_backend_available() {
            eprintln!("skipping: no OS keyring backend available in this environment");
            return;
        }
        let host = "mygitui-auth-test.example.invalid";
        let user_entry = keyring::Entry::new(SERVICE, &https_key_user(host)).unwrap();
        let pass_entry = keyring::Entry::new(SERVICE, &https_key_pass(host)).unwrap();
        let _ = user_entry.delete_credential();
        let _ = pass_entry.delete_credential();

        let broker = broker();
        assert!(
            broker.keyring_lookup(host).is_none(),
            "no entries yet → miss"
        );

        user_entry.set_password("the-user").unwrap();
        pass_entry.set_password("the-pass").unwrap();
        assert_eq!(
            broker.keyring_lookup(host),
            Some(("the-user".to_string(), "the-pass".to_string()))
        );
        // The hit warmed the in-process cache.
        assert_eq!(
            broker
                .cache
                .lock()
                .get(host)
                .map(|(u, p)| (u.as_str(), p.as_str())),
            Some(("the-user", "the-pass"))
        );

        let _ = user_entry.delete_credential();
        let _ = pass_entry.delete_credential();
    }

    fn keyring_backend_available() -> bool {
        if cfg!(any(target_os = "windows", target_os = "macos")) {
            return true;
        }
        let Ok(probe) = keyring::Entry::new(SERVICE, "test/__auth_backend_probe__") else {
            return false;
        };
        let ok = probe.set_password("probe").is_ok();
        let _ = probe.delete_credential();
        ok
    }
}
