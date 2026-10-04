//! Undo/redo as a pure data structure: transactions of recorded source changes.
//!
//! Applying changes to files is the caller's job ([`crate::session`]); this only decides what
//! goes on which stack. A drag records one change per frame under a group key; while the group
//! stays open those merge into one transaction that keeps the first previous value and the last
//! next value of every field.

use std::collections::BTreeMap;
use struction_data::edit::FieldEdit;

#[derive(Clone, Debug, PartialEq)]
pub struct SourceChange {
    pub before: String,
    pub after: String,
}

impl SourceChange {
    /// The change that undoes this one.
    pub fn reversed(&self) -> Self {
        Self {
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }
}

/// One field edit in one project file (project-relative path with `/` separators).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub file: String,
    pub edit: FieldEdit,
}

impl Change {
    fn is_noop(&self) -> bool {
        self.edit.previous == self.edit.next
    }
}

/// What one undo step reverts.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub label: String,
    pub changes: Vec<Change>,
    pub sources: BTreeMap<String, SourceChange>,
}

impl Transaction {
    pub fn touches(&self, file: &str) -> bool {
        self.sources.contains_key(file) || self.changes.iter().any(|c| c.file == file)
    }

    pub fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.changes.iter().map(|c| c.file.clone()).collect();
        files.extend(self.sources.keys().cloned());
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

#[derive(Clone, Debug)]
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
        if change.is_noop() {
            return;
        }
        self.redo.clear();
        if let Some(key) = group
            && self.open_group.as_deref() == Some(key)
            && let Some(top) = self.undo.last_mut()
        {
            top.merge(change);
            return;
        }
        self.close_group();
        self.undo.push(Transaction {
            label: label.to_owned(),
            changes: vec![change],
            sources: BTreeMap::new(),
        });
        self.open_group = group.map(str::to_owned);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    /// Records a complete transaction, such as several field edits made as one operation.
    pub fn record_transaction(&mut self, transaction: Transaction) {
        self.record_transaction_grouped(transaction, None);
    }

    pub fn record_transaction_grouped(&mut self, transaction: Transaction, group: Option<&str>) {
        if transaction.changes.iter().all(Change::is_noop) {
            return;
        }
        self.redo.clear();
        if let Some(key) = group
            && self.open_group.as_deref() == Some(key)
            && let Some(top) = self.undo.last_mut()
        {
            for change in transaction.changes {
                top.merge(change);
            }
            for (file, source) in transaction.sources {
                top.sources
                    .entry(file)
                    .and_modify(|saved| saved.after = source.after.clone())
                    .or_insert(source);
            }
            return;
        }
        self.close_group();
        self.undo.push(transaction);
        self.open_group = group.map(str::to_owned);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    /// Records both semantic edits and exact source text for lossless undo.
    pub fn record_source(
        &mut self,
        label: &str,
        change: Change,
        source: SourceChange,
        group: Option<&str>,
    ) {
        if change.is_noop() {
            return;
        }
        let file = change.file.clone();
        self.record(label, change, group);
        if let Some(top) = self.undo.last_mut() {
            top.sources
                .entry(file)
                .and_modify(|saved| saved.after = source.after.clone())
                .or_insert(source);
        }
    }

    /// Ends the open group. A group whose fields all ended where they started leaves nothing.
    pub fn close_group(&mut self) {
        if self.open_group.take().is_none() {
            return;
        }
        if let Some(top) = self.undo.last_mut() {
            top.changes.retain(|c| !c.is_noop());
            if (top.sources.is_empty() && top.changes.is_empty())
                || (!top.sources.is_empty()
                    && top
                        .sources
                        .values()
                        .all(|source| source.before == source.after))
            {
                self.undo.pop();
            }
        }
    }

    pub fn is_grouping(&self) -> bool {
        self.open_group.is_some()
    }

    /// The transaction [`Self::undo`] would revert. Close the group first to see past it.
    pub fn next_undo(&self) -> Option<&Transaction> {
        self.undo.last()
    }

    /// The transaction [`Self::redo`] would re-apply.
    pub fn next_redo(&self) -> Option<&Transaction> {
        self.redo.last()
    }

    /// Moves the transaction to revert to the redo stack, once the caller reverted its sources.
    pub fn undo(&mut self) -> Option<&Transaction> {
        self.close_group();
        let transaction = self.undo.pop()?;
        self.redo.push(transaction);
        self.redo.last()
    }

    /// Moves the transaction to re-apply back to the undo stack, once the caller re-applied it.
    pub fn redo(&mut self) -> Option<&Transaction> {
        self.close_group();
        let transaction = self.redo.pop()?;
        self.undo.push(transaction);
        self.undo.last()
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

        assert_eq!(history.undo().unwrap().label, "b");
        assert_eq!(history.redo_label(), Some("b"));
        assert_eq!(history.undo_label(), Some("a"));

        assert_eq!(history.redo().unwrap().label, "b");
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
    fn a_group_merges_per_field_in_order_of_first_touch() {
        let mut history = History::default();
        history.record(
            "g",
            change("s", "a", Some(json!(0)), Some(json!(1))),
            Some("k"),
        );
        history.record("g", change("s", "b", None, Some(json!(1))), Some("k"));
        history.record(
            "g",
            change("s", "a", Some(json!(1)), Some(json!(2))),
            Some("k"),
        );
        let changes = &history.undo().unwrap().changes;
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].edit.path, parse_path("a"));
        assert_eq!(changes[0].edit.previous, Some(json!(0)));
        assert_eq!(changes[0].edit.next, Some(json!(2)));
        assert_eq!(changes[1].edit.path, parse_path("b"));
    }

    #[test]
    fn different_group_keys_are_separate_transactions() {
        let mut history = History::default();
        history.record(
            "g",
            change("s", "a", Some(json!(0)), Some(json!(1))),
            Some("one"),
        );
        history.record(
            "g",
            change("s", "a", Some(json!(1)), Some(json!(2))),
            Some("two"),
        );
        history.record("h", change("s", "a", Some(json!(2)), Some(json!(3))), None);
        assert_eq!(history.undo_stack().len(), 3);
    }

    #[test]
    fn a_drag_back_to_the_start_leaves_nothing() {
        let mut history = History::default();
        history.record(
            "g",
            change("s", "a", Some(json!(0)), Some(json!(1))),
            Some("k"),
        );
        history.record(
            "g",
            change("s", "a", Some(json!(1)), Some(json!(0))),
            Some("k"),
        );
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
        history.record(
            "a",
            change("one", "x", Some(json!(1)), Some(json!(2))),
            None,
        );
        history.record(
            "b",
            change("two", "x", Some(json!(1)), Some(json!(2))),
            None,
        );
        history.record(
            "c",
            change("one", "y", Some(json!(1)), Some(json!(2))),
            None,
        );
        history.undo();
        assert_eq!(history.invalidate_file("one"), 2);
        assert_eq!(history.undo_label(), Some("b"));
        assert!(!history.can_redo());
        assert_eq!(history.undo_stack()[0].files(), ["two"]);
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
