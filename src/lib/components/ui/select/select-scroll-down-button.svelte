<script lang="ts">
	import { Select as SelectPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: WithoutChildrenOrChild<SelectPrimitive.ScrollDownButtonProps> = $props();

	const chevronDownIcon = $derived(appIcon("chevron-down"));
</script>

<SelectPrimitive.ScrollDownButton
	bind:ref
	data-slot="select-scroll-down-button"
	class={cn("cn-select-scroll-down-button", className)}
	{...restProps}
>
	{#if chevronDownIcon}
		<svg
			viewBox="{chevronDownIcon.left ?? 0} {chevronDownIcon.top ?? 0} {chevronDownIcon.width} {chevronDownIcon.height}"
			width={chevronDownIcon.width}
			height={chevronDownIcon.height}
			aria-hidden="true"
		>{@html chevronDownIcon.body}</svg>
	{/if}
</SelectPrimitive.ScrollDownButton>
