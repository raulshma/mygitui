import { describe, expect, it } from "vitest";

import { describeError, describeGitError } from "./errors";

describe("describeError", () => {
  it("unwraps Error.message", () => {
    expect(describeError(new Error("boom"))).toBe("boom");
  });

  it("reads message off plain objects", () => {
    expect(describeError({ message: "obj fail" })).toBe("obj fail");
  });

  it("falls back to String()", () => {
    expect(describeError(42)).toBe("42");
  });
});

describe("describeGitError", () => {
  it("maps the server's current-branch refusal", () => {
    const raw =
      "push of `refs/heads/main` rejected: refusing to delete the current branch";
    expect(describeGitError(raw)).toBe(
      "The remote protects this branch (its default or currently checked-out branch) and refused to delete it.",
    );
  });

  it("maps protected-branch refusals", () => {
    expect(
      describeGitError(
        "remote: error: GH006: Protected branch update failed for refs/heads/main",
      ),
    ).toBe("The remote refuses this change — the branch is protected there.");
  });

  it("maps hook declines", () => {
    expect(describeGitError("pre-receive hook declined")).toBe(
      "A server-side hook on the remote refused this operation.",
    );
  });

  it("maps auth failures ahead of the generic permission bucket", () => {
    // libgit2 phrasing — contains "not allowed", but auth must win.
    expect(describeGitError("git error: authentication not allowed")).toBe(
      "Authentication with the remote failed — check your credentials and try again.",
    );
    // SSH phrasing — contains "permission denied", but auth must win.
    expect(describeGitError("permission denied (publickey)")).toBe(
      "Authentication with the remote failed — check your credentials and try again.",
    );
  });

  it("maps permission failures", () => {
    expect(describeGitError("403 Forbidden")).toBe(
      "You don't have permission to do this on the remote.",
    );
  });

  it("maps connectivity failures", () => {
    expect(describeGitError("failed to connect: No route to host")).toBe(
      "Could not reach the remote — check your connection and the remote's URL.",
    );
    expect(describeGitError("authentication timeout")).toBe(
      "Authentication with the remote failed — check your credentials and try again.",
    );
  });

  it("maps missing refs, naming the branch when a subject is given", () => {
    expect(
      describeGitError("remote ref does not exist", "feature"),
    ).toBe(
      "The branch feature no longer exists — it may already have been deleted.",
    );
    expect(describeGitError("not found")).toBe(
      "Not found — it may already have been deleted.",
    );
  });

  it("maps checked-out-in-worktree refusals", () => {
    expect(
      describeGitError(
        "git error: Cannot delete branch 'feature' checked out at '/tmp/wt'",
      ),
    ).toBe(
      "This branch is checked out in a worktree — switch away from it there first.",
    );
  });

  it("maps not-fully-merged refusals", () => {
    expect(
      describeGitError(
        "invalid path or ref: branch `feature` is not fully merged into HEAD; delete with force to bypass",
      ),
    ).toBe(
      "This branch has commits that would be lost — a force delete is needed (the app offers one after a preview).",
    );
  });

  it("passes unknown text through verbatim", () => {
    expect(describeGitError("Switch to feature failed: conflict")).toBe(
      "Switch to feature failed: conflict",
    );
  });

  it("keeps the backend's own friendly messages untouched", () => {
    const raw = "cannot delete the current branch `main`; switch away first";
    expect(describeGitError(raw)).toBe(raw);
  });
});
