<script lang="ts">
	import type { Snippet } from "svelte";
	import { Tooltip as TooltipPrimitive } from "bits-ui";
	import Provider from "./tooltip-provider.svelte";
	import Content from "./tooltip-content.svelte";

	/**
	 * `Hint` — one-liner wrapper for the canonical tooltip: the small
	 * primary-colored pill the activity-rail buttons show (restyled
	 * `tooltip-content`), appearing after ~300ms with a fade + slide.
	 * Self-contained (own Provider), so it works anywhere, including
	 * popout windows, with no global setup. Content is portal-mounted,
	 * so it is never clipped by scroll containers.
	 *
	 * Replaces native `title=` attributes app-wide:
	 *
	 * ```svelte
	 * <Hint label="Close" side="bottom">
	 *   {#snippet child({ props })}
	 *     <button {...props} onclick={close}>×</button>
	 *   {/snippet}
	 * </Hint>
	 * ```
	 *
	 * `props` carries the trigger behavior plus `aria-label={label}`;
	 * attributes written after the spread (e.g. your own `aria-label`)
	 * win. Dynamic labels are fine: `label={cond ? "A" : "B"}`.
	 */
	let {
		label,
		side = "bottom",
		delayDuration,
		child,
		...restProps
	}: TooltipPrimitive.RootProps & {
		/** Tooltip text; also merged into the trigger as `aria-label`. */
		label: string;
		/** Which side of the trigger the pill appears on. */
		side?: "top" | "bottom" | "left" | "right";
		/** Hover delay in ms (default: the provider's ~300ms). */
		delayDuration?: number | undefined;
		/** Your element becomes the trigger — spread `props` onto it. */
		child: Snippet<[ { props: Record<string, unknown> } ]>;
	} = $props();

	/** Blank label → no pill and no aria-label (dynamic labels can be empty). */
	const empty = $derived(label.trim() === "");
</script>

<!--
	Spread `props` FIRST and write your own attributes AFTER it: `props`
	carries the trigger's `id`, `tabindex`, `type="button"`, `disabled`,
	state data-attrs and pointer/focus/click handlers, and a later
	attribute overrides the spread. On triggers with visible text, write
	`aria-label` after the spread to keep the visible label as the
	accessible name (the Hint label is otherwise merged as aria-label for
	icon-only elements, and the tooltip itself is wired up as
	aria-describedby while open).
-->

<Provider>
	<TooltipPrimitive.Root {delayDuration} disabled={empty} {...restProps}>
		<TooltipPrimitive.Trigger aria-label={empty ? undefined : label} {child} />
		<Content {side} sideOffset={6}>{label}</Content>
	</TooltipPrimitive.Root>
</Provider>
