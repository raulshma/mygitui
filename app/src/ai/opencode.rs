//! opencode server adapter: strict health probing, autodiscovery, and the
//! two server generations' generation flows (ported from the old
//! `opencode.ts` — the TS is the protocol spec).
//!
//! Health: a candidate base URL only counts as an opencode server when an
//! endpoint answers `application/json` with a body that decodes to a known
//! shape — `/api/info` `{version, pid}` for 2.x, `/global/health`
//! `{healthy: true, version}` for 1.x, with a 2s timeout. A 200 alone is
//! NOT enough: opencode servers serve their web UI's HTML with a 200 on
//! unknown paths, so a lenient probe would misclassify any local server.
//! A 401/403 with a JSON body means an opencode server is listening but
//! needs the password ([`HealthKind::Unauthed`]); a 401/403 without JSON is
//! some unrelated guarded service and probing continues.
//!
//! Auth: every request carries `Authorization: Basic base64("opencode:" +
//! password)` — the secret lives in memory only and never in error strings.
//!
//! Sessions: v1 sessions are created titled [`SESSION_TITLE`] and reused
//! per session key (the repo id) in an in-memory map; a session that
//! vanished server-side is recreated once. v2 generations run in a
//! TEMPORARY session (the 2.x `generate` endpoints 403 on the free tier):
//! create → prompt → wait for the execution end on `/api/event` → delete,
//! rejecting any permission ask (nothing in a text generation needs a
//! tool).

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::Duration;

use futures_util::StreamExt;
use parking_lot::Mutex;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sse::{decode_opencode_event, SseDecoder};
use super::{http, AiConfig, AiError, GenRequest};

/// Health probe timeout — the server is local; slow means absent.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// v2 one-shot generation timeout (t3's `GENERATION_TIMEOUT`).
pub const V2_GENERATION_TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// Title given to sessions mygitui creates on the server.
pub const SESSION_TITLE: &str = "mygitui";
/// Default opencode server base URL (autodiscovery target).
pub const DEFAULT_OPENCODE_URL: &str = "http://127.0.0.1:4096";

/// Which server API generation answered the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthKind {
    /// opencode 1.x: sync prompt round trip, `/event` stream.
    V1,
    /// opencode 2.x: temp-session generation, `/api/event` stream.
    V2,
    /// An opencode server answered but rejected the credentials.
    Unauthed,
    /// No opencode server identified at this URL.
    Unreachable,
}

/// Result of probing one candidate base URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub kind: HealthKind,
    /// Server-reported `x.y.z`, when the body carried one.
    pub version: Option<String>,
}

impl Health {
    fn unreachable() -> Self {
        Health {
            kind: HealthKind::Unreachable,
            version: None,
        }
    }
}

/// What [`discover`] found across its candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// The first candidate that identified itself as opencode: base URL +
    /// its [`Health`].
    pub server: Option<(String, Health)>,
    /// First candidate that answered 401/403 (server present, auth needed).
    pub unauthorized_base: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit tested without IO)
// ---------------------------------------------------------------------------

/// Standard base64 (with padding) — dependency-free because only one
/// header value ever needs it.
pub(crate) fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Builds the `Authorization: Basic …` header value for a password
/// (`opencode:` is the fixed username, UTF-8, per the old adapter).
pub fn basic_auth_header(password: &str) -> String {
    format!(
        "Basic {}",
        base64(format!("opencode:{password}").as_bytes())
    )
}

/// True when the content type advertises JSON (HTML web-UI replies fail).
pub fn is_json_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    let lower = content_type.to_ascii_lowercase();
    match lower.find("application/") {
        Some(position) => lower[position + "application/".len()..].contains("json"),
        None => false,
    }
}

/// Decodes the v1 `/global/health` body; resolves the version or `None`.
pub fn decode_health_v1(text: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(text).ok()?;
    let object = parsed.as_object()?;
    if object.get("healthy").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    object
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Decodes the v2 `/api/info` body; resolves the version or `None`.
pub fn decode_info_v2(text: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(text).ok()?;
    let object = parsed.as_object()?;
    let version = object.get("version").and_then(Value::as_str)?;
    object.get("pid").and_then(Value::as_i64)?;
    Some(version.to_string())
}

/// Normalizes a workspace directory for server calls. OpenCode 1.x resolves
/// the `x-opencode-directory` value with forward slashes only: a Windows
/// backslash path (git2's `C:\repo`, or Rust's canonicalized `\\?\C:\repo`)
/// is mis-parsed into a garbage project directory and every prompt fails
/// with a 500 "Unexpected server error". Verified against opencode 1.18.35.
pub fn normalize_directory(directory: &str) -> String {
    let stripped = directory.strip_prefix(r"\\?\").unwrap_or(directory);
    stripped.replace('\\', "/")
}

/// Parses a `provider/model` slug into its parts (`None` when malformed).
/// Shared by both generations: v1 splits the same way for its prompt body,
/// v2 needs a `{providerID, id}` ref (t3's `parseOpenCodeModelSlug`).
pub fn parse_model_slug(slug: &str) -> Option<(String, String)> {
    let trimmed = slug.trim();
    let separator = trimmed.find('/')?;
    if separator == 0 || separator == trimmed.len() - 1 {
        return None;
    }
    Some((
        trimmed[..separator].to_string(),
        trimmed[separator + 1..].to_string(),
    ))
}

/// True when `part` looks like an opencode text part.
fn is_text_part(part: &Value) -> Option<&str> {
    if part.get("type").and_then(Value::as_str) == Some("text") {
        part.get("text").and_then(Value::as_str)
    } else {
        None
    }
}

/// Concatenates the text parts of an opencode message-parts array (the
/// sync prompt response's `parts`). Non-text parts (tool calls, reasoning,
/// step markers) are ignored; `None` when no text parts exist.
pub fn parts_to_text(parts: &Value) -> Option<String> {
    let array = parts.as_array()?;
    let joined = array
        .iter()
        .filter_map(is_text_part)
        .collect::<Vec<_>>()
        .join("");
    let joined = joined.trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Last-error text from a JSON error shape (safe, no request dump):
/// `{name, data: {message, ref}}` POJOs have no top-level message, so read
/// both shapes. Capped like the old adapter (300 chars).
pub(crate) fn summarize_value_error(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.chars().take(300).collect();
    }
    let data = value.get("data");
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| data.and_then(|d| d.get("message")).and_then(Value::as_str))
        .filter(|m| !m.is_empty());
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty());
    let reference = data
        .and_then(|d| d.get("ref"))
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty());
    let mut out = match (name, message) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (None, Some(message)) => message.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => "unknown server error".to_string(),
    };
    if let Some(reference) = reference {
        out = format!("{out} (ref {reference})");
    }
    out.chars().take(300).collect()
}

