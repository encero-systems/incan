//! A focused test's installed set, selected for its own thread in place of the one the process's pin names, so a test
//! can observe which records a whole compiler pass reads of an installed Loaf.

use std::cell::{Cell, RefCell};

use super::RecordSet;

thread_local! {
    static SELECTED: Cell<Option<&'static RecordSet>> = const { Cell::new(None) };
    static READ: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The set this thread's test selected, if any.
pub(super) fn selected() -> Option<&'static RecordSet> {
    SELECTED.with(Cell::get)
}

/// Select `set` for this thread until the returned selection drops, with no record read yet.
pub(super) fn select(set: RecordSet) -> Selection {
    let set: &'static RecordSet = Box::leak(Box::new(set));
    SELECTED.with(|selected| selected.set(Some(set)));
    READ.with(|read| read.borrow_mut().clear());
    Selection(set)
}

/// Note that this thread read the value of the record `key`.
pub(super) fn note_read(key: &str) {
    READ.with(|read| read.borrow_mut().push(key.to_owned()));
}

/// The keys of the records this thread read since its test selected a set, in reading order.
pub(super) fn read_keys() -> Vec<String> {
    READ.with(|read| read.borrow().clone())
}

/// One test's selection; dropping it leaves this thread with the process's own set again.
pub(super) struct Selection(pub &'static RecordSet);

impl Drop for Selection {
    fn drop(&mut self) {
        SELECTED.with(|selected| selected.set(None));
    }
}
