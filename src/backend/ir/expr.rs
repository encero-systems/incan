//! IR expression definitions.
//!
//! These types represent expressions with resolved types and ownership.
//!
//! ## Enum-based dispatch
//!
//! Built-in functions and known methods are represented as enums (`BuiltinFn`, `MethodKind`) rather than stringly-typed
//! names. This enables:
//!
//! - Compile-time exhaustiveness checking in the emitter
//! - Easier refactoring (rename a variant → compiler shows all call sites)
//! - Clear documentation of supported builtins/methods
//!
//! Unknown methods (e.g., Rust interop) remain string-based via `MethodCall`.

use super::decl::IrInteropAdapterKind;
use super::{FunctionSignature, IrSpan, IrType, Ownership};
use incan_core::interop::CoercionPolicy;
use incan_core::lang::builtins::{self as core_builtins, BuiltinFnId};
use incan_core::lang::surface::{
    dict_methods, iterator_methods, list_methods, result_methods, set_methods, string_methods,
};
use incan_core::lang::traits::{self as core_traits, TraitId};
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};

/// A typed expression in IR
#[derive(Debug, Clone)]
pub struct TypedExpr {
    /// The expression kind
    pub kind: IrExprKind,
    /// Resolved type
    pub ty: IrType,
    /// Ownership semantics (owned, borrowed, etc.)
    pub ownership: Ownership,
    /// Source span for error reporting
    pub span: IrSpan,
}

impl TypedExpr {
    pub fn new(kind: IrExprKind, ty: IrType) -> Self {
        Self {
            kind,
            ty,
            ownership: Ownership::Owned,
            span: IrSpan::default(),
        }
    }

    pub fn with_ownership(mut self, ownership: Ownership) -> Self {
        self.ownership = ownership;
        self
    }

    pub fn with_span(mut self, span: IrSpan) -> Self {
        self.span = span;
        self
    }
}

/// IR expression (alias for TypedExpr for convenience)
pub type IrExpr = TypedExpr;

/// A call argument in IR.
///
/// This preserves named-argument information (`foo(x=1)`) so codegen can reorder arguments by parameter name
/// (or apply targeted policies for known APIs).
///
/// For positional and unpack arguments, `name` is `None`.
#[derive(Debug, Clone)]
pub struct IrCallArg {
    /// Optional argument name (present for `foo(x=1)`, absent for positional args).
    pub name: Option<String>,
    /// Surface argument kind used by rest-call lowering.
    pub kind: IrCallArgKind,
    /// Argument expression.
    pub expr: IrExpr,
}

/// Surface call-argument kind preserved for emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrCallArgKind {
    /// Ordinary positional argument.
    Positional,
    /// Ordinary named argument.
    Named,
    /// Positional unpack argument (`*expr`).
    PositionalUnpack,
    /// Keyword unpack argument (`**expr`).
    KeywordUnpack,
}

/// Lowering hint for ordinary method-call argument conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MethodCallArgPolicy {
    /// Use the emitter's default receiver-based conversion behavior.
    #[default]
    Default,
    /// Preserve Rust method-call lookup shape for borrow-sensitive APIs such as `HashMap::get`.
    PreserveShape,
}

/// Expression kinds in IR
#[derive(Debug, Clone)]
pub enum IrExprKind {
    // Literals
    Unit,
    None,
    Bool(bool),
    Int(i64),
    /// Integer literal preserved by source text because it does not fit in the legacy `i64` literal slot.
    ///
    /// The emitter relies on surrounding Rust type context for literals such as `u128::MAX`.
    IntLiteral(String),
    Float(f64),
    Decimal(String),
    String(String),
    Bytes(Vec<u8>),

    // Variable reference
    Var {
        name: String,
        /// Whether this is a move, borrow, or copy
        access: VarAccess,
        /// Whether this identifier refers to a value binding or a type-like name.
        ///
        /// This is used to eliminate emitter heuristics like TitleCase detection for deciding whether
        /// `Type.method(...)` should be emitted as `Type::method(...)`.
        ref_kind: VarRefKind,
    },

    /// Read from a compiler-managed module static storage cell.
    StaticRead {
        name: String,
    },

    /// Create a live local binding wrapper from a compiler-managed module static.
    StaticBinding {
        name: String,
    },