/// Renders the `info.error` field of a successful prompt reply (200 with a
/// failed model call): `Some((unauthenticated, message))` when the reply
/// carries error info. `ProviderAuthError` maps to unauthenticated.
fn describe_info_error(info: &Value) -> Option<(bool, String)> {
    let error = info.get("error")?.as_object()?;
    let name = error.get("name").and_then(Value::as_str);
    let message = error
        .get("data")
        .and_then(|d| d.get("message"))
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .or_else(|| name.map(str::to_string))?;
    Some((name == Some("ProviderAuthError"), message))
}

/// Joins a base URL and a server path, keeping reverse-proxy path prefixes
/// (`None` when `base` is not a usable http(s) URL).
pub(crate) fn join_url(base: &str, path: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(base).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    let prefix = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("{prefix}{path}"));
    Some(url.to_string())
}

/// v1 prompt body's `model` field: `None` for unset/empty/`opencode/default`
/// (the server default), else the first-`/` split (malformed slugs repeat
/// as both fields, like the old adapter).
fn v1_model_field(model: Option<&str>) -> Option<Value> {
    let model = model?.trim();
    if model.is_empty() || model == "opencode/default" {
        return None;
    }
    Some(match parse_model_slug(model) {
        Some((provider_id, model_id)) => json!({"providerID": provider_id, "modelID": model_id}),
        None => json!({"providerID": model, "modelID": model}),
    })
}

/// Maps a reqwest failure to the transport error (message capped at 200
/// chars like the old adapter; reqwest errors never carry the password).
fn transport(err: &reqwest::Error) -> AiError {
    AiError::Unavailable(format!("opencode request failed: {}", err))
}

// ---------------------------------------------------------------------------
// Probing + discovery
// ---------------------------------------------------------------------------

/// Probes `base` for an opencode server, t3-order: `/api/info` (2.x) first,
/// then `/global/health` (1.x). Strictly identifies by decoding the body
/// (see the module docs). Never fails: every failure maps to
/// [`HealthKind::Unreachable`].
pub async fn probe(base: &str, password: Option<&str>) -> Health {
    for (path, kind) in [
        ("/api/info", HealthKind::V2),
        ("/global/health", HealthKind::V1),
    ] {
        let Some(url) = join_url(base, path) else {
            return Health::unreachable();
        };
        let mut request = http().get(&url);
        if let Some(password) = password {
            request = request.header(AUTHORIZATION, basic_auth_header(password));
        }
        // The whole round trip (send + body) is bounded: slow means absent.
        let reply = match tokio::time::timeout(HEALTH_TIMEOUT, async move {
            let response = request.send().await?;
            let status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let text = response.text().await.unwrap_or_default();
            Ok::<_, reqwest::Error>((status, content_type, text))
        })
        .await
        {
            Ok(Ok(reply)) => reply,
            // Network failure/timeout: no server here (the old probe stops
            // at the first dead candidate instead of trying the next path).
            Ok(Err(_)) | Err(_) => return Health::unreachable(),
        };
        let (status, content_type, text) = reply;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            if is_json_content_type(content_type.as_deref()) {
                return Health {
                    kind: HealthKind::Unauthed,
                    version: None,
                };
            }
            continue;
        }
        if status.as_u16() == 200 && is_json_content_type(content_type.as_deref()) {
            let version = if path == "/api/info" {
                decode_info_v2(&text)
            } else {
                decode_health_v1(&text)
            };
            if let Some(version) = version {
                return Health {
                    kind,
                    version: Some(version),
                };
            }
        }
    }
    Health::unreachable()
}

/// Autodiscovers a running opencode server. Order: manual `url` (when set),
/// then [`DEFAULT_OPENCODE_URL`]; each candidate is strictly identified by
/// [`probe`].
pub async fn discover(manual_url: Option<&str>, password: Option<&str>) -> Discovery {
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(manual_url) = manual_url {
        candidates.push(manual_url);
    }
    if !candidates.contains(&DEFAULT_OPENCODE_URL) {
        candidates.push(DEFAULT_OPENCODE_URL);
    }
    let mut unauthorized_base: Option<String> = None;
    for base in candidates {
        let health = probe(base, password).await;
        match health.kind {
            HealthKind::V1 | HealthKind::V2 => {
                return Discovery {
                    server: Some((base.to_string(), health)),
                    unauthorized_base,
                }
            }
            HealthKind::Unauthed if unauthorized_base.is_none() => {
                unauthorized_base = Some(base.to_string());
            }
            HealthKind::Unauthed | HealthKind::Unreachable => {}
        }
    }
    Discovery {
        server: None,
        unauthorized_base,
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// v1 session cache: `(base, directory, session_key)` → session id
/// (in-memory only; sessions die with the process or the server restart,
/// which the recreate-once path handles).
static V1_SESSIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn v1_sessions() -> &'static Mutex<HashMap<String, String>> {
    V1_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Test/teardown helper: forgets every cached v1 session.
pub fn reset_v1_sessions() {
    v1_sessions().lock().clear();
}

/// Runs one generation against the opencode backend: probes candidates
/// (configured URL first, then the default), routes by server generation.
pub(crate) async fn generate(req: GenRequest, cfg: &AiConfig) -> Result<String, AiError> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(url) = cfg.opencode_url.as_deref().filter(|url| !url.is_empty()) {
        candidates.push(url.to_string());
    }
    if !candidates.iter().any(|url| url == DEFAULT_OPENCODE_URL) {
        candidates.push(DEFAULT_OPENCODE_URL.to_string());
    }
    let mut unauthorized: Option<String> = None;
    for base in candidates {
        let health = probe(&base, cfg.opencode_password.as_deref()).await;
        match health.kind {
            HealthKind::V1 => return generate_v1(&base, cfg, &req).await,
            HealthKind::V2 => return generate_v2(&base, cfg, &req).await,
            HealthKind::Unauthed if unauthorized.is_none() => unauthorized = Some(base),
            HealthKind::Unauthed | HealthKind::Unreachable => {}
        }
    }
    match unauthorized {
        Some(base) => Err(AiError::Unauthenticated(format!(
            "opencode server at {base} requires the basic-auth password (AI settings)"
        ))),
        None => Err(AiError::Unavailable(match cfg.opencode_url.as_deref() {
            Some(manual) => format!(
                "opencode server not reachable at {manual} (health probe failed) — \
                 start it, clear the URL, or switch to Managed in AI settings"
            ),
            None => format!(
                "no opencode server found (probed {DEFAULT_OPENCODE_URL}) — \
                 start one or switch to Managed in AI settings"
            ),
        })),
    }
}

fn auth_headers(
    mut request: reqwest::RequestBuilder,
    cfg: &AiConfig,
    directory: Option<&str>,
) -> reqwest::RequestBuilder {
    if let Some(password) = cfg.opencode_password.as_deref() {
        request = request.header(AUTHORIZATION, basic_auth_header(password));
    }
    if let Some(directory) = directory.filter(|d| !d.is_empty()) {
        // v1 resolves the workspace through this header (backslash paths
        // break the server's project resolution — see normalize_directory).
        request = request.header("x-opencode-directory", directory);
    }
    request
}

async fn body_of(response: reqwest::Response) -> Result<String, AiError> {
    response.text().await.map_err(|err| transport(&err))
}

fn status_error(operation: &str, status: reqwest::StatusCode) -> AiError {
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return AiError::Unauthenticated("the opencode server rejected the password".into());
    }
    AiError::BadResponse(format!(
        "opencode {operation} failed: HTTP {}",
        status.as_u16()
    ))
}

