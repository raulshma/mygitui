/**
 * Vitest environment shims (jsdom gaps).
 *
 * jsdom implements no `window.matchMedia`, but Svelte's media-query
 * reactivity constructs one at module load — components that transitively
 * import m3-svelte (through the `$lib/components/m3` barrel) explode while
 * being imported. Stub it as "no match"; component tests never assert on
 * media queries.
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