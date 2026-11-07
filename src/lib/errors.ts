/**
 * Shared user-facing error formatting. Panels used to each re-declare a
 * local `describeError`; import this one so error text stays consistent
 * and typed IPC errors (e.g. ForgeError) surface their message instead
 * of `[object Object]`.
 */

/** Best-effort human text for any thrown value. */
export function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "object" && err !== null) {
    const msg = (err as { message?: unknown }).message;
    if (typeof msg === "string" && msg !== "") return msg;
  }
  return String(err);
}
