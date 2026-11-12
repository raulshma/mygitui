/**
 * Vitest environment shims (jsdom gaps).
 *
 * jsdom implements no `window.matchMedia`, but Svelte's media-query
 * reactivity constructs one at module load. Stub it as "no match";
 * component tests never assert on media queries.
 */
if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList;
}

/**
 * jsdom defines no `ResizeObserver`, which bits-ui's shared resize observer
 * (slider/dialog content measuring) constructs inside an `$effect`. A no-op
 * stub keeps component tests free of unhandled rejections; no test asserts
 * on resize callbacks.
 */
if (typeof globalThis.ResizeObserver !== "function") {
  class ResizeObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
}

/**
 * jsdom ships no web streams either. The `forks`/`threads` pools inherit
 * them from Node, but `vmThreads` builds the global object from the jsdom
 * window alone, so SSE tests would see `ReadableStream is not defined`.
 * Node's own implementations are the ones undici brand-checks against, so
 * `new Response(stream)` keeps working across both pools.
 */
const streamWeb = await import("node:stream/web");
for (const name of [
  "ReadableStream",
  "ReadableStreamDefaultReader",
  "ReadableStreamBYOBReader",
  "ReadableStreamDefaultController",
  "ReadableByteStreamController",
  "WritableStream",
  "WritableStreamDefaultWriter",
  "WritableStreamDefaultController",
  "TransformStream",
  "TransformStreamDefaultController",
  "TextEncoderStream",
  "TextDecoderStream",
  "ByteLengthQueuingStrategy",
  "CountQueuingStrategy",
] as const) {
  const globals = globalThis as unknown as Record<string, unknown>;
  if (globals[name] === undefined) globals[name] = streamWeb[name];
}

export {};