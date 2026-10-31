<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import * as InputGroup from "$lib/components/ui/input-group/index.js";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		value = $bindable(""),
		...restProps
	}: CommandPrimitive.InputProps = $props();

	const searchIcon = $derived(appIcon("search"));
</script>

<div data-slot="command-input-wrapper" class="cn-command-input-wrapper">
	<InputGroup.Root class="cn-command-input-group">
		<CommandPrimitive.Input
			{value}
			data-slot="command-input"
			class={cn(
				"cn-command-input outline-hidden disabled:cursor-not-allowed disabled:opacity-50",
				className
			)}
			{...restProps}
		>
			{#snippet child({ props })}
				<InputGroup.Input {...props} bind:value bind:ref />
			{/snippet}
		</CommandPrimitive.Input>
		<InputGroup.Addon>
			{#if searchIcon}
				<svg
					class="cn-command-input-icon"
					viewBox="{searchIcon.left ?? 0} {searchIcon.top ?? 0} {searchIcon.width} {searchIcon.height}"
					width={searchIcon.width}
					height={searchIcon.height}
					aria-hidden="true"
				>{@html searchIcon.body}</svg>
			{/if}
		</InputGroup.Addon>
	</InputGroup.Root>
</div>
