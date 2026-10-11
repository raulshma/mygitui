/**
 * opencode SSE event stream (M12) — a fetch-based reader over the server's
 * event stream (`EventSource` can't carry the basic-auth header opencode
 * expects): `data: {json}\n\n` frames, one event per frame (`{type,
 * properties}` per the 1.x server, `{type, data}` per 2.x).
 *
 * The READY stream is handed in by the provider
 * ({@link OpenCodeProvider.subscribeLiveness} resolves the connection —
 * base URL, built auth header, and the generation-specific path
 * `/event` vs `/api/event` — fresh per attempt); this module only does SSE
 * framing. It never probes or discovers.
 *
 * Retry ownership stays with the connection supervisor (plan: "single
 * retry owner") — this module never reconnects. It consumes until the
 * signal aborts, the server closes the stream, or a read fails; the caller
 * restarts it after the next successful probe.
 */

import type { FetchLike } from "./opencode";

/** Consumer of one decoded opencode event. */
export type OpencodeEventHandler = (
  type: string,
  properties: unknown,
) => void;

export interface OpencodeEventsOptions {
  /** Ready stream URL (base + generation-specific path), pre-resolved. */
  url: string;
  /** Built `Authorization` header value; unset/`null` when none is needed. */
  authHeader?: string | null;
  /** Decoded event callback (transport-level liveness + activity). */
  onEvent: OpencodeEventHandler;
  /** Stream ended (server closed, stream fetch failed, or read failed). Never reconnects. */
  onEnded?: (reason: "aborted" | "closed" | "error", detail?: string) => void;
  fetchImpl?: FetchLike;
  now?: () => number;
}

export interface OpencodeEventSession {
  /** Epoch ms of the last decoded event (null until one arrives). */
  lastEventAt(): number | null;
  /** Aborts the stream; idempotent. */
  stop(): void;
}

/** Minimal SSE frame parser: handles multi-line data and CRLF. */
export function parseSseFrame(frame: string): string | null {
  const dataLines = frame
    .split(/\r?\n/)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).replace(/^ /, ""));
  if (dataLines.length === 0) return null;
  return dataLines.join("\n");
}

/** Decodes one SSE `data:` payload into an opencode event, when shaped right. */
export function decodeOpencodeEvent(
  data: string,
): { type: string; properties: unknown } | null {
  try {
    const parsed: unknown = JSON.parse(data);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      "type" in parsed &&
      typeof (parsed as { type: unknown }).type === "string"
    ) {
      const { type } = parsed as { type: string };
      // v1 frames carry `properties`; v2 frames carry `data`.
      const properties = (parsed as { properties?: unknown }).properties
        ?? (parsed as { data?: unknown }).data;
      return { type, properties };
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Subscribes to the ready SSE stream URL and invokes `onEvent` per decoded
 * event. Resolves the session immediately (the fetch is async inside);
 * `onEnded` fires when the stream fetch fails or ends. The returned
 * `stop()` aborts the underlying fetch.
 */
export function subscribeOpencodeEvents(
  options: OpencodeEventsOptions,
): OpencodeEventSession {
  const fetchImpl = options.fetchImpl ?? fetch.bind(globalThis);
  const now = options.now ?? Date.now;
  const controller = new AbortController();
  let last = null as number | null;
  let stopped = false;

  const stop = (): void => {
    if (stopped) return;
    stopped = true;
    try {
      controller.abort();
    } catch {
      // Already aborted — nothing to do.
    }
  };

  void (async () => {
    const headers: Record<string, string> = {
      Accept: "text/event-stream",
    };
    if (options.authHeader) headers.Authorization = options.authHeader;

    try {
      const response = await fetchImpl(new URL(options.url), {
        headers,
        signal: controller.signal,
      });
      if (!response.ok || !response.body) {
        options.onEnded?.("closed", `event stream status ${response.status}`);
        return;
      }
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      for (;;) {
        const { done, value } = await reader.read();
        if (done || stopped) {
          options.onEnded?.(stopped ? "aborted" : "closed");
          return;
        }
        buffer += decoder.decode(value, { stream: true });
        let boundary = buffer.indexOf("\n\n");
        while (boundary !== -1) {
          const frame = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const data = parseSseFrame(frame);
          if (data) {
            const event = decodeOpencodeEvent(data);
            if (event) {
              last = now();
              options.onEvent(event.type, event.properties);
            }
          }
          boundary = buffer.indexOf("\n\n");
        }
      }
    } catch (err) {
      if (stopped) {
        options.onEnded?.("aborted");
        return;
      }
      options.onEnded?.(
        "error",
        err instanceof Error ? err.message : String(err),
      );
    }
  })();

  return {
    lastEventAt: () => last,
    stop,
  };
}
