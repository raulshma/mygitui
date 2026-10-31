<script lang="ts">
	import { Select as SelectPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		value,
		label,
		children: childrenProp,
		...restProps
	}: WithoutChild<SelectPrimitive.ItemProps> = $props();

	const checkIcon = $derived(appIcon("check"));
</script>

<SelectPrimitive.Item
	bind:ref
	{value}
	{label}
	data-slot="select-item"
	class={cn(
		"cn-select-item relative flex w-full cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
		"data-highlighted:bg-accent data-highlighted:text-accent-foreground",
		className
	)}
	{...restProps}
>
	{#snippet children({ selected, highlighted })}
		<span class="cn-select-item-indicator">
			{#if selected && checkIcon}
				<svg
					class="pointer-events-none"
					viewBox="{checkIcon.left ?? 0} {checkIcon.top ?? 0} {checkIcon.width} {checkIcon.height}"
					width={checkIcon.width}
					height={checkIcon.height}
					aria-hidden="true"
				>{@html checkIcon.body}</svg>
			{/if}
		</span>
		<span class="cn-select-item-text shrink-0 whitespace-nowrap">
			{#if childrenProp}
				{@render childrenProp({ selected, highlighted })}
			{:else}
				{label || value}
			{/if}
		</span>
	{/snippet}
</SelectPrimitive.Item>
