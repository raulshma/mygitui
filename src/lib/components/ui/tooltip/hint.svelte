<script lang="ts">
	import type { Snippet } from "svelte";
	import { Tooltip as TooltipPrimitive } from "bits-ui";
	import Provider from "./tooltip-provider.svelte";
	import Content from "./tooltip-content.svelte";
	import { claimActiveHint, recentlyScrolled, releaseActiveHint } from "./hint-active.js";

	/**
	 * `Hint` — one-liner wrapper for the canonical tooltip: the small
	 * primary-colored pill the activity-rail buttons show (restyled
	 * `tooltip-content`), appearing after ~300ms with a fade + slide.
	 * Self-contained (own Provider), so it works anywhere, including
	 * popout windows, with no global setup. Content is portal-mounted,
	 * so it is never clipped by scroll containers.
	 *
	 * Replaces native `title=` attributes app-wide:
	 *
	 * ```svelte
	 * <Hint label="Close" side="bottom">
	 *   {#snippet child({ props })}
	 *     <button {...props} onclick={close}>×</button>
	 *   {/snippet}
	 * </Hint>
	 * ```
	 *
	 * `props` carries the trigger behavior (`id`, `tabindex`, `type`,
	 * pointer/focus handlers, state data-attrs). If your trigger is
	 * icon-only, give it an `aria-label` yourself — the hint text is NOT
	 * injected as a name (it would shadow text buttons). Dynamic labels are
	 * fine: `label={cond ? "A" : "B"}`.
	 *
	 * **Lazy mounting**: virtualized lists (history rows, blame lines,
	 * file lists) render a Hint per row — mounting the full bits-ui stack
	 * (Root + Trigger + portal Content + contexts) for every row dominated
	 * per-row render cost there. This wrapper therefore renders the child
	 * with plain idle props until the pointer (or focus) actually dwells
	 * on THIS instance's trigger; only then does the tooltip machinery
	 * mount, already open. Hovering past during a scroll costs nothing.
	 *
	 * **Dismissal guarantees**: at most one pill is open app-wide (opening
	 * one closes the previous — nested row/path/badge triggers can't stack
	 * pills), the pill is not hoverable (`pointer-events: none`, so it can
	 * neither trap the pointer nor block the rows beneath it), and while a
	 * pill is open a sweep guard closes it on scroll, wheel, pointer
	 * movement outside the trigger, pointerdown outside the trigger,
	 * window blur and window exit — covering the swaps and scrolls where
	 * `pointerleave` never arrives.
	 */
	let {
		label,
		side = "bottom",
		delayDuration,
		child: children,
		...restProps
	}: TooltipPrimitive.RootProps & {
		/** Tooltip text; also merged into the trigger as `aria-label`. */
		label: string;
		/** Which side of the trigger the pill appears on. */
		side?: "top" | "bottom" | "left" | "right";
		/** Hover delay in ms (default: the provider's ~300ms). */
		delayDuration?: number | undefined;
		/** Your element becomes the trigger — spread `props` onto it. */
		child: Snippet<[ { props: Record<string, unknown> } ]>;
	} = $props();

	/** Blank label → no pill and no aria-label (dynamic labels can be empty). */
	const empty = $derived(label.trim() === "");

	const HOVER_DELAY_MS = $derived(delayDuration ?? 300);
	/** How long a closed tooltip's machinery stays mounted (fade-out room). */
	const UNARM_DELAY_MS = 400;

	let armed = $state(false); // full tooltip machinery mounted
	let open = $state(false);
	let hoverTimer: ReturnType<typeof setTimeout> | null = null;
	let unarmTimer: ReturnType<typeof setTimeout> | null = null;
	let hoverEl: Element | null = null;

	function clearHoverTimer(): void {
		if (hoverTimer !== null) {
			clearTimeout(hoverTimer);
			hoverTimer = null;
		}
	}

	/** Dwell timer fired (or focus): mount the machinery, already open. */
	function activate(): void {
		if (empty) return;
		armed = true;
		open = true;
		if (unarmTimer !== null) {
			clearTimeout(unarmTimer);
			unarmTimer = null;
		}
	}

	/** Close the pill (and drop the machinery once faded out). */
	function closePill(): void {
		disarm();
	}

	/**
	 * While a pill is open, one pointer sweep guard closes it the moment a
	 * pointer event lands OUTSIDE the trigger — the dismissal that pointer
	 * events alone cannot deliver. Activating swaps the idle trigger for the
	 * real one (the element under the pointer is replaced), and a browser
	 * whose hover chain still references the replaced element may never
	 * fire `pointerleave` on the new trigger; scrolling moves rows under a
	 * stationary pointer with no boundary events at all. The sweep catches
	 * every such case: the next pointer movement, scroll, wheel, click
	 * outside, window blur or window exit closes the pill.
	 */
	function sweepPointer(event: PointerEvent): void {
		const target = event.target;
		// Unknown trigger yet (mid-swap): don't judge by a stale anchor.
		if (triggerEl === null) return;
		if (target instanceof Node && triggerEl.contains(target)) return;
		disarm();
	}

	/** Clicking inside the trigger keeps the pill (the pointer is there). */
	function sweepPointerDown(event: PointerEvent): void {
		const target = event.target;
		if (triggerEl === null) return;
		if (target instanceof Node && triggerEl.contains(target)) return;
		disarm();
	}

	/** `pointerout` with no `relatedTarget` = the pointer left the window. */
	function sweepWindowExit(event: PointerEvent): void {
		if (event.relatedTarget === null) disarm();
	}

	// The pill never traps the pointer either: `disableHoverableContent`
	// renders the portal content with `pointer-events: none`, so a pill
	// hanging below a list row can no longer swallow the hovers meant for
	// the rows beneath it (and keep itself open).
	$effect(() => {
		if (!open) return;
		claimActiveHint(closePill);
		const capture = { capture: true } as const;
		window.addEventListener("scroll", disarm, capture);
		window.addEventListener("wheel", disarm, capture);
		window.addEventListener("pointerdown", sweepPointerDown, capture);
		window.addEventListener("pointerout", sweepWindowExit);
		window.addEventListener("blur", disarm);
		document.addEventListener("pointermove", sweepPointer, capture);
		return () => {
			releaseActiveHint(closePill);
			window.removeEventListener("scroll", disarm, capture);
			window.removeEventListener("wheel", disarm, capture);
			window.removeEventListener("pointerdown", sweepPointerDown, capture);
			window.removeEventListener("pointerout", sweepWindowExit);
			window.removeEventListener("blur", disarm);
			document.removeEventListener("pointermove", sweepPointer, capture);
		};
	});

	/**
	 * Arm the dwell timer. Pointer arming flows ONLY through `pointermove`:
	 * a real hover always produces one on the trigger (the same gesture
	 * dispatches it, and it bubbles from the row's children), while the
	 * browser's hover recompute for a stationary pointer — scroll moving
	 * rows underneath, or the virtualizer replacing the hovered row —
	 * re-sends `pointerenter`/`pointerover` with no movement at all.
	 * Arming from those would open a pill the pointer never moved onto.
	 * Focus (`el === null`) arms directly.
	 */
	function armEnter(el: EventTarget | null): void {
		if (empty) return;
		hoverEl = el as Element | null;
		if (armed) {
			activate(); // re-enter while machinery is up: show immediately
			return;
		}
		if (hoverTimer === null) {
			hoverTimer = setTimeout(() => {
				hoverTimer = null;
				// The row may have scrolled away mid-dwell; a detached trigger
				// has nowhere to anchor the pill. `null` is the focus path
				// (no element to detach).
				if (hoverEl === null || hoverEl.isConnected) activate();
			}, HOVER_DELAY_MS);
		}
	}

	/** Pointer/focus left: drop the dwell timer and (once closed) the machinery. */
	function disarm(): void {
		hoverEl = null;
		clearHoverTimer();
		if (open) {
			open = false;
			if (unarmTimer === null) {
				unarmTimer = setTimeout(() => {
					unarmTimer = null;
					if (!open) armed = false;
				}, UNARM_DELAY_MS);
			}
		}
	}

	/**
	 * Accessible-name sync. The hint doubles as the name ONLY for icon-only
	 * triggers that carry no name of their own — injected AFTER mount by the
	 * capture action (marked with `data-hint-named` so later label changes
	 * stay in sync). Injecting via the spread props instead would shadow
	 * every text button ("Commit", "Unified"…) with the hint sentence — per
	 * ARIA an `aria-label` beats the visible text — and would clobber
	 * consumer-written labels. Consumer-named triggers are never touched;
	 * symbol glyphs (×, →) don't count as visible text.
	 */
	let triggerEl: HTMLElement | null = null;

	function syncName(): void {
		const el = triggerEl;
		if (!el || !("dataset" in el)) return;
		if (el.dataset.hintNamed === "1") {
			// Ours from a previous sync — follow dynamic label changes.
			if (empty) {
				el.removeAttribute("aria-label");
				delete el.dataset.hintNamed;
			} else {
				el.setAttribute("aria-label", label);
			}
			return;
		}
		if (empty || el.hasAttribute("aria-label")) return; // consumer named it
		if (/[\p{L}\p{N}]/u.test(el.textContent ?? "")) return; // text names it
		// Icon-only and unnamed: the hint doubles as the accessible name.
		el.setAttribute("aria-label", label);
		el.dataset.hintNamed = "1";
	}

	function captureTrigger(el: HTMLElement): void {
		triggerEl = el.firstElementChild as HTMLElement | null;
		syncName();
	}

	$effect(() => {
		void label; // track: a dynamic hint re-evaluates the injected name
		syncName();
	});

	/** Idle-phase props: shape-compatible with the real trigger props. */
	const idleProps = $derived({
		type: "button",
		// `pointermove` is the sole pointer-driven arming path — see
		// `armEnter`. No `pointerenter` arming: the browser re-sends it for
		// a stationary pointer after scrolls/DOM changes.
		onpointermove: (event: Event) => armEnter(event.currentTarget),
		onpointerleave: () => disarm(),
		onfocus: () => armEnter(null),
		onblur: () => disarm(),
	});
