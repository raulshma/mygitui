/**
 * Reactive appearance store: the shadcn-svelte customizer state (style,
 * base color, theme, chart color, fonts, icon library, radius, menu) and
 * the runtime applier.
 *
 * Application mechanism mirrors the shadcn-svelte /create preview: a single
 * managed `<style>` element whose `:root` / `.dark` blocks override the
 * static token sets in app.css by cascade order, plus a `style-<name>` class
 * on <html> that selects the active style sheet. Both schemes are written
 * once per change; scheme switching stays class-based (see `./index.ts`) and
 * never recomputes tokens.
 *
 * Fonts load on demand: each fontsource package is a dynamic import that
 * Vite code-splits into its own CSS/font chunk, so only the selected faces
 * are ever fetched (and cached for the session). Inter ships statically —
 * it is the default body font and has no chunk at all.
 *
 * The config persists to localStorage "mygitui.appearance". Every mutation
 * normalizes through {@link normalizeAppearance}, so persisted or decoded
 * values can never put the store into an invalid state. Never throws.
 */
import {
	DEFAULT_APPEARANCE,
	decodeAppearance,
	encodeAppearance,
	normalizeAppearance,
	randomAppearance,
	type AppearanceConfig,
} from "./preset/codec.js";
import { buildFontVars, buildThemeVars } from "./preset/composer.js";
import { getFont, type FontHeadingName } from "./preset/values.js";

/** localStorage key holding the serialized {@link AppearanceConfig}. */
export const APPEARANCE_STORAGE_KEY = "mygitui.appearance";

/** Window event dispatched on every applied change (detail = config). */
export const APPEARANCE_EVENT = "mygitui:appearance";

/** id of the managed style element (stable: re-created, never duplicated). */
const STYLE_ELEMENT_ID = "mygitui-appearance";

/** id of the managed font-vars element (separate: swaps on face readiness). */
const FONT_ELEMENT_ID = "mygitui-appearance-fonts";

const FALLBACK_SANS = "'Inter Variable', sans-serif";

/**
 * Preset code of the defaults. The default config's tokens and fonts are
 * exactly what app.css ships, so an exact-default store (first run, reset)
 * injects no runtime CSS at all.
 */
const DEFAULT_CODE = encodeAppearance(DEFAULT_APPEARANCE);

// ---------------------------------------------------------------------------
// Font loading: dynamic per-font chunks (see module docstring)
// ---------------------------------------------------------------------------

const FONT_LOADERS: Record<string, () => Promise<unknown>> = {
	"noto-sans": () => import("@fontsource-variable/noto-sans"),
	"nunito-sans": () => import("@fontsource-variable/nunito-sans"),
	figtree: () => import("@fontsource-variable/figtree"),
	roboto: () => import("@fontsource-variable/roboto"),
	raleway: () => import("@fontsource-variable/raleway"),
	"dm-sans": () => import("@fontsource-variable/dm-sans"),
	"public-sans": () => import("@fontsource-variable/public-sans"),
	outfit: () => import("@fontsource-variable/outfit"),
	"jetbrains-mono": () => import("@fontsource-variable/jetbrains-mono"),
	geist: () => import("@fontsource-variable/geist"),
	"geist-mono": () => import("@fontsource-variable/geist-mono"),
	lora: () => import("@fontsource-variable/lora"),
	merriweather: () => import("@fontsource-variable/merriweather"),
	"playfair-display": () => import("@fontsource-variable/playfair-display"),
	"noto-serif": () => import("@fontsource-variable/noto-serif"),
	"roboto-slab": () => import("@fontsource-variable/roboto-slab"),
	oxanium: () => import("@fontsource-variable/oxanium"),
	manrope: () => import("@fontsource-variable/manrope"),
	"space-grotesk": () => import("@fontsource-variable/space-grotesk"),
	montserrat: () => import("@fontsource-variable/montserrat"),
	"ibm-plex-sans": () => import("@fontsource-variable/ibm-plex-sans"),
	"source-sans-3": () => import("@fontsource-variable/source-sans-3"),
	"instrument-sans": () => import("@fontsource-variable/instrument-sans"),
	"eb-garamond": () => import("@fontsource-variable/eb-garamond"),
	"instrument-serif": () => import("@fontsource/instrument-serif"),
};

/**
 * Style sheets beyond the default load on demand (one code-split CSS chunk
 * per style, kept in the document once fetched). Nova ships with app.css —
 * see the `@import` there.
 */
const STYLE_LOADERS: Partial<Record<string, () => Promise<unknown>>> = {
	vega: () => import("./styles/style-vega.css"),
	maia: () => import("./styles/style-maia.css"),
	lyra: () => import("./styles/style-lyra.css"),
	mira: () => import("./styles/style-mira.css"),
	luma: () => import("./styles/style-luma.css"),
	sera: () => import("./styles/style-sera.css"),
	rhea: () => import("./styles/style-rhea.css"),
};

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

