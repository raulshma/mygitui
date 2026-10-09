/**
 * Component tests for SignatureBadge (M12, lane C) — jsdom render with the
 * IPC transport mocked via `setTransport` (the codebase's standard seam):
 * each verification outcome renders its chip (or nothing), and transport
 * failures render nothing.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/svelte";
import type { Transport } from "$lib/ipc/client";
import { resetTransport, setTransport } from "$lib/ipc/client";
import type { CommitSignature } from "$lib/ipc/types";
import SignatureBadge from "$lib/components/commit/SignatureBadge.svelte";

// The locked-down vite.config.js has no `resolve.conditions: ["browser"]` for
// vitest, so the `svelte` bare import resolves to index-server.js and `mount`
// throws. Redirect it to the client build (same trick GraphCanvas.test.ts
// uses). Applies to this test file's whole module graph.
vi.mock("svelte", () =>
  // @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
  import("../../../../node_modules/svelte/src/index-client.js"),
);

function sig(overrides: Partial<CommitSignature>): CommitSignature {
  return { signed: true, kind: "gpg", valid: true, detail: "gpg: Good", ...overrides };
}

/** Transport stub answering only `commit_signature`. */
function transportOf(result: CommitSignature | Error): Transport {
  return (command: string) => {
    if (command !== "commit_signature") {
      return Promise.reject(new Error(`unexpected command ${command}`));
    }
    return result instanceof Error ? Promise.reject(result) : Promise.resolve(result);
  };
}

beforeEach(() => {
  resetTransport();
});

afterEach(() => {
  resetTransport();
  cleanup();
});

describe("SignatureBadge", () => {
  it("renders nothing while the signature is loading", () => {
    setTransport(() => new Promise(() => {})); // never resolves
    const { container } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    expect(container.querySelector(".sig-badge")).toBeNull();
  });

  it("hides unsigned commits by default (even after the IPC resolves)", async () => {
    const transport = vi.fn(transportOf(sig({ signed: false, kind: null, valid: null, detail: "" })));
    setTransport(transport);
    const { container } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    await vi.waitFor(() => {
      expect(transport).toHaveBeenCalled();
    });
    expect(container.querySelector(".sig-badge")).toBeNull();
  });

  it("shows a neutral Unsigned chip when hideUnsigned is false", async () => {
    setTransport(transportOf(sig({ signed: false, kind: null, valid: null, detail: "" })));
    const { container, getByText } = render(SignatureBadge, {
      props: { repoId: "r1", sha: "abc", hideUnsigned: false },
    });
    await vi.waitFor(() => {
      expect(getByText("Unsigned")).toBeTruthy();
    });
    expect(container.querySelector(".sig-badge.neutral")).toBeTruthy();
  });

  it("shows a green Verified chip for a valid GPG signature", async () => {
    setTransport(transportOf(sig({ kind: "gpg", valid: true })));
    const { container, getByText } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    await vi.waitFor(() => {
      expect(getByText("Verified")).toBeTruthy();
    });
    const chip = container.querySelector<HTMLElement>(".sig-badge.verified");
    expect(chip).toBeTruthy();
    expect(chip?.title).toBe("gpg: Good");
  });

  it("labels a valid SSH signature Verified (SSH)", async () => {
    setTransport(transportOf(sig({ kind: "ssh", valid: true, detail: "ssh: Good" })));
    const { getByText } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    await vi.waitFor(() => {
      expect(getByText("Verified (SSH)")).toBeTruthy();
    });
  });

  it("shows a red Bad signature chip when verification fails", async () => {
    setTransport(transportOf(sig({ valid: false, detail: "gpg: BAD" })));
    const { container, getByText } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    await vi.waitFor(() => {
      expect(getByText("Bad signature")).toBeTruthy();
    });
    expect(container.querySelector(".sig-badge.bad")).toBeTruthy();
  });

  it("shows an amber Unverifiable chip when valid is null", async () => {
    setTransport(transportOf(sig({ valid: null, detail: "no public key" })));
    const { container, getByText } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    await vi.waitFor(() => {
      expect(getByText("Unverifiable")).toBeTruthy();
    });
    const chip = container.querySelector<HTMLElement>(".sig-badge.unverifiable");
    expect(chip?.title).toBe("no public key");
  });

  it("renders nothing when the IPC call fails", async () => {
    setTransport(transportOf(new Error("backend exploded")));
    const { container } = render(SignatureBadge, { props: { repoId: "r1", sha: "abc" } });
    await new Promise((r) => setTimeout(r, 10));
    expect(container.querySelector(".sig-badge")).toBeNull();
  });

  it("re-queries when the sha prop changes", async () => {
    const transport = vi.fn((command: string, args?: Record<string, unknown>) => {
      const sha = String(args?.sha);
      return Promise.resolve(
        sig({ valid: sha === "one", detail: `checked ${sha}` }),
      );
    });
    setTransport(transport);
    const rendered = render(SignatureBadge, { props: { repoId: "r1", sha: "one" } });
    await vi.waitFor(() => {
      expect(rendered.getByText("Verified")).toBeTruthy();
    });
    // @testing-library/svelte v5: rerender takes the props object directly.
    await rendered.rerender({ repoId: "r1", sha: "two" });
    await vi.waitFor(() => {
      expect(rendered.getByText("Bad signature")).toBeTruthy();
    });
    expect(transport).toHaveBeenCalledWith("commit_signature", { repo_id: "r1", sha: "two" });
  });
});
