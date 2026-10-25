/**
 * Shared wrap-height measurement for diff rows.
 *
 * Every mounted row used to own a ResizeObserver observing its root and its
 * `.txt` spans (~3 observations per row). Scrolling a wrapped diff mounts and
 * destroys dozens of rows per frame, so the observer churn itself became
 * measurable. One module-level observer now serves all rows; callbacks are
 * deduped per row because a row's root and both text spans can land in the
 * same notification batch.
 */

type MeasureReport = () => void;

const reports = new WeakMap<Element, MeasureReport>();
let observer: ResizeObserver | null = null;

function sharedObserver(): ResizeObserver {
  observer ??= new ResizeObserver((entries) => {
    const called = new Set<MeasureReport>();
    for (const entry of entries) {
      const report = reports.get(entry.target);
      if (report && !called.has(report)) {
        called.add(report);
        report();
      }
    }
  });
  return observer;
}

/**
 * Observe a row root plus its text spans. Returns the teardown (unobserve and
 * drop the mappings) for the row's destroy callback.
 */
export function observeRowMeasure(
  node: HTMLElement,
  report: MeasureReport,
): () => void {
  // jsdom / non-DOM hosts: height correction is a browser-only refinement.
  if (typeof ResizeObserver === "undefined") return () => {};
  const ro = sharedObserver();
  const targets = [node, ...node.querySelectorAll<HTMLElement>(".txt")];
  for (const target of targets) {
    reports.set(target, report);
    ro.observe(target);
  }
  return () => {
    for (const target of targets) {
      ro.unobserve(target);
      reports.delete(target);
    }
  };
}