/// v1 one-shot generation: shared titled session + sync prompt round trip.
async fn generate_v1(base: &str, cfg: &AiConfig, req: &GenRequest) -> Result<String, AiError> {
    let directory = cfg
        .directory
        .as_deref()
        .map(normalize_directory)
        .filter(|d| !d.is_empty());
    let key = format!(
        "{base}\0{}\0{}",
        directory.as_deref().unwrap_or(""),
        req.session_key
    );
    let body = v1_prompt_body(req, cfg.model.as_deref());

    let mut session_id = v1_ensure_session(base, cfg, &key, directory.as_deref()).await?;
    let mut reply = v1_prompt(base, cfg, &session_id, &body, directory.as_deref()).await;
    if reply.is_err() {
        let cached = v1_sessions().lock().contains_key(&key);
        let auth_failure = matches!(reply, Err(AiError::Unauthenticated(_)));
        // Session known to us but gone server-side (server restart):
        // recreate once. Auth failures are never worth a second prompt.
        if cached && !auth_failure {
            v1_sessions().lock().remove(&key);
            session_id = v1_ensure_session(base, cfg, &key, directory.as_deref()).await?;
            reply = v1_prompt(base, cfg, &session_id, &body, directory.as_deref()).await;
        }
    }
    let value = reply?;

    let text = parts_to_text(value.get("parts").unwrap_or(&Value::Null)).ok_or_else(|| {
        // 200 with no text usually carries the failure on info.error (e.g.
        // the model's provider has no API key on the server) — surface it
        // instead of a generic "no text" that hides the actual cause.
        match value.get("info").and_then(describe_info_error) {
            Some((unauthenticated, message)) => {
                let kind_message = format!(
                    "opencode model failed: {message} — check the model/API key in AI settings or opencode"
                );
                if unauthenticated {
                    AiError::Unauthenticated(kind_message)
                } else {
                    AiError::BadResponse(kind_message)
                }
            }
            None => AiError::BadResponse("opencode returned no text output for the prompt".into()),
        }
    })?;
    Ok(text)
}

/// The v1 `POST /session/{id}/message` body (parts + optional system/model).
fn v1_prompt_body(req: &GenRequest, model: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    body.insert(
        "parts".into(),
        json!([{"type": "text", "text": req.prompt}]),
    );
    if !req.system.is_empty() {
        body.insert("system".into(), json!(req.system));
    }
    if let Some(model) = v1_model_field(model) {
        body.insert("model".into(), model);
    }
    Value::Object(body)
}

/// Returns (creating if needed) the v1 session id for `key`. Sessions are
/// titled [`SESSION_TITLE`].
async fn v1_ensure_session(
    base: &str,
    cfg: &AiConfig,
    key: &str,
    directory: Option<&str>,
) -> Result<String, AiError> {
    if let Some(id) = v1_sessions().lock().get(key) {
        return Ok(id.clone());
    }
    let url = join_url(base, "/session").ok_or_else(unreachable_base)?;
    let request = auth_headers(
        http().post(url).json(&json!({ "title": SESSION_TITLE })),
        cfg,
        directory,
    );
    let response = request.send().await.map_err(|err| transport(&err))?;
    let status = response.status();
    let text = body_of(response).await?;
    if !status.is_success() {
        return Err(status_error("session create", status));
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| bad_json("session create"))?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AiError::BadResponse("opencode refused to create a session".into()))?
        .to_string();
    v1_sessions().lock().insert(key.to_string(), id.clone());
    Ok(id)
}

/// The synchronous v1 prompt round trip: the response already contains the
/// assistant parts, so no event stream is needed for one-shot features.
async fn v1_prompt(
    base: &str,
    cfg: &AiConfig,
    session_id: &str,
    body: &Value,
    directory: Option<&str>,
) -> Result<Value, AiError> {
    let url =
        join_url(base, &format!("/session/{session_id}/message")).ok_or_else(unreachable_base)?;
    let request = auth_headers(http().post(url).json(body), cfg, directory);
    let response = request.send().await.map_err(|err| transport(&err))?;
    let status = response.status();
    let text = body_of(response).await?;
    if !status.is_success() {
        return Err(status_error("prompt", status));
    }
    serde_json::from_str(&text).map_err(|_| bad_json("prompt reply"))
}

/// v2 one-shot generation (t3's `textGeneration`): the `generate` endpoints
/// 403 on the free tier, so each request prompts in a temporary session and
/// waits on `/api/event` for its execution end, then removes the session.
/// Permission asks are rejected (nothing in a generation needs a tool).
async fn generate_v2(base: &str, cfg: &AiConfig, req: &GenRequest) -> Result<String, AiError> {
    let directory = cfg
        .directory
        .as_deref()
        .map(normalize_directory)
        .filter(|d| !d.is_empty());
    let model = match cfg
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        None => None,
        Some(slug) => match parse_model_slug(slug) {
            Some((provider_id, model_id)) => Some((provider_id, model_id)),
            None => {
                return Err(AiError::BadResponse(format!(
                    "OpenCode model '{slug}' must use provider/model format"
                )))
            }
        },
    };
    // v2 prompts carry no `system` field — fold it into the text.
    let text = if req.system.is_empty() {
        req.prompt.clone()
    } else {
        format!("{}\n\n{}", req.system, req.prompt)
    };

    let session_id = v2_create_session(base, cfg, model.as_ref(), directory.as_deref()).await?;
    let outcome = v2_wait_for_execution(base, cfg, &session_id, &text).await;
    // The temp session is always removed (best-effort; free-tier-safe).
    let _ = v2_delete_session(base, cfg, &session_id).await;
    outcome
}

