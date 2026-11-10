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

/**
 * Turns raw git/network failure text into an actionable sentence for
 * toasts and dialogs. Unknown text passes through unchanged so new
 * server-side messages still surface verbatim.
 *
 * `subject` (e.g. a branch name) lets the not-found pattern name the
 * thing that is missing.
 *
 * Match order matters: the specific server-refusal phrases win over the
 * generic permission bucket, and auth wins over "denied"/"not allowed"
 * (SSH key failures read "permission denied (publickey)").
 */
export function describeGitError(raw: string, subject?: string): string {
  const text = raw.toLowerCase();

  // Server explicitly refused a branch deletion (its default or
  // currently checked-out branch).
  if (
    text.includes("refusing to delete the current branch") ||
    text.includes("deletion of the current branch")
  ) {
    return "The remote protects this branch (its default or currently checked-out branch) and refused to delete it.";
  }
  if (text.includes("protected branch")) {
    return "The remote refuses this change — the branch is protected there.";
  }
  // Server-side hooks.
  if (text.includes("hook declined")) {
    return "A server-side hook on the remote refused this operation.";
  }
  // Auth before the generic permission bucket (see doc comment).
  if (
    text.includes("authenticat") ||
    text.includes("credential") ||
    text.includes("publickey")
  ) {
    return "Authentication with the remote failed — check your credentials and try again.";
  }
  if (
    text.includes("permission") ||
    text.includes("not allowed") ||
    text.includes("forbidden") ||
    text.includes("403")
  ) {
    return "You don't have permission to do this on the remote.";
  }
  // Connectivity.
  if (
    text.includes("failed to connect") ||
    text.includes("could not resolve") ||
    text.includes("connection refused") ||
    text.includes("timed out") ||
    text.includes("timeout") ||
    text.includes("network")
  ) {
    return "Could not reach the remote — check your connection and the remote's URL.";
  }
  // Missing ref: already deleted, or never existed.
  if (
    text.includes("not found") ||
    text.includes("does not exist") ||
    text.includes("ambiguous argument") ||
    text.includes("unknown revision") ||
    text.includes("bad revision")
  ) {
    return subject
      ? `The branch ${subject} no longer exists — it may already have been deleted.`
      : "Not found — it may already have been deleted.";
  }
  // Local worktree protection (libgit2 phrasing).
  if (text.includes("checked out")) {
    return "This branch is checked out in a worktree — switch away from it there first.";
  }
  if (text.includes("not fully merged")) {
    return "This branch has commits that would be lost — a force delete is needed (the app offers one after a preview).";
  }
  return raw;
}
