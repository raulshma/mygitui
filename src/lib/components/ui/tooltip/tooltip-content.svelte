<script lang="ts">
	import { Tooltip as TooltipPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";
	import type { WithoutChildrenOrChild } from "$lib/utils.js";
	import TooltipPortal from "./tooltip-portal.svelte";
	import type { ComponentProps } from "svelte";

	/**
	 * Canonical tooltip surface: the same small pill the activity-rail
	 * buttons fly out (`@utility activity-btn` in app.css) — primary-colored,
	 * `radius-sm`, `text-xs`, fading in after ~300ms with a slight slide.
	 * Portal-mounted so it is never clipped by scroll containers.
	 */
	let {
		ref = $bindable(null),
		class: className,
		sideOffset = 0,
		side = "top",
		arrow = false,
		children,
		arrowClasses,
		portalProps,
		...restProps
	}: TooltipPrimitive.ContentProps & {
		/** Render the caret pointing at the trigger (the canonical pill has none). */
		arrow?: boolean;
		arrowClasses?: string;
		portalProps?: WithoutChildrenOrChild<ComponentProps<typeof TooltipPortal>>;
	} = $props();
</script>

<TooltipPortal {...portalProps}>
	<TooltipPrimitive.Content
		bind:ref
		data-slot="tooltip-content"
		{sideOffset}
		{side}
		class={cn(
			"cn-tooltip-content z-50 w-fit max-w-xs origin-(--bits-tooltip-content-transform-origin) rounded-sm bg-primary py-1 px-[0.55rem] text-xs leading-[1.3] text-primary-foreground shadow-sm whitespace-pre-line [overflow-wrap:anywhere]",
			className
		)}
		{...restProps}
	>
		{@render children?.()}
		{#if arrow}
			<TooltipPrimitive.Arrow>
				{#snippet child({ props })}
					<div
						class={cn(
							"cn-tooltip-arrow z-50 bg-primary fill-primary data-[side=bottom]:-translate-x-1/2 data-[side=bottom]:-translate-y-[calc(-50%_+_1px)] data-[side=left]:-translate-y-[calc(50%_-_3px)] data-[side=right]:translate-x-[calc(50%_+_2px)] data-[side=right]:translate-y-1/2 data-[side=top]:translate-x-1/2 data-[side=top]:translate-y-[calc(-50%_+_2px)]",
							arrowClasses
						)}
						{...props}
					></div>
				{/snippet}
			</TooltipPrimitive.Arrow>
		{/if}
	</TooltipPrimitive.Content>
</TooltipPortal>
