/**
 * `onLoadImage` factory for DiffViewer hosts (RepoView's editor pane,
 * CommitDetail, AgentDiff): image-diff URLs resolved per diff side.
 *
 * The worktree side reads the working file (`readFile`); the index/head/
 * commit sides read the stored blob (`readBlob`) — commit-to-commit image
 * diffs need sides `readFile` (workdir only) cannot reach. Hosts create
 * the loader through `$derived` so its identity only changes when the
 * repo or the sides change: ImageCompare reloads whenever the function
 * reference moves, and a per-render closure would reload on every stream
 * page.
 */
import { readBlob, readFile } from "$lib/ipc/client";
import type { DiffSide } from "$lib/ipc/types";
import type { LoadImageFn } from "$lib/components/diff/rowModel";

/** Content types for the extensions the backend flags `is_image`. */
const IMAGE_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  svg: "image/svg+xml",
  ico: "image/x-icon",
  avif: "image/avif",
};

function imageMime(path: string): string {
  const dot = path.lastIndexOf(".");
  const ext = dot === -1 ? "" : path.slice(dot + 1).toLowerCase();
  return IMAGE_MIME[ext] ?? "application/octet-stream";
}

/**
 * Builds a `LoadImageFn` for one diff sides pair: `old` resolves through
 * `sides.old`, the new side through `sides.new`. Empty content (a missing
 * worktree file — `readFile`'s documented fallback — or a zero-byte blob)
 * and failures both resolve `null`, the placeholder ImageCompare renders;
 * rejections never escape as unhandled (ImageCompare also catches them).
 */
export function createImageLoader(
  repoId: string,
  sides: { old: DiffSide; new: DiffSide },
): LoadImageFn {
  return async (path: string, old: boolean): Promise<string | null> => {
    const side = old ? sides.old : sides.new;
    const bytes =
      side === "worktree"
        ? await readFile(repoId, path)
        : await readBlob(repoId, side, path);
    if (bytes.length === 0) return null;
    return URL.createObjectURL(new Blob([bytes], { type: imageMime(path) }));
  };
}
