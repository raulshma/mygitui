import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Asserts the target is on-screen and not clipped by an ancestor.
 *
 * `boundingBox()` ignores ancestor clipping (the layout SplitPanes use
 * `overflow: hidden`), so hit-test the corners: a clipped edge resolves to
 * whatever paints over it instead of the target itself.
 */
export async function expectFullyVisible(
  page: Page,
  target: Locator | string,
): Promise<void> {
  const locator = typeof target === "string" ? page.locator(target) : target;
  const name =
    typeof target === "string"
      ? target
      : ((await locator.getAttribute("aria-label")) ?? "element");
  const box = await locator.boundingBox();
  expect(box, `${name} has a bounding box`).not.toBeNull();

  const vp = page.viewportSize() ?? { width: 0, height: 0 };
  expect(box!.y, `${name} top edge on screen`).toBeGreaterThanOrEqual(0);
  expect(box!.x, `${name} left edge on screen`).toBeGreaterThanOrEqual(0);
  expect(box!.y + box!.height, `${name} bottom edge on screen`).toBeLessThanOrEqual(vp.height);
  expect(box!.x + box!.width, `${name} right edge on screen`).toBeLessThanOrEqual(vp.width);

  const clipped = await locator.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const inset = 6;
    const points: Array<[number, number]> = [
      [r.left + inset, r.top + inset],
      [r.right - inset, r.top + inset],
      [r.left + inset, r.bottom - inset],
      [r.right - inset, r.bottom - inset],
    ];
    return points.some(([x, y]) => {
      const hit = document.elementFromPoint(x, y);
      return !hit || !el.contains(hit);
    });
  });
  expect(clipped, `${name} is not clipped by an ancestor`).toBe(false);
}
