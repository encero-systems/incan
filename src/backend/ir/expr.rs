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
use incan_core::lang::surface::{dict_methods, list_methods, set_methods, string_methods};

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
    Float(f64),
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
    /// Coercion admitted by the scalar matrix (`int -> i64`, `str -> &str`, `float -> f32`, ...).
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
    Expr(IrExpr),
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
    pub guard: Option<IrExpr>,
    pub body: IrExpr,
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

/// Internal compiler-only method variants handled during emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalMethodKind {
    /// `x.__slice__(start, end)` → `x[start..end]`
    Slice,
}

impl MethodKind {
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
                let id = list_methods::from_str(name)?;
                use list_methods::ListMethodId as L;
                Some(Self::Collection(match id {
                    L::Append => CollectionMethodKind::Append,
                    L::Extend => CollectionMethodKind::Extend,
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
                if set_methods::from_str(name).is_some() {
                    return Some(Self::Collection(CollectionMethodKind::Contains));
                }
                None
            }
            _ => None,
        }
    }
}
