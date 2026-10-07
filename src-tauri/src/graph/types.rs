//! Commit-graph lane layout contract.
//!
//! CONTRACT: pure functions, no git2 dependency — unit-testable on generated
//! DAGs. The layout consumes `CommitInfo` pages in walk order and produces
//! render-ready rows for the canvas graph.

use serde::{Deserialize, Serialize};

/// Connection from one row to the row below (or within-row bends).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: u16,
    pub to: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRow {
    pub sha: String,
    /// Lane index of this commit's node.
    pub lane: u16,
    /// Edges drawn from this row to the next page row.
    pub edges: Vec<GraphEdge>,
    /// Total lanes in use at this row (for column sizing).
    pub lane_count: u16,
}

/// Incremental layout state carried between pages.
#[derive(Debug, Default)]
pub struct LaneState {
    /// sha -> lane assignment for active (unmerged) branches.
    pub lanes: Vec<Option<String>>,
    pub next_free: u16,
}

/// Lay out one page of commits (walk order, topo-sorted upstream).
/// Mutates `state` so the next page continues seamlessly.
pub fn layout_page(
    commits: &[crate::engine::types::CommitInfo],
    state: &mut LaneState,
) -> Vec<GraphRow> {
    // Implementation lands with the graph lane (M1 wave 1). The signature and
    // state semantics above are the contract: first parent continues the same
    // lane; additional parents open new lanes; lanes close when exhausted.
    let _ = (commits, state);
    Vec::new()
}
