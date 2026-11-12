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
	 *
	 * **Lazy mounting**: virtualized lists (history rows, blame lines,
	 * file lists) render a Hint per row — mounting the full bits-ui stack
	 * (Root + Trigger + portal Content + contexts) for every row dominated
	 * per-row render cost there. This wrapper therefore renders the child
	 * with plain idle props until the pointer (or focus) actually dwells
	 * on THIS instance's trigger; only then does the tooltip machinery
	 * mount, already open. Hovering past during a scroll costs nothing.
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

	const HOVER_DELAY_MS = $derived(delayDuration ?? 300);
	/** How long a closed tooltip's machinery stays mounted (fade-out room). */
	const UNARM_DELAY_MS = 400;

	let armed = $state(false); // full tooltip machinery mounted
	let open = $state(false);
	let hoverTimer: ReturnType<typeof setTimeout> | null = null;
	let unarmTimer: ReturnType<typeof setTimeout> | null = null;
	let hoverEl: Element | null = null;

	function clearHoverTimer(): void {
		if (hoverTimer !== null) {
			clearTimeout(hoverTimer);
			hoverTimer = null;
		}
	}

	/** Dwell timer fired (or focus): mount the machinery, already open. */
	function activate(): void {
		if (empty) return;
		armed = true;
		open = true;
		if (unarmTimer !== null) {
			clearTimeout(unarmTimer);
			unarmTimer = null;
		}
	}

	/** Pointer/focus entered the idle trigger: start the dwell timer. */
	function armEnter(el: EventTarget | null): void {
		if (empty) return;
		hoverEl = el as Element | null;
		if (armed) {
			activate(); // re-enter while machinery is up: show immediately
			return;
		}
		if (hoverTimer === null) {
			hoverTimer = setTimeout(() => {
				hoverTimer = null;
				// The row may have scrolled away mid-dwell; a detached trigger
				// has nowhere to anchor the pill.
				if (hoverEl?.isConnected) activate();
			}, HOVER_DELAY_MS);
		}
	}

	/** Pointer/focus left: drop the dwell timer and (once closed) the machinery. */
	function disarm(): void {
		hoverEl = null;
		clearHoverTimer();
		if (open) {
			open = false;
			if (unarmTimer === null) {
				unarmTimer = setTimeout(() => {
					unarmTimer = null;
					if (!open) armed = false;
				}, UNARM_DELAY_MS);
			}
		}
	}

	/** Idle-phase props: shape-compatible with the real trigger props. */
	const idleProps = $derived({
		type: "button",
		"aria-label": empty ? undefined : label,
		onpointerenter: (event: Event) => armEnter(event.currentTarget),
		onpointerleave: () => disarm(),
		onfocus: () => armEnter(null),
		onblur: () => disarm(),
	});
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

{#if armed}
	<Provider>
		<TooltipPrimitive.Root
			{delayDuration}
			{open}
			onOpenChange={(next) => {
				open = next;
				if (next) {
					if (unarmTimer !== null) {
						clearTimeout(unarmTimer);
						unarmTimer = null;
					}
				} else {
					disarm();
				}
			}}
			disabled={empty}
			{...restProps}
		>
			<TooltipPrimitive.Trigger aria-label={empty ? undefined : label} {child} />
			<Content {side} sideOffset={6}>{label}</Content>
		</TooltipPrimitive.Root>
	</Provider>
{:else}
	{@render child({ props: idleProps })}
{/if}
