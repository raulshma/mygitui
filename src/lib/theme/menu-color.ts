/**
 * Menu color engine: applies the appearance config's menuColor
 * (default / inverted / *-translucent) to menu surfaces at runtime.
 *
 * shadcn-svelte bakes these classes into menu content components at install
 * time (transform-menu); the /create preview instead toggles classes live —
 * which is what we need in a running app. Menu contents carry the
 * `cn-menu-target` marker (vendored with the components); this module keeps
 * them in sync with the store:
 *
 * - "inverted" adds `.dark` to the content element. The `.dark { --… }`
 *   token blocks match any element carrying the class, so the portaled
 *   menu (outside <html class="dark"> in light mode) re-themes dark.
 * - "*-translucent" adds `.cn-menu-translucent`, whose per-style rule
 *   provides the `bg-popover/70` + `backdrop-blur` treatment.
 *
 * bits-ui portals mount and unmount menu content dynamically, so syncing
 * runs through a MutationObserver (rAF-throttled), mirroring upstream's
 * preview implementation.
 */
import { appearance, APPEARANCE_EVENT } from "./appearance.svelte.js";

const TARGET_SELECTOR = ".cn-menu-target";

/** Exposed for tests (and any future direct consumer of the targets). */
export const TARGET_SELECTOR_FOR_TESTS = TARGET_SELECTOR;

// Module state; startMenuColor re-runs cleanly by disposing the previous one.
let stopObserver = () => {};

/** Syncs the menuColor classes onto every mounted menu target. */
function sync(): void {
	try {
		if (typeof document === "undefined") return;
		const menuColor = appearance.config.menuColor;
		const inverted = menuColor === "inverted" || menuColor === "inverted-translucent";
		const translucent = menuColor.endsWith("translucent");
		for (const el of document.querySelectorAll(TARGET_SELECTOR)) {
			el.classList.toggle("dark", inverted);
			el.classList.toggle("cn-menu-translucent", translucent);
		}
	} catch {
		// Menu styling is best-effort; a failed sync keeps the current classes.
	}
}

/**
 * Starts keeping menu surfaces in sync with the appearance store. Idempotent
 * and never throws; returns a disposer (mainly for tests).
 */
export function startMenuColor(): () => void {
	try {
		stopObserver();

		if (typeof document === "undefined" || typeof MutationObserver === "undefined") {
			return () => {};
		}

		sync();
		window.addEventListener(APPEARANCE_EVENT, sync);

		// bits-ui portals mount menu content per open; watch for new targets.
		let scheduled = false;
		const schedule = () => {
			if (scheduled) return;
			scheduled = true;
			// rAF may not exist in every environment (jsdom); setTimeout covers it.
			const run = () => {
				scheduled = false;
				// The default menu needs no classes, and transitions to it are
				// handled by the event listener's sync above. Every DOM mutation
				// in the app lands here, so keep that common case free.
				if (appearance.config.menuColor === "default") return;
				sync();
			};
			if (typeof requestAnimationFrame === "function") {
				requestAnimationFrame(run);
			} else {
				setTimeout(run, 16);
			}
		};
		const observer = new MutationObserver((records) => {
			// Only mounts can need classes; unmounts leave nothing behind.
			if (!records.some((record) => record.addedNodes.length > 0)) return;
			schedule();
		});
		observer.observe(document.body, { childList: true, subtree: true });

		stopObserver = () => {
			try {
				observer.disconnect();
				window.removeEventListener(APPEARANCE_EVENT, sync);
			} catch {
				// Ignore.
			}
			stopObserver = () => {};
		};
	} catch {
		// Observation failures degrade to static menu styling.
	}

	return stopObserver;
}
