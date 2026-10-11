/**
 * opencodeEvents (M12) — SSE-over-fetch consumer for the opencode event
 * stream. Pure framing/decoding tests plus a mocked-stream lifecycle test
 * (no network, no EventSource — the module reads the body reader directly).
 * The ready stream URL + auth header are handed in (the provider resolves
 * the connection); these tests exercise only the framing/transport side.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  decodeOpencodeEvent,
  parseSseFrame,
  subscribeOpencodeEvents,
} from "./opencodeEvents";

describe("parseSseFrame", () => {
  it("joins multi-line data fields with newline, strips one leading space", () => {
    // SSE spec: multi-line data joins with \n (real events are single-line
    // JSON; the join is protocol fidelity, not JSON repair).
    expect(parseSseFrame("data: {\"a\"\ndata: :1}")).toBe('{"a"\n:1}');
  });

  it("ignores comment and event lines", () => {
    expect(parseSseFrame(": keep-alive\nevent: message\ndata: x")).toBe("x");
  });

  it("returns null for data-less frames", () => {
    expect(parseSseFrame(": ping")).toBeNull();
    expect(parseSseFrame("")).toBeNull();
  });

  it("handles CRLF frames", () => {
    expect(parseSseFrame("data: ok\r\n")).toBe("ok");
  });
});

describe("decodeOpencodeEvent", () => {
  it("decodes {type, properties} payloads", () => {
    expect(decodeOpencodeEvent('{"type":"session.updated","properties":{"x":1}}')).toEqual({
      type: "session.updated",
      properties: { x: 1 },
    });
  });

  it("decodes v2 {type, data} payloads", () => {
    expect(
      decodeOpencodeEvent('{"type":"session.text.ended","data":{"sessionID":"s","text":"hi"}}'),
    ).toEqual({
      type: "session.text.ended",
      properties: { sessionID: "s", text: "hi" },
    });
  });

  it("rejects non-objects and objects without a string type", () => {
    expect(decodeOpencodeEvent("[1]")).toBeNull();
    expect(decodeOpencodeEvent('{"nope":true}')).toBeNull();
    expect(decodeOpencodeEvent("not json")).toBeNull();
  });
});

describe("subscribeOpencodeEvents", () => {
  let events: { type: string; properties: unknown }[] = [];

  /** Abort-aware SSE response: errors the stream when the signal fires
   * (what a real fetch's body does), so stop() ends the reader loop. */
  function sseResponse(frames: string[], signal?: AbortSignal | null): Response {
    const encoder = new TextEncoder();
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        for (const frame of frames) controller.enqueue(encoder.encode(frame));
        signal?.addEventListener("abort", () =>
          controller.error(new DOMException("aborted", "AbortError")),
        );
      },
    });
    return new Response(stream, { status: 200 });
  }

  beforeEach(() => {
    events = [];
  });

  it("delivers decoded events and tracks lastEventAt", async () => {
    // No discovery here: the ready stream URL is handed in and fetched
    // as-is on the first call.
    const urls: string[] = [];
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      void init;
      urls.push(String(input));
      return sseResponse([
        'data: {"type":"session.updated","properties":{}}\n\n',
        'data: {"type":"file.edited"}\n\n',
      ]);
    }) as unknown as typeof fetch;

    let ended: string | undefined;
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096/event",
      onEvent: (type, properties) => events.push({ type, properties }),
      onEnded: (reason) => (ended = reason),
      fetchImpl,
    });

    await vi.waitFor(() => expect(events.length).toBe(2));
    expect(events[0]).toEqual({ type: "session.updated", properties: {} });
    expect(events[1]?.type).toBe("file.edited");
    expect(session.lastEventAt()).not.toBeNull();
    // Exactly the ready URL the provider resolved — no probing first.
    expect(urls).toEqual(["http://127.0.0.1:4096/event"]);
    session.stop();
    void ended;
  });

  it("fetches exactly the ready URL it is given (v2 path chosen by the provider)", async () => {
    const urls: string[] = [];
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      urls.push(String(input));
      return sseResponse(['data: {"type":"v2.event"}\n\n']);
    }) as unknown as typeof fetch;

    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4097/api/event",
      onEvent: (type, properties) => events.push({ type, properties }),
      fetchImpl,
    });
    await vi.waitFor(() => expect(events.length).toBe(1));
    expect(events[0]?.type).toBe("v2.event");
    expect(urls).toEqual(["http://127.0.0.1:4097/api/event"]);
    session.stop();
  });

  it("reports stream-fetch failures as onEnded('error') and never throws", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new TypeError("connection refused");
    }) as unknown as typeof fetch;
    const ended: Array<[string, string | undefined]> = [];
    subscribeOpencodeEvents({
      url: "http://127.0.0.1:1/event",
      onEvent: () => {},
      onEnded: (reason, detail) => ended.push([reason, detail]),
      fetchImpl,
    });
    await vi.waitFor(() => expect(ended.length).toBe(1), { timeout: 5000 });
    expect(ended[0]?.[0]).toBe("error");
    expect(ended[0]?.[1]).toContain("connection refused");
  });

  it("non-ok event-stream status ends the session with 'closed'", async () => {
    const fetchImpl = vi.fn(async () => new Response("no", { status: 404 })) as unknown as typeof fetch;
    const ended: string[] = [];
    subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096/event",
      onEvent: () => {},
      onEnded: (reason) => ended.push(reason),
      fetchImpl,
    });
    await vi.waitFor(() => expect(ended).toEqual(["closed"]), { timeout: 5000 });
  });

  it("stop() aborts the stream and reports 'aborted'", async () => {
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) =>
      sseResponse(['data: {"type":"first"}\n\n'], init?.signal),
    ) as unknown as typeof fetch;
    const ended: string[] = [];
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096/event",
      onEvent: (type) => events.push({ type, properties: null }),
      onEnded: (reason) => ended.push(reason),
      fetchImpl,
    });
    await vi.waitFor(() => expect(events.length).toBe(1));
    session.stop();
    session.stop(); // idempotent
    await vi.waitFor(() => expect(ended).toEqual(["aborted"]), {
      timeout: 5000,
    });
  });

  it("carries the given auth header on the stream request", async () => {
    const seen: Array<string | null> = [];
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      seen.push(new Headers(init?.headers ?? {}).get("authorization"));
      return sseResponse([]);
    }) as unknown as typeof fetch;
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096/api/event",
      authHeader: `Basic ${btoa("opencode:sekrit")}`,
      onEvent: () => {},
      fetchImpl,
    });
    await vi.waitFor(() => expect(seen.length).toBe(1), { timeout: 5000 });
    expect(seen[0]).toBe(`Basic ${btoa("opencode:sekrit")}`);
    session.stop();
  });

  it("sends no Authorization header when no authHeader is given", async () => {
    const seen: Array<string | null> = [];
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      seen.push(new Headers(init?.headers ?? {}).get("authorization"));
      return sseResponse([]);
    }) as unknown as typeof fetch;
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096/event",
      onEvent: () => {},
      fetchImpl,
    });
    await vi.waitFor(() => expect(seen.length).toBe(1), { timeout: 5000 });
    expect(seen[0]).toBeNull();
    session.stop();
  });
});

