/**
 * mygitui's component surface over m3-svelte.
 *
 * Per the architecture decision, app code imports components from
 * `$lib/components/m3` only — never from `m3-svelte` directly. Each wrapper
 * pins a fixed local API (types + defaults) and forwards to the underlying
 * m3-svelte component, isolating the young library behind our own boundary.
 *
 * Styling note: the wrappers rely on the `--m3c-*` color aliases published by
 * `initTheme()` (see src/lib/theme) plus the static `--m3-shape-*` /
 * `--m3-elevation-*` tokens in src/app.css.
 */
export { default as Button } from "./Button.svelte";
export { default as Chip } from "./Chip.svelte";
export { default as Switch } from "./Switch.svelte";

export type { ButtonVariant, ButtonSize, ButtonIconType } from "./Button.svelte";
export type { ChipVariant } from "./Chip.svelte";
