/**
 * opencodeEvents (M12) — SSE-over-fetch consumer for the opencode /event
 * stream. Pure framing/decoding tests plus a mocked-stream lifecycle test
 * (no network, no EventSource — the module reads the body reader directly).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  decodeOpencodeEvent,
  parseSseFrame,
  subscribeOpencodeEvents,
  type OpencodeEventsOptions,
} from "./opencodeEvents";

function sseResponse(frames: string[]): Response {
  const encoder = new TextEncoder();
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const frame of frames) controller.enqueue(encoder.encode(frame));
      // Never closes: the test controls lifetime via stop().
    },
  });
  return new Response(stream, { status: 200 });
}

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

  it("rejects non-objects and objects without a string type", () => {
    expect(decodeOpencodeEvent("[1]")).toBeNull();
    expect(decodeOpencodeEvent('{"nope":true}')).toBeNull();
    expect(decodeOpencodeEvent("not json")).toBeNull();
  });
});

describe("subscribeOpencodeEvents", () => {
  let events: { type: string; properties: unknown }[] = [];
  let options: OpencodeEventsOptions;

  /** v1-shaped health reply — strict discovery needs JSON on /global/health. */
  function healthReply(): Response {
    return new Response('{"healthy":true,"version":"1.18.35"}', {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }

  beforeEach(() => {
    events = [];
  });

  it("delivers decoded events and tracks lastEventAt", async () => {
    // Strict discovery answers /api/info (HTML 404-ish) then
    // /global/health (JSON), then the /event stream opens.
    let call = 0;
    const urls: string[] = [];
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      call += 1;
      urls.push(String(input));
      if (call <= 2) return healthReply();
      return sseResponse([
        'data: {"type":"session.updated","properties":{}}\n\n',
        'data: {"type":"file.edited"}\n\n',
      ]);
    }) as unknown as typeof fetch;

    let ended: string | undefined;
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096",
      onEvent: (type, properties) => events.push({ type, properties }),
      onEnded: (reason) => (ended = reason),
      fetchImpl,
    });

    await vi.waitFor(() => expect(events.length).toBe(2));
    expect(events[0]).toEqual({ type: "session.updated", properties: {} });
    expect(events[1]?.type).toBe("file.edited");
    expect(session.lastEventAt()).not.toBeNull();
    // 1.x server → stream read from /event.
    expect(urls[urls.length - 1]).toBe("http://127.0.0.1:4096/event");
    session.stop();
    void ended;
  });

  it("reads a 2.x server's stream from /api/event", async () => {
    const urls: string[] = [];
    // A v2 /api/info body identifies the server as 2.x; the next call is
    // the stream itself.
    let call = 0;
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      urls.push(String(input));
      call += 1;
      if (call === 1) {
        return new Response('{"version":"2.0.18","pid":9}', {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      return sseResponse(['data: {"type":"v2.event"}\n\n']);
    }) as unknown as typeof fetch;

    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4097",
      onEvent: (type, properties) => events.push({ type, properties }),
      fetchImpl,
    });
    await vi.waitFor(() => expect(events.length).toBe(1));
    expect(events[0]?.type).toBe("v2.event");
    // /api/info identified v2 → the FIRST stream fetch is /api/event.
    expect(urls[1]).toBe("http://127.0.0.1:4097/api/event");
    session.stop();
  });

  it("reports unreachable servers and never throws", async () => {
    // Strict discovery: only a network failure (rejecting fetch) reads as
    // unreachable — auth replies surface elsewhere.
    const fetchImpl = vi.fn(async () => {
      throw new TypeError("connection refused");
    }) as unknown as typeof fetch;
    const unreachable: string[] = [];
    subscribeOpencodeEvents({
      url: "http://127.0.0.1:1",
      onEvent: () => {},
      onUnreachable: (detail) => unreachable.push(detail),
      fetchImpl,
    });
    await vi.waitFor(() => expect(unreachable.length).toBe(1), { timeout: 5000 });
  });

  it("non-ok event-stream status ends the session with 'closed'", async () => {
    let call = 0;
    const fetchImpl = vi.fn(async () => {
      call += 1;
      if (call <= 2) return healthReply();
      return new Response("no", { status: 404 });
    }) as unknown as typeof fetch;
    const ended: string[] = [];
    subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096",
      onEvent: () => {},
      onEnded: (reason) => ended.push(reason),
      fetchImpl,
    });
    await vi.waitFor(() => expect(ended).toEqual(["closed"]), { timeout: 5000 });
  });

  it("stop() aborts the stream and reports 'aborted'", async () => {
    // A bare Response body ignores the abort signal, so the test emulates
    // what a real fetch does: erroring the stream when stop() aborts.
    let streamController: ReadableStreamDefaultController<Uint8Array> | null =
      null;
    let call = 0;
    const fetchImpl = vi.fn(async () => {
      call += 1;
      if (call <= 2) return healthReply();
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          streamController = controller;
          controller.enqueue(
            new TextEncoder().encode('data: {"type":"first"}\n\n'),
          );
        },
      });
      return new Response(stream, { status: 200 });
    }) as unknown as typeof fetch;
    const ended: string[] = [];
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096",
      onEvent: (type) => events.push({ type, properties: null }),
      onEnded: (reason) => ended.push(reason),
      fetchImpl,
    });
    await vi.waitFor(() => expect(events.length).toBe(1));
    session.stop();
    session.stop(); // idempotent
    (
      streamController as unknown as ReadableStreamDefaultController<Uint8Array>
    ).error(new DOMException("aborted", "AbortError"));
    await vi.waitFor(() => expect(ended).toEqual(["aborted"]), {
      timeout: 5000,
    });
  });

  it("carries the basic-auth header when a password resolves", async () => {
    const seen: string[][] = [];
    let call = 0;
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      call += 1;
      seen.push([...new Headers(init?.headers ?? {}).keys()]);
      if (call <= 2) return healthReply();
      return sseResponse([]);
    }) as unknown as typeof fetch;
    const session = subscribeOpencodeEvents({
      url: "http://127.0.0.1:4096",
      password: () => Promise.resolve("sekrit"),
      onEvent: () => {},
      fetchImpl,
    });
    await vi.waitFor(() => expect(seen.length).toBe(3), { timeout: 5000 });
    expect(seen[2]?.map((h) => h.toLowerCase())).toContain("authorization");
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
