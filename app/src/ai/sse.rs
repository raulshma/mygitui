//! Minimal SSE framing for the opencode event stream (ported from the old
//! `opencodeEvents.ts`) plus OpenRouter's `[DONE]` sentinel.
//!
//! Frame shape: `data: {json}\n\n`, one event per frame (`{type,
//! properties}` per 1.x servers, `{type, data}` per 2.x). Comment lines
//! (`:`), `event:` and `id:` fields are ignored — only `data:` payloads
//! surface. Retry ownership stays with the caller (this module never
//! reconnects); it consumes until the stream ends or a read fails.
//!
//! Two entry points:
//!   - [`parse_sse`] — one-shot: all complete frames' data payloads in a
//!     buffer that is assumed to hold whole frames.
//!   - [`SseDecoder`] — incremental: feed network chunks of any size,
//!     partial frames across chunk boundaries are buffered until complete.

/// OpenRouter's end-of-stream sentinel (passes through as a data payload).
pub const SSE_DONE: &str = "[DONE]";

/// True when `line` is an SSE comment/ignore line (not `data:`).
fn data_payload(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Extracts the joined `data:` payload of ONE frame (`None` when the frame
/// carries no `data:` lines). Handles CRLF line endings, multi-line data
/// (joined with `\n`) and a single trailing `\r` (per-line CR strip).
pub fn parse_sse_frame(frame: &str) -> Option<String> {
    let lines: Vec<String> = frame
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter_map(data_payload)
        .map(str::to_string)
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

/// One-shot parse: every complete frame's data payload, in order. A
/// trailing frame without its blank-line terminator is ignored (use
/// [`SseDecoder`] for chunked input). CRLF streams work: `\r\n\r\n`
/// separates frames like `\n\n` does.
pub fn parse_sse(chunk: &str) -> Vec<String> {
    let normalized = chunk.replace("\r\n", "\n");
    // `split` always yields the segment after the last blank line: an empty
    // string when the buffer ends on a frame boundary, the PARTIAL tail
    // otherwise. Either way it is not a complete frame — drop it.
    let segments: Vec<&str> = normalized.split("\n\n").collect();
    let complete = &segments[..segments.len().saturating_sub(1)];
    complete
        .iter()
        .filter_map(|frame| parse_sse_frame(frame))
        .collect()
}

/// Incremental SSE frame decoder: buffers partial frames across chunk
/// boundaries. Feed network chunks (bytes); collect complete data payloads.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
    /// Bytes not yet decodable (a multi-byte character split across chunks).
    pending: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one UTF-8 chunk, returning every data payload that became
    /// complete with it (possibly several, possibly none).
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        // Normalize CR line endings up front: CRLF becomes LF, and a CR
        // split across a chunk boundary still terminates its line once the
        // LF arrives (a lone CR is a legal SSE line terminator too).
        self.buffer
            .push_str(&chunk.replace("\r\n", "\n").replace('\r', "\n"));
        let mut out = Vec::new();
        while let Some(boundary) = self.buffer.find("\n\n") {
            let frame: String = self.buffer.drain(..boundary + 2).collect();
            if let Some(data) = parse_sse_frame(&frame) {
                out.push(data);
            }
        }
        out
    }

    /// Feeds raw network bytes: the longest valid UTF-8 prefix is decoded
    /// (a character split across chunks waits for the rest), invalid bytes
    /// are dropped, and the prefix goes through [`SseDecoder::push`].
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let (valid, invalid) = match std::str::from_utf8(&self.pending) {
            Ok(_) => (self.pending.len(), 0),
            Err(err) => (err.valid_up_to(), err.error_len().unwrap_or(0)),
        };
        let text = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
        self.pending.drain(..valid + invalid);
        self.push(&text)
    }
}

/// One decoded opencode event: `type` plus its payload (1.x `properties`
/// or 2.x `data` — normalized here so callers read one field).
#[derive(Debug, Clone, PartialEq)]
pub struct OpencodeEvent {
    pub event_type: String,
    pub properties: serde_json::Value,
}

