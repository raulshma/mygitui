<script lang="ts">
	import { Select as SelectPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		size = "default",
		...restProps
	}: WithoutChild<SelectPrimitive.TriggerProps> & {
		size?: "sm" | "default";
	} = $props();

	const chevronDownIcon = $derived(appIcon("chevron-down"));
</script>

<SelectPrimitive.Trigger
	bind:ref
	data-slot="select-trigger"
	data-size={size}
	class={cn(
		"cn-select-trigger flex w-fit items-center justify-between whitespace-nowrap outline-none disabled:cursor-not-allowed disabled:opacity-50 *:data-[slot=select-value]:line-clamp-1 *:data-[slot=select-value]:flex *:data-[slot=select-value]:items-center [&_svg]:pointer-events-none [&_svg]:shrink-0",
		className
	)}
	{...restProps}
>
	{@render children?.()}
	{#if chevronDownIcon}
		<svg
			class="cn-select-trigger-icon pointer-events-none"
			viewBox="{chevronDownIcon.left ?? 0} {chevronDownIcon.top ?? 0} {chevronDownIcon.width} {chevronDownIcon.height}"
			width={chevronDownIcon.width}
			height={chevronDownIcon.height}
			aria-hidden="true"
		>{@html chevronDownIcon.body}</svg>
	{/if}
</SelectPrimitive.Trigger>
