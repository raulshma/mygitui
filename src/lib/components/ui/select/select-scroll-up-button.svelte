<script lang="ts">
	import { Select as SelectPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: WithoutChildrenOrChild<SelectPrimitive.ScrollUpButtonProps> = $props();

	const chevronUpIcon = $derived(appIcon("chevron-up"));
</script>

<SelectPrimitive.ScrollUpButton
	bind:ref
	data-slot="select-scroll-up-button"
	class={cn("cn-select-scroll-up-button", className)}
	{...restProps}
>
	{#if chevronUpIcon}
		<svg
			viewBox="{chevronUpIcon.left ?? 0} {chevronUpIcon.top ?? 0} {chevronUpIcon.width} {chevronUpIcon.height}"
			width={chevronUpIcon.width}
			height={chevronUpIcon.height}
			aria-hidden="true"
		>{@html chevronUpIcon.body}</svg>
	{/if}
</SelectPrimitive.ScrollUpButton>
