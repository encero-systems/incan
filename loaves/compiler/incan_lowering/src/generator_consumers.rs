//! Generator consumers: iterator operations that drain their receiver into one value.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;

/// Report whether a checker-selected iterator operation drains its receiver into one value.
pub fn is_generator_consumer(method: IteratorMethodId) -> bool {
    matches!(
        method,
        IteratorMethodId::Count
            | IteratorMethodId::Sum
            | IteratorMethodId::Fold
            | IteratorMethodId::Reduce
            | IteratorMethodId::Any
            | IteratorMethodId::All
            | IteratorMethodId::Find
            | IteratorMethodId::ForEach
    )
}
