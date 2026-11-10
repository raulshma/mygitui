//! Tests for the shared in-progress detection ([`super::in_progress`]) —
//! the precedence table every detector now shares.

use super::in_progress::{InProgress, Probe};
use git2::RepositoryState;

fn probe(patch: impl FnOnce(&mut Probe)) -> Probe {
    let mut probe = Probe::default();
    patch(&mut probe);
    probe
}

#[test]
fn nothing_set_is_clean() {
    assert_eq!(probe(|_| {}).in_progress(), None);
    assert!(!probe(|_| {}).bisecting());
}

#[test]
fn rebase_wins_over_every_other_marker() {
    // During a rebase the merge/sequencer marker files may also exist;
    // `git status` reports only the rebase (the rule merge.rs duplicated).
    let p = probe(|p| {
        p.rebase_dirs = true;
        p.merge_head = true;
        p.cherry_pick_head = true;
        p.revert_head = true;
        p.sequencer_dir = true;
        p.state = Some(RepositoryState::Merge);
    });
    assert_eq!(p.in_progress(), Some(InProgress::Rebasing));
}

#[test]
fn custom_rebase_json_counts_as_rebase() {
    let p = probe(|p| {
        p.custom_rebase = true;
        p.merge_head = true;
    });
    assert_eq!(p.in_progress(), Some(InProgress::Rebasing));
}

#[test]
fn state_enums_drive_detection_alone() {
    let p = probe(|p| p.state = Some(RepositoryState::RebaseInteractive));
    assert_eq!(p.in_progress(), Some(InProgress::Rebasing));
    let p = probe(|p| p.state = Some(RepositoryState::ApplyMailboxOrRebase));
    assert_eq!(p.in_progress(), Some(InProgress::Rebasing));
    let p = probe(|p| p.state = Some(RepositoryState::Merge));
    assert_eq!(p.in_progress(), Some(InProgress::Merging));
    let p = probe(|p| p.state = Some(RepositoryState::CherryPickSequence));
    assert_eq!(p.in_progress(), Some(InProgress::CherryPicking));
    let p = probe(|p| p.state = Some(RepositoryState::RevertSequence));
    assert_eq!(p.in_progress(), Some(InProgress::Reverting));
}

#[test]
fn cherry_pick_beats_revert_beats_merge() {
    let p = probe(|p| {
        p.cherry_pick_head = true;
        p.revert_head = true;
        p.merge_head = true;
    });
    assert_eq!(p.in_progress(), Some(InProgress::CherryPicking));

    let p = probe(|p| {
        p.revert_head = true;
        p.merge_head = true;
    });
    assert_eq!(p.in_progress(), Some(InProgress::Reverting));

    let p = probe(|p| p.merge_head = true);
    assert_eq!(p.in_progress(), Some(InProgress::Merging));
}

#[test]
fn sequencer_dir_alone_reports_the_sequence() {
    // Between steps of a pick/revert sequence the HEAD markers may be
    // absent; the dir alone must not read as clean.
    let p = probe(|p| p.sequencer_dir = true);
    assert_eq!(p.in_progress(), Some(InProgress::CherryPicking));
    let p = probe(|p| {
        p.sequencer_dir = true;
        p.revert_head = true;
    });
    assert_eq!(p.in_progress(), Some(InProgress::Reverting));
}

#[test]
fn bisect_is_reported_independently() {
    let p = probe(|p| p.bisect_log = true);
    assert!(p.bisecting());
    // A bisect alone must not read as an operation on the working tree.
    assert_eq!(p.in_progress(), None);
    // And it coexists with the sequencer without overriding it.
    let p = probe(|p| {
        p.bisect_log = true;
        p.cherry_pick_head = true;
    });
    assert!(p.bisecting());
    assert_eq!(p.in_progress(), Some(InProgress::CherryPicking));
}

#[test]
fn labels_match_the_conflict_source_strings() {
    assert_eq!(InProgress::Merging.label(), "merge");
    assert_eq!(InProgress::Rebasing.label(), "rebase");
    assert_eq!(InProgress::CherryPicking.label(), "cherry-pick");
    assert_eq!(InProgress::Reverting.label(), "revert");
}