class AppearanceStore {
	#config = $state<AppearanceConfig>(DEFAULT_APPEARANCE);
	/** In-flight / landed chunk loads, keyed by font or style name. */
	#fontChunks = new Map<string, Promise<unknown>>();
	#styleChunks = new Map<string, Promise<unknown>>();
	/** Fonts whose chunks are in the document (Inter is static). */
	#readyFonts = new Set<string>(["inter"]);
	#initialized = false;

	/** The current customizer config (normalized; safe to persist/share). */
	get config(): AppearanceConfig {
		return this.#config;
	}

	/** The current preset code — paste-able on shadcn-svelte.com/create. */
	get presetCode(): string {
		return encodeAppearance(this.#config);
	}

	/**
	 * Warms a font chunk without applying anything (customizer hover).
	 * Selecting the font afterwards then swaps families instantly.
	 */
	preloadFont(name: string): void {
		this.#fontChunk(name);
	}

	/**
	 * Warms a style sheet chunk without applying anything (customizer
	 * hover). Selecting the style afterwards then applies on click.
	 */
	preloadStyle(name: string): void {
		this.#styleChunk(name);
	}

	/** Init from storage and apply. Safe to call again (re-applies). */
	init(): void {
		this.#initialized = true;
		const stored = loadStoredConfig();
		if (stored) {
			this.#config = stored;
		}
		this.#applyToDocument();
	}

	/**
	 * Patches the config (a picker writes one field; the customizer may
	 * write several). Normalizes, persists, applies. Returns the effective
	 * config after normalization.
	 */
	set(patch: Partial<AppearanceConfig>): AppearanceConfig {
		this.#config = normalizeAppearance({ ...this.#config, ...patch });
		this.#persistAndApply();
		return this.#config;
	}

	/**
	 * Applies a preset code (from /create or the app's own copy action).
	 * Returns false (unchanged) when the code is malformed.
	 */
	applyPresetCode(code: string): boolean {
		const config = decodeAppearance(code.trim());
		if (!config) return false;
		this.#config = config;
		this.#persistAndApply();
		return true;
	}

	/** Randomizes everything (the "Shuffle" button). */
	shuffle(): void {
		this.#config = randomAppearance();
		this.#persistAndApply();
	}

	/** Resets to the app's default appearance. */
	reset(): void {
		this.#config = DEFAULT_APPEARANCE;
		this.#persistAndApply();
	}

	#persistAndApply(): void {
		if (this.#initialized) {
			saveConfig(this.#config);
		}
		this.#applyToDocument();
		notify(this.#config);
	}

	/**
	 * Loads a chunk once per key, cached in `cache`; a failure is forgotten
	 * so a later call retries the load. `onLanded` fires once, when the chunk
	 * first succeeds.
	 */
	#chunk(
		cache: Map<string, Promise<unknown>>,
		key: string,
		load: () => Promise<unknown>,
		onLanded?: () => void,
	): Promise<unknown> {
		let chunk = cache.get(key);
		if (!chunk) {
			chunk = load();
			cache.set(key, chunk);
			void chunk.then(
				() => onLanded?.(),
				() => cache.delete(key), // forget failures so a later selection retries
			);
		}
		return chunk;
	}

	/** The font chunk for a font (null: static like Inter, or unknown). */
	#fontChunk(name: string): Promise<unknown> | null {
		const fontLoader = FONT_LOADERS[name];
		if (!fontLoader) return null;
		return this.#chunk(this.#fontChunks, name, fontLoader, () => {
			this.#readyFonts.add(name);
			// The face is in the document: if the current config was
			// waiting on it, its family swap lands now (single reflow).
			this.#applyFontVars();
		});
	}

	/** The style sheet chunk for a style (null: static like nova, unknown). */
	#styleChunk(name: string): Promise<unknown> | null {
		const styleLoader = STYLE_LOADERS[name];
		if (!styleLoader) return null;
		return this.#chunk(this.#styleChunks, name, styleLoader);
	}

	/**
	 * Writes the `--font-sans` / `--font-heading` overrides — but only once
	 * every face the current config needs is in the document. Until then the
	 * previous write (or app.css's Inter default) keeps rendering, so picking
	 * a font swaps straight to the real face with one reflow instead of a
	 * fallback flash plus a swap reflow. A font whose chunk never lands
	 * (offline) simply keeps the previous family.
	 */
	#applyFontVars(): void {
		try {
			const config = this.#config;
			for (const name of fontNames(config)) {
				if (!this.#readyFonts.has(name)) return; // previous family keeps rendering
			}
			const element = document.getElementById(FONT_ELEMENT_ID);
			const css = buildFontCss(config);
			if (!css) {
				element?.remove(); // defaults: app.css already ships these vars
			} else if (element) {
				element.textContent = css;
			} else {
				const created = document.createElement("style");
				created.id = FONT_ELEMENT_ID;
				created.textContent = css;
				document.head.appendChild(created);
			}
		} catch {
			// A theming failure must never propagate to the caller.
		}
	}

	/** Applies the current config to the DOM (fonts, tokens, style sheet). */
	#applyToDocument(): void {
		if (typeof document === "undefined") return;
		try {
			const root = document.documentElement;
			const config = this.#config;

			// Token overrides — one style element, rebuilt per change. The
			// exact default needs no element: app.css already ships those
			// tokens, and injecting them again only adds matching work.
			const element = document.getElementById(STYLE_ELEMENT_ID) as HTMLStyleElement | null;
			if (encodeAppearance(config) === DEFAULT_CODE) {
				element?.remove();
			} else if (element) {
				element.textContent = buildCss(config);
			} else {
				const created = document.createElement("style");
				created.id = STYLE_ELEMENT_ID;
				created.textContent = buildCss(config);
				document.head.appendChild(created);
			}

			// Fonts: kick the chunk loads; the family vars swap in through
			// #applyFontVars when the faces land (see it for the reasoning).
			for (const name of fontNames(config)) void this.#fontChunk(name);
			this.#applyFontVars();

			// Style sheet: the class activates a sheet, so defer it until that
			// sheet's chunk is in the document (nova is static, others load on
			// demand once and stay — preloads share the same in-flight chunk).
			// Last selection wins the race.
			const chunk = this.#styleChunk(config.style);
			if (chunk) {
				void chunk
					.then(() => {
						if (this.#config.style === config.style) {
							setStyleClass(root, config.style);
						}
					})
					.catch(() => {
						// Chunk failed (offline, stubbed env): select the style
						// anyway — tokens/fonts still apply, sheet rules are
						// degraded — and let a later change retry the chunk.
						if (this.#config.style === config.style) {
							setStyleClass(root, config.style);
						}
					});
				return; // Class arrives with the chunk; tokens/fonts applied.
			}
			setStyleClass(root, config.style);
		} catch {
			// A theming failure must never propagate to the caller.
		}
	}
}

