/**
 * Shared dialog-state machine: one value per dialog carrying its open
 * flag and its args together.
 *
 * Canonical encoding for registry archetype 1 (nullable-pile mode fields)
 * as applied to dialogs: instead of `open` + N arg fields that every
 * transition must hand-reset, one `DialogState<A>` union per dialog —
 * args exist only while the dialog is open, and `open` can never be true
 * without a fresh, complete arg set. ("Add" vs "edit" mode is just an
 * arg: `{ device: DeviceProfile | null }`.)
 */

/** Closed — or open together with the dialog's full argument set. */
export type DialogState<A> = { open: false } | ({ open: true } & A);

/** The closed state (type as `DialogState<typeof args>` at the call site). */
export function closedDialog<A>(): DialogState<A> {
  return { open: false };
}

/** An open state carrying `args` (spread so args read as direct fields). */
export function openDialog<A extends object>(args: A): DialogState<A> {
  return { open: true, ...args };
}
