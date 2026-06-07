//! Generated-code support descriptors shared by the compiler and runtime stdlib.
//!
//! These descriptors describe toolchain-owned support hooks without making the compiler depend on `incan_stdlib`.
//! Runtime bodies still live in the stdlib crate; this module only carries pure metadata.

/// A Rust macro that should be expanded inside one generated Incan module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedModuleSupport {
    /// Source module name such as `std.collections`.
    pub source_module: &'static str,
    /// Generated Rust module name such as `__incan_std.collections`.
    pub generated_module: &'static str,
    /// Fully qualified macro path without the trailing `!`.
    pub macro_path: &'static str,
    /// Local generated items that must stay reachable because the macro expands against them.
    pub required_items: &'static [&'static str],
}

/// Source items directly referenced by compiler-generated Rust paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedPathSupport {
    /// Source module name such as `std.derives.collection`.
    pub source_module: &'static str,
    /// Generated Rust module name such as `__incan_std.derives.collection`.
    pub generated_module: &'static str,
    /// Local generated items directly named by backend emission.
    pub required_items: &'static [&'static str],
    /// Lowered semantic surface that makes these generated paths reachable.
    pub trigger: GeneratedPathSupportTrigger,
}

/// Lowered semantic surfaces that make generated Rust path support items reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedPathSupportTrigger {
    /// Iterator method lowering routes through `std.derives.collection` support types.
    IteratorMethod,
}

/// Borrowed argument shape expected by a generated-method fast path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodFastPathArgShape {
    /// Borrow one string-like key as `&str`.
    BorrowedStr,
    /// Borrow a list of owned strings as `&[String]`.
    BorrowedStringList,
}

/// A generated method that can replace a source-level method call for a concrete receiver family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodFastPath {
    /// Source module that owns the receiver type.
    pub source_module: &'static str,
    /// Generated module that owns the receiver type.
    pub generated_module: &'static str,
    /// Receiver type name without type arguments.
    pub receiver_type: &'static str,
    /// Concrete first type argument needed by this fast path.
    pub receiver_arg_type: &'static str,
    /// Source-level method name.
    pub method: &'static str,
    /// Generated Rust method name supplied by the support macro.
    pub target_method: &'static str,
    /// Required argument shape.
    pub arg_shape: MethodFastPathArgShape,
}

const ORDINAL_MAP_MODULE_SUPPORTS: &[GeneratedModuleSupport] = &[GeneratedModuleSupport {
    source_module: "std.collections",
    generated_module: "__incan_std.collections",
    macro_path: "incan_stdlib::__incan_ordinal_map_string_fast_impls",
    required_items: &[
        "OrdinalMap",
        "OrdinalMapError",
        "OrdinalMapErrorKind",
        "_missing_ordinal",
        "_ordinal_map_error",
        "_ordinal_hash",
    ],
}];

const DIRECT_GENERATED_PATH_SUPPORTS: &[GeneratedPathSupport] = &[GeneratedPathSupport {
    source_module: "std.derives.collection",
    generated_module: "__incan_std.derives.collection",
    required_items: &[
        "Iterator",
        "ListIterator",
        "MapIterator",
        "FilterIterator",
        "EnumerateIterator",
        "ZipIterator",
        "TakeIterator",
        "SkipIterator",
        "TakeWhileIterator",
        "SkipWhileIterator",
        "ChainIterator",
        "FlatMapIterator",
        "BatchIterator",
    ],
    trigger: GeneratedPathSupportTrigger::IteratorMethod,
}];

const ORDINAL_MAP_METHOD_FAST_PATHS: &[MethodFastPath] = &[
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "__contains__",
        target_method: "__incan_ordinal_contains_str",
        arg_shape: MethodFastPathArgShape::BorrowedStr,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "__getitem__",
        target_method: "__incan_ordinal_getitem_str",
        arg_shape: MethodFastPathArgShape::BorrowedStr,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "get",
        target_method: "__incan_ordinal_get_str",
        arg_shape: MethodFastPathArgShape::BorrowedStr,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "require",
        target_method: "__incan_ordinal_require_str",
        arg_shape: MethodFastPathArgShape::BorrowedStr,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "get_unchecked",
        target_method: "__incan_ordinal_get_unchecked_str",
        arg_shape: MethodFastPathArgShape::BorrowedStr,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "get_many",
        target_method: "__incan_ordinal_get_many_str",
        arg_shape: MethodFastPathArgShape::BorrowedStringList,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "require_many",
        target_method: "__incan_ordinal_require_many_str",
        arg_shape: MethodFastPathArgShape::BorrowedStringList,
    },
    MethodFastPath {
        source_module: "std.collections",
        generated_module: "__incan_std.collections",
        receiver_type: "OrdinalMap",
        receiver_arg_type: "str",
        method: "get_many_unchecked",
        target_method: "__incan_ordinal_get_many_unchecked_str",
        arg_shape: MethodFastPathArgShape::BorrowedStringList,
    },
];

/// Return module-level support macros published for generated stdlib modules.
#[must_use]
pub fn generated_module_supports() -> &'static [GeneratedModuleSupport] {
    ORDINAL_MAP_MODULE_SUPPORTS
}

/// Return compiler-generated path support items that pruning must keep reachable.
#[must_use]
pub fn generated_path_supports() -> &'static [GeneratedPathSupport] {
    DIRECT_GENERATED_PATH_SUPPORTS
}

/// Return method fast paths published for generated stdlib modules.
#[must_use]
pub fn method_fast_paths() -> &'static [MethodFastPath] {
    ORDINAL_MAP_METHOD_FAST_PATHS
}
