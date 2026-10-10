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
//!   bends (edge lane -> parent lane) and the commit's lane is freed. When
//!   that earlier claim sits in a lane to the RIGHT of the commit's lane
//!   (a merged branch that rendered before the merge), the claim is
//!   re-homed onto the commit's lane instead and the side line bends in —
//!   git's leftmost-wins column mapping keeps the mainline straight.
//! * Additional parents reuse an existing lane assignment for their sha or
//!   open the lowest free lane not used as a bend source on this row; this
//!   keeps the row's independent parent edges from crossing.
//! * Every lane that was already open before a row and remains open after it
//!   carries its own outgoing edge into the next row, unless a row edge already
//!   leaves that lane. A convergence edge into the lane does not replace this
//!   continuation. Newly opened parent lanes are carried by the edge that
//!   opens them.
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
    /// mark so the vector never exceeds the peak graph width.
    fn allocate_lane(&mut self) -> u16 {
        self.allocate_lane_excluding(&[])
    }

    /// Lowest free lane, excluding bend sources on the current row. These
    /// lanes are empty in the next-row state, but reusing one for a different
    /// parent would make the row's edges cross.
    fn allocate_lane_excluding(&mut self, reserved: &[u16]) -> u16 {
        if let Some(i) = self
            .lanes
            .iter()
            .enumerate()
            .position(|(i, slot)| slot.is_none() && !reserved.contains(&(i as u16)))
        {
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
        // Existing lines may converge with a parent edge on this row, but
        // still need their own outgoing segment to reach that parent.
        let open_before: Vec<bool> = state.lanes.iter().map(Option::is_some).collect();

        // 1. Resolve this commit's lane: an earlier child may have placed it.
        let lane = match state.lane_of(&commit.sha) {
            Some(lane) => lane,
            None => state.allocate_lane(),
        };
        // The commit is consumed; its slot is free until a parent reclaims it.
        state.lanes[usize::from(lane)] = None;

        // 2. Wire parents, collecting edges from this row to the parent
        //    nodes. First-parent special case: when the first parent was
        //    already claimed by an earlier child in a lane to the RIGHT of
        //    ours (a merged branch that rendered before the merge), our
        //    mainline lane wins — the earlier claim bends in over this row
        //    (git's leftmost-wins column mapping). Without this the
        //    mainline would step into the side lane and stay there.
        let mut edges: Vec<GraphEdge> = Vec::new();
        let mut reserved_bend_sources: Vec<u16> = Vec::new();
        for (idx, parent) in commit.parents.iter().enumerate() {
            let to = match state.lane_of(parent) {
                Some(existing) => {
                    if idx == 0 && existing > lane {
                        // Re-home the first parent onto our lane; the side
                        // line that claimed it bends in below this row. Keep
                        // its source lane clear for this row so another
                        // parent edge cannot cross the bend.
                        state.lanes[usize::from(existing)] = None;
                        reserved_bend_sources.push(existing);
                        state.lanes[usize::from(lane)] = Some(parent.clone());
                        edges.push(GraphEdge {
                            from: existing,
                            to: lane,
                        });
                        lane
                    } else {
                        // Convergence: another child already placed this
                        // parent (at our lane or to its left).
                        existing
                    }
                }
                None => {
                    if idx == 0 {
                        // First parent continues in the commit's own lane.
                        state.lanes[usize::from(lane)] = Some(parent.clone());
                        lane
                    } else {
                        let opened = state.allocate_lane_excluding(&reserved_bend_sources);
                        state.lanes[usize::from(opened)] = Some(parent.clone());
                        opened
                    }
                }
            };
            edges.push(GraphEdge { from: lane, to });
        }

        // Pass-through lanes: lines that were open before this row continue
        // unless an edge explicitly leaves their lane. An incoming edge to
        // that lane brings another line in; it does not carry the existing
        // line onward to the same parent.
        for (m, slot) in state.lanes.iter().enumerate() {
            if slot.is_some()
                && open_before.get(m).copied().unwrap_or(false)
                && !edges.iter().any(|edge| usize::from(edge.from) == m)
            {
                let m = m as u16;
                edges.push(GraphEdge { from: m, to: m });
            }
        }
        edges.sort_unstable();
        edges.dedup();

        // 3. Column sizing: the node lane, every lane still open below this
        //    row (parents and pass-throughs), and every lane an edge of
        //    this row touches (bend sources included).
        let mut width = usize::from(lane) + 1;
        if let Some(open) = state.highest_open() {
            width = width.max(usize::from(open) + 1);
        }
        for edge in &edges {
            width = width
                .max(usize::from(edge.from) + 1)
                .max(usize::from(edge.to) + 1);
        }
        let lane_count = width as u16;
        rows.push(GraphRow {
            sha: commit.sha.clone(),
            lane,
            edges,
            lane_count,
        });
    }

    rows
}
