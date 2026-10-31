<script lang="ts">
	import { DropdownMenu as DropdownMenuPrimitive } from "bits-ui";
	import Icon from "$lib/components/icons/Icon.svelte";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children: childrenProp,
		closeOnSelect = false,
		...restProps
	}: WithoutChild<DropdownMenuPrimitive.RadioItemProps> = $props();

	const checkIcon = $derived(appIcon("check")!);
</script>

<DropdownMenuPrimitive.RadioItem
	bind:ref
	{closeOnSelect}
	data-slot="dropdown-menu-radio-item"
	class={cn(
		"cn-dropdown-menu-radio-item relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
		className
	)}
	{...restProps}
>
	{#snippet children({ checked })}
		<span
			class="cn-dropdown-menu-item-indicator pointer-events-none"
			data-slot="dropdown-menu-radio-item-indicator"
		>
			{#if checked}
				<Icon icon={checkIcon} />
			{/if}
		</span>
		{@render childrenProp?.({ checked })}
	{/snippet}
</DropdownMenuPrimitive.RadioItem>