async fn v2_create_session(
    base: &str,
    cfg: &AiConfig,
    model: Option<&(String, String)>,
    directory: Option<&str>,
) -> Result<String, AiError> {
    let url = join_url(base, "/session").ok_or_else(unreachable_base)?;
    let mut body = serde_json::Map::new();
    body.insert("title".into(), json!(SESSION_TITLE));
    if let Some(directory) = directory {
        body.insert("location".into(), json!({ "directory": directory }));
    }
    if let Some((provider_id, model_id)) = model {
        body.insert(
            "model".into(),
            json!({"providerID": provider_id, "id": model_id}),
        );
    }
    // Nothing in a text generation needs a tool: every tool ask is rejected
    // as it arrives (denying outright gets the session refused on free-tier
    // models; an unanswered ask would hang the wait).
    body.insert(
        "permissions".into(),
        json!([{ "action": "*", "resource": "*", "effect": "ask" }]),
    );

    let request = auth_headers(http().post(url).json(&Value::Object(body)), cfg, None);
    let response = request.send().await.map_err(|err| transport(&err))?;
    let status = response.status();
    let text = body_of(response).await?;
    if !status.is_success() {
        return Err(status_error("session create", status));
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| bad_json("session create"))?;
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AiError::BadResponse("opencode refused to create a session".into()))
}

/// Subscribes to the v2 `/api/event` stream before prompting (the stream
/// is volatile — the reply only arrives on it), prompts, then waits for
/// this session's execution end while collecting its `session.text.ended`
/// parts. Bounded by [`V2_GENERATION_TIMEOUT`] (timeout interrupts the
/// session best-effort).
async fn v2_wait_for_execution(
    base: &str,
    cfg: &AiConfig,
    session_id: &str,
    text: &str,
) -> Result<String, AiError> {
    let stream_url = join_url(base, "/api/event").ok_or_else(unreachable_base)?;
    // The v2 client carried no workspace header (the location lives in the
    // session-create body), so auth is the only injected header here.
    let request = auth_headers(
        http().get(stream_url).header(ACCEPT, "text/event-stream"),
        cfg,
        None,
    );
    let response = request.send().await.map_err(|err| transport(&err))?;
    let status = response.status();
    if !status.is_success() {
        return Err(status_error("event stream", status));
    }

    // Prompt now: the stream is already open, so its events (buffered by
    // TCP) are all still readable afterwards.
    v2_prompt(base, cfg, session_id, text).await?;

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    // Insertion of one assistant message's parts is by ordinal; keyed by
    // `assistantMessageID:ordinal` like the old adapter, so a BTreeMap both
    // dedupes and orders the parts deterministically.
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let deadline = tokio::time::Instant::now() + V2_GENERATION_TIMEOUT;

    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Err(_) => {
                // Deadline: interrupt the session best-effort, then report.
                let _ = v2_interrupt(base, cfg, session_id).await;
                return Err(AiError::Unavailable(
                    "OpenCode did not finish generating in time.".into(),
                ));
            }
            Ok(None) => {
                return Err(AiError::BadResponse(
                    "The OpenCode event stream ended before the reply arrived.".into(),
                ));
            }
            Ok(Some(chunk)) => {
                let chunk = chunk.map_err(|err| transport(&err))?;
                for data in decoder.push_bytes(&chunk) {
                    let Some(event) = decode_opencode_event(&data) else {
                        continue;
                    };
                    if event.properties.get("sessionID").and_then(Value::as_str) != Some(session_id)
                    {
                        continue;
                    }
                    match event.event_type.as_str() {
                        "session.text.ended" => {
                            let key = format!(
                                "{}:{}",
                                event
                                    .properties
                                    .get("assistantMessageID")
                                    .and_then(Value::as_str)
                                    .unwrap_or(""),
                                event
                                    .properties
                                    .get("ordinal")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                            );
                            let part = event
                                .properties
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            texts.insert(key, part);
                        }
                        "permission.asked" => {
                            if let Some(request_id) =
                                event.properties.get("id").and_then(Value::as_str)
                            {
                                let _ =
                                    v2_reject_permission(base, cfg, session_id, request_id).await;
                            }
                        }
                        "session.execution.succeeded" => {
                            let joined = texts.values().cloned().collect::<Vec<_>>().join("\n");
                            let joined = joined.trim().to_string();
                            if joined.is_empty() {
                                return Err(AiError::BadResponse(
                                    "opencode returned no text output for the prompt".into(),
                                ));
                            }
                            return Ok(joined);
                        }
                        "session.execution.failed" => {
                            let error = summarize_value_error(
                                event.properties.get("error").unwrap_or(&Value::Null),
                            );
                            return Err(AiError::BadResponse(format!(
                                "opencode prompt failed: {error} — \
                                 check the model/API key in AI settings or opencode"
                            )));
                        }
                        "session.execution.interrupted" => return Err(AiError::Cancelled),
                        _ => {}
                    }
                }
            }
        }
    }
}

async fn v2_prompt(
    base: &str,
    cfg: &AiConfig,
    session_id: &str,
    text: &str,
) -> Result<(), AiError> {
    let url =
        join_url(base, &format!("/session/{session_id}/message")).ok_or_else(unreachable_base)?;
    let body = json!({ "parts": [{ "type": "text", "text": text }] });
    let request = auth_headers(http().post(url).json(&body), cfg, None);
    let response = request.send().await.map_err(|err| transport(&err))?;
    let status = response.status();
    if !status.is_success() {
        return Err(status_error("prompt", status));
    }
    // The reply arrives on the event stream, not here; drain the body so
    // the connection can be reused.
    let _ = body_of(response).await;
    Ok(())
}

async fn v2_delete_session(base: &str, cfg: &AiConfig, session_id: &str) -> Result<(), AiError> {
    let url = join_url(base, &format!("/session/{session_id}")).ok_or_else(unreachable_base)?;
    let request = auth_headers(http().delete(url), cfg, None);
    let response = request.send().await.map_err(|err| transport(&err))?;
    let _ = body_of(response).await;
    Ok(())
}

