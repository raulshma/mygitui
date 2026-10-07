//! Unit tests for the commit-graph lane layout algorithm.
//!
//! All DAGs are hand-built or deterministically generated with an inline
//! xorshift RNG (no external `rand` dependency). Invariants exercised:
//!
//! * first parent continues the child's lane; extra parents open/reuse lanes
//! * converging branches bend onto the existing parent lane
//! * roots free their lane; freed lanes are reused before extending
//! * paged layout with a shared `LaneState` is byte-identical to single-page
//! * every distinct parent receives an edge ending on the parent's row lane
//! * edges are unique, sorted, originate at the row's node lane, and every
//!   lane they touch stays within `lane_count` from source row to target row
//! * `lanes` vector never exceeds the peak number of concurrently open lanes

use std::collections::HashMap;

use super::types::{layout_page, GraphEdge, GraphRow, LaneState};
use crate::engine::types::{CommitInfo, GitSignature};

fn sig() -> GitSignature {
    GitSignature {
        name: "Tester".to_string(),
        email: "tester@example.com".to_string(),
        time: 0,
        offset_minutes: 0,
    }
}

/// CommitInfo filler constructor: sha plus parent shas.
fn mk(sha: &str, parents: &[&str]) -> CommitInfo {
    CommitInfo {
        sha: sha.to_string(),
        parents: parents.iter().map(|p| (*p).to_string()).collect(),
        author: sig(),
        committer: sig(),
        message: String::new(),
        summary: String::new(),
        refs: Vec::new(),
    }
}

fn e(from: u16, to: u16) -> GraphEdge {
    GraphEdge { from, to }
}

fn all_lanes_free(state: &LaneState) -> bool {
    state.lanes.iter().all(|slot| slot.is_none())
}

/// Deterministic xorshift64* generator (no `rand` crate).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Force a nonzero state; xorshift cannot recover from zero.
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// Uniform-ish value in 0..n (n > 0).
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

#[test]
fn linear_chain_stays_in_lane_zero() {
    let commits = vec![mk("A", &["B"]), mk("B", &["C"]), mk("C", &[])];
    let mut state = LaneState::default();
    let rows = layout_page(&commits, &mut state);

    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.lane, 0);
        assert_eq!(row.lane_count, 1);
    }
    assert_eq!(rows[0].sha, "A");
    assert_eq!(rows[0].edges, vec![e(0, 0)]);
    assert_eq!(rows[1].sha, "B");
    assert_eq!(rows[1].edges, vec![e(0, 0)]);
    assert_eq!(rows[2].sha, "C");
    assert!(rows[2].edges.is_empty());
    assert!(all_lanes_free(&state));
}

#[test]
fn diamond_branch_and_merge() {
    let commits = vec![
        mk("A", &["B", "C"]), // merge of B and C
        mk("B", &["D"]),
        mk("C", &["D"]), // both branches share parent D
        mk("D", &[]),
    ];
    let mut state = LaneState::default();
    let rows = layout_page(&commits, &mut state);

    assert_eq!(rows[0].lane, 0); // A
    assert_eq!(rows[0].edges, vec![e(0, 0), e(0, 1)]);
    assert_eq!(rows[0].lane_count, 2);

    assert_eq!(rows[1].lane, 0); // B keeps lane 0
    assert_eq!(rows[1].edges, vec![e(0, 0)]);
    assert_eq!(rows[1].lane_count, 2); // C still passes through lane 1

    assert_eq!(rows[2].lane, 1); // C gets lane 1
    assert_eq!(rows[2].edges, vec![e(1, 0)]); // converges onto D's lane
    assert_eq!(rows[2].lane_count, 2);

    assert_eq!(rows[3].lane, 0); // D ends in lane 0
    assert!(rows[3].edges.is_empty());
    assert_eq!(rows[3].lane_count, 1);
    assert!(all_lanes_free(&state));
}