</script>

<!--
	Spread `props` FIRST and write your own attributes AFTER it: `props`
	carries the trigger's `id`, `tabindex`, `type="button"`, `disabled`,
	state data-attrs and pointer/focus/click handlers, and a later
	attribute overrides the spread. The hint text is wired to the trigger
	as `aria-describedby` while the pill is open; for icon-only triggers,
	write your own `aria-label` — the hint never injects one (it would
	shadow the visible-text names of labeled buttons).
-->

{#if armed}
	<Provider>
		<TooltipPrimitive.Root
			{delayDuration}
			{open}
			disableHoverableContent
			onOpenChange={(next) => {
				if (next) {
					// bits-ui's trigger-level `pointerenter` (and the provider's
					// pointerover tracking) also fire on the browser's hover
					// recompute — scroll/virtualizer waves move elements under a
					// stationary pointer and would re-open the very pill the
					// dismissal just closed. Our own pointer path opens via
					// `activate()` on real movement instead.
					if (!open && recentlyScrolled()) return;
					open = true;
					if (unarmTimer !== null) {
						clearTimeout(unarmTimer);
						unarmTimer = null;
					}
				} else {
					disarm();
				}
			}}
			disabled={empty}
			{...restProps}
		>
			<TooltipPrimitive.Trigger>
				{#snippet child({ props })}
					<span
						class="hint-trigger-scope"
						style="display: contents"
						role="presentation"
						use:captureTrigger
						onpointermove={() => {
							// Closed but still armed (just dismissed): movement within
							// the trigger shows the pill again — `pointermove` is real
							// pointer intent by definition, unlike the re-sent
							// `pointerenter` the sweep-guarded dismissal filters out.
							if (!open) activate();
						}}
					>
						{@render children({ props })}
					</span>
				{/snippet}
			</TooltipPrimitive.Trigger>
			<Content {side} sideOffset={6}>{label}</Content>
		</TooltipPrimitive.Root>
	</Provider>
{:else}
	<span class="hint-trigger-scope" style="display: contents" use:captureTrigger>
		{@render children({ props: idleProps })}
	</span>
{/if}