    /// Reference an associated function item such as `Type::method`.
    AssociatedFunction {
        type_name: String,
        function_name: String,
    },
    /// Zero-sized value-level marker for a source type.
    TypeToken {
        ty: IrType,
    },

    /// Reference a free function item with optional explicit type arguments.
    ///
    /// Generic decorated wrappers need to pass `__incan_original_name::<T>` as a callable value after the wrapper has
    /// a concrete type-parameter environment. A plain variable reference cannot carry that turbofish.
    FunctionItem {
        name: String,
        type_args: Vec<IrType>,
    },

    /// Register a generated function pointer with its source callable name.
    ///
    /// Generic decorated wrappers instantiate `__incan_original_name::<T>` at runtime. Rust can coerce that
    /// monomorphized item to a function pointer, but a global function-pointer trait impl cannot name the originating
    /// generic declaration. This expression records explicit compiler metadata for that concrete pointer before the
    /// decorator sees it.
    RegisterCallableName {
        callable: Box<IrExpr>,
        source_name: String,
    },

    /// Cache one decorated generic function value by concrete type-argument key.
    ///
    /// Generic decorators that return the same callable surface are still declaration-side metadata hooks. When the
    /// callable signature itself does not mention the generic type parameters, generated Rust can keep one decorated
    /// function pointer per concrete type-argument tuple and avoid replaying decorator side effects on every call.
    CacheGenericDecoratedFunction {
        cache_name: String,
        type_param_names: Vec<String>,
        value: Box<IrExpr>,
    },

    // Binary operations
    BinOp {
        op: BinOp,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },

    // Unary operations
    UnaryOp {
        op: UnaryOp,
        operand: Box<IrExpr>,
    },

    // Function call (unknown/user-defined function)
    Call {
        func: Box<IrExpr>,
        /// Explicit call-site type arguments (`f[T](...)`) when provided.
        type_args: Vec<IrType>,
        args: Vec<IrCallArg>,
        /// Resolved callable signature when the callee expression carries metadata that is not represented by the
        /// flattened IR function type, such as RFC 038 rest-parameter markers.
        callable_signature: Option<FunctionSignature>,
        /// Canonical callee path when known (e.g. `["std","testing","assert_eq"]`).
        /// This lets emission/type-directed policies resolve calls independent of local import style.
        canonical_path: Option<Vec<String>>,
    },

    /// Built-in function call (enum-dispatched).
    /// Used for known builtins like `print`, `len`, `range`, etc.
    /// The emitter matches on `BuiltinFn` instead of string names.
    BuiltinCall {
        func: BuiltinFn,
        args: Vec<IrExpr>,
    },

    // Method call (unknown/user-defined method)
    MethodCall {
        receiver: Box<IrExpr>,
        method: String,
        /// Typechecker-selected dispatch target when this call must not emit as an ordinary Rust method lookup.
        dispatch: Option<IrMethodDispatch>,
        /// Explicit call-site type arguments (`obj.method[T](...)`) when provided.
        type_args: Vec<IrType>,
        args: Vec<IrCallArg>,
        /// Resolved method signature when rest markers must survive into emission.
        callable_signature: Option<FunctionSignature>,
        arg_policy: MethodCallArgPolicy,
    },

    /// Known method call (enum-dispatched).
    /// Used for known methods like `upper`, `append`, `contains`, etc.
    /// The emitter matches on `MethodKind` instead of string names.
    KnownMethodCall {
        receiver: Box<IrExpr>,
        kind: MethodKind,
        args: Vec<IrCallArg>,
    },

    // Field access
    Field {
        object: Box<IrExpr>,
        field: String,
    },

    // Index access (list[i], dict[k])
    Index {
        object: Box<IrExpr>,
        index: Box<IrExpr>,
    },

    // Slice access (list[start:end[:step]])
    Slice {
        target: Box<IrExpr>,
        start: Option<Box<IrExpr>>,
        end: Option<Box<IrExpr>>,
        step: Option<Box<IrExpr>>,
    },

    // List comprehension
    ListComp {
        element: Box<IrExpr>,
        pattern: Box<Pattern>,
        iterable: Box<IrExpr>,
        filter: Option<Box<IrExpr>>,
    },
    DictComp {
        key: Box<IrExpr>,
        value: Box<IrExpr>,
        pattern: Box<Pattern>,
        iterable: Box<IrExpr>,
        filter: Option<Box<IrExpr>>,
    },
    Generator {
        element: Box<IrExpr>,
        clauses: Vec<IrGeneratorClause>,
    },

