//! Small, testable GitHub REST transport built on `gh api --include`.
//!
//! `gh` remains the only process that sees credentials. Endpoints, query
//! values, methods, and JSON bodies are passed as argv/stdin, never joined into
//! a local shell command. Device repositories use the same argv and are quoted
//! by the existing remote-exec bridge.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use super::{
    forge_failure, ForgeError, ForgeRepo, ResolvedRemote, KIND_NOT_AUTHED, KIND_RATE_LIMITED,
};

/// One complete HTTP response emitted by `gh api --include`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    /// Lower-case header names. Repeated values are joined with `, `.
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

/// Pagination relations from the HTTP `Link` header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiLinks {
    pub next: Option<String>,
    pub prev: Option<String>,
    pub first: Option<String>,
    pub last: Option<String>,
}

/// Quota and retry metadata useful to the frontend without parsing gh text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitInfo {
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    pub used: Option<u64>,
    pub reset_at: Option<u64>,
    pub retry_after_seconds: Option<u64>,
}

impl ApiResponse {
    pub fn links(&self) -> ApiLinks {
        ApiLinks::parse(self.header("link").unwrap_or_default())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn rate_limit(&self) -> RateLimitInfo {
        RateLimitInfo {
            remaining: self.header_number("x-ratelimit-remaining"),
            limit: self.header_number("x-ratelimit-limit"),
            used: self.header_number("x-ratelimit-used"),
            reset_at: self.header_number("x-ratelimit-reset"),
            retry_after_seconds: self.header_number("retry-after"),
        }
    }

    fn header_number(&self, name: &str) -> Option<u64> {
        self.header(name)?.trim().parse().ok()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Result<Value, ForgeError> {
        if self.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&self.body)
            .map_err(|err| ForgeError::other(format!("invalid GitHub API JSON: {err}")))
    }
}

impl ApiLinks {
    pub fn parse(header: &str) -> Self {
        let mut links = Self::default();
        for part in split_link_values(header) {
            let Some(rest) = part.strip_prefix('<') else {
                continue;
            };
            let Some((url, params)) = rest.split_once('>') else {
                continue;
            };
            if url.is_empty() {
                continue;
            }
            for param in params.split(';').skip(1) {
                let Some((name, value)) = param.trim().split_once('=') else {
                    continue;
                };
                if !name.trim().eq_ignore_ascii_case("rel") {
                    continue;
                }
                for relation in value.trim().trim_matches('"').split_whitespace() {
                    match relation {
                        "next" => links.next = Some(url.to_owned()),
                        "prev" => links.prev = Some(url.to_owned()),
                        "first" => links.first = Some(url.to_owned()),
                        "last" => links.last = Some(url.to_owned()),
                        _ => {}
                    }
                }
            }
        }
        links
    }
}

/// Split a Link header without treating commas inside `<...>` or quoted
/// parameters as separators.
fn split_link_values(header: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut angle = false;
    let mut quoted = false;
    let mut escaped = false;
    for (idx, ch) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        match ch {
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            '"' if !angle => quoted = !quoted,
            ',' if !angle && !quoted => {
                let part = header[start..idx].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    let part = header[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// Parse `gh api --include` output. Both CRLF and LF header separators are
/// accepted. A bare JSON body is tolerated for older gh builds; an empty body
/// is treated as HTTP 204. Transport failures are classified by the caller,
/// not disguised as a successful empty response.
pub fn parse_api_response(output: &str) -> Result<ApiResponse, String> {
    if output.trim().is_empty() {
        return Ok(ApiResponse {
            status: 204,
            headers: BTreeMap::new(),
            body: String::new(),
        });
    }
    if !output.starts_with("HTTP/") {
        let trimmed = output.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return Ok(ApiResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: output.to_owned(),
            });
        }
        return Err("missing HTTP status line".to_string());
    }

    let separator = if let Some(idx) = output.find("\r\n\r\n") {
        Some((idx, 4))
    } else {
        output.find("\n\n").map(|idx| (idx, 2))
    };
    let Some((idx, width)) = separator else {
        return Err("missing HTTP header/body separator".to_string());
    };
    let head = &output[..idx];
    let body = &output[idx + width..];
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let mut fields = status_line.split_whitespace();
    if !fields
        .next()
        .is_some_and(|version| version.starts_with("HTTP/"))
    {
        return Err("invalid HTTP status line".to_string());
    }
    let status = fields
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|status| (100..600).contains(status))
        .ok_or_else(|| "invalid HTTP status code".to_string())?;

    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        let value = value.trim();
        if let Some(previous) = headers.get_mut(&name) {
            previous.push_str(", ");
            previous.push_str(value);
        } else {
            headers.insert(name, value.to_owned());
        }
    }
    Ok(ApiResponse {
        status,
        headers,
        body: body.to_owned(),
    })
}

/// Build the argv for one REST call. Query values and bodies remain separate
/// argv/stdin values; no value is interpreted by a local shell.
pub fn api_args(
    host: &str,
    method: &str,
    endpoint: &str,
    params: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Vec<String>, ForgeError> {
    if host.trim().is_empty() || endpoint.is_empty() || endpoint.starts_with('-') {
        return Err(ForgeError::other("invalid GitHub API host or endpoint"));
    }
    if !matches!(method, "GET" | "POST" | "PATCH" | "PUT" | "DELETE") {
        return Err(ForgeError::other(format!(
            "unsupported HTTP method: {method}"
        )));
    }
    let mut args = vec![
        "api".to_string(),
        "--include".to_string(),
        "--hostname".to_string(),
        host.to_string(),
        "--method".to_string(),
        method.to_string(),
    ];
    for (name, value) in params {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(ForgeError::other("invalid GitHub API query parameter"));
        }
        // `--raw-field` keeps the value literal (no `-F` type, `@file`, or
        // `{owner}` coercion); under an explicit GET method gh puts the pairs
        // on the query string, not in a form body.
        args.push("--raw-field".to_string());
        args.push(format!("{name}={value}"));
    }
    if body.is_some() {
        args.push("--input".to_string());
        args.push("-".to_string());
    }
    args.push(endpoint.to_string());
    Ok(args)
}

/// Execute one REST request and turn non-2xx responses into structured
/// [`ForgeError`] values carrying HTTP, request-id, and rate metadata.
pub fn request(
    repo: &ForgeRepo,
    remote: &ResolvedRemote,
    method: &str,
    endpoint: &str,
    params: &[(&str, &str)],
    body: Option<&Value>,
    timeout: Duration,
) -> Result<ApiResponse, ForgeError> {
    let args = api_args(&remote.host, method, endpoint, params, body)?;
    let stdin = body.map(serde_json::to_vec).transpose().map_err(|err| {
        ForgeError::other(format!("could not encode GitHub API request body: {err}"))
    })?;
    let outcome = repo.run_gh_stdin(&args, stdin.as_deref(), timeout);
    if let Some(spawn_error) = outcome.spawn_error.as_deref() {
        return Err(forge_failure(Some(spawn_error), &outcome.stderr));
    }
    if outcome.timed_out {
        return Err(ForgeError::other("GitHub API request timed out"));
    }
    if outcome.stdout.trim().is_empty() && outcome.code != Some(0) {
        return Err(forge_failure(None, &outcome.stderr));
    }

    let response = match parse_api_response(&outcome.stdout) {
        Ok(response) => response,
        Err(_parse_error) if outcome.code != Some(0) => {
            let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
            return Err(forge_failure(None, &combined));
        }
        Err(parse_error) => {
            return Err(ForgeError::other(format!(
                "could not parse gh api response: {parse_error}"
            )));
        }
    };

    if !response.is_success() {
        return Err(response_error(&response, &outcome.stderr));
    }
    if outcome.code != Some(0) {
        return Err(forge_failure(None, &outcome.stderr));
    }
    Ok(response)
}

/// Convert an HTTP error response without treating every permission 403 as a
/// quota error. Only an explicit 429 or a 403 with zero remaining quota is
/// classified as `RateLimited`.
pub fn response_error(response: &ApiResponse, stderr: &str) -> ForgeError {
    let rate = response.rate_limit();
    let request_id = response
        .header("x-github-request-id")
        .map(|value| value.to_owned().into_boxed_str());
    let body_message = api_error_message(&response.body);
    let combined = if body_message.is_empty() {
        stderr.trim().to_owned()
    } else {
        body_message
    };
    let quota_exhausted =
        response.status == 429 || (response.status == 403 && rate.remaining == Some(0));
    let mut error = if quota_exhausted {
        ForgeError {
            kind: KIND_RATE_LIMITED.to_string(),
            message: "GitHub API rate limit reached — retry after the reported reset time"
                .to_string(),
            ..ForgeError::default()
        }
    } else if response.status == 401 {
        ForgeError {
            kind: KIND_NOT_AUTHED.to_string(),
            message: "gh is not authenticated — run `gh auth login`".to_string(),
            ..ForgeError::default()
        }
    } else {
        forge_failure(None, &combined)
    };
    if error.kind == super::KIND_OTHER && !combined.trim().is_empty() {
        error.message = format!("GitHub API returned HTTP {}: {combined}", response.status);
    }
    error.http_status = Some(response.status);
    error.request_id = request_id;
    error.retry_after_seconds = rate.retry_after_seconds;
    error.rate_limit_reset_at = rate.reset_at;
    error
}

fn api_error_message(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return body.trim().chars().take(400).collect();
    };
    value
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(400).collect())
        .unwrap_or_default()
}

