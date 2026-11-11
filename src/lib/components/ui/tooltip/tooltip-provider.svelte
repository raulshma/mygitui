<script lang="ts">
	import { Tooltip as TooltipPrimitive } from "bits-ui";

	/**
	 * Sidebar-flyout timing: the label pill appears after ~300ms (matching
	 * the `activity-btn` CSS flyout's transition delay) and stays instant
	 * while moving between nearby triggers.
	 */
	let {
		delayDuration = 300,
		skipDelayDuration = 300,
		...restProps
	}: TooltipPrimitive.ProviderProps = $props();
</script>

<TooltipPrimitive.Provider {delayDuration} {skipDelayDuration} {...restProps} />
