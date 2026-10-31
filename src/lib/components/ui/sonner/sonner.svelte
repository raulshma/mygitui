<script lang="ts">
	import { mode } from "mode-watcher";
	import { Toaster as Sonner, type ToasterProps as SonnerProps } from "svelte-sonner";
	import { appIcon } from "$lib/components/icons/library.svelte";

	let { ...restProps }: SonnerProps = $props();

	const progressIconData = $derived(appIcon("progress-activity"));
	const checkCircleIconData = $derived(appIcon("check-circle"));
	const octagonXIconData = $derived(appIcon("octagon-x"));
	const infoIconData = $derived(appIcon("info"));
	const triangleAlertIconData = $derived(appIcon("triangle-alert"));
</script>

<Sonner
	theme={mode.current}
	class="toaster group"
	style="--normal-bg: var(--color-popover); --normal-text: var(--color-popover-foreground); --normal-border: var(--color-border);"
	toastOptions={{
		classes: {
			toast: "cn-toast",
		},
	}}
	{...restProps}
>
	{#snippet loadingIcon()}
		{#if progressIconData}
			<svg
				class="size-4 animate-spin"
				viewBox="{progressIconData.left ?? 0} {progressIconData.top ?? 0} {progressIconData.width} {progressIconData.height}"
				width={progressIconData.width}
				height={progressIconData.height}
				aria-hidden="true"
			>{@html progressIconData.body}</svg>
		{/if}
	{/snippet}
	{#snippet successIcon()}
		{#if checkCircleIconData}
			<svg
				class="size-4"
				viewBox="{checkCircleIconData.left ?? 0} {checkCircleIconData.top ?? 0} {checkCircleIconData.width} {checkCircleIconData.height}"
				width={checkCircleIconData.width}
				height={checkCircleIconData.height}
				aria-hidden="true"
			>{@html checkCircleIconData.body}</svg>
		{/if}
	{/snippet}
	{#snippet errorIcon()}
		{#if octagonXIconData}
			<svg
				class="size-4"
				viewBox="{octagonXIconData.left ?? 0} {octagonXIconData.top ?? 0} {octagonXIconData.width} {octagonXIconData.height}"
				width={octagonXIconData.width}
				height={octagonXIconData.height}
				aria-hidden="true"
			>{@html octagonXIconData.body}</svg>
		{/if}
	{/snippet}
	{#snippet infoIcon()}
		{#if infoIconData}
			<svg
				class="size-4"
				viewBox="{infoIconData.left ?? 0} {infoIconData.top ?? 0} {infoIconData.width} {infoIconData.height}"
				width={infoIconData.width}
				height={infoIconData.height}
				aria-hidden="true"
			>{@html infoIconData.body}</svg>
		{/if}
	{/snippet}
	{#snippet warningIcon()}
		{#if triangleAlertIconData}
			<svg
				class="size-4"
				viewBox="{triangleAlertIconData.left ?? 0} {triangleAlertIconData.top ?? 0} {triangleAlertIconData.width} {triangleAlertIconData.height}"
				width={triangleAlertIconData.width}
				height={triangleAlertIconData.height}
				aria-hidden="true"
			>{@html triangleAlertIconData.body}</svg>
		{/if}
	{/snippet}
</Sonner>
