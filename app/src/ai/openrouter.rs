//! OpenRouter backend adapter (ported from the old `openrouter.ts`): direct
//! OpenRouter API access with the user's API key (OS keyring, filled into
//! [`AiConfig`] by the service layer) through one non-streaming
//! chat/completions POST.
//!
//! Secret hygiene: the API key is read per call, never cached in module
//! state, and scrubbed from any error message before it can reach a toast
//! or log.

use reqwest::header::AUTHORIZATION;
use serde_json::{json, Value};

use super::{http, AiConfig, AiError, GenRequest};

/// Public OpenRouter API base (models listing + generation).
pub const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";

/// Default OpenRouter model (old `DEFAULT_OPENROUTER_MODEL`).
pub const DEFAULT_OPENROUTER_MODEL: &str = "anthropic/claude-sonnet-4.5";

/// Removes every occurrence of `secret` from `text` (defensive scrubbing;
/// empty/absent secrets are left alone).
pub fn scrub_secret(text: &str, secret: Option<&str>) -> String {
    match secret.filter(|secret| !secret.is_empty()) {
        Some(secret) => text.split(secret).collect::<Vec<_>>().join("***"),
        None => text.to_string(),
    }
}

/// One non-streaming generation against the public API base.
pub(crate) async fn generate(req: GenRequest, cfg: &AiConfig) -> Result<String, AiError> {
    generate_at(OPENROUTER_API_BASE, req, cfg).await
}

/// The POST itself (`base` is overridable so tests run against a mock).
async fn generate_at(base: &str, req: GenRequest, cfg: &AiConfig) -> Result<String, AiError> {
    let Some(key) = cfg
        .openrouter_api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        return Err(AiError::Unauthenticated(
            "no OpenRouter API key configured — add one in AI settings".into(),
        ));
    };
    let model = cfg
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_OPENROUTER_MODEL);

    let mut messages: Vec<Value> = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({"role": "system", "content": req.system}));
    }
    messages.push(json!({"role": "user", "content": req.prompt}));
    let mut body = json!({ "model": model, "messages": messages });
    if let Some(max_tokens) = req.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }

    let url = format!("{base}/chat/completions");
    let response = http()
        .post(&url)
        .header(AUTHORIZATION, format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
        .map_err(|err| transport_error(&err, Some(key)))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| transport_error(&err, Some(key)))?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(AiError::Unauthenticated(
            "openrouter rejected the API key (HTTP 401) — check it in AI settings".into(),
        ));
    }
    if !status.is_success() {
        // Body first (may name the real problem), then scrub defensively.
        return Err(AiError::BadResponse(scrub_secret(
            &format!(
                "openrouter generation failed: HTTP {} — {}",
                status.as_u16(),
                text
            ),
            Some(key),
        )));
    }

    let value: Value = serde_json::from_str(&text)
        .map_err(|_| AiError::BadResponse("openrouter reply was not valid JSON".into()))?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err(AiError::BadResponse(
            "openrouter returned no text output".into(),
        ));
    }
    Ok(content)
}

/// Transport failures are scrubbed (error strings may echo request
/// material) and capped.
fn transport_error(err: &reqwest::Error, key: Option<&str>) -> AiError {
    AiError::Unavailable(scrub_secret(
        &format!("openrouter request failed: {err}"),
        key,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(key: Option<&str>, model: Option<&str>) -> AiConfig {
        AiConfig {
            provider: super::super::AiProvider::OpenRouter,
            openrouter_api_key: key.map(str::to_string),
            model: model.map(str::to_string),
            ..Default::default()
        }
    }

    fn gen_req() -> GenRequest {
        GenRequest {
            feature: super::super::AiFeature::CommitMessage,
            system: "system text".into(),
            prompt: "user text".into(),
            max_tokens: Some(512),
            session_key: "repo-1".into(),
        }
    }

    #[test]
    fn scrub_secret_removes_every_occurrence() {
        assert_eq!(
            scrub_secret("key is abc123, abc123!", Some("abc123")),
            "key is ***, ***!"
        );
        assert_eq!(scrub_secret("untouched", None), "untouched");
        assert_eq!(scrub_secret("untouched", Some("")), "untouched");
    }

    #[tokio::test]
    async fn missing_key_is_unauthenticated_before_any_network() {
        let cfg = AiConfig::default();
        let err = generate(gen_req(), &cfg).await.unwrap_err();
        assert!(
            matches!(err, AiError::Unauthenticated(ref m) if m.contains("no OpenRouter API key"))
        );
    }

    #[tokio::test]
    async fn roundtrip_carries_bearer_system_and_max_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header(AUTHORIZATION.as_str(), "Bearer sk-or-test"))
            .and(body_partial_json(json!({
                "model": DEFAULT_OPENROUTER_MODEL,
                "messages": [
                    {"role": "system", "content": "system text"},
                    {"role": "user", "content": "user text"}
                ],
                "max_tokens": 512
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"role": "assistant", "content": "  feat: works  "}}]
            })))
            .mount(&server)
            .await;

        let text = generate_at(&server.uri(), gen_req(), &cfg(Some("sk-or-test"), None))
            .await
            .unwrap();
        assert_eq!(text, "feat: works", "content is trimmed");
    }

    #[tokio::test]
    async fn model_override_wins_over_default() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(json!({"model": "vendor/small-model"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"content": "ok"}}]
            })))
            .mount(&server)
            .await;
        let text = generate_at(
            &server.uri(),
            gen_req(),
            &cfg(Some("k"), Some("vendor/small-model")),
        )
        .await
        .unwrap();
        assert_eq!(text, "ok");
    }

    #[tokio::test]
    async fn key_rejection_maps_to_unauthenticated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let err = generate_at(&server.uri(), gen_req(), &cfg(Some("sk-or-bad"), None))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AiError::Unauthenticated(ref m) if m.contains("rejected the API key"))
        );
    }

    #[tokio::test]
    async fn server_errors_and_empty_output_map_to_bad_response() {
        // Non-2xx with a body: body text is included, key scrubbed.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(502).set_body_string("sk-or-test leaked upstream"))
            .mount(&server)
            .await;
        let err = generate_at(&server.uri(), gen_req(), &cfg(Some("sk-or-test"), None))
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::BadResponse(ref m) if m.contains("HTTP 502")));
        assert!(
            !err.message().contains("sk-or-test"),
            "key must be scrubbed: {err}"
        );

        // 200 with no usable content: no-text failure.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices": []})))
            .mount(&server)
            .await;
        let err = generate_at(&server.uri(), gen_req(), &cfg(Some("k"), None))
            .await
            .unwrap_err();
        assert_eq!(err.message(), "openrouter returned no text output");

        // 200 with a non-JSON body: parse failure.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        let err = generate_at(&server.uri(), gen_req(), &cfg(Some("k"), None))
            .await
            .unwrap_err();
        assert_eq!(err.message(), "openrouter reply was not valid JSON");
    }

    #[tokio::test]
    async fn dead_endpoint_maps_to_unavailable() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let err = generate_at(
            &format!("http://127.0.0.1:{port}"),
            gen_req(),
            &cfg(Some("k"), None),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AiError::Unavailable(ref m) if m.contains("openrouter request failed"))
        );
    }
}