describe("supervisor.noteTransportEvent", () => {
  it("refreshes lastCheck while ok and triggers a probe while down", async () => {
    const { ConnectionSupervisor } = await import("./connection");
    let probeCalls = 0;
    let nowMs = 1_000;
    const supervisor = new ConnectionSupervisor({
      providers: {
        opencode: {
          id: "opencode" as const,
          available: async () => {
            probeCalls += 1;
            return true;
          },
          listModels: async () => [],
          generate: async () => {
            throw new Error("not used in this test");
          },
        } as never,
        openrouter: {
          id: "openrouter" as const,
          available: async () => true,
          listModels: async () => [],
          generate: async () => {
            throw new Error("not used in this test");
          },
        } as never,
      },
      config: () =>
        ({
          backend: "opencode",
          allowFallback: false,
          repoOptIn: {},
        }) as never,
      now: () => nowMs,
      scheduler: {
        setTimeout: (handler: () => void) => {
          handler();
          return 0;
        },
        clearTimeout: () => {},
      },
      jitter: () => 0,
    });

    await supervisor.check("opencode");
    expect(supervisor.status("opencode").status).toBe("ok");
    const first = supervisor.status("opencode").lastCheck;

    nowMs = 5_000;
    supervisor.noteTransportEvent("opencode");
    expect(supervisor.status("opencode").lastCheck).toBe(5_000);
    expect(probeCalls).toBe(1);

    // Down: the note asks for a real probe instead of blind freshness.
    supervisor.statuses.opencode.status = "down";
    await supervisor.noteTransportEvent("opencode");
    await vi.waitFor(() => expect(probeCalls).toBe(2));
    void first;
  });
});
