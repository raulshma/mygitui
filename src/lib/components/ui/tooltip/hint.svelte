<script lang="ts">
	import type { Snippet } from "svelte";
	import { Tooltip as TooltipPrimitive } from "bits-ui";
	import Provider from "./tooltip-provider.svelte";
	import Content from "./tooltip-content.svelte";
	import {
		claimActiveHint,
		deferWhilePressed,
		isPressInFlight,
		recentlyScrolled,
		releaseActiveHint,
	} from "./hint-active.js";

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
	 * `eager` opts out of the idle→armed swap for controls that open a
	 * bits-ui floating layer (select, menu, popover): those keep a
	 * reference to the trigger element they first saw, so replacing it
	 * on the dwell detaches the reference and the layer positions
	 * against a zero rect — the control reports expanded but its list
	 * never shows. Header/bar controls are few, so mounting their stack
	 * up front costs nothing measurable.
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
		eager = false,
		child: children,
		...restProps
	}: TooltipPrimitive.RootProps & {
		/** Tooltip text; also merged into the trigger as `aria-label`. */
		label: string;
		/** Which side of the trigger the pill appears on. */
		side?: "top" | "bottom" | "left" | "right";
		/** Hover delay in ms (default: the provider's ~300ms). */
		delayDuration?: number | undefined;
		/**
		 * Mount the tooltip stack up front instead of swapping the trigger
		 * on the dwell. Required when the child opens a bits-ui floating
		 * layer (select/menu/popover): those anchor to the trigger element
		 * they first saw, and the lazy swap would leave them pointing at a
		 * detached node (the layer never shows).
		 */
		eager?: boolean;
		/** Your element becomes the trigger — spread `props` onto it. */
		child: Snippet<[ { props: Record<string, unknown> } ]>;
	} = $props();

	/** Blank label → no pill and no aria-label (dynamic labels can be empty). */
	const empty = $derived(label.trim() === "");

	const HOVER_DELAY_MS = $derived(delayDuration ?? 300);
	/** How long a closed tooltip's machinery stays mounted (fade-out room). */
	const UNARM_DELAY_MS = 400;

	// svelte-ignore state_referenced_locally -- `eager` is a mount-time mode,
	// not a live toggle: the trigger must never swap once the layer anchored
	// to it (see the prop docs).
	let armed = $state(eager); // full tooltip machinery mounted
	/** Our dwell timer has fired at least once: the pill may show now. */
	let dwelled = $state(false);
	let open = $state(false);
	/**
	 * The trigger was pressed: its pill closed for the interaction (and the
	 * press focused it, which would otherwise re-arm after the dwell). The
	 * pill stays suppressed — no re-show from pointer moves or focus while
	 * the pointer rests on the control the user is operating — until the
	 * pointer genuinely exits the trigger, after which hovering re-arms as
	 * usual. Cleared by a connected `pointerleave` (swap artifacts don't).
	 */
	let pressDismissed = $state(false);
	let hoverTimer: ReturnType<typeof setTimeout> | null = null;
	let unarmTimer: ReturnType<typeof setTimeout> | null = null;
	let hoverEl: Element | null = null;

	function clearHoverTimer(): void {
		if (hoverTimer !== null) {
			clearTimeout(hoverTimer);
			hoverTimer = null;
		}
	}

	/**
	 * Dwell timer fired (or focus): mount the machinery, already open.
	 *
	 * The mount replaces the idle trigger element, and a browser only
	 * dispatches `click` when `mousedown` and `mouseup` land on the same
	 * element — so the swap must never fire under an in-flight press (the
	 * dwell lands ~300ms after the pointer arrives, exactly when an aiming
	 * user clicks). Deferring the mount to after the click is not enough
	 * either: the late swap destroys the now-focused trigger, and a select
	 * opened by that press closes again (focus-outside dismissal — its
	 * "nothing opens, the label just fixes itself" symptom). When the press
	 * landed on THIS trigger the pill is dismissed for the interaction
	 * (`pressDismissed`): skip the mount outright and let the interaction
	 * finish on the stable idle trigger. Without a press in flight the
	 * mount runs inline.
	 */
	function activate(): void {
		if (empty || pressDismissed) return;
		deferWhilePressed(() => {
			armed = true;
			dwelled = true;
			open = true;
			if (unarmTimer !== null) {
				clearTimeout(unarmTimer);
				unarmTimer = null;
			}
		});
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

	/**
	 * `pointerout` with no `relatedTarget` = the pointer left the window.
	 * A `pointerout` aimed at a DETACHED node is not that: it is the
	 * boundary artifact of the idle→armed swap removing the trigger under a
	 * stationary pointer — the pointer never went anywhere. Swallowing it
	 * keeps the freshly mounted pill from being closed by the very swap
	 * that opened it (the close used to be followed by bits-ui's own
	 * delayed re-open ~250ms later, replaying the open animation — the
	 * pill visibly flashed, then re-showed).
	 */
	function sweepWindowExit(event: PointerEvent): void {
		if (event.target instanceof Node && !event.target.isConnected) return;
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
		if (empty || pressDismissed) return;
		hoverEl = el as Element | null;
		if (armed && dwelled) {
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

	/**
	 * A press landed on this trigger — the interaction dismisses its pill
	 * and must not have it flash back over the control being operated (the
	 * press also focuses the trigger, whose focus-dwell would re-show the
	 * pill ~300ms later, and pointer micro-jitter re-armed it within
	 * ~30ms). Suppression lifts when the pointer exits the trigger — see
	 * the `pointerleave` handlers.
	 */
	function recordPress(event: Event): void {
		if (
			triggerEl !== null &&
			event.target instanceof Node &&
			triggerEl.contains(event.target)
		) {
			pressDismissed = true;
		}
	}

	/** Pointer/focus left: drop the dwell timer and (once closed) the machinery. */
	function disarm(): void {
		hoverEl = null;
		clearHoverTimer();
		// An eager trigger stays mounted, but the next entry must dwell
		// again just like the lazy one does after unarming.
		dwelled = false;
		if (open) {
			open = false;
			// An eager trigger never swaps back to the idle branch: the
			// floating layer it anchors keeps the element it first saw.
			if (!eager && unarmTimer === null) {
				unarmTimer = setTimeout(() => {
					unarmTimer = null;
					// The swap back to the idle trigger is click-hazardous
					// exactly like arming — see `activate`. While the pointer
					// rests on the trigger after a press (`pressDismissed`),
					// it would also destroy the trigger the press focused and
					// dismiss whatever that press opened (a select's
					// focus-outside path) — keep the machinery mounted until
					// the pointer exits instead.
					deferWhilePressed(() => {
						if (!open && !pressDismissed) armed = false;
					});
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

	/**
	 * Boundary-event artifact guard, shared by the idle and armed trigger
	 * handlers. The idle→armed swap removes the element under a stationary
	 * pointer, and the browser announces that with `pointerleave`/`blur` on
	 * the element it just removed — which read as "the pointer left" and
	 * would close the pill the swap just opened (the close is followed by
	 * bits-ui's own re-open, restarting the enter animation: the pill
	 * visibly flashes, then re-shows). A handler whose target is already
	 * detached is by definition such an artifact.
	 */
	function swapArtifact(event: Event): boolean {
		return event.target instanceof Node && !event.target.isConnected;
	}

	/**
	 * A `pointerleave` mid-press is not an exit: opening a select (or any
	 * modal layer) makes its own trigger hit-test-invisible under the
	 * stationary pointer, and the browser answers that with `pointerleave`
	 * on a perfectly connected element. Treating it as an exit would lift
	 * the post-press suppression and let the pending dwell mount the pill
	 * mid-press — whose trigger swap then destroys the focused trigger and
	 * dismisses the very menu the press opened. The suppression lifts on
	 * the first leave while NO press is in flight (a real exit, or the
	 * post-open leave after the press ended).
	 */
	function onTriggerLeave(): void {
		if (isPressInFlight()) return;
		pressDismissed = false;
		disarm();
	}

	/** Idle-phase props: shape-compatible with the real trigger props. */
	const idleProps = $derived({
		type: "button",
		// `pointermove` is the sole pointer-driven arming path — see
		// `armEnter`. No `pointerenter` arming: the browser re-sends it for
		// a stationary pointer after scrolls/DOM changes.
		onpointermove: (event: Event) => armEnter(event.currentTarget),
		onpointerdown: recordPress,
		onpointerleave: (event: Event) => {
			if (swapArtifact(event)) return;
			onTriggerLeave();
		},
		onfocus: () => armEnter(null),
		onblur: (event: Event) => {
			if (swapArtifact(event)) return;
			disarm();
		},
	});

	const guardedChildProps = (props: Record<string, unknown>) => ({
		...props,
		onpointerleave: (event: PointerEvent) => {
			if (swapArtifact(event)) return;
			onTriggerLeave();
			(props.onpointerleave as (e: PointerEvent) => void)?.(event);
		},
		onpointerdown: (event: PointerEvent) => {
			if (swapArtifact(event)) return;
			recordPress(event);
			(props.onpointerdown as (e: PointerEvent) => void)?.(event);
		},
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
		<!--
			Hint opens the Root controlled (open=true at mount, bypassing
			bits-ui's `handleOpen`), so bits-ui still counts the pill as
			"not yet delayed-open". The pointer resting on the freshly
			swapped trigger then starts bits-ui's own delayDuration timer;
			when it fires ~300ms later it flips the pill's data-state
			instant-open → delayed-open — and the theme animates on
			`delayed-open`, restarting the fade-in on the fully visible
			pill (it visibly blinks once — "tooltips flicker"). The dwell
			delay lives in OUR timer (`HOVER_DELAY_MS`), so bits-ui's own
			is given 0 and its timer never arms; the data-state stays
			`instant-open` for the pill's whole life.
		-->
		<TooltipPrimitive.Root
			{open}
			delayDuration={0}
			disableHoverableContent
			onOpenChange={(next) => {
				if (next) {
					// `eager` mounts bits-ui's trigger before the pointer
					// arrives, so its own hover/focus tracking fires right
					// away. Only OUR dwell timer may open the pill; without
					// this gate eager controls would show it instantly and
					// bypass HOVER_DELAY_MS.
					if (!open && !dwelled) return;
					// bits-ui's trigger-level `pointerenter` (and the provider's
					// pointerover tracking) also fire on the browser's hover
					// recompute — scroll/virtualizer waves move elements under a
					// stationary pointer and would re-open the very pill the
					// dismissal just closed. Our own pointer path opens via
					// `activate()` on real movement instead.
					if (!open && recentlyScrolled()) return;
					// A press on the trigger dismissed the pill for the
					// interaction; bits-ui's delayed open (armed on its own
					// pointer tracking) must not override that.
					if (!open && pressDismissed) return;
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
							// Eager machinery is mounted before any dwell: the
							// first real movement starts OUR dwell timer.
							if (!dwelled) {
								armEnter(triggerEl);
								return;
							}
							// Closed but still armed (just dismissed): movement within
							// the trigger shows the pill again — `pointermove` is real
							// pointer intent by definition, unlike the re-sent
							// `pointerenter` the sweep-guarded dismissal filters out.
							// A press on the trigger keeps the pill suppressed until
							// the pointer exits (see `recordPress`).
							if (!open && !pressDismissed) activate();
						}}
						onfocusin={() => {
							if (!dwelled) armEnter(null);
						}}
						onfocusout={(event) => {
							if (
								event.relatedTarget instanceof Node &&
								event.currentTarget.contains(event.relatedTarget)
							)
								return;
							disarm();
						}}
					>
						{@render children({ props: guardedChildProps(props) })}
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
