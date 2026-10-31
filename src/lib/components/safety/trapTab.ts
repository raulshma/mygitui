/**
 * Shared dialog focus trap: keeps Tab (and Shift+Tab) cycling inside
 * `container` while a dialog is open. Kept for AddDeviceDialog, which still
 * renders a hand-rolled dialog; DialogShell and the other migrated dialogs
 * get their focus trap from bits-ui instead — keep the behavior identical
 * when touching either.
 */
const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])';

export function trapTab(
  container: HTMLElement | null | undefined,
  event: KeyboardEvent,
): void {
  if (event.key !== "Tab" || !container) return;
  const focusable = Array.from(
    container.querySelectorAll<HTMLElement>(FOCUSABLE),
  ).filter((el) => el.offsetParent !== null);
  if (focusable.length === 0) return;
  const first = focusable[0]!;
  const last = focusable[focusable.length - 1]!;
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
