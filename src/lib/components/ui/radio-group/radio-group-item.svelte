<script lang="ts">
	import { RadioGroup as RadioGroupPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: WithoutChildrenOrChild<RadioGroupPrimitive.ItemProps> = $props();

	const circleIcon = $derived(appIcon("circle"));
</script>

<RadioGroupPrimitive.Item
	bind:ref
	data-slot="radio-group-item"
	class={cn(
		"cn-radio-group-item group/radio-group-item peer relative aspect-square shrink-0 border outline-none after:absolute after:-inset-x-3 after:-inset-y-2 disabled:cursor-not-allowed disabled:opacity-50",
		className
	)}
	{...restProps}
>
	{#snippet children({ checked })}
		<div data-slot="radio-group-indicator" class="cn-radio-group-indicator">
			{#if checked && circleIcon}
				<svg
					class="cn-radio-group-indicator-icon"
					viewBox="{circleIcon.left ?? 0} {circleIcon.top ?? 0} {circleIcon.width} {circleIcon.height}"
					width={circleIcon.width}
					height={circleIcon.height}
					aria-hidden="true"
				>{@html circleIcon.body}</svg>
			{/if}
		</div>
	{/snippet}
</RadioGroupPrimitive.Item>