async fn v2_interrupt(base: &str, cfg: &AiConfig, session_id: &str) -> Result<(), AiError> {
    let url =
        join_url(base, &format!("/session/{session_id}/interrupt")).ok_or_else(unreachable_base)?;
    let request = auth_headers(http().post(url), cfg, None);
    let response = request.send().await.map_err(|err| transport(&err))?;
    let _ = body_of(response).await;
    Ok(())
}

async fn v2_reject_permission(
    base: &str,
    cfg: &AiConfig,
    session_id: &str,
    request_id: &str,
) -> Result<(), AiError> {
    let url = join_url(
        base,
        &format!("/session/{session_id}/permissions/{request_id}"),
    )
    .ok_or_else(unreachable_base)?;
    let body = json!({ "response": "rejected" });
    let request = auth_headers(http().post(url).json(&body), cfg, None);
    let response = request.send().await.map_err(|err| transport(&err))?;
    let _ = body_of(response).await;
    Ok(())
}

/// Bad-response shorthand for undecodable reply bodies.
fn bad_json(operation: &str) -> AiError {
    AiError::BadResponse(format!("opencode {operation} was not valid JSON"))
}

fn unreachable_base() -> AiError {
    AiError::Unavailable("opencode server URL is not a usable http(s) address".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg_for(url: &str) -> AiConfig {
        AiConfig {
            provider: super::super::AiProvider::Opencode,
            opencode_url: Some(url.to_string()),
            ..Default::default()
        }
    }

    /// Health mock so the probe identifies the server as opencode 1.x.
    async fn mount_v1_health(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/global/health"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"healthy": true, "version": "1.18.35"})),
            )
            .mount(server)
            .await;
    }

    /// Health mock so the probe identifies the server as opencode 2.x.
    async fn mount_v2_health(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/info"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"version": "2.0.18", "pid": 7})),
            )
            .mount(server)
            .await;
    }

    fn gen_req(prompt: &str, session_key: &str) -> GenRequest {
        GenRequest {
            feature: super::super::AiFeature::CommitMessage,
            system: String::new(),
            prompt: prompt.to_string(),
            max_tokens: None,
            // Unique per test: the v1 session cache is process-global and
            // keyed on base+key, and parallel tests can recycle ports.
            session_key: session_key.into(),
        }
    }

    /// Waits until the mock's request log satisfies `pred` (the log lands
    /// asynchronously after responses complete, so immediate reads can lag
    /// a finished generation; full-suite parallel load can delay both
    /// probes and logging, hence the generous budget).
    async fn wait_for_requests(
        server: &MockServer,
        pred: impl Fn(&[wiremock::Request]) -> bool,
    ) -> Vec<wiremock::Request> {
        for _ in 0..600 {
            let requests = server.received_requests().await.unwrap_or_default();
            if pred(&requests) {
                return requests;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let requests = server.received_requests().await.unwrap_or_default();
        let seen = requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.url))
            .collect::<Vec<_>>();
        panic!("mock request log never matched; seen: {seen:?}");
    }

    // -- pure helpers --------------------------------------------------------

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn basic_auth_header_encodes_opencode_username() {
        assert_eq!(
            basic_auth_header("pw"),
            format!("Basic {}", base64(b"opencode:pw"))
        );
        // UTF-8 passwords encode as bytes (browser btoa parity).
        assert_eq!(
            basic_auth_header("päss"),
            format!("Basic {}", base64("opencode:päss".as_bytes()))
        );
    }

    #[test]
    fn json_content_type_detection() {
        assert!(is_json_content_type(Some("application/json")));
        assert!(is_json_content_type(Some(
            "application/json; charset=utf-8"
        )));
        assert!(is_json_content_type(Some("application/vnd.api+json")));
        assert!(is_json_content_type(Some("APPLICATION/JSON")));
        assert!(!is_json_content_type(Some("text/html")));
        assert!(!is_json_content_type(Some("text/event-stream")));
        assert!(!is_json_content_type(None));
    }

    #[test]
    fn health_decoders_are_strict() {
        assert_eq!(
            decode_health_v1(r#"{"healthy":true,"version":"1.18.35"}"#),
            Some("1.18.35".into())
        );
        assert_eq!(
            decode_health_v1(r#"{"healthy":false,"version":"1.0"}"#),
            None
        );
        assert_eq!(decode_health_v1(r#"{"healthy":true}"#), None);
        assert_eq!(decode_health_v1("<html>ok</html>"), None);
        assert_eq!(
            decode_info_v2(r#"{"version":"2.0.18","pid":123}"#),
            Some("2.0.18".into())
        );
        assert_eq!(
            decode_info_v2(r#"{"version":"2.0.18"}"#),
            None,
            "pid required"
        );
        assert_eq!(decode_info_v2(r#"{"version":"2.0.18","pid":"x"}"#), None);
        assert_eq!(decode_info_v2("nope"), None);
    }

    #[test]
    fn directory_normalization_strips_verbatim_and_backslashes() {
        assert_eq!(normalize_directory(r"C:\repos\alpha"), "C:/repos/alpha");
        assert_eq!(normalize_directory(r"\\?\C:\repos\alpha"), "C:/repos/alpha");
        assert_eq!(normalize_directory("/home/me/repo"), "/home/me/repo");
    }

    #[test]
    fn model_slug_parsing() {
        assert_eq!(
            parse_model_slug("anthropic/claude"),
            Some(("anthropic".into(), "claude".into()))
        );
        assert_eq!(
            parse_model_slug("openai/gpt/oss"),
            Some(("openai".into(), "gpt/oss".into())),
            "split at the first slash only"
        );
        assert_eq!(parse_model_slug("/lead"), None);
        assert_eq!(parse_model_slug("lead/"), None);
        assert_eq!(parse_model_slug("noslash"), None);
        assert_eq!(
            parse_model_slug(" spaced/model "),
            Some(("spaced".into(), "model".into()))
        );
    }

    #[test]
    fn v1_model_field_skips_default_and_splits_slug() {
        assert_eq!(v1_model_field(None), None);
        assert_eq!(v1_model_field(Some("opencode/default")), None);
        assert_eq!(v1_model_field(Some("  ")), None);
        assert_eq!(
            v1_model_field(Some("anthropic/claude-sonnet")),
            Some(json!({"providerID": "anthropic", "modelID": "claude-sonnet"}))
        );
        assert_eq!(
            v1_model_field(Some("bare")),
            Some(json!({"providerID": "bare", "modelID": "bare"}))
        );
    }

    #[test]
    fn parts_to_text_joins_text_parts_only() {
        let parts = json!([
            {"type": "text", "text": "feat: "},
            {"type": "reasoning", "text": "thinking"},
            {"type": "text", "text": "add thing"},
            {"type": "tool-call"}
        ]);
        assert_eq!(parts_to_text(&parts), Some("feat: add thing".into()));
        assert_eq!(parts_to_text(&json!([])), None);
        assert_eq!(parts_to_text(&json!([{"type": "tool-call"}])), None);
        assert_eq!(
            parts_to_text(&json!([{"type": "text", "text": "   "}])),
            None
        );
        assert_eq!(parts_to_text(&json!("not an array")), None);
    }

    #[test]
    fn summarize_error_reads_namederror_shapes() {
        assert_eq!(
            summarize_value_error(&json!("plain string")),
            "plain string"
        );
        assert_eq!(
            summarize_value_error(&json!({"name": "NamedError"})),
            "NamedError"
        );
        assert_eq!(
            summarize_value_error(&json!({"name": "NamedError", "data": {"message": "boom"}})),
            "NamedError: boom"
        );
        assert_eq!(
            summarize_value_error(
                &json!({"name": "NamedError", "data": {"message": "boom", "ref": "abc"}})
            ),
            "NamedError: boom (ref abc)"
        );
        assert_eq!(summarize_value_error(&json!({})), "unknown server error");
        let long = summarize_value_error(&json!("x".repeat(500)));
        assert_eq!(long.chars().count(), 300);
    }

    #[test]
    fn info_error_describes_provider_auth() {
        let info =
            json!({"error": {"name": "ProviderAuthError", "data": {"message": "no api key"}}});
        assert_eq!(
            describe_info_error(&info),
            Some((true, "no api key".into()))
        );
        let info = json!({"error": {"name": "UnknownError", "data": {"message": "kaputt"}}});
        assert_eq!(describe_info_error(&info), Some((false, "kaputt".into())));
        assert_eq!(describe_info_error(&json!({})), None);
        assert_eq!(describe_info_error(&json!({"error": "str"})), None);
    }

    #[test]
    fn join_url_keeps_proxy_prefixes_and_rejects_schemes() {
        assert_eq!(
            join_url("http://127.0.0.1:4096", "/api/info").as_deref(),
            Some("http://127.0.0.1:4096/api/info")
        );
        assert_eq!(
            join_url("https://proxy.example/oc/", "/global/health").as_deref(),
            Some("https://proxy.example/oc/global/health")
        );
        assert!(join_url("ftp://x/", "/api/info").is_none());
        assert!(join_url("not a url", "/api/info").is_none());
    }

    // -- health probing over HTTP --------------------------------------------

    #[tokio::test]
    async fn probe_identifies_v2_server_by_info_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/info"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"version": "2.0.18", "pid": 7})),
            )
            .mount(&server)
            .await;
        let health = probe(&server.uri(), None).await;
        assert_eq!(health.kind, HealthKind::V2);
        assert_eq!(health.version.as_deref(), Some("2.0.18"));
    }

    #[tokio::test]
    async fn probe_identifies_v1_server_by_health_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/info"))
            .respond_with(ResponseTemplate::new(404).set_body_string("nope"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/global/health"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"healthy": true, "version": "1.18.35"})),
            )
            .mount(&server)
            .await;
        let health = probe(&server.uri(), None).await;
        assert_eq!(health.kind, HealthKind::V1);
        assert_eq!(health.version.as_deref(), Some("1.18.35"));
    }

    #[tokio::test]
    async fn probe_rejects_html_web_ui_on_200() {
        let server = MockServer::start().await;
        // opencode servers serve their web UI with a 200 on unknown paths;
        // a lenient probe would misclassify it as opencode.
        let html = ResponseTemplate::new(200)
            .set_body_string("<html><body>web ui</body></html>")
            .append_header("content-type", "text/html");
        Mock::given(method("GET"))
            .respond_with(html)
            .mount(&server)
            .await;
        let health = probe(&server.uri(), None).await;
        assert_eq!(health.kind, HealthKind::Unreachable);
        assert_eq!(health.version, None);
    }

    #[tokio::test]
    async fn probe_maps_401_json_to_unauthed() {
        let server = MockServer::start().await;
        let denied = ResponseTemplate::new(401)
            .set_body_json(json!({"error": "Unauthorized"}))
            .append_header("content-type", "application/json");
        Mock::given(method("GET"))
            .respond_with(denied)
            .mount(&server)
            .await;
        let health = probe(&server.uri(), Some("pw")).await;
        assert_eq!(health.kind, HealthKind::Unauthed);
    }

    #[tokio::test]
    async fn probe_ignores_401_without_json_body() {
        let server = MockServer::start().await;
        // An unrelated guarded service's login page is NOT opencode.
        let login = ResponseTemplate::new(401)
            .set_body_string("<html>login</html>")
            .append_header("content-type", "text/html");
        Mock::given(method("GET"))
            .respond_with(login)
            .mount(&server)
            .await;
        let health = probe(&server.uri(), None).await;
        assert_eq!(health.kind, HealthKind::Unreachable);
    }

    #[tokio::test]
    async fn probe_sends_basic_auth_when_password_set() {
        let server = MockServer::start().await;
        let expected = basic_auth_header("s3cret");
        Mock::given(method("GET"))
            .and(path("/api/info"))
            .and(header(AUTHORIZATION.as_str(), expected.as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"version": "2.1", "pid": 1})),
            )
            .mount(&server)
            .await;
        let health = probe(&server.uri(), Some("s3cret")).await;
        assert_eq!(
            health.kind,
            HealthKind::V2,
            "the authed probe must identify the server"
        );
    }

    #[tokio::test]
    async fn probe_reports_unreachable_for_dead_server() {
        // Bind a port, then drop the listener so nothing answers.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let health = probe(&format!("http://127.0.0.1:{port}"), None).await;
        assert_eq!(health.kind, HealthKind::Unreachable);
    }

    #[tokio::test]
    async fn discover_finds_manual_url_and_records_unauthorized() {
        let server = MockServer::start().await;
        let denied = ResponseTemplate::new(403)
            .set_body_json(json!({"error": "Unauthorized"}))
            .append_header("content-type", "application/json");
        Mock::given(method("GET"))
            .respond_with(denied)
            .mount(&server)
            .await;
        let discovery = discover(Some(&server.uri()), None).await;
        assert!(discovery.server.is_none());
        assert_eq!(
            discovery.unauthorized_base.as_deref(),
            Some(server.uri().as_str())
        );

        let ok_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/info"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"version": "2.0", "pid": 1})),
            )
            .mount(&ok_server)
            .await;
        let discovery = discover(Some(ok_server.uri().as_str()), None).await;
        let (base, health) = discovery.server.expect("manual candidate identified");
        assert_eq!(base, ok_server.uri());
        assert_eq!(health.kind, HealthKind::V2);
        assert_eq!(discovery.unauthorized_base, None);
    }

    // -- v1 generation -------------------------------------------------------

    /// Mounts the v1 session-create + prompt mocks; returns (server, text
    /// the model "answers").
    async fn mount_v1_ok(server: &MockServer, answer: &str) {
        mount_v1_health(server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": "ses_1", "title": "mygitui"})),
            )
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "info": {"id": "ses_1"},
                "parts": [{"type": "text", "text": answer}]
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn v1_generation_roundtrip_carries_auth_and_body() {
        let server = MockServer::start().await;
        mount_v1_health(&server).await;
        let auth = basic_auth_header("pw1");
        Mock::given(method("POST"))
            .and(path("/session"))
            .and(header(AUTHORIZATION.as_str(), auth.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_1"})))
            .mount(&server)
            .await;
        let captured = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = captured.clone();
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(move |request: &wiremock::Request| {
                sink.lock()
                    .push(serde_json::from_slice(&request.body).unwrap_or(Value::Null));
                ResponseTemplate::new(200).set_body_json(json!({
                    "info": {},
                    "parts": [{"type": "text", "text": "feat(ui): add panel"}]
                }))
            })
            .mount(&server)
            .await;

        let cfg = AiConfig {
            opencode_password: Some("pw1".into()),
            model: Some("anthropic/claude-sonnet".into()),
            ..cfg_for(&server.uri())
        };
        let text = generate(
            GenRequest {
                feature: super::super::AiFeature::CommitMessage,
                system: "You write commit messages.".into(),
                prompt: "do it".into(),
                max_tokens: None,
                session_key: "roundtrip-body".into(),
            },
            &cfg,
        )
        .await
        .unwrap();
        assert_eq!(text, "feat(ui): add panel");
        let body = captured.lock().pop().unwrap();
        assert_eq!(
            body["model"],
            json!({"providerID": "anthropic", "modelID": "claude-sonnet"}),
            "model slug split onto the prompt body"
        );
        assert_eq!(body["system"], "You write commit messages.");
        assert_eq!(body["parts"][0]["text"], "do it");
    }

    #[tokio::test]
    async fn v1_session_is_reused_per_key_and_isolated_across_keys() {
        let server = MockServer::start().await;
        mount_v1_ok(&server, "ok").await;
        let cfg = cfg_for(&server.uri());

        let request = gen_req("first", "reuse-a");
        generate(request.clone(), &cfg).await.unwrap();
        generate(request, &cfg).await.unwrap();

        let is_create = |r: &wiremock::Request| r.url.path() == "/session" && r.method == "POST";
        let requests = wait_for_requests(&server, |requests| {
            requests.iter().filter(|r| is_create(r)).count() >= 1
        })
        .await;
        let creates = requests.iter().filter(|r| is_create(r)).count();
        assert_eq!(
            creates, 1,
            "the second generation reuses the cached session"
        );

        let cfg_other = cfg_for(&server.uri());
        let mut request_other = gen_req("other repo", "reuse-b");
        request_other.session_key = "repo-2".into();
        generate(request_other, &cfg_other).await.unwrap();
        let requests = wait_for_requests(&server, |requests| {
            requests.iter().filter(|r| is_create(r)).count() >= 2
        })
        .await;
        let creates = requests.iter().filter(|r| is_create(r)).count();
        assert_eq!(creates, 2, "a different session key gets its own session");
    }

    #[tokio::test]
    async fn v1_missing_session_is_recreated_once() {
        let server = MockServer::start().await;
        mount_v1_health(&server).await;
        // Mount order is match order: the one-shot 404 mock answers the
        // first message request only, then the ok mock takes over.
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "info": {}, "parts": [{"type": "text", "text": "recovered"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_1"})))
            .mount(&server)
            .await;

        // Fresh cache (unique port): the first prompt 404s against the
        // vanished session, the session is recreated once, and the retried
        // prompt answers.
        let cfg = cfg_for(&server.uri());
        let text = generate(gen_req("hello", "recreate"), &cfg).await.unwrap();
        assert_eq!(text, "recovered");
        let is_create = |r: &wiremock::Request| r.url.path() == "/session" && r.method == "POST";
        let requests = wait_for_requests(&server, |requests| {
            requests.iter().filter(|r| is_create(r)).count() >= 2
        })
        .await;
        let creates = requests.iter().filter(|r| is_create(r)).count();
        assert_eq!(creates, 2, "the vanished session is recreated exactly once");
        let prompts = requests
            .iter()
            .filter(|r| r.url.path() == "/session/ses_1/message")
            .count();
        assert_eq!(prompts, 2);
    }

    #[tokio::test]
    async fn v1_empty_parts_surfaces_info_error_or_generic() {
        let server = MockServer::start().await;
        mount_v1_health(&server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_1"})))
            .mount(&server)
            .await;
        // 200 with a failed model call: info.error names the cause.
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "info": {"error": {"name": "ProviderAuthError", "data": {"message": "no api key"}}},
                "parts": []
            })))
            .mount(&server)
            .await;
        let cfg = cfg_for(&server.uri());
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert!(matches!(err, AiError::Unauthenticated(ref m) if m.contains("no api key")));

        // No info.error either: generic no-text failure.
        let server = MockServer::start().await;
        mount_v1_health(&server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_1"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"info": {}, "parts": []})),
            )
            .mount(&server)
            .await;
        let cfg = cfg_for(&server.uri());
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert_eq!(
            err.message(),
            "opencode returned no text output for the prompt"
        );
    }

    #[tokio::test]
    async fn v1_401_maps_to_unauthenticated() {
        let server = MockServer::start().await;
        // The probe itself hits the 401 first (JSON body = opencode server
        // present, auth needed), so generate() reports it before any
        // session create.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"error": "Unauthorized"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(401).set_body_string("no"))
            .mount(&server)
            .await;
        let mut cfg = cfg_for(&server.uri());
        cfg.opencode_password = Some("wrong".into());
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert!(matches!(err, AiError::Unauthenticated(_)));
    }

    // -- v2 generation -------------------------------------------------------

    /// SSE body for one successful v2 generation (delayed so the prompt
    /// POST lands before the events are consumed).
    fn v2_success_stream(session_id: &str, text: &str) -> ResponseTemplate {
        let body = format!(
            "data: {{\"type\":\"server.connected\",\"properties\":{{}}}}\n\n\
             data: {{\"type\":\"session.text.ended\",\"properties\":{{\"sessionID\":\"{session_id}\",\"assistantMessageID\":\"m1\",\"ordinal\":0,\"text\":\"{text}\"}}}}\n\n\
             data: {{\"type\":\"session.execution.succeeded\",\"properties\":{{\"sessionID\":\"{session_id}\"}}}}\n\n"
        );
        ResponseTemplate::new(200)
            .set_body_string(body)
            .append_header("content-type", "text/event-stream")
            .set_delay(Duration::from_millis(250))
    }

    #[tokio::test]
    async fn v2_generation_roundtrip_removes_temp_session() {
        let server = MockServer::start().await;
        mount_v2_health(&server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_2"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/event"))
            .respond_with(v2_success_stream("ses_2", "Hello world"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_2/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/session/ses_2"))
            .respond_with(ResponseTemplate::new(200).set_body_string("true"))
            .mount(&server)
            .await;

        let cfg = AiConfig {
            model: Some("anthropic/claude".into()),
            directory: Some(r"\\?\C:\repos\demo".into()),
            ..cfg_for(&server.uri())
        };
        let text = generate(
            GenRequest {
                feature: super::super::AiFeature::ReviewStaged,
                system: "You review code.".into(),
                prompt: "review it".into(),
                max_tokens: None,
                session_key: "repo-1".into(),
            },
            &cfg,
        )
        .await
        .unwrap();
        assert_eq!(text, "Hello world");

        let requests = server.received_requests().await.unwrap();
        let create = requests
            .iter()
            .find(|r| r.url.path() == "/session" && r.method == "POST")
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
            .unwrap();
        assert_eq!(create["title"], "mygitui");
        assert_eq!(
            create["model"],
            json!({"providerID": "anthropic", "id": "claude"})
        );
        assert_eq!(create["location"], json!({"directory": "C:/repos/demo"}));
        assert_eq!(
            create["permissions"],
            json!([{ "action": "*", "resource": "*", "effect": "ask" }])
        );
        let prompt = requests
            .iter()
            .find(|r| r.url.path() == "/session/ses_2/message")
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
            .unwrap();
        assert_eq!(prompt["parts"][0]["text"], "You review code.\n\nreview it");
        let requests = wait_for_requests(&server, |requests| {
            requests
                .iter()
                .any(|r| r.url.path() == "/session/ses_2" && r.method == "DELETE")
        })
        .await;
        let deletes = requests
            .iter()
            .filter(|r| r.url.path() == "/session/ses_2" && r.method == "DELETE")
            .count();
        assert_eq!(deletes, 1, "the temp session is removed afterwards");
    }

    #[tokio::test]
    async fn v2_permission_asks_are_rejected() {
        let server = MockServer::start().await;
        mount_v2_health(&server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_3"})))
            .mount(&server)
            .await;
        let body = "data: {\"type\":\"server.connected\",\"properties\":{}}\n\n\
             data: {\"type\":\"permission.asked\",\"properties\":{\"id\":\"p1\",\"sessionID\":\"ses_3\"}}\n\n\
             data: {\"type\":\"session.text.ended\",\"properties\":{\"sessionID\":\"ses_3\",\"assistantMessageID\":\"m1\",\"ordinal\":0,\"text\":\"done\"}}\n\n\
             data: {\"type\":\"session.execution.succeeded\",\"properties\":{\"sessionID\":\"ses_3\"}}\n\n";

        Mock::given(method("GET"))
            .and(path("/api/event"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .append_header("content-type", "text/event-stream")
                    .set_delay(Duration::from_millis(250)),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_3/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_3/permissions/p1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("true"))
            .mount(&server)
            .await;

        let cfg = cfg_for(&server.uri());
        let text = generate(gen_req("go", "v2-perm"), &cfg).await.unwrap();
        assert_eq!(text, "done");
        let requests = wait_for_requests(&server, |requests| {
            requests
                .iter()
                .any(|r| r.url.path() == "/session/ses_3/permissions/p1")
        })
        .await;
        let rejects = requests
            .iter()
            .filter(|r| r.url.path() == "/session/ses_3/permissions/p1")
            .count();
        assert_eq!(rejects, 1, "the ask is rejected so the wait cannot hang");
    }

    #[tokio::test]
    async fn v2_execution_failure_and_missing_id_map_to_errors() {
        // execution.failed surfaces the server error message.
        let server = MockServer::start().await;
        mount_v2_health(&server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "ses_4"})))
            .mount(&server)
            .await;
        let body = "data: {\"type\":\"session.execution.failed\",\"properties\":{\"sessionID\":\"ses_4\",\"error\":{\"name\":\"NamedError\",\"data\":{\"message\":\"no key\"}}}}\n\n";
        Mock::given(method("GET"))
            .and(path("/api/event"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .append_header("content-type", "text/event-stream")
                    .set_delay(Duration::from_millis(250)),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_4/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let cfg = cfg_for(&server.uri());
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert!(matches!(err, AiError::BadResponse(ref m) if m.contains("no key")));

        // Session create without an id: bad response, nothing hangs.
        let server = MockServer::start().await;
        mount_v2_health(&server).await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"nope": true})))
            .mount(&server)
            .await;
        let cfg = cfg_for(&server.uri());
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert_eq!(err.message(), "opencode refused to create a session");
    }

    #[tokio::test]
    async fn v2_model_slug_must_be_provider_model() {
        // A v2 server so routing reaches the slug check before any prompt.
        let server = MockServer::start().await;
        mount_v2_health(&server).await;
        let cfg = AiConfig {
            model: Some("nonsense".into()),
            ..cfg_for(&server.uri())
        };
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert_eq!(
            err.message(),
            "OpenCode model 'nonsense' must use provider/model format"
        );
    }

    #[tokio::test]
    async fn unreachable_server_yields_actionable_unavailable() {
        // Nothing listens on this port.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let cfg = AiConfig {
            opencode_url: Some(format!("http://127.0.0.1:{port}")),
            ..Default::default()
        };
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert!(
            matches!(err, AiError::Unavailable(ref m) if m.contains("start it, clear the URL"))
        );

        // No URL at all: the default target is named.
        let cfg = AiConfig::default();
        let err = generate(gen_req("x", "solo-key"), &cfg).await.unwrap_err();
        assert!(
            matches!(err, AiError::Unavailable(ref m) if m.contains(DEFAULT_OPENCODE_URL)),
            "default probe failure names the probed URL: {err}"
        );
    }
}