#[test]
fn orphan_start_without_prior_state() {
    let mut state = LaneState::default();
    let rows = layout_page(&[mk("O", &[])], &mut state);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].lane, 0);
    assert_eq!(rows[0].lane_count, 1);
    assert!(rows[0].edges.is_empty());
    assert!(all_lanes_free(&state));

    // A second unrelated root reuses the freed lane 0.
    let rows = layout_page(&[mk("P", &[])], &mut state);
    assert_eq!(rows[0].lane, 0);
}

#[test]
fn page_splits_match_single_page_layout() {
    let commits = vec![
        mk("A", &["B"]),
        mk("B", &["C"]),
        mk("C", &["D"]),
        mk("D", &["E"]),
        mk("E", &[]),
    ];
    let single = layout_page(&commits, &mut LaneState::default());

    // Split 2 + 3 across a shared state.
    let mut state = LaneState::default();
    let mut paged = layout_page(&commits[..2], &mut state);
    paged.extend(layout_page(&commits[2..], &mut state));
    assert_eq!(paged, single);

    // Degenerate split: one commit per page.
    let mut state = LaneState::default();
    let mut one_by_one = Vec::new();
    for commit in &commits {
        one_by_one.extend(layout_page(std::slice::from_ref(commit), &mut state));
    }
    assert_eq!(one_by_one, single);
}

#[test]
fn freed_lane_is_reused_not_extended() {
    let commits = vec![
        mk("A", &["B", "C"]),
        mk("B", &["D"]),
        mk("C", &["D"]),      // lane 1 freed here
        mk("D", &["E", "F"]), // F must reuse lane 1
        mk("E", &[]),
        mk("F", &[]),
    ];
    let mut state = LaneState::default();
    let rows = layout_page(&commits, &mut state);

    assert_eq!(rows[3].sha, "D");
    assert_eq!(rows[3].edges, vec![e(0, 0), e(0, 1)]);
    assert_eq!(rows[4].lane, 0); // E
    assert_eq!(rows[5].lane, 1); // F reuses the freed lane
    for row in &rows {
        assert!(row.lane_count <= 2);
    }
    assert_eq!(state.lanes.len(), 2); // never grew past two lanes
    assert_eq!(usize::from(state.next_free), state.lanes.len());
    assert!(all_lanes_free(&state));
}

#[test]
fn two_heads_converge_onto_same_parent_lane() {
    let commits = vec![
        mk("M", &["P"]),
        mk("N", &["Q", "P"]), // N is itself a merge: one row, two edges
        mk("Q", &[]),
        mk("P", &[]),
    ];
    let mut state = LaneState::default();
    let rows = layout_page(&commits, &mut state);

    assert_eq!(rows[0].lane, 0);
    assert_eq!(rows[0].edges, vec![e(0, 0)]); // M -> P in lane 0
    assert_eq!(rows[1].lane, 1);
    // N's row carries its own continuation (Q, lane 1) plus the convergence
    // onto the parent lane P already occupies (lane 0).
    assert_eq!(rows[1].edges, vec![e(1, 0), e(1, 1)]);
    assert_eq!(rows[2].lane, 1); // Q inherits N's lane
    assert_eq!(rows[3].lane, 0); // P stays in M's lane
    assert!(all_lanes_free(&state));
}

