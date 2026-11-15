/**
 * Shared clipboard helper. Panels used to write the clipboard several
 * different ways (fire-and-forget with a premature "Copied" toast, unhandled
 * rejections, silent no-feedback variants); route every copy through here so
 * the write is awaited and the user gets one toast either way.
 */
import { toast } from "./toast";

/**
 * Writes `text` to the clipboard with toast feedback.
 *
 * `label` names the payload in the toasts ("Copied branch name"); it
 * defaults to the text itself. Resolves `true` when the write succeeded,
 * `false` otherwise — including when `navigator.clipboard` is unavailable
 * (non-secure context, test environment), so callers never need a
 * `?.writeText` guard of their own.
 */
export async function copyText(text: string, label?: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    toast(`Copied ${label ?? text}`, { kind: "info" });
    return true;
  } catch {
    toast(`Copy failed: ${label ?? "clipboard"}`, { kind: "error" });
    return false;
  }
}