export const appearance = new AppearanceStore();

// ---------------------------------------------------------------------------
// Persistence (defensive like ./index.ts: corrupt values fall back to defaults)
// ---------------------------------------------------------------------------

function loadStoredConfig(): AppearanceConfig | null {
	try {
		if (typeof localStorage === "undefined") return null;
		const raw = localStorage.getItem(APPEARANCE_STORAGE_KEY);
		if (!raw) return null;
		const parsed = JSON.parse(raw);
		if (typeof parsed !== "object" || parsed === null) return null;
		return normalizeAppearance(parsed as Partial<AppearanceConfig>);
	} catch {
		return null;
	}
}

function saveConfig(config: AppearanceConfig): void {
	try {
		if (typeof localStorage === "undefined") return;
		localStorage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(config));
	} catch {
		// Storage may be full or unavailable; the choice stays in memory.
	}
}

function notify(config: AppearanceConfig): void {
	try {
		if (typeof window === "undefined") return;
		window.dispatchEvent(new CustomEvent<AppearanceConfig>(APPEARANCE_EVENT, { detail: config }));
	} catch {
		// CustomEvent/window may be unavailable in tests; the state is set.
	}
}

// ---------------------------------------------------------------------------
// Application helpers
// ---------------------------------------------------------------------------

/** Root class selecting the active style sheet (`.style-<name>`). */
function setStyleClass(root: HTMLElement, style: string): void {
	for (const cls of Array.from(root.classList)) {
		if (cls.startsWith("style-")) root.classList.remove(cls);
	}
	root.classList.add(`style-${style}`);
}

function fontNames(config: AppearanceConfig): string[] {
	const names = [config.font];
	if (config.fontHeading !== "inherit") names.push(config.fontHeading);
	return names;
}

/** Builds the `:root` / `.dark` token override block for a config. */
export function buildCss(config: AppearanceConfig): string {
	const { light, dark } = buildThemeVars(config);
	return `${schemeBlock(":root", light)}\n${schemeBlock(".dark", dark)}`;
}

/**
 * The `--font-sans` / `--font-heading` override block for a config. Null for
 * the defaults: app.css ships those exact vars, so nothing gets injected.
 */
export function buildFontCss(config: AppearanceConfig): string | null {
	if (config.font === "inter" && config.fontHeading === "inherit") return null;
	const fonts = buildFontVars(config);
	return schemeBlock(":root", { "--font-sans": fonts.sans, "--font-heading": fonts.heading });
}

function schemeBlock(selector: string, vars: Record<string, string>): string {
	const lines = Object.entries(vars)
		.map(([name, value]) => `\t--${name}: ${value};`)
		.join("\n");
	return `${selector} {\n${lines}\n}`;
}

/** The CSS font-family value for a heading choice (UI preview labels). */
export function fontPreviewFamily(name: FontHeadingName): string {
	return getFont(name)?.family ?? FALLBACK_SANS;
}
