/**
 * Shared compare-selection shape for the history views: what CompareBar
 * hands back and what HistoryView / FileHistoryView hold as `compare`
 * state and pass into CommitDetail's `compare` prop.
 */
import type { FileDiff } from "$lib/ipc/types";

export interface CompareResult {
  /** Aggregate diff files across the selection. */
  files: FileDiff[];
  /** Base side sha (a range start carries the `^` parent marker). */
  base: string;
  /** Target side sha (the newest selected commit). */
  target: string;
}
