<script lang="ts">
	import { ContextMenu as ContextMenuPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		inset,
		children,
		...restProps
	}: WithoutChild<ContextMenuPrimitive.SubTriggerProps> & {
		inset?: boolean;
	} = $props();

	const chevronRightIcon = $derived(appIcon("chevron-right"));
</script>

<ContextMenuPrimitive.SubTrigger
	bind:ref
	data-slot="context-menu-sub-trigger"
	data-inset={inset}
	class={cn(
		"cn-context-menu-sub-trigger flex cursor-default items-center outline-hidden select-none [&_svg]:pointer-events-none [&_svg]:shrink-0",
		className
	)}
	{...restProps}
>
	{@render children?.()}
	{#if chevronRightIcon}
		<svg
			class="cn-rtl-flip ml-auto"
			viewBox="{chevronRightIcon.left ?? 0} {chevronRightIcon.top ?? 0} {chevronRightIcon.width} {chevronRightIcon.height}"
			width={chevronRightIcon.width}
			height={chevronRightIcon.height}
			aria-hidden="true"
		>{@html chevronRightIcon.body}</svg>
	{/if}
</ContextMenuPrimitive.SubTrigger>
