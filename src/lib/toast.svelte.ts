/**
 * Toast store internals (Svelte 5 runes).
 *
 * Lives in a `.svelte.ts` module because `$state` is only compiled in
 * Svelte-module files. The public surface is re-exported from `$lib/toast`.
 */

export type ToastKind = "info" | "success" | "error";

export interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
}

export interface ToastOptions {
  kind?: ToastKind;
}

const AUTO_DISMISS_MS = 4_000;

let toasts = $state<ToastItem[]>([]);
let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();
/** Absolute dismissal deadline per toast (lets hover-pause preserve time). */
const deadlines = new Map<number, number>();

function dismiss(id: number): void {
  const timer = timers.get(id);
  if (timer !== undefined) {
    clearTimeout(timer);
    timers.delete(id);
  }
  deadlines.delete(id);
  toasts = toasts.filter((toastItem) => toastItem.id !== id);
}

/** Arms the auto-dismiss timer, tracking the deadline for pause/resume. */
function schedule(id: number, delay: number): void {
  deadlines.set(id, Date.now() + delay);
  timers.set(
    id,
    setTimeout(() => dismiss(id), delay),
  );
}

/** Removes a toast immediately (used by the Toaster's close button). */
export function dismissToast(id: number): void {
  dismiss(id);
}

/** Shows a toast; auto-dismisses after 4 seconds. */
export function toast(message: string, opts?: ToastOptions): void {
  const id = nextId++;
  const kind: ToastKind = opts?.kind ?? "info";
  toasts = [...toasts, { id, message, kind }];
  schedule(id, AUTO_DISMISS_MS);
}

/**
 * Pauses a toast's auto-dismiss timer (Toaster pause-on-hover). The
 * remaining time is preserved; {@link resumeToast} continues from it.
 * Idempotent — pausing twice is a no-op.
 */
export function pauseToast(id: number): void {
  const timer = timers.get(id);
  if (timer === undefined) return; // already paused or already gone
  clearTimeout(timer);
  timers.delete(id);
  deadlines.set(id, Math.max((deadlines.get(id) ?? Date.now()) - Date.now(), 0));
}

/**
 * Resumes a toast paused by {@link pauseToast}, counting down its remaining
 * time. Idempotent; unknown/never-paused ids are ignored.
 */
export function resumeToast(id: number): void {
  const remaining = deadlines.get(id);
  if (remaining === undefined || timers.has(id)) return;
  schedule(id, Math.max(remaining, 1));
}

/** Snapshot of the active toasts for rendering. */
export function getToasts(): ToastItem[] {
  return toasts;
}