#[test]
fn fuzz_random_dags_hold_all_invariants() {
    const DAGS: u64 = 200;

    for dag in 0..DAGS {
        let mut rng = Rng::new(0x9E3779B97F4A7C15 ^ dag.wrapping_mul(0xD1B54A32D192ED03));

        // Generate a random DAG in walk order: commit i may only parent
        // commits with index > i, so index order is a valid topo order.
        let n = 1 + usize::try_from(rng.below(40)).expect("bounded");
        let mut commits = Vec::with_capacity(n);
        for i in 0..n {
            let mut parents: Vec<String> = Vec::new();
            if i + 1 < n {
                for _ in 0..rng.below(4) {
                    let p = i + 1 + usize::try_from(rng.below((n - i - 1) as u64)).expect("b");
                    parents.push(format!("c{p}")); // duplicates allowed on purpose
                }
            }
            let refs: Vec<&str> = parents.iter().map(String::as_str).collect();
            commits.push(mk(&format!("c{i}"), &refs));
        }

        // Determinism: same input, fresh state, identical output.
        let rows_a = layout_page(&commits, &mut LaneState::default());
        let rows_b = layout_page(&commits, &mut LaneState::default());
        assert_eq!(rows_a, rows_b, "dag {dag} not deterministic");

        // Random page splits with a shared state must reproduce single-page.
        let mut state = LaneState::default();
        let mut paged: Vec<GraphRow> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut parents_seen: Vec<&str> = Vec::new();
        let mut idx = 0;
        while idx < commits.len() {
            let take = (1 + usize::try_from(rng.below(5)).expect("b")).min(commits.len() - idx);
            for commit in &commits[idx..idx + take] {
                seen.push(commit.sha.as_str());
                parents_seen.extend(commit.parents.iter().map(String::as_str));
            }
            paged.extend(layout_page(&commits[idx..idx + take], &mut state));

            // State sanity after every page.
            let waiting: Vec<&str> = state.lanes.iter().filter_map(|s| s.as_deref()).collect();
            let mut unique = waiting.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), waiting.len(), "dag {dag}: sha in two lanes");
            for sha in &waiting {
                assert!(!seen.contains(sha), "dag {dag}: consumed sha waits");
                assert!(parents_seen.contains(sha), "dag {dag}: stray lane sha");
            }
            let widest = paged.iter().map(|r| usize::from(r.lane_count)).max();
            assert!(
                state.lanes.len() <= widest.unwrap_or(0),
                "dag {dag}: lanes vector grew past peak width"
            );
            assert_eq!(
                usize::from(state.next_free),
                state.lanes.len(),
                "dag {dag}: next_free out of sync"
            );
            idx += take;
        }
        assert_eq!(paged, rows_a, "dag {dag}: paged layout differs from single");
        assert!(
            all_lanes_free(&state),
            "dag {dag}: lanes leak after full DAG"
        );

        // Structural invariants over the single-page rows.
        let lane_of: HashMap<&str, (usize, u16)> = rows_a
            .iter()
            .enumerate()
            .map(|(i, r)| (r.sha.as_str(), (i, r.lane)))
            .collect();
        assert_eq!(lane_of.len(), rows_a.len(), "dag {dag}: duplicate sha rows");

        for (i, row) in rows_a.iter().enumerate() {
            assert!(
                usize::from(row.lane) < usize::from(row.lane_count),
                "dag {dag}"
            );
            for pair in row.edges.windows(2) {
                assert!(pair[0] < pair[1], "dag {dag}: unsorted/duplicate edges");
            }
            for edge in &row.edges {
                assert_eq!(edge.from, row.lane, "dag {dag}: edge from foreign lane");
                assert!(
                    usize::from(edge.to) < usize::from(row.lane_count),
                    "dag {dag}: edge target exceeds lane_count"
                );
            }

            let commit = &commits[i];
            assert_eq!(commit.sha, row.sha);
            let mut distinct: Vec<&str> = commit.parents.iter().map(String::as_str).collect();
            distinct.sort_unstable();
            distinct.dedup();
            for parent in distinct {
                let &(j, parent_lane) = lane_of
                    .get(parent)
                    .unwrap_or_else(|| panic!("dag {dag}: parent {parent} missing from walk"));
                assert!(j > i, "dag {dag}: topo order violated");
                let edge = e(row.lane, parent_lane);
                assert!(
                    row.edges.contains(&edge),
                    "dag {dag}: missing edge {edge:?}"
                );
                // The target lane must stay sized from this row to the
                // parent's row (pass-throughs included).
                for mid in &rows_a[i..=j] {
                    assert!(
                        usize::from(edge.to) < usize::from(mid.lane_count),
                        "dag {dag}: lane {} undersized between rows {i}..{j}",
                        edge.to
                    );
                }
            }
        }
    }
}
