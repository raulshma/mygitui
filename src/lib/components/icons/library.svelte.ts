/**
 * Reactive resolver for chrome icons (navigation, actions, status, menus):
 * turns a semantic icon name (see `./sets/mapping.json`) into `IconifyIcon`
 * data for the active icon library from the appearance setting
 * `appearance.config.iconLibrary`.
 *
 * Lucide — the default set — is imported statically so first paint never
 * waits on a chunk and never flashes; the other libraries are code-split
 * chunks loaded once on first need (and cached). Material Symbols is also
 * static: it is the final fallback, so every semantic name renders even when
 * a library has no equivalent (and file-type glyphs stay available here).
 *
 * File-type icons (language/media glyphs in file trees) intentionally bypass
 * this module — they stay Material Symbols always (see `./fileIcons.ts`).
 *
 * Like `src/lib/theme`, this module never throws: a failed or missing chunk
 * degrades to the fallbacks instead of breaking the caller.
 */
import type { IconifyIcon } from "@iconify/types";
import { appearance, APPEARANCE_EVENT } from "$lib/theme/appearance.svelte";
import { LUCIDE_ICONS } from "./sets/generated/lucide";
import { MATERIAL_SYMBOLS_ICONS } from "./sets/generated/material-symbols";

/** The statically imported default set (keys: semantic icon names). */
const DEFAULT_SET = "lucide";

/**
 * Chunk loaders for the non-default icon libraries, keyed by the
 * `iconLibrary` value in the appearance config (`ri` is the remixicon set).
 * Each resolves to the generated `Record<string, IconifyIcon>`.
 */
const SET_LOADERS: Record<string, () => Promise<Record<string, IconifyIcon>>> = {
	tabler: () => import("./sets/generated/tabler").then((module) => module.TABLER_ICONS),
	hugeicons: () => import("./sets/generated/hugeicons").then((module) => module.HUGEICONS_ICONS),
	phosphor: () => import("./sets/generated/phosphor").then((module) => module.PHOSPHOR_ICONS),
	remixicon: () => import("./sets/generated/ri").then((module) => module.RI_ICONS),
};

// Module-level reactive state: reads inside component `$derived`/templates
// track these, so swapping the active set re-renders every consumer.
let activeSet = $state(DEFAULT_SET);
let loadedSets = $state<Record<string, Record<string, IconifyIcon>>>({});
// Bumped whenever a chunk lands; read by appIcon so dependents always
// re-run on load (belt-and-suspenders for first-write reactivity).
let revision = $state(0);

/** Chunk loads in flight (prevents duplicate imports; retried on failure). */
const pending = new Set<string>();

/**
 * Loads a set chunk ahead of need. Shared by the config sync (the active
 * set) and the customizer's hover preloads, so a warmed library makes the
 * later selection instant. Never throws; failures stay retryable.
 */
export function preloadIconSet(library: string): void {
	try {
		if (library === DEFAULT_SET || loadedSets[library] || pending.has(library)) return;
		const loader = SET_LOADERS[library];
		if (!loader) return;
		pending.add(library);
		void loader()
			.then((icons) => {
				loadedSets[library] = icons;
				revision += 1;
			})
			.catch(() => {
				// Load failed (offline, chunk error): drop the pending mark so a
				// later call or appearance event can retry; callers fall back.
				pending.delete(library);
			});
	} catch {
		// Icon plumbing must never propagate to the caller.
	}
}

/** Applies the current config: remembers the active set, loads its chunk. */
function syncIconSet(): void {
	try {
		activeSet = appearance.config.iconLibrary;
		preloadIconSet(activeSet);
	} catch {
		// Icon plumbing must never propagate to the caller.
	}
}

let started = false;

/**
 * Subscribes the resolver to appearance changes and loads the active set's
 * chunk if needed. Idempotent: repeated calls reuse the single listener (the
 * returned disposer tears it down and a later call re-subscribes). Call it
 * after `appearance.init()` — init applies the persisted config without
 * dispatching {@link APPEARANCE_EVENT}, and this reads the config once here.
 * Returns an unsubscribe function.
 */
export function startIconSets(): () => void {
	if (!started) {
		started = true;
		syncIconSet();
		try {
			if (typeof window !== "undefined") {
				window.addEventListener(APPEARANCE_EVENT, syncIconSet);
			}
		} catch {
			// Non-browser or torn-down window: the sync above already applied.
		}
	}
	return () => {
		try {
			if (typeof window !== "undefined") {
				window.removeEventListener(APPEARANCE_EVENT, syncIconSet);
			}
		} catch {
			// Nothing to unwind.
		}
		started = false;
	};
}

/**
 * docs: resolves a semantic chrome icon for the active icon library.
 *
 * Reactive ($state-backed): components that call this inside `$derived`
 * re-render when the library switches or a chunk finishes loading. Falls
 * back active set → lucide → material-symbols, and returns null only for
 * unknown names.
 */
export function appIcon(name: string): IconifyIcon | null {
	if (!name) return null;
	void revision; // touch: re-run dependents whenever a set chunk lands
	const set = loadedSets[activeSet];
	return set?.[name] ?? LUCIDE_ICONS[name] ?? MATERIAL_SYMBOLS_ICONS[name] ?? null;
}