    // List literal
    List(Vec<IrListEntry>),

    // Dict literal
    Dict(Vec<IrDictEntry>),

    // Set literal
    Set(Vec<IrExpr>),

    // Tuple literal
    Tuple(Vec<IrExpr>),

    // Struct construction
    Struct {
        name: String,
        fields: Vec<(String, IrExpr)>,
    },

    // If expression
    If {
        condition: Box<IrExpr>,
        then_branch: Box<IrExpr>,
        else_branch: Option<Box<IrExpr>>,
    },

    // Match expression
    Match {
        scrutinee: Box<IrExpr>,
        arms: Vec<MatchArm>,
    },

    // Closure
    Closure {
        params: Vec<(String, IrType)>,
        body: Box<IrExpr>,
        captures: Vec<String>,
    },

    // Block expression (sequence of statements with optional trailing expr)
    Block {
        stmts: Vec<super::IrStmt>,
        value: Option<Box<IrExpr>>,
    },

    // Loop expression (`loop { ... break value; }`)
    Loop {
        body: Vec<super::IrStmt>,
    },

    // Await expression (async)
    Await(Box<IrExpr>),

    // Race expression (`race for value:`)
    Race {
        binding: String,
        arms: Vec<RaceArm>,
    },

    // Try/Propogate expression (i.e. the Rust-like `?` operator)
    Try(Box<IrExpr>),

    // Range (start..end)
    Range {
        start: Option<Box<IrExpr>>,
        end: Option<Box<IrExpr>>,
        inclusive: bool,
    },

    // Cast expression (as Type)
    Cast {
        expr: Box<IrExpr>,
        to_type: IrType,
    },

    /// RFC 009 numeric resize helper lowered from `resize()`, `try_resize()`, `wrapping_resize()`, and
    /// `saturating_resize()`.
    NumericResize {
        expr: Box<IrExpr>,
        policy: NumericResizePolicy,
        to_type: IrType,
    },

    /// RFC 041: Explicit interop coercion inserted by lowering for Rust-boundary calls.
    InteropCoerce {
        expr: Box<IrExpr>,
        from_ty: IrType,
        to_ty: IrType,
        kind: IrInteropCoercionKind,
    },

    // Format string (f-string)
    Format {
        parts: Vec<FormatPart>,
    },

    // Literal value (used for generated code)
    Literal(Literal),

    // List of field names for reflection
    FieldsList(Vec<String>),

    // `incan_stdlib::json::__private::stringify_or_raise(self, type_name)`
    SerdeToJson,

    // serde_json::from_str(s) - contains the target type name
    SerdeFromJson(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericResizePolicy {
    Lossless,
    Try,
    Wrapping,
    Saturating,
}

#[derive(Debug, Clone)]
pub enum IrGeneratorClause {
    For { pattern: Pattern, iterable: Box<IrExpr> },
    If(IrExpr),
}

/// One lowered arm in an async race expression.
#[derive(Debug, Clone)]
pub struct RaceArm {
    pub awaitable: IrExpr,
    pub body: IrExpr,
}

/// One lowered entry in a list literal.
#[derive(Debug, Clone)]
pub enum IrListEntry {
    /// Direct element expression.
    Element(IrExpr),
    /// Spread list expression.
    Spread(IrExpr),
}

/// One lowered entry in a dict literal.
#[derive(Debug, Clone)]
pub enum IrDictEntry {
    /// Direct key/value pair.
    Pair(IrExpr, Box<IrExpr>),
    /// Spread dict expression.
    Spread(IrExpr),
}

/// Coercion strategy at a Rust interop boundary.
#[derive(Debug, Clone)]
pub enum IrInteropCoercionKind {
    /// Coercion admitted by the boundary matrix (`int -> i64`, `i16 -> i64`, `str -> &str`, ...).
    Builtin {
        policy: CoercionPolicy,
        rust_target: String,
    },
    /// Adapter call from a `rusttype` `interop:` edge.
    AdapterCall {
        adapter: Box<IrExpr>,
        adapter_kind: IrInteropAdapterKind,
    },
    /// Rusttype wrapper unwrap (`.0`) when lowering a wrapper-backed edge.
    RustTypeUnwrap,
    /// Borrow an owned source value for a concrete Rust reference parameter.
    RustBorrow {
        /// Whether the Rust parameter requires `&mut` rather than a shared reference.
        mutable: bool,
    },
}

/// Literal values for generated code
#[derive(Debug, Clone)]
pub enum Literal {
    /// Static string literal (&'static str)
    StaticStr(String),
}

/// Part of a format string
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum FormatPart {
    /// Literal text
    Literal(String),
    /// Expression to interpolate
    Expr { expr: IrExpr, style: FormatStyle },
}

/// Formatting style requested by one f-string interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormatStyle {
    /// User-facing display formatting (`{value}`).
    #[default]
    Display,
    /// Structured debug formatting (`{value:?}`).
    Debug,
}

impl FormatStyle {
    /// Return whether this interpolation should emit Rust debug formatting for the resolved backend type.
    pub fn emits_rust_debug(self, ty: &IrType) -> bool {
        matches!(self, Self::Debug) || matches!(self, Self::Display) && display_style_uses_structured_debug(ty)
    }
}

/// Return whether default Incan f-string display should use structured formatting for a backend representation that
/// does not expose Rust `Display` directly.
pub fn display_style_uses_structured_debug(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::List(_)
            | IrType::Dict(_, _)
            | IrType::Set(_)
            | IrType::Tuple(_)
            | IrType::Option(_)
            | IrType::Result(_, _)
    )
}

