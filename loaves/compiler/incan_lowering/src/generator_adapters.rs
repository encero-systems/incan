//! Generator adapters: iterator operations that chain a lazy adapter onto their receiver.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;

/// Report whether a checker-selected iterator operation chains a lazy adapter.
pub fn is_generator_adapter(method: IteratorMethodId) -> bool {
    matches!(
        method,
        IteratorMethodId::Take
            | IteratorMethodId::Skip
            | IteratorMethodId::Batch
            | IteratorMethodId::Map
            | IteratorMethodId::Filter
            | IteratorMethodId::TakeWhile
            | IteratorMethodId::SkipWhile
            | IteratorMethodId::FlatMap
            | IteratorMethodId::Chain
            | IteratorMethodId::Enumerate
            | IteratorMethodId::Zip
    )
}
