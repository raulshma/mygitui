<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { appIcon } from "$lib/components/icons/library.svelte";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: CommandPrimitive.ItemProps = $props();

	const checkIcon = $derived(appIcon("check"));
</script>


<CommandPrimitive.Item
	bind:ref
	data-slot="command-item"
	class={cn(
		"cn-command-item group/command-item [&_svg]:pointer-events-none [&_svg]:shrink-0",
		"data-disabled:pointer-events-none data-disabled:opacity-50",
		className
	)}
	{...restProps}
>
	{@render children?.()}
	{#if checkIcon}
		<svg
			class="ml-auto opacity-0 group-has-data-[slot=command-shortcut]/command-item:hidden group-data-[checked=true]/command-item:opacity-100"
			viewBox="{checkIcon.left ?? 0} {checkIcon.top ?? 0} {checkIcon.width} {checkIcon.height}"
			width={checkIcon.width}
			height={checkIcon.height}
			aria-hidden="true"
		>{@html checkIcon.body}</svg>
	{/if}
</CommandPrimitive.Item>
