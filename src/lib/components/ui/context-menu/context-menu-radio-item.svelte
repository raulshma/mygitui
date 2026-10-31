<script lang="ts">
	import { ContextMenu as ContextMenuPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		inset,
		children: childrenProp,
		closeOnSelect = false,
		...restProps
	}: WithoutChild<ContextMenuPrimitive.RadioItemProps> & {
		inset?: boolean;
	} = $props();

	const checkIcon = $derived(appIcon("check"));
</script>

<ContextMenuPrimitive.RadioItem
	bind:ref
	{closeOnSelect}
	data-slot="context-menu-radio-item"
	data-inset={inset}
	class={cn(
		"cn-context-menu-radio-item relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
		className
	)}
	{...restProps}
>
	{#snippet children({ checked })}
		<span class="cn-context-menu-item-indicator pointer-events-none">
			{#if checked && checkIcon}
				<svg
					viewBox="{checkIcon.left ?? 0} {checkIcon.top ?? 0} {checkIcon.width} {checkIcon.height}"
					width={checkIcon.width}
					height={checkIcon.height}
					aria-hidden="true"
				>{@html checkIcon.body}</svg>
			{/if}
		</span>
		{@render childrenProp?.({ checked })}
	{/snippet}
</ContextMenuPrimitive.RadioItem>