/// How a variable is accessed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VarAccess {
    /// Move the value (consumes ownership)
    #[default]
    Move,
    /// Non-consuming read; caller-side conversion policy decides borrow/clone.
    ///
    /// Used by lowering when a non-Copy binding is read but may still be used
    /// later in the same block.
    Read,
    /// Borrow immutably (&)
    Borrow,
    /// Borrow mutably (&mut)
    BorrowMut,
    /// Copy the value (for Copy types)
    Copy,
}

/// Whether an identifier expression refers to a value binding or a type-like name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VarRefKind {
    /// A value binding (local/param/field), i.e. should behave like `value.method(...)`.
    #[default]
    Value,
    /// A local binding wrapper that may point at a module static storage cell.
    StaticBinding,
    /// A type name (models/classes/enums/newtypes) in expression position, i.e. `Type.method(...)`.
    TypeName,
    /// A non-local imported/module name that behaves namespace-like in emitted Rust.
    ExternalName,
    /// A `rust::...` imported name that should use Rust interop argument conversion rules.
    ExternalRustName,
}

/// Binary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv, // // (Python-style floor division)
    Mod,
    Pow,

    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Logical
    And,
    Or,

    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// Unary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Deref,
    Ref,
    RefMut,
}

/// A match arm
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub bindings: Vec<MatchArmBinding>,
    pub guard: Option<IrExpr>,
    pub body: IrExpr,
}

/// Compiler-inserted binding materialized before a match-arm body runs.
#[derive(Debug, Clone)]
pub struct MatchArmBinding {
    pub name: String,
    pub ty: IrType,
    pub value: IrExpr,
    pub guard_value: Option<IrExpr>,
}

/// Pattern for match expressions
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Literal(IrExpr),
    Tuple(Vec<Pattern>),
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
    },
    Enum {
        name: String,
        variant: String,
        fields: Vec<Pattern>,
    },
    Or(Vec<Pattern>),
}

// ============================================================================
// Enum-based dispatch for builtins and methods
// ============================================================================

