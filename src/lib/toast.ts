/**
 * Imperative toast API (M0).
 *
 * ```ts
 * import { toast } from "$lib/toast";
 * toast("Rebase started");
 * toast("Push failed", { kind: "error" });
 * ```
 *
 * The rune-based store itself lives in `./toast.svelte` (Svelte 5 only
 * compiles runes inside `.svelte.ts` modules); this module is the stable
 * import path other lanes code against.
 */

export {
  dismissToast,
  getToasts,
  toast,
} from "./toast.svelte";
export type { ToastItem, ToastKind, ToastOptions } from "./toast.svelte";
