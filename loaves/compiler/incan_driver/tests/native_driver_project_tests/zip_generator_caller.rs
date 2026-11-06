//! Isolated review probe against the already-baked runtime; not production code.
use incan_native_runtime::{clone_zip, poll_zip, zip_lists, zip_lists_generator};
use std::cell::RefCell;
use std::rc::Rc;

/// Construction must retain an unpolled owned source even when its partner is empty.
#[test]
fn unpolled_source_owner_lives_until_generator_drop() {
    let token = Rc::new(7_i64);
    let source = zip_lists(vec![Rc::clone(&token), Rc::clone(&token)], Vec::<i64>::new());
    assert_eq!(Rc::strong_count(&token), 3);
    let deferred = zip_lists_generator(source);
    let count = Rc::strong_count(&token);
    println!("unpolled_rc_count={count}");
    drop(deferred);
    assert_eq!(Rc::strong_count(&token), 1);
    assert_eq!(count, 3, "construction eagerly released the unpolled source");
}

/// Exhaustion preserves shortest-input semantics and releases owners when dropped.
#[test]
fn empty_and_unequal_sources_exhaust_correctly() {
    let empty_left = zip_lists_generator(zip_lists(Vec::<i64>::new(), vec![1, 2]));
    assert_eq!(empty_left.collect(), vec![]);
    let empty_right = zip_lists_generator(zip_lists(vec![1, 2], Vec::<i64>::new()));
    assert_eq!(empty_right.collect(), vec![]);
    let short_left = zip_lists_generator(zip_lists(vec![1], vec![10, 20, 30]));
    assert_eq!(short_left.collect(), vec![(1, 10)]);
    let short_right = zip_lists_generator(zip_lists(vec![1, 2, 3], vec![10]));
    assert_eq!(short_right.collect(), vec![(1, 10)]);
    let token = Rc::new(4_i64);
    let mut exhausted = zip_lists_generator(zip_lists(vec![Rc::clone(&token)], Vec::<i64>::new()));
    assert_eq!(exhausted.next(), None);
    assert_eq!(exhausted.next(), None);
    drop(exhausted);
    assert_eq!(Rc::strong_count(&token), 1);
}

/// Converting a partly polled source preserves its position and an independent clone.
#[test]
fn conversion_preserves_cursor_and_independent_source() {
    let mut original = zip_lists(vec![10_i64, 20, 30], vec![1_i64, 2, 3]);
    let saved = clone_zip(&original);
    let mut prefix = vec![];
    assert!(poll_zip(&mut original, &mut prefix));
    assert_eq!(prefix, vec![(10, 1)]);
    assert_eq!(zip_lists_generator(original).collect(), vec![(20, 2), (30, 3)]);
    assert_eq!(zip_lists_generator(saved).collect(), vec![(10, 1), (20, 2), (30, 3)]);
}

#[derive(Debug)]
struct ObservedClone {
    label: &'static str,
    events: Rc<RefCell<Vec<&'static str>>>,
}
impl Clone for ObservedClone {
    /// Record source polling independently of generator element-expression evaluation.
    fn clone(&self) -> Self {
        self.events.borrow_mut().push(self.label);
        Self { label: self.label, events: Rc::clone(&self.events) }
    }
}

/// No source item is polled during construction; each requested pair polls left then right.
#[test]
fn cloning_waits_for_poll_and_respects_left_before_right() {
    let events = Rc::new(RefCell::new(vec![]));
    let make = |label| ObservedClone { label, events: Rc::clone(&events) };
    let source = zip_lists(vec![make("L0"), make("L1"), make("L2")], vec![make("R0"), make("R1")]);
    let mut generator = zip_lists_generator(source);
    let construction = events.borrow().clone();
    println!("construction_clones={construction:?}");
    events.borrow_mut().clear();
    let first = generator.next().map(|(left, right)| (left.label, right.label));
    let first_poll = events.borrow().clone();
    println!("first_poll_clones={first_poll:?}");
    events.borrow_mut().clear();
    assert_eq!(first, Some(("L0", "R0")));
    assert_eq!(generator.next().map(|(left, right)| (left.label, right.label)), Some(("L1", "R1")));
    assert_eq!(generator.next().map(|(left, right)| (left.label, right.label)), None);
    let tail = events.borrow().clone();
    println!("tail_clones={tail:?}");
    assert!(construction.is_empty(), "construction polled or copied source items");
    assert_eq!(first_poll, vec!["L0", "R0"]);
    assert_eq!(tail, vec!["L1", "R1", "L2"]);
}