/// Built-in functions recognized by the Incan compiler.
///
/// These are functions that lower to specific Rust code patterns rather than regular function calls.
/// The emitter matches on this enum instead of string names to avoid stringly-typing.
///
/// ## Adding a new builtin
///
/// 1. Add a variant here
/// 2. Update `BuiltinFn::from_name()` to map the string name
/// 3. Update `emit_builtin_call()` in `expressions/builtins.rs` to emit the Rust code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFn {
    /// `print(x)` / `println(x)` → `println!("{}", x)`
    Print,
    /// `len(x)` → `::std::convert::identity(x.len() as i64)`
    Len,
    /// `sum(x)` → `x.iter().sum::<i64>()`
    Sum,
    /// `min(xs)` → minimum element
    Min,
    /// `max(xs)` → maximum element
    Max,
    /// `str(x)` → `x.to_string()`
    Str,
    /// `int(x)` → parse or cast to i64
    Int,
    /// `float(x)` → parse or cast to f64
    Float,
    /// `bool(x)` → convert to bool
    Bool,
    /// `abs(x)` → `x.abs()`
    Abs,
    /// `range(...)` → Rust range expressions
    Range,
    /// `enumerate(x)` → `x.iter().enumerate()` with the index cast to Incan `int`.
    Enumerate,
    /// `zip(a, b)` → `a.iter().zip(b.iter())`
    Zip,
    /// `sorted(xs)` → sorted copy
    Sorted,
    /// `read_file(path)` → `std::fs::read_to_string(path)`
    ReadFile,
    /// `write_file(path, content)` → `std::fs::write(path, content)`
    WriteFile,
    /// `json_stringify(x)` → `incan_stdlib::json::__private::stringify_or_raise(&x, type_name)`
    JsonStringify,
    /// `list.repeat(value, count)` → `incan_stdlib::collections::list_repeat(value, count)`
    ListRepeat,
}

impl BuiltinFn {
    /// Try to resolve a function name to a known builtin.
    ///
    /// Returns `None` for unknown functions (which are treated as user-defined).
    pub fn from_name(name: &str) -> Option<Self> {
        let id = core_builtins::from_str(name)?;
        Some(match id {
            BuiltinFnId::Print => Self::Print,
            BuiltinFnId::Len => Self::Len,
            BuiltinFnId::Sum => Self::Sum,
            BuiltinFnId::Min => Self::Min,
            BuiltinFnId::Max => Self::Max,
            BuiltinFnId::Str => Self::Str,
            BuiltinFnId::Int => Self::Int,
            BuiltinFnId::Float => Self::Float,
            BuiltinFnId::Bool => Self::Bool,
            BuiltinFnId::Abs => Self::Abs,
            BuiltinFnId::Range => Self::Range,
            BuiltinFnId::Enumerate => Self::Enumerate,
            BuiltinFnId::Zip => Self::Zip,
            BuiltinFnId::Sorted => Self::Sorted,
            BuiltinFnId::ReadFile => Self::ReadFile,
            BuiltinFnId::WriteFile => Self::WriteFile,
            BuiltinFnId::JsonStringify => Self::JsonStringify,
            BuiltinFnId::IsInstance => return None,
        })
    }
}

/// Backend dispatch target selected by frontend method resolution.
#[derive(Debug, Clone, PartialEq)]
pub enum IrMethodDispatch {
    /// Emit a fully-qualified trait method call.
    Trait { trait_path: String, type_args: Vec<IrType> },
    /// Keep the emitted call as regular Rust method lookup while retaining this extension-trait import binding.
    RustExtensionTraitImport { binding: String },
}

/// Known method kinds recognized by the Incan compiler.
///
/// These are methods that have special lowering or emit behavior. The emitter matches on this enum instead of string
/// names. Classification is receiver-aware so ambiguous names like `join` and `contains` only become known methods
/// when the receiver type is one of the builtin families handled by the compiler.
///
/// ## Adding a new method
///
/// 1. Add a variant to the appropriate method-family enum
/// 2. Update `MethodKind::for_receiver()` to classify the method for supported receiver types
/// 3. Update `emit_known_method_call()` in `expressions/methods.rs` to emit the Rust code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// String and frozen-string methods that route through the string emitter.
    String(StringMethodKind),
    /// Collection methods recognized for builtin list/dict/set receivers.
    Collection(CollectionMethodKind),
    /// Iterator adapter and terminal methods recognized for `Iterator[T]` receivers.
    Iterator(IteratorMethodKind),
    /// Result combinators recognized for `Result[T, E]` receivers.
    Result(result_methods::ResultMethodId),
    /// Internal helper methods that lower to dedicated runtime support.
    Internal(InternalMethodKind),
}

/// Known string-method variants handled by the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringMethodKind {
    /// `s.upper()` → `s.to_uppercase()`
    Upper,
    /// `s.lower()` → `s.to_lowercase()`
    Lower,
    /// `s.strip()` → `s.trim().to_string()`
    Strip,
    /// `s.split(sep)` → `s.split(sep).map(...).collect()`
    Split,
    /// `s.replace(old, new)` → `s.replace(old, new)`
    Replace,
    /// `sep.join(items)` → `items.join(sep)`
    Join,
    /// `s.startswith(prefix)` → `s.starts_with(prefix)`
    StartsWith,
    /// `s.endswith(suffix)` → `s.ends_with(suffix)`
    EndsWith,
    /// `s.contains(needle)` → `str_contains(s, needle)`
    Contains,
}

