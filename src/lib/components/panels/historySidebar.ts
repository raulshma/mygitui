/**
 * Contract between the sidebar history list (`HistoryView` in `sidebar`
 * variant) and the workspace's main editor pane. The detail view no longer
 * lives inside the history panel: selecting a commit reports it upward, and
 * the commit's action toolbar calls back into the list so multi-selection
 * cherry-pick / revert / rebase keep working from the center editor.
 */
export interface HistoryCommitActions {
  /** Number of selected commits (the action toolbar's count). */
  count: number;
  /** True while a cherry-pick / revert IPC is in flight. */
  busy: boolean;
  cherryPick: () => void;
  revert: () => void;
  rebaseFromHere: () => void;
}
