/// A nested module whose function is imported under a different spelling.
pub mod provider {
    /// A function referenced through a local alias.
    pub fn helper() -> i32 {
        7
    }
}
use graph_provider::leaf as external;
use provider::{helper as closure_helper, helper as renamed};
/// A receiver that shares a method name with another receiver.
pub struct Widget;
impl Widget {
    /// The method selected by `Widget.value()`.
    pub fn value(&self) -> i32 {
        1
    }
}
/// A distinct receiver must never become the target of `Widget.value()`.
pub struct Other;
impl Other {
    /// A same-spelled method with a different checked owner.
    pub fn value(&self) -> i32 {
        2
    }
}
/// A concrete caller combines an aliased local call, a dependency call and a receiver-resolved method.
pub fn caller() -> i32 {
    let number = renamed();
    number + Widget.value() + external()
}
/// A function-pointer call remains explicitly unresolved.
pub fn indirect(call: fn() -> i32) -> i32 {
    call()
}
/// A trait-object call names a requirement rather than an invented implementor set.
pub trait Value {
    /// A dynamic dispatch slot.
    fn value(&self) -> i32;
}
/// Record the requirement as a lower bound.
pub fn dynamic(value: &dyn Value) -> i32 {
    value.value()
}
/// Type references also occur outside bodies.
pub struct Holder {
    pub value: Widget,
}

/// A borrowed opaque result has synthetic lifetime definitions without HIR nodes.
pub fn borrowed<'a>(items: &'a [Widget]) -> impl Iterator<Item = &'a Widget> {
    items.iter()
}

/// The consumer's method target must link to the dependency's definition.
pub fn dependency_method() -> i32 {
    graph_provider::Gadget.value()
}

/// A closure expression keeps its enclosing declared caller.
pub fn closure_caller() -> i32 {
    let invoke = || closure_helper();
    invoke()
}