/// Decodes one SSE `data:` payload into an opencode event, when shaped
/// right (JSON object with a string `type`; `properties` preferred, `data`
/// as the 2.x fallback). `None` for comments, `[DONE]`, or any non-event
/// JSON (numbers, arrays, other objects).
pub fn decode_opencode_event(data: &str) -> Option<OpencodeEvent> {
    let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
    let object = parsed.as_object()?;
    let event_type = object.get("type")?.as_str()?.to_string();
    let properties = match object.get("properties") {
        Some(value) if !value.is_null() => value.clone(),
        _ => object
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    };
    Some(OpencodeEvent {
        event_type,
        properties,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_single_and_multiple_events() {
        let events = parse_sse("data: {\"type\":\"a\"}\n\ndata: {\"type\":\"b\"}\n\n");
        assert_eq!(events, vec!["{\"type\":\"a\"}", "{\"type\":\"b\"}"]);
    }

    #[test]
    fn ignores_comments_event_and_id_fields() {
        let events =
            parse_sse(": keep-alive\n\nevent: ping\nid: 42\ndata: {\"type\":\"x\"}\n\n: tail\n");
        assert_eq!(events, vec!["{\"type\":\"x\"}"]);
    }

    #[test]
    fn joins_multiline_data_with_newlines() {
        let events = parse_sse("data: line1\ndata: line2\n\n");
        assert_eq!(events, vec!["line1\nline2"]);
    }

    #[test]
    fn handles_crlf_frames_and_data_lines() {
        let events = parse_sse("data: {\"type\":\"a\"}\r\n\r\ndata: l1\r\ndata: l2\r\n\r\n");
        assert_eq!(events, vec!["{\"type\":\"a\"}", "l1\nl2"]);
    }

    #[test]
    fn space_after_colon_is_optional() {
        assert_eq!(parse_sse("data:x\n\n"), vec!["x"]);
        assert_eq!(parse_sse("data: x\n\n"), vec!["x"]);
    }

    #[test]
    fn done_sentinel_passes_through() {
        let events = parse_sse("data: {\"choices\":[]}\n\ndata: [DONE]\n\n");
        assert_eq!(events, vec!["{\"choices\":[]}", SSE_DONE]);
        // [DONE] is not an opencode event.
        assert!(decode_opencode_event(SSE_DONE).is_none());
    }

    #[test]
    fn frames_without_data_yield_nothing() {
        assert!(parse_sse(": only a comment\n\n\n\n").is_empty());
        assert!(parse_sse("").is_empty());
        // A trailing partial frame (no blank line yet) is not complete.
        assert!(parse_sse("data: partial").is_empty());
    }

    #[test]
    fn decoder_buffers_partial_frames_across_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push("data: {\"type\":\"ev").is_empty());
        assert!(decoder.push("ent\",\"properties\":{\"n\":1}}\n").is_empty());
        let events = decoder.push("\ndata: {\"type\":\"two\"}\n\n");
        assert_eq!(
            events,
            vec![
                "{\"type\":\"event\",\"properties\":{\"n\":1}}",
                "{\"type\":\"two\"}"
            ]
        );
        // Decoder stays usable for the next frames.
        assert_eq!(decoder.push("data: [DONE]\n\n"), vec!["[DONE]"]);
    }

    #[test]
    fn decoder_handles_crlf_split_across_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push("data: a\r").is_empty());
        assert_eq!(decoder.push("\ndata: b\r\n\r\n"), vec!["a", "b"]);
    }

    #[test]
    fn decoder_drains_multiple_complete_frames_per_chunk() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push("data: 1\n\ndata: 2\n\ndata: 3\n\n");
        assert_eq!(events, vec!["1", "2", "3"]);
    }

    #[test]
    fn decoder_push_bytes_awaits_split_multibyte_characters() {
        let mut decoder = SseDecoder::new();
        // "é" is two bytes; split INSIDE the character across the boundary.
        let payload = "data: {\"type\":\"t\",\"properties\":{\"s\":\"é\"}}\n\n";
        let bytes = payload.as_bytes();
        let split = payload.find("é").unwrap() + 1;
        assert!(decoder.push_bytes(&bytes[..split]).is_empty());
        let events = decoder.push_bytes(&bytes[split..]);
        assert_eq!(events.len(), 1);
        let event = decode_opencode_event(&events[0]).unwrap();
        assert_eq!(event.properties["s"], "é");
    }

    #[test]
    fn decoder_push_bytes_drops_invalid_sequences() {
        let mut decoder = SseDecoder::new();
        // An invalid byte ahead of valid frames must not wedge the stream.
        let events = decoder.push_bytes(&[0xFF, b'd', b'a', b't', b'a', b':', b' ', b'x']);
        assert!(events.is_empty(), "frame not terminated yet");
        let events = decoder.push_bytes(b"\n\n");
        assert_eq!(
            events,
            vec!["x"],
            "the invalid byte was skipped, the frame survived"
        );
    }

    #[test]
    fn decodes_v1_properties_and_v2_data_payloads() {
        let v1 =
            decode_opencode_event("{\"type\":\"session.updated\",\"properties\":{\"id\":\"s1\"}}")
                .unwrap();
        assert_eq!(v1.event_type, "session.updated");
        assert_eq!(v1.properties, json!({"id": "s1"}));

        let v2 = decode_opencode_event(
            "{\"type\":\"session.execution.succeeded\",\"data\":{\"sessionID\":\"s2\"}}",
        )
        .unwrap();
        assert_eq!(v2.event_type, "session.execution.succeeded");
        assert_eq!(v2.properties, json!({"sessionID": "s2"}));

        // A literal-null properties field falls through to `data`.
        let nulled =
            decode_opencode_event("{\"type\":\"t\",\"properties\":null,\"data\":{\"k\":true}}")
                .unwrap();
        assert_eq!(nulled.properties, json!({"k": true}));

        // Neither field: Null payload.
        let bare = decode_opencode_event("{\"type\":\"server.connected\"}").unwrap();
        assert_eq!(bare.properties, serde_json::Value::Null);
    }

    #[test]
    fn rejects_malformed_event_payloads() {
        assert!(decode_opencode_event("not json").is_none());
        assert!(decode_opencode_event("42").is_none());
        assert!(decode_opencode_event("[1,2]").is_none());
        assert!(decode_opencode_event("{\"no_type\":1}").is_none());
        assert!(decode_opencode_event("{\"type\":42}").is_none());
    }
}