/// RFC 3986 unreserved encoding for one owner/repo path segment.
pub fn path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[(byte >> 4) as usize]));
            out.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    out
}

/// Reduce an absolute Link URL to the path/query accepted by `gh api`.
pub fn link_path(url: &str) -> Option<String> {
    let url = url.trim();
    let path = if let Some((_scheme, rest)) = url.split_once("://") {
        let authority_end = rest.find('/')?;
        if rest[..authority_end].contains('@') {
            return None;
        }
        &rest[authority_end..]
    } else {
        url
    };
    if !path.starts_with('/') || path.contains('#') || path.contains('\0') {
        return None;
    }
    Some(path.to_owned())
}

/// A validated continuation cursor for one endpoint family.
pub fn continuation_cursor(response: &ApiResponse, endpoint_prefix: &str) -> Option<String> {
    let next = response.links().next?;
    let path = link_path(&next)?;
    path.starts_with(endpoint_prefix).then_some(path)
}

/// Extract a numeric `page` query parameter from a Link relation URL.
pub fn link_page(response: &ApiResponse, relation: &str) -> Option<u64> {
    let url = match relation {
        "next" => response.links().next,
        "prev" => response.links().prev,
        "first" => response.links().first,
        "last" => response.links().last,
        _ => None,
    }?;
    let (_path, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        name.eq_ignore_ascii_case("page")
            .then(|| value.parse::<u64>().ok())
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_status_headers_body_and_rate_metadata() {
        let response = parse_api_response(
            "HTTP/2 201 Created\r\nContent-Type: application/json\r\nX-GitHub-Request-Id: abc-123\r\nX-RateLimit-Remaining: 42\r\nX-RateLimit-Reset: 1730000000\r\nRetry-After: 7\r\n\r\n{\"number\":12,\"body\":\"line 1\\nline 2\"}\r\n",
        )
        .unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert!(response.is_success());
        assert_eq!(response.json().unwrap()["number"], 12);
        assert_eq!(
            response.rate_limit(),
            RateLimitInfo {
                remaining: Some(42),
                reset_at: Some(1_730_000_000),
                retry_after_seconds: Some(7),
                ..RateLimitInfo::default()
            }
        );
        assert_eq!(response.header("x-github-request-id"), Some("abc-123"));
    }

    #[test]
    fn parses_link_relations_without_splitting_commas_in_urls() {
        let links = ApiLinks::parse(
            "<https://api.github.com/repos/a/b/issues?page=2&labels=a,b>; rel=\"next\", <https://api.github.com/repos/a/b/issues?page=5>; rel=\"last\"",
        );
        assert_eq!(
            links.next.as_deref(),
            Some("https://api.github.com/repos/a/b/issues?page=2&labels=a,b")
        );
        assert_eq!(
            links.last.as_deref(),
            Some("https://api.github.com/repos/a/b/issues?page=5")
        );
        assert!(links.prev.is_none());
    }

    #[test]
    fn accepts_lf_headers_bare_json_and_empty_204() {
        let response = parse_api_response("HTTP/1.1 200 OK\nLink: <x>; rel=next\n\n[]").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "[]");
        assert_eq!(parse_api_response("[]").unwrap().status, 200);
        assert_eq!(parse_api_response("").unwrap().status, 204);
        assert!(parse_api_response("not http").is_err());
    }

    #[test]
    fn builds_argv_only_requests_with_json_stdin() {
        let args = api_args(
            "ghe.example.com",
            "POST",
            "/repos/a/b/issues",
            &[("labels", "bug,help wanted")],
            Some(&json!({"title": "a; echo nope", "body": "x\ny"})),
        )
        .unwrap();
        // Exact argv: the body stays on stdin and every token stays separate.
        assert_eq!(
            args,
            vec![
                "api",
                "--include",
                "--hostname",
                "ghe.example.com",
                "--method",
                "POST",
                "--raw-field",
                "labels=bug,help wanted",
                "--input",
                "-",
                "/repos/a/b/issues",
            ]
        );
        assert!(api_args("", "GET", "/x", &[], None).is_err());
        assert!(api_args("github.com", "GET", "-x", &[], None).is_err());
        assert!(api_args("github.com", "FETCH", "/x", &[], None).is_err());
    }

    #[test]
    fn builds_get_queries_with_raw_fields() {
        // The production read path: gh rejects any flag it does not define,
        // so pin the full argv instead of matching loose `contains`.
        let args = api_args(
            "github.com",
            "GET",
            "/repos/a/b/issues",
            &[
                ("state", "open"),
                ("labels", "bug,help wanted"),
                ("q", "a&b c=d"),
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "api",
                "--include",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                "--raw-field",
                "state=open",
                "--raw-field",
                "labels=bug,help wanted",
                "--raw-field",
                "q=a&b c=d",
                "/repos/a/b/issues",
            ]
        );
        for params in [[("", "1")], [("a=b", "1")], [("a", "1\0")]] {
            assert!(api_args("github.com", "GET", "/x", &params, None).is_err());
        }
    }

    #[test]
    fn preserves_http_error_metadata_and_distinguishes_permission_403() {
        let quota = parse_api_response(
            "HTTP/2 403 Forbidden\nX-RateLimit-Remaining: 0\nX-RateLimit-Reset: 99\nX-GitHub-Request-Id: req\nRetry-After: 12\n\n{\"message\":\"rate limited\"}",
        )
        .unwrap();
        let error = response_error(&quota, "");
        assert_eq!(error.kind, KIND_RATE_LIMITED);
        assert_eq!(error.http_status, Some(403));
        assert_eq!(error.request_id.as_deref(), Some("req"));
        assert_eq!(error.retry_after_seconds, Some(12));
        assert_eq!(error.rate_limit_reset_at, Some(99));

        let permission = parse_api_response(
            "HTTP/2 403 Forbidden\nX-RateLimit-Remaining: 4000\n\n{\"message\":\"Resource not accessible\"}",
        )
        .unwrap();
        let error = response_error(&permission, "");
        assert_eq!(error.kind, super::super::KIND_OTHER);
        assert!(error.message.contains("Resource not accessible"));
    }

    #[test]
    fn validates_link_cursors_and_encodes_path_segments() {
        assert_eq!(path_segment("a b/c"), "a%20b%2Fc");
        let response = parse_api_response(
            "HTTP/2 200 OK\nLink: <https://api.github.com/repos/a/b/issues?page=2>; rel=\"next\"\n\n[]",
        )
        .unwrap();
        assert_eq!(
            continuation_cursor(&response, "/repos/a/b/issues").as_deref(),
            Some("/repos/a/b/issues?page=2")
        );
        assert!(continuation_cursor(&response, "/repos/other/repo").is_none());
        assert_eq!(link_page(&response, "next"), Some(2));
        assert!(link_path("https://user@example.com/repos/a/b").is_none());
    }
}
