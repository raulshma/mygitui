<script lang="ts">
	import { DropdownMenu as DropdownMenuPrimitive } from "bits-ui";
	import Icon from "$lib/components/icons/Icon.svelte";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";
	import type { Snippet } from "svelte";

	let {
		ref = $bindable(null),
		checked = $bindable(false),
		indeterminate = $bindable(false),
		class: className,
		children: childrenProp,
		closeOnSelect = false,
		...restProps
	}: WithoutChildrenOrChild<DropdownMenuPrimitive.CheckboxItemProps> & {
		children?: Snippet;
	} = $props();

	const minusIcon = $derived(appIcon("remove")!);
	const checkIcon = $derived(appIcon("check")!);
</script>

<DropdownMenuPrimitive.CheckboxItem
	bind:ref
	{closeOnSelect}
	bind:checked
	bind:indeterminate
	data-slot="dropdown-menu-checkbox-item"
	class={cn(
		"cn-dropdown-menu-checkbox-item relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
		className
	)}
	{...restProps}
>
	{#snippet children({ checked, indeterminate })}
		<span
			class="cn-dropdown-menu-item-indicator pointer-events-none"
			data-slot="dropdown-menu-checkbox-item-indicator"
		>
			{#if indeterminate}
				<Icon icon={minusIcon} />
			{:else if checked}
				<Icon icon={checkIcon} />
			{/if}
		</span>
		{@render childrenProp?.()}
	{/snippet}
</DropdownMenuPrimitive.CheckboxItem>
