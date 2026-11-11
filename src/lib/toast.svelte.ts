/**
 * Toast store internals (Svelte 5 runes).
 *
 * Lives in a `.svelte.ts` module because `$state` is only compiled in
 * Svelte-module files. The public surface is re-exported from `$lib/toast`.
 */

export type ToastKind = "info" | "success" | "error";

/** One inline button on a toast ("View update", …). Clicking it runs the
 * callback and dismisses the toast. */
export interface ToastAction {
  label: string;
  run: () => void;
}

export interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
  /** Repeat count for a coalesced message (1 for a first occurrence). */
  count: number;
  /** Optional inline action button (rendered by the Toaster). */
  action: ToastAction | null;
}

export interface ToastOptions {
  kind?: ToastKind;
  /** Inline action button (click runs it and dismisses the toast). */
  action?: ToastAction;
}

const AUTO_DISMISS_MS = 4_000;
/** Errors linger past the 4s default so failures can be read (and copied). */
const ERROR_AUTO_DISMISS_MS = 9_000;
/** Beyond this the stack would climb past the viewport; oldest makes way. */
const MAX_VISIBLE = 4;

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

/**
 * Shows a toast; auto-dismisses after 4 seconds (errors after 9).
 *
 * A message already on screen is coalesced into a repeat count and its
 * timer restarted rather than stacked again — retry loops and per-file
 * operations otherwise bury the viewport in identical rows. Past
 * {@link MAX_VISIBLE} the oldest toast is dropped to make room.
 */
export function toast(message: string, opts?: ToastOptions): void {
  const kind: ToastKind = opts?.kind ?? "info";
  const delay = kind === "error" ? ERROR_AUTO_DISMISS_MS : AUTO_DISMISS_MS;

  const existing = toasts.find((item) => item.message === message && item.kind === kind);
  if (existing !== undefined) {
    existing.count += 1;
    // Keep the freshest action; a repeat usually re-fires with the same one.
    existing.action = opts?.action ?? existing.action;
    if (timers.has(existing.id)) {
      clearTimeout(timers.get(existing.id)!);
      timers.delete(existing.id);
      schedule(existing.id, delay);
    } else {
      // Paused under the pointer: bank the full duration for the resume.
      deadlines.set(existing.id, delay);
    }
    return;
  }

  const id = nextId++;
  toasts = [
    ...toasts,
    { id, message, kind, count: 1, action: opts?.action ?? null },
  ];
  schedule(id, delay);
  while (toasts.length > MAX_VISIBLE) dismiss(toasts[0]!.id);
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
