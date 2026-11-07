/**
 * About-dialog host (singleton) — the seam between the command palette and
 * the "About mygitui" modal.
 *
 * The palette registry (`$lib/palette/commands`) is plain data + closures so
 * it unit-tests without a DOM; it therefore must not statically import a
 * Svelte component. This host is plain TS instead: {@link openAboutDialog}
 * dynamic-imports `./AboutDialog.svelte`, `mount()`s it onto a host element
 * appended to `document.body` and keeps the teardown around for
 * {@link closeAboutDialog} (the dialog itself calls that from Escape,
 * outside-click and its Close button).
 */

import { mount, unmount } from "svelte";

/** Tears the mounted dialog down; null while the dialog is closed. */
let teardown: (() => void) | null = null;

/** Opens the About dialog (a no-op while it is already open). */
export function openAboutDialog(): void {
  if (teardown !== null) return;
  if (typeof document === "undefined") return; // non-DOM environment guard
  const target = document.createElement("div");
  document.body.append(target);
  void import("./AboutDialog.svelte")
    .then(({ default: AboutDialog }) => {
      // Closed again while the chunk was loading: drop the host, stay shut.
      if (teardown !== null) {
        target.remove();
        return;
      }
      const instance = mount(AboutDialog, { target });
      teardown = () => unmount(instance);
    })
    .catch((err: unknown) => {
      console.error("[mygitui] About dialog failed to open:", err);
      target.remove();
    });
}

/** Closes (unmounts) the About dialog; a no-op when it is not open. */
export function closeAboutDialog(): void {
  const stop = teardown;
  if (stop === null) return;
  teardown = null;
  stop();
}
