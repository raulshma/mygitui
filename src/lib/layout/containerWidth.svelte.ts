/**
 * Container width tracking for panel-internal responsive behavior (plan
 * §4.2: "a panel adapts to its width, not the window's") — the JS side of
 * the `@container` convention. CSS container queries cannot change Svelte
 * state, so layouts that must SWITCH STRUCTURE (files column ↔ chip strip,
 * split ↔ unified diff defaults) observe the panel root's content-box
 * width with a ResizeObserver through this helper.
 *
 * Bind the tracker's `el` to the panel root (`bind:this={tracker.el}`); the
 * observer attaches in an effect once the element exists and the width
 * reads synchronously on attach (no zero-width first paint), re-reads on
 * resize, and disconnects on teardown.
 */

/** Content width below which a panel counts as "narrow". Plan §7.5. */
export const NARROW_PANEL_PX = 700;

export interface WidthTracker {
	/**
	 * The observed element — bind a panel root here
	 * (`bind:this={tracker.el}`; the setter (re)arms the observer).
	 */
	el: HTMLElement | undefined;
	/** Latest content-box width; 0 before the element attaches. */
	readonly width: number;
	/**
	 * `width < threshold`, but false while unmeasured (width 0) so mount
	 * renders the wide layout instead of flashing the collapsed one.
	 */
	readonly narrow: boolean;
}

export function createWidthTracker(threshold: number = NARROW_PANEL_PX): WidthTracker {
	let el = $state<HTMLElement | undefined>(undefined);
	let width = $state(0);

	$effect(() => {
		const node = el;
		if (!node) return;
		width = node.clientWidth;
		const observer = new ResizeObserver((entries) => {
			width = entries[0]?.contentRect.width ?? node.clientWidth;
		});
		observer.observe(node);
		return () => {
			observer.disconnect();
			width = 0;
		};
	});

	return {
		get el() {
			return el;
		},
		set el(next: HTMLElement | undefined) {
			el = next;
		},
		get width() {
			return width;
		},
		get narrow() {
			return width > 0 && width < threshold;
		},
	};
}
