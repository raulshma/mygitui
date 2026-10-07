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

function dismiss(id: number): void {
  const timer = timers.get(id);
  if (timer !== undefined) {
    clearTimeout(timer);
    timers.delete(id);
  }
  toasts = toasts.filter((toastItem) => toastItem.id !== id);
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
  timers.set(
    id,
    setTimeout(() => dismiss(id), AUTO_DISMISS_MS),
  );
}

/** Snapshot of the active toasts for rendering. */
export function getToasts(): ToastItem[] {
  return toasts;
}
