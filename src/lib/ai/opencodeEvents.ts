/**
 * opencode SSE event stream (M12) — plan §15 specified consuming the
 * server's `/event` stream; until now only the synchronous prompt round
 * trip existed.
 *
 * `EventSource` can't carry the basic-auth header opencode expects, so this
 * is a fetch-based reader over the response body: `data: {json}\n\n`
 * frames, one event per frame (`{type, properties}` per the opencode
 * server).
 *
 * Retry ownership stays with the connection supervisor (plan: "single
 * retry owner") — this module never reconnects. It consumes until the
 * signal aborts, the server closes the stream, or a read fails; the caller
 * restarts it after the next successful probe.
 */

import { basicAuthHeader, discoverOpencode, type FetchLike } from "./opencode";

/** Consumer of one decoded opencode event. */
export type OpencodeEventHandler = (
  type: string,
  properties: unknown,
) => void;

export interface OpencodeEventsOptions {
  /** Manual base URL; autodiscovers 127.0.0.1:4096 when unset. */
  url?: string;
  /** Resolves the basic-auth password (keyring), when configured. */
  password?: () => Promise<string | null>;
  /** Decoded event callback (transport-level liveness + activity). */
  onEvent: OpencodeEventHandler;
  /** Stream ended (server closed or read failed). Never reconnects. */
  onEnded?: (reason: "aborted" | "closed" | "error", detail?: string) => void;
  /** Undiscoverable server — nothing to subscribe to. */
  onUnreachable?: (detail: string) => void;
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
      const { type, properties } = parsed as {
        type: string;
        properties?: unknown;
      };
      return { type, properties };
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Subscribes to `GET {base}/event` (SSE) and invokes `onEvent` per decoded
 * event. Resolves the session immediately (discovery is async inside);
 * `onUnreachable` fires when no server answers. The returned `stop()`
 * aborts the underlying fetch.
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
    const base = await discoverOpencode(options.url, fetchImpl);
    if (!base) {
      options.onUnreachable?.(
        options.url
          ? `no opencode server at ${options.url}`
          : "no opencode server found",
      );
      return;
    }
    if (stopped) return;

    const headers: Record<string, string> = {
      Accept: "text/event-stream",
    };
    const password = (await options.password?.()) ?? null;
    if (password) headers.Authorization = basicAuthHeader(password);

    try {
      const response = await fetchImpl(new URL("/event", base), {
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
