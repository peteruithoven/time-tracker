//! What a pull from a connected service would change in the app, so syncing
//! can pause when that's more than usual (see [`PullImpact::should_hold`]).
//!
//! The impact compares a client's entries before and after applying the pull
//! (in a transaction that can be rolled back), so every cause shows up the
//! same way: logs deleted in Fibery, a task moved to another project, a field
//! that suddenly reads as 0, a changed mapping.

use serde::{Deserialize, Serialize};

/// Pause when more than this many entries would be removed or changed…
pub const MAX_REMOVED_OR_CHANGED: usize = 5;
/// …or tracked time would drop by more than this…
pub const MAX_LOST_SECS: i64 = 4 * 60 * 60;
/// …or more than this many entries would be added…
pub const MAX_ADDED: usize = 50;
// …or every synced entry in the pulled range would be removed.

/// An entry as far as a pull can change it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryView {
    pub task_id: i64,
    pub task: String,
    pub project_id: Option<i64>,
    pub project: Option<String>,
    pub date: String,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    /// Duration; 0 for a running timer.
    pub secs: i64,
}

impl EntryView {
    /// Same task, project, date, times and duration. Notes and renames
    /// don't count as changes.
    pub fn same_as(&self, other: &Self) -> bool {
        self.task_id == other.task_id
            && self.project_id == other.project_id
            && self.date == other.date
            && self.start_at == other.start_at
            && self.end_at == other.end_at
            && self.secs == other.secs
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub before: EntryView,
    pub after: EntryView,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PullImpact {
    /// The connected client's name, for messages.
    pub client: String,
    pub removed: Vec<EntryView>,
    pub changed: Vec<Change>,
    pub added: Vec<EntryView>,
    /// Synced entries in the pulled range that a pull could remove.
    pub synced_before: usize,
}

impl PullImpact {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.changed.is_empty() && self.added.is_empty()
    }

    /// Every synced entry in the pulled range would be removed: more likely
    /// lost access or changed settings than real deletions.
    pub fn all_missing(&self) -> bool {
        self.synced_before > 0 && self.removed.len() >= self.synced_before
    }

    /// Tracked time that would disappear: removed entries plus shortened ones.
    pub fn lost_secs(&self) -> i64 {
        let removed: i64 = self.removed.iter().map(|e| e.secs).sum();
        let shortened: i64 = self.changed.iter().map(|c| (c.before.secs - c.after.secs).max(0)).sum();
        removed + shortened
    }

    /// More than usual: pause syncing and ask.
    pub fn should_hold(&self) -> bool {
        self.removed.len() + self.changed.len() > MAX_REMOVED_OR_CHANGED
            || self.all_missing()
            || self.lost_secs() > MAX_LOST_SECS
            || self.added.len() > MAX_ADDED
    }

    /// Not more than what was approved earlier.
    pub fn within(&self, approved: &Self) -> bool {
        self.removed.len() <= approved.removed.len()
            && self.changed.len() <= approved.changed.len()
            && self.added.len() <= approved.added.len()
            && self.lost_secs() <= approved.lost_secs()
    }

    /// The impact without the per-entry lists' order mattering: newest first.
    pub fn sorted(mut self) -> Self {
        let key = |e: &EntryView| (e.date.clone(), e.start_at);
        self.removed.sort_by_key(|e| std::cmp::Reverse(key(e)));
        self.added.sort_by_key(|e| std::cmp::Reverse(key(e)));
        self.changed.sort_by_key(|c| std::cmp::Reverse(key(&c.before)));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(secs: i64) -> EntryView {
        EntryView {
            task_id: 1,
            task: "Wireframes".into(),
            project_id: Some(1),
            project: Some("Website relaunch".into()),
            date: "2026-11-19".into(),
            start_at: None,
            end_at: None,
            secs,
        }
    }

    fn impact(removed: usize, changed: usize, added: usize, synced_before: usize) -> PullImpact {
        PullImpact {
            client: "Acme Studio".into(),
            removed: vec![entry(600); removed],
            changed: vec![Change { before: entry(600), after: entry(600) }; changed],
            added: vec![entry(600); added],
            synced_before,
        }
    }

    #[test]
    fn holds_above_the_thresholds() {
        assert!(!impact(3, 2, 0, 100).should_hold(), "5 removed or changed is fine");
        assert!(impact(3, 3, 0, 100).should_hold(), "6 is not");
        assert!(!impact(0, 0, 50, 100).should_hold());
        assert!(impact(0, 0, 51, 100).should_hold());
        assert!(impact(2, 0, 0, 2).should_hold(), "all synced entries gone");
        assert!(!impact(0, 0, 0, 0).should_hold(), "nothing to lose");

        let mut long = impact(1, 0, 0, 100);
        long.removed[0].secs = 4 * 3600 + 60;
        assert!(long.should_hold(), "more than 4 hours gone");
        let mut shortened = impact(0, 1, 0, 100);
        shortened.changed[0].before.secs = 5 * 3600;
        shortened.changed[0].after.secs = 0;
        assert_eq!(shortened.lost_secs(), 5 * 3600);
        assert!(shortened.should_hold(), "a duration that drops to 0 counts as lost time");
    }

    #[test]
    fn approval_covers_the_same_or_less() {
        let approved = impact(8, 2, 0, 100);
        assert!(impact(8, 2, 0, 100).within(&approved));
        assert!(impact(6, 0, 0, 100).within(&approved));
        assert!(!impact(9, 2, 0, 100).within(&approved));
        assert!(!impact(8, 2, 1, 100).within(&approved));
    }

    #[test]
    fn notes_and_renames_are_not_changes() {
        let a = entry(600);
        let b = EntryView { task: "Renamed".into(), project: Some("Renamed too".into()), ..a.clone() };
        assert!(a.same_as(&b));
        assert!(!a.same_as(&EntryView { project_id: Some(2), ..a.clone() }));
    }
}
