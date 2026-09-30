//! Undo/redo as a pure data structure: transactions of recorded source changes.
//!
//! Applying changes to files is the caller's job ([`crate::session`]); this only decides what
//! goes on which stack. A drag records one change per frame under a group key; while the group
//! stays open those merge into one transaction that keeps the first previous value and the last
//! next value of every field.

use struction_data::edit::FieldEdit;

/// One field edit in one project file (project-relative path with `/` separators).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub file: String,
    pub edit: FieldEdit,
}

impl Change {
    pub fn inverse(&self) -> Change {
        Change {
            file: self.file.clone(),
            edit: self.edit.inverse(),
        }
    }

    fn is_noop(&self) -> bool {
        self.edit.previous == self.edit.next
    }
}

/// What one undo step reverts.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub label: String,
    pub changes: Vec<Change>,
}

impl Transaction {
    /// The changes that revert this transaction, in the order to apply them.
    pub fn inverse(&self) -> Vec<Change> {
        self.changes.iter().rev().map(Change::inverse).collect()
    }

    pub fn touches(&self, file: &str) -> bool {
        self.changes.iter().any(|c| c.file == file)
    }

    pub fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.changes.iter().map(|c| c.file.clone()).collect();
        files.sort();
        files.dedup();
        files
    }

    fn merge(&mut self, change: Change) {
        match self
            .changes
            .iter_mut()
            .find(|c| c.file == change.file && c.edit.path == change.edit.path)
        {
            Some(existing) => existing.edit.next = change.edit.next,
            None => self.changes.push(change),
        }
    }
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// Records with this key merge into the top of the undo stack.
    open_group: Option<String>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::with_limit(500)
    }
}