/// Known collection-method variants handled by the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionMethodKind {
    /// `x.contains(item)` → varies by collection type
    Contains,
    /// `x.get(key)` → `x.get(key)`
    Get,
    /// `x.insert(k, v)` → `x.insert(k, v)`
    Insert,
    /// `x.remove(key)` → `x.remove(key)`
    Remove,
    /// `list.append(item)` → `list.push(item)`
    Append,
    /// `list.extend(items)` → `incan_stdlib::collections::list_extend(...)`
    Extend,
    /// `list.clone()` → `list.clone()`
    Clone,
    /// `list.pop()` lowers via `incan_stdlib::collections::__private::list_pop(...)`, which preserves the `T` return
    /// type while raising `IndexError: pop from empty list` on the runtime side (#194).
    Pop,
    /// `list.swap(i, j)` → `incan_stdlib::collections::list_swap(...)`
    Swap,
    /// `list.count(value)` → `incan_stdlib::collections::list_count(...)`
    Count,
    /// `list.index(value)` → `incan_stdlib::collections::list_index(...)`
    Index,
    /// `list.reserve(n)` → `list.reserve(n as usize)`
    Reserve,
    /// `list.reserve_exact(n)` → `list.reserve_exact(n as usize)`
    ReserveExact,
}

/// Known iterator-method variants handled by the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IteratorMethodKind {
    /// `iterable.iter()` returns an iterator over owned Incan surface items.
    Iter,
    /// `iter.map(f)` lazily maps each item through `f`.
    Map,
    /// `iter.filter(f)` lazily keeps items where `f(item)` returns true.
    Filter,
    /// `iter.enumerate()` yields `(index, item)` pairs.
    Enumerate,
    /// `iter.zip(other)` yields pairs until either input is exhausted.
    Zip,
    /// `iter.take(n)` yields at most `n` items.
    Take,
    /// `iter.skip(n)` discards at most `n` items.
    Skip,
    /// `iter.take_while(f)` yields items until `f(item)` first returns false.
    TakeWhile,
    /// `iter.skip_while(f)` discards items while `f(item)` returns true.
    SkipWhile,
    /// `iter.chain(other)` yields receiver items followed by `other` items.
    Chain,
    /// `iter.flat_map(f)` maps each item to an iterable and flattens the result.
    FlatMap,
    /// `iter.batch(size)` yields fixed-size lists with a final short list included.
    Batch,
    /// `iter.collect()` consumes into a list.
    Collect,
    /// `iter.count()` consumes and returns the count as Incan `int`.
    Count,
    /// `iter.reduce(init, f)` consumes and folds with an explicit initial accumulator.
    Reduce,
    /// `iter.fold(init, f)` consumes and folds with an explicit initial accumulator.
    Fold,
    /// `iter.any(f)` short-circuits when `f(item)` is true.
    Any,
    /// `iter.all(f)` short-circuits when `f(item)` is false.
    All,
    /// `iter.find(f)` returns the first item where `f(item)` is true.
    Find,
    /// `iter.for_each(f)` consumes the iterator for side effects.
    ForEach,
    /// `iter.sum()` consumes and sums numeric items.
    Sum,
}

/// Internal compiler-only method variants handled during emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalMethodKind {
    /// `x.__slice__(start, end)` → `x[start..end]`
    Slice,
}

impl MethodKind {
    /// Try to resolve an RFC 088 iterator method name without considering a receiver type.
    pub fn for_iterator_method_name(name: &str) -> Option<Self> {
        iterator_method_kind(name).map(Self::Iterator)
    }

    /// Try to resolve a Result method name without considering a receiver type.
    pub fn for_result_method_name(name: &str) -> Option<Self> {
        result_methods::from_str(name).map(Self::Result)
    }

    /// Try to resolve a method name to a known method kind for the given receiver type.
    ///
    /// Returns `None` for unknown methods (which pass through as regular method calls).
    pub fn for_receiver(receiver_ty: &IrType, name: &str) -> Option<Self> {
        let mut receiver_ty = receiver_ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
            receiver_ty = inner.as_ref();
        }

