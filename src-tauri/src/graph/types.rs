//! Commit-graph lane layout contract.
//!
//! CONTRACT: pure functions, no git2 dependency — unit-testable on generated
//! DAGs. The layout consumes `CommitInfo` pages in walk order and produces
//! render-ready rows for the canvas graph.
//!
//! Algorithm (classic lane assignment, git-graph style):
//!
//! * Input is topological walk order (children before parents, newest first).
//! * A commit whose sha already occupies a lane (placed there by an earlier
//!   child) renders in that lane; otherwise it takes the lowest free lane
//!   (`next_free` strategy: reuse freed slots first, extend only when full).
//! * The first parent continues in the commit's own lane unless that parent
//!   was already assigned elsewhere by another child, in which case the lane
//!   bends (edge lane -> parent lane) and the commit's lane is freed.
//! * Additional parents reuse an existing lane assignment for their sha or
//!   open a new lane via the same lowest-free allocation.
//! * Every lane that is still open after a row (a parent placed by an earlier
//!   child that has not rendered yet) carries an edge into the next row: the
//!   row's own wiring edges, plus a pass-through `(m, m)` vertical for each
//!   open lane the wiring does not already target. Without pass-throughs a
//!   branch line would break on every row it merely passes by.
//! * A root commit (no parents) frees its lane. Lane indices are never
//!   reassigned (no compaction), so layout is page-boundary independent:
//!   feeding the same commits through any page split with a shared
//!   `LaneState` yields byte-identical rows.

use serde::{Deserialize, Serialize};

/// Connection from one row to the row below (or within-row bends).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: u16,
    pub to: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl LaneState {
    /// Lane currently holding `sha`, if any.
    fn lane_of(&self, sha: &str) -> Option<u16> {
        self.lanes
            .iter()
            .position(|slot| slot.as_deref() == Some(sha))
            .map(|i| i as u16)
    }

    /// Lowest free lane; reuses freed slots first, extends only when every
    /// allocated lane is occupied. `next_free` tracks the never-used high-water
    /// mark so the vector never exceeds the peak number of open branches.
    fn allocate_lane(&mut self) -> u16 {
        if let Some(i) = self.lanes.iter().position(|slot| slot.is_none()) {
            return i as u16;
        }
        let i = usize::from(self.next_free).max(self.lanes.len());
        self.next_free = i as u16 + 1;
        self.lanes.push(None);
        i as u16
    }

    /// Highest occupied lane index (pass-through lanes included).
    fn highest_open(&self) -> Option<u16> {
        self.lanes
            .iter()
            .rposition(|slot| slot.is_some())
            .map(|i| i as u16)
    }
}

/// Lay out one page of commits (walk order, topo-sorted upstream).
/// Mutates `state` so the next page continues seamlessly.
pub fn layout_page(
    commits: &[crate::engine::types::CommitInfo],
    state: &mut LaneState,
) -> Vec<GraphRow> {
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        // 1. Resolve this commit's lane: an earlier child may have placed it.
        let lane = match state.lane_of(&commit.sha) {
            Some(lane) => lane,
            None => state.allocate_lane(),
        };
        // The commit is consumed; its slot is free until a parent reclaims it.
        state.lanes[usize::from(lane)] = None;

        // 2. Wire parents, collecting edges from this row to the parent nodes.
        let mut edges: Vec<GraphEdge> = Vec::new();
        for (idx, parent) in commit.parents.iter().enumerate() {
            let to = match state.lane_of(parent) {
                // Convergence: another child already placed this parent.
                Some(existing) => existing,
                None => {
                    if idx == 0 {
                        // First parent continues in the commit's own lane.
                        state.lanes[usize::from(lane)] = Some(parent.clone());
                        lane
                    } else {
                        let opened = state.allocate_lane();
                        state.lanes[usize::from(opened)] = Some(parent.clone());
                        opened
                    }
                }
            };
            edges.push(GraphEdge { from: lane, to });
        }

        // Pass-through lanes: any open lane the wiring above doesn't already
        // target keeps its line running into the next row. Without this the
        // canvas (which draws each edge exactly one row down) shows a break
        // on every row a branch passes by before its commit renders.
        for (m, slot) in state.lanes.iter().enumerate() {
            if slot.is_some() {
                let m = m as u16;
                if !edges.iter().any(|edge| edge.to == m) {
                    edges.push(GraphEdge { from: m, to: m });
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();

        // 3. Column sizing: node lane plus every lane still open below this
        //    row (parents and pass-throughs).
        let lane_count =
            (usize::from(lane) + 1).max(usize::from(state.highest_open().unwrap_or(0)) + 1) as u16;
        rows.push(GraphRow {
            sha: commit.sha.clone(),
            lane,
            edges,
            lane_count,
        });
    }

    rows
}