impl History {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            open_group: None,
            limit: limit.max(1),
        }
    }

    /// Records an applied change. With a `group`, consecutive records under the same key form one
    /// transaction until [`Self::close_group`] or a record under another key.
    pub fn record(&mut self, label: &str, change: Change, group: Option<&str>) {
        self.redo.clear();
        if let Some(key) = group
            && self.open_group.as_deref() == Some(key)
            && let Some(top) = self.undo.last_mut()
        {
            top.merge(change);
            return;
        }
        self.close_group();
        if group.is_none() && change.is_noop() {
            return;
        }
        self.undo.push(Transaction {
            label: label.to_owned(),
            changes: vec![change],
        });
        self.open_group = group.map(str::to_owned);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    /// Ends the open group. A group whose fields all ended where they started leaves nothing.
    pub fn close_group(&mut self) {
        if self.open_group.take().is_none() {
            return;
        }
        if let Some(top) = self.undo.last_mut() {
            top.changes.retain(|c| !c.is_noop());
            if top.changes.is_empty() {
                self.undo.pop();
            }
        }
    }

    pub fn is_grouping(&self) -> bool {
        self.open_group.is_some()
    }

    /// Takes the transaction to revert and moves it to the redo stack. The caller applies
    /// [`Transaction::inverse`]; if that fails it calls [`Self::discard_redo`].
    pub fn undo(&mut self) -> Option<Transaction> {
        self.close_group();
        let transaction = self.undo.pop()?;
        self.redo.push(transaction.clone());
        Some(transaction)
    }

    /// Takes the transaction to re-apply and moves it back to the undo stack.
    pub fn redo(&mut self) -> Option<Transaction> {
        self.close_group();
        let transaction = self.redo.pop()?;
        self.undo.push(transaction.clone());
        Some(transaction)
    }

    /// Drops the transaction that the last [`Self::undo`] moved, when reverting it failed.
    pub fn discard_redo(&mut self) {
        self.redo.pop();
    }

    /// Drops the transaction that the last [`Self::redo`] moved, when re-applying it failed.
    pub fn discard_undo(&mut self) {
        self.undo.pop();
    }

    /// Forgets every transaction touching `file`, on both stacks. A file changed outside the
    /// editor no longer matches the recorded previous values, so those steps cannot be trusted.
    /// Returns how many transactions were dropped.
    pub fn invalidate_file(&mut self, file: &str) -> usize {
        let before = self.undo.len() + self.redo.len();
        if self.undo.last().is_some_and(|t| t.touches(file)) {
            self.open_group = None;
        }
        self.undo.retain(|t| !t.touches(file));
        self.redo.retain(|t| !t.touches(file));
        before - self.undo.len() - self.redo.len()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open_group = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    /// Undo stack, oldest first.
    pub fn undo_stack(&self) -> &[Transaction] {
        &self.undo
    }

    /// Redo stack, next to redo last.
    pub fn redo_stack(&self) -> &[Transaction] {
        &self.redo
    }

    /// Files with recorded changes on either stack.
    pub fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = self
            .undo
            .iter()
            .chain(&self.redo)
            .flat_map(Transaction::files)
            .collect();
        files.sort();
        files.dedup();
        files
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use struction_data::edit::parse_path;

    use super::*;

    fn change(file: &str, path: &str, previous: Option<Value>, next: Option<Value>) -> Change {
        Change {
            file: file.into(),
            edit: FieldEdit {
                path: parse_path(path),
                previous,
                next,
            },
        }
    }

    #[test]
    fn undo_redo_moves_transactions_between_stacks() {
        let mut history = History::default();
        history.record("a", change("f", "x", Some(json!(1)), Some(json!(2))), None);
        history.record("b", change("f", "y", None, Some(json!(3))), None);
        assert_eq!(history.undo_label(), Some("b"));

        let undone = history.undo().unwrap();
        assert_eq!(undone.label, "b");
        assert_eq!(undone.inverse()[0].edit.next, None);
        assert_eq!(history.redo_label(), Some("b"));
        assert_eq!(history.undo_label(), Some("a"));

        let redone = history.redo().unwrap();
        assert_eq!(redone.label, "b");
        assert!(!history.can_redo());
    }

    #[test]
    fn a_new_record_clears_redo() {
        let mut history = History::default();
        history.record("a", change("f", "x", Some(json!(1)), Some(json!(2))), None);
        history.undo();
        history.record("c", change("f", "x", Some(json!(1)), Some(json!(5))), None);
        assert!(!history.can_redo());
        assert_eq!(history.undo_stack().len(), 1);
    }

    #[test]
    fn a_drag_coalesces_into_one_transaction() {
        let mut history = History::default();
        for step in 0..10 {
            history.record(
                "Move",
                change("s", "offset", Some(json!(step)), Some(json!(step + 1))),
                Some("drag"),
            );
        }
        history.close_group();
        assert_eq!(history.undo_stack().len(), 1);
        let top = &history.undo_stack()[0];
        assert_eq!(top.changes.len(), 1);
        assert_eq!(top.changes[0].edit.previous, Some(json!(0)));
        assert_eq!(top.changes[0].edit.next, Some(json!(10)));
    }

    #[test]
    fn a_group_merges_per_field_and_keeps_order_for_undo() {
        let mut history = History::default();
        history.record("g", change("s", "a", Some(json!(0)), Some(json!(1))), Some("k"));
        history.record("g", change("s", "b", None, Some(json!(1))), Some("k"));
        history.record("g", change("s", "a", Some(json!(1)), Some(json!(2))), Some("k"));
        let undone = history.undo().unwrap();
        let inverse = undone.inverse();
        assert_eq!(inverse.len(), 2);
        // Reverted in reverse order of first touch: b, then a.
        assert_eq!(inverse[0].edit.path, parse_path("b"));
        assert_eq!(inverse[0].edit.next, None);
        assert_eq!(inverse[1].edit.next, Some(json!(0)));
    }

    #[test]
    fn different_group_keys_are_separate_transactions() {
        let mut history = History::default();
        history.record("g", change("s", "a", Some(json!(0)), Some(json!(1))), Some("one"));
        history.record("g", change("s", "a", Some(json!(1)), Some(json!(2))), Some("two"));
        history.record("h", change("s", "a", Some(json!(2)), Some(json!(3))), None);
        assert_eq!(history.undo_stack().len(), 3);
    }

    #[test]
    fn a_drag_back_to_the_start_leaves_nothing() {
        let mut history = History::default();
        history.record("g", change("s", "a", Some(json!(0)), Some(json!(1))), Some("k"));
        history.record("g", change("s", "a", Some(json!(1)), Some(json!(0))), Some("k"));
        history.close_group();
        assert!(!history.can_undo());
    }

    #[test]
    fn noop_edits_are_not_recorded() {
        let mut history = History::default();
        history.record("a", change("f", "x", Some(json!(1)), Some(json!(1))), None);
        assert!(!history.can_undo());
    }

    #[test]
    fn invalidating_a_file_drops_its_transactions_on_both_stacks() {
        let mut history = History::default();
        history.record("a", change("one", "x", Some(json!(1)), Some(json!(2))), None);
        history.record("b", change("two", "x", Some(json!(1)), Some(json!(2))), None);
        history.record("c", change("one", "y", Some(json!(1)), Some(json!(2))), None);
        history.undo();
        assert_eq!(history.invalidate_file("one"), 2);
        assert_eq!(history.undo_label(), Some("b"));
        assert!(!history.can_redo());
        assert_eq!(history.files(), vec!["two".to_owned()]);
    }

    #[test]
    fn the_limit_drops_the_oldest() {
        let mut history = History::with_limit(2);
        for i in 0..3 {
            history.record(
                &i.to_string(),
                change("f", "x", Some(json!(i)), Some(json!(i + 1))),
                None,
            );
        }
        let labels: Vec<&str> = history
            .undo_stack()
            .iter()
            .map(|t| t.label.as_str())
            .collect();
        assert_eq!(labels, ["1", "2"]);
    }
}
