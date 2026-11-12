/**
 * App-wide open-pill registry for `Hint`.
 *
 * Every `Hint` is self-contained (own `Provider`), so bits-ui cannot
 * coordinate them — nested hints (a whole-row trigger wrapping
 * path/dir/badge triggers) could otherwise hold several pills open at
 * once. This registry enforces the tooltip invariant instead: opening
 * one pill closes whoever held the slot before it.
 *
 * Per-JS-context state, so popout windows coordinate among themselves.
 */

let activeCloser: (() => void) | null = null;

/**
 * Take the open slot, closing the pill that currently holds it (if any).
 * `close` must be idempotent — it can be invoked here and again by the
 * caller's own teardown.
 */
export function claimActiveHint(close: () => void): void {
	if (activeCloser !== null && activeCloser !== close) activeCloser();
	activeCloser = close;
}

/** Give the slot back. No-op when someone else already claimed it. */
export function releaseActiveHint(close: () => void): void {
	if (activeCloser === close) activeCloser = null;
}

/**
 * Arming grace after a scroll, covering the browser's hover recompute
 * waves: when a scroll (or the virtualizer's follow-up row replacement)
 * moves elements under a stationary pointer, the browser re-sends
 * `pointerenter` to bits-ui's trigger — which would re-open the very pill
 * the scroll dismissal just closed. The waves land within ~0-350ms of the
 * scroll; a real hover after a scroll re-arms through the ungated
 * `pointermove` path instead (pointer movement always accompanies it).
 */
const SCROLL_GRACE_MS = 600;

let lastScrollAt = 0;

/** True when a scroll happened within the arming grace window. */
export function recentlyScrolled(): boolean {
	return performance.now() - lastScrollAt < SCROLL_GRACE_MS;
}

if (typeof window !== "undefined") {
	window.addEventListener(
		"scroll",
		() => {
			lastScrollAt = performance.now();
		},
		{ capture: true, passive: true },
	);
}
