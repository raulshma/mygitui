<script lang="ts">
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn } from "$lib/utils.js";
	import type { SVGAttributes } from "svelte/elements";

	let {
		class: className,
		role = "status",
		// we add name, color, and stroke for compatibility with different icon libraries props.
		// only forward them when set, otherwise `stroke: undefined` overrides the icon's `currentColor`
		name,
		color,
		stroke,
		className: legacyClassName,
		"aria-label": ariaLabel = "Loading",
		...restProps
	}: SVGAttributes<SVGSVGElement> = $props();

	const progressIcon = $derived(appIcon("progress-activity"));
</script>

{#if progressIcon}
	<svg
		{role}
		{...name != null ? { name } : {}}
		{...color != null ? { color } : {}}
		{...stroke != null ? { stroke } : {}}
		{...legacyClassName != null ? { className: legacyClassName } : {}}
		aria-label={ariaLabel}
		class={cn("size-4 animate-spin", className)}
		viewBox="{progressIcon.left ?? 0} {progressIcon.top ?? 0} {progressIcon.width} {progressIcon.height}"
		width={progressIcon.width}
		height={progressIcon.height}
		{...restProps}
	>{@html progressIcon.body}</svg>
{/if}
