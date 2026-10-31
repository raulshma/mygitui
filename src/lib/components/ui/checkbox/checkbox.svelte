<script lang="ts">
	import { Checkbox as CheckboxPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		checked = $bindable(false),
		indeterminate = $bindable(false),
		class: className,
		...restProps
	}: WithoutChildrenOrChild<CheckboxPrimitive.RootProps> = $props();

	const checkIcon = $derived(appIcon("check"));
	const removeIcon = $derived(appIcon("remove"));
</script>

<CheckboxPrimitive.Root
	bind:ref
	data-slot="checkbox"
	class={cn(
		"cn-checkbox peer relative shrink-0 outline-none after:absolute after:-inset-x-3 after:-inset-y-2 disabled:cursor-not-allowed disabled:opacity-50",
		className
	)}
	bind:checked
	bind:indeterminate
	{...restProps}
>
	{#snippet children({ checked, indeterminate })}
		<div
			data-slot="checkbox-indicator"
			class="cn-checkbox-indicator grid place-content-center text-current transition-none"
		>
			{#if checked && checkIcon}
				<svg
					viewBox="{checkIcon.left ?? 0} {checkIcon.top ?? 0} {checkIcon.width} {checkIcon.height}"
					width={checkIcon.width}
					height={checkIcon.height}
					aria-hidden="true"
				>{@html checkIcon.body}</svg>
			{:else if indeterminate && removeIcon}
				<svg
					viewBox="{removeIcon.left ?? 0} {removeIcon.top ?? 0} {removeIcon.width} {removeIcon.height}"
					width={removeIcon.width}
					height={removeIcon.height}
					aria-hidden="true"
				>{@html removeIcon.body}</svg>
			{/if}
		</div>
	{/snippet}
</CheckboxPrimitive.Root>