        // Internal
        if incan_core::lang::magic_methods::from_str(name)
            == Some(incan_core::lang::magic_methods::MagicMethodId::Slice)
        {
            return Some(Self::Internal(InternalMethodKind::Slice));
        }

        match receiver_ty {
            IrType::Result(_, _) => result_methods::from_str(name).map(Self::Result),
            IrType::String | IrType::FrozenStr | IrType::StaticStr | IrType::StrRef => {
                let id = string_methods::from_str(name)?;
                use string_methods::StringMethodId as S;
                Some(Self::String(match id {
                    S::Upper => StringMethodKind::Upper,
                    S::Lower => StringMethodKind::Lower,
                    S::Strip => StringMethodKind::Strip,
                    S::Split => StringMethodKind::Split,
                    S::Replace => StringMethodKind::Replace,
                    S::Join => StringMethodKind::Join,
                    S::StartsWith => StringMethodKind::StartsWith,
                    S::EndsWith => StringMethodKind::EndsWith,
                    S::Contains => StringMethodKind::Contains,
                    // The rest are either typechecker-only (return types) or normal method calls:
                    _ => return None,
                }))
            }
            IrType::List(_) => {
                if iterator_methods::from_str(name) == Some(iterator_methods::IteratorMethodId::Iter) {
                    return Some(Self::Iterator(IteratorMethodKind::Iter));
                }
                let id = list_methods::from_str(name)?;
                use list_methods::ListMethodId as L;
                Some(Self::Collection(match id {
                    L::Append => CollectionMethodKind::Append,
                    L::Extend => CollectionMethodKind::Extend,
                    L::Clone => CollectionMethodKind::Clone,
                    L::Pop => CollectionMethodKind::Pop,
                    L::Swap => CollectionMethodKind::Swap,
                    L::Reserve => CollectionMethodKind::Reserve,
                    L::ReserveExact => CollectionMethodKind::ReserveExact,
                    L::Contains => CollectionMethodKind::Contains,
                    L::Remove => CollectionMethodKind::Remove,
                    L::Count => CollectionMethodKind::Count,
                    L::Index => CollectionMethodKind::Index,
                }))
            }
            IrType::Dict(_, _) => {
                let id = dict_methods::from_str(name)?;
                use dict_methods::DictMethodId as D;
                Some(Self::Collection(match id {
                    D::Get => CollectionMethodKind::Get,
                    D::Insert => CollectionMethodKind::Insert,
                    // keys/values are emitted as normal method calls.
                    D::Keys | D::Values => return None,
                }))
            }
            IrType::Set(_) => {
                if iterator_methods::from_str(name) == Some(iterator_methods::IteratorMethodId::Iter) {
                    return Some(Self::Iterator(IteratorMethodKind::Iter));
                }
                if set_methods::from_str(name).is_some() {
                    return Some(Self::Collection(CollectionMethodKind::Contains));
                }
                None
            }
            IrType::NamedGeneric(type_name, _)
                if matches!(
                    collection_types::from_str(type_name),
                    Some(CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet)
                ) && iterator_methods::from_str(name) == Some(iterator_methods::IteratorMethodId::Iter) =>
            {
                Some(Self::Iterator(IteratorMethodKind::Iter))
            }
            IrType::NamedGeneric(type_name, _) | IrType::Struct(type_name)
                if is_iterator_protocol_type_name(type_name) =>
            {
                iterator_method_kind(name).map(Self::Iterator)
            }
            _ => None,
        }
    }
}

/// Return whether a nominal IR type name denotes the standard `Iterator` protocol.
///
/// Lowering can preserve either short stdlib names (`Iterator`) or qualified paths, depending on import and metadata
/// context. Method classification only needs the nominal protocol family, so it accepts the final path segment.
fn is_iterator_protocol_type_name(name: &str) -> bool {
    name.rsplit("::").next() == Some(core_traits::as_str(TraitId::Iterator))
}

