/**
 * Component tests for ImageCompare: the placeholder contract (no loader,
 * rejecting loader, null resolutions) and the object-URL lifecycle — the
 * loader hands back fresh blob URLs per load, and the row must revoke the
 * ones it adopted (on unmount and on late arrivals after the row went
 * away) or every virtualized scroll leaks a Blob.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import type { FileDiff } from "$lib/ipc/types";
import type { LoadImageFn } from "$lib/components/diff/rowModel";
import ImageCompare from "$lib/components/diff/ImageCompare.svelte";

afterEach(() => cleanup());

let created = 0;
const revoke = vi.fn();

beforeEach(() => {
  created = 0;
  revoke.mockClear();
  // jsdom has no blob URL store; these two are the component's whole
  // interaction with URL.
  Object.defineProperty(URL, "createObjectURL", {
    configurable: true,
    writable: true,
    value: vi.fn(() => `blob:mock-${++created}`),
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    configurable: true,
    writable: true,
    value: revoke,
  });
});

function imageFile(oldPath = "assets/old.png"): FileDiff {
  return {
    path: "assets/logo.png",
    old_path: oldPath,
    binary: true,
    is_image: true,
    additions: 0,
    deletions: 0,
    hunks: [],
  };
}

/** A loader that hands out mock object URLs (never null). */
function urlLoader(): LoadImageFn {
  return vi.fn(async () => `blob:mock-${++created}`);
}

describe("ImageCompare placeholders", () => {
  it("shows both placeholders when no onLoadImage is passed", async () => {
    render(ImageCompare, { props: { file: imageFile() } });
    await tick();

    expect(screen.getByText("Old image unavailable")).toBeTruthy();
    expect(screen.getByText("New image unavailable")).toBeTruthy();
    expect(revoke).not.toHaveBeenCalled();
  });

  it("falls back to the placeholders when the loader rejects (no unhandled rejection)", async () => {
    const reject = vi.fn(async () => {
      throw new Error("blob missing");
    });
    render(ImageCompare, {
      props: { file: imageFile(), onLoadImage: reject },
    });

    await screen.findByText("Old image unavailable");
    await screen.findByText("New image unavailable");
    expect(reject).toHaveBeenCalledTimes(2);
    expect(revoke).not.toHaveBeenCalled();
  });

  it("falls back to the placeholder for a null resolution (empty side)", async () => {
    const loader = vi.fn(async () => null);
    render(ImageCompare, {
      props: { file: imageFile(), onLoadImage: loader },
    });

    await screen.findByText("New image unavailable");
  });
});

describe("ImageCompare object-URL lifecycle", () => {
  it("renders both images from resolved URLs and revokes them on unmount", async () => {
    const { unmount } = render(ImageCompare, {
      props: { file: imageFile(), onLoadImage: urlLoader() },
    });

    const oldImg = await screen.findByRole("img", {
      name: "Old version of assets/logo.png",
    });
    const newImg = await screen.findByRole("img", {
      name: "New version of assets/logo.png",
    });
    expect(oldImg.getAttribute("src")).toMatch(/^blob:mock-/);
    expect(newImg.getAttribute("src")).toMatch(/^blob:mock-/);
    expect(revoke).not.toHaveBeenCalled();

    unmount();
    expect(revoke).toHaveBeenCalledTimes(2);
    const revoked = revoke.mock.calls.map((call) => call[0]);
    expect(revoked).toContain(oldImg.getAttribute("src"));
    expect(revoked).toContain(newImg.getAttribute("src"));
  });

  it("revokes URLs the row never adopted when it unmounts mid-load", async () => {
    // Both loads stay pending until the test releases them (one resolver
    // per call — the loads settle independently).
    const releases: Array<() => void> = [];
    const loader = vi.fn(
      (_path: string, old: boolean) =>
        new Promise<string | null>((resolve) => {
          releases.push(() => resolve(old ? "blob:late-old" : "blob:late-new"));
        }),
    );
    const { unmount } = render(ImageCompare, {
      props: { file: imageFile(), onLoadImage: loader },
    });
    // The row goes away before either load settles.
    unmount();
    expect(revoke).not.toHaveBeenCalled();

    for (const release of releases) release();
    await tick();
    await tick();

    const revoked = revoke.mock.calls.map((call) => call[0]);
    expect(revoked).toContain("blob:late-old");
    expect(revoked).toContain("blob:late-new");
  });
});