/// Classify an RFC 088 iterator method name into the structured backend method family.
fn iterator_method_kind(name: &str) -> Option<IteratorMethodKind> {
    let id = iterator_methods::from_str(name)?;
    use iterator_methods::IteratorMethodId as M;
    Some(match id {
        M::Iter => IteratorMethodKind::Iter,
        M::Map => IteratorMethodKind::Map,
        M::Filter => IteratorMethodKind::Filter,
        M::Enumerate => IteratorMethodKind::Enumerate,
        M::Zip => IteratorMethodKind::Zip,
        M::Take => IteratorMethodKind::Take,
        M::Skip => IteratorMethodKind::Skip,
        M::TakeWhile => IteratorMethodKind::TakeWhile,
        M::SkipWhile => IteratorMethodKind::SkipWhile,
        M::Chain => IteratorMethodKind::Chain,
        M::FlatMap => IteratorMethodKind::FlatMap,
        M::Batch => IteratorMethodKind::Batch,
        M::Collect => IteratorMethodKind::Collect,
        M::Count => IteratorMethodKind::Count,
        M::Reduce => IteratorMethodKind::Reduce,
        M::Fold => IteratorMethodKind::Fold,
        M::Any => IteratorMethodKind::Any,
        M::All => IteratorMethodKind::All,
        M::Find => IteratorMethodKind::Find,
        M::ForEach => IteratorMethodKind::ForEach,
        M::Sum => IteratorMethodKind::Sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterator_method_kind_for_receiver_classifies_rfc088_surface() {
        let iterator_ty = IrType::NamedGeneric(core_traits::as_str(TraitId::Iterator).to_string(), vec![IrType::Int]);
        for (name, expected) in [
            ("map", IteratorMethodKind::Map),
            ("filter", IteratorMethodKind::Filter),
            ("enumerate", IteratorMethodKind::Enumerate),
            ("zip", IteratorMethodKind::Zip),
            ("take", IteratorMethodKind::Take),
            ("skip", IteratorMethodKind::Skip),
            ("take_while", IteratorMethodKind::TakeWhile),
            ("skip_while", IteratorMethodKind::SkipWhile),
            ("chain", IteratorMethodKind::Chain),
            ("flat_map", IteratorMethodKind::FlatMap),
            ("batch", IteratorMethodKind::Batch),
            ("collect", IteratorMethodKind::Collect),
            ("count", IteratorMethodKind::Count),
            ("reduce", IteratorMethodKind::Reduce),
            ("fold", IteratorMethodKind::Fold),
            ("any", IteratorMethodKind::Any),
            ("all", IteratorMethodKind::All),
            ("find", IteratorMethodKind::Find),
            ("for_each", IteratorMethodKind::ForEach),
            ("sum", IteratorMethodKind::Sum),
        ] {
            assert_eq!(
                MethodKind::for_receiver(&iterator_ty, name),
                Some(MethodKind::Iterator(expected)),
                "expected iterator method classification for `{name}`"
            );
        }
        assert_eq!(
            MethodKind::for_receiver(&IrType::List(Box::new(IrType::Int)), "iter"),
            Some(MethodKind::Iterator(IteratorMethodKind::Iter))
        );
    }

    #[test]
    fn iterator_method_kind_for_receiver_does_not_capture_iterable_or_plain_structs() {
        assert_eq!(
            MethodKind::for_receiver(
                &IrType::NamedGeneric(
                    core_traits::as_str(TraitId::IntoIterator).to_string(),
                    vec![IrType::Int]
                ),
                "map"
            ),
            None
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::Struct("Dataset".to_string()), "map"),
            None
        );
    }

    #[test]
    fn result_method_kind_for_receiver_classifies_result_surface() {
        let result_ty = IrType::Result(Box::new(IrType::Int), Box::new(IrType::String));
        for (name, expected) in [
            ("map", result_methods::ResultMethodId::Map),
            ("map_err", result_methods::ResultMethodId::MapErr),
            ("and_then", result_methods::ResultMethodId::AndThen),
            ("or_else", result_methods::ResultMethodId::OrElse),
            ("inspect", result_methods::ResultMethodId::Inspect),
            ("inspect_err", result_methods::ResultMethodId::InspectErr),
            ("unwrap", result_methods::ResultMethodId::Unwrap),
            ("unwrap_or", result_methods::ResultMethodId::UnwrapOr),
        ] {
            assert_eq!(
                MethodKind::for_receiver(&result_ty, name),
                Some(MethodKind::Result(expected)),
                "expected Result method classification for `{name}`"
            );
        }
        assert_eq!(MethodKind::for_receiver(&result_ty, "missing"), None);
    }
}
