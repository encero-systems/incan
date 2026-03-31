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
use super::{IrSpan, IrType, Ownership};
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
/// For positional arguments, `name` is `None`.
#[derive(Debug, Clone)]
pub struct IrCallArg {
    /// Optional argument name (present for `foo(x=1)`, absent for positional args).
    pub name: Option<String>,
    /// Argument expression.
    pub expr: IrExpr,
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
        args: Vec<IrCallArg>,
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
        args: Vec<IrCallArg>,
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
        variable: String,
        iterable: Box<IrExpr>,
        filter: Option<Box<IrExpr>>,
    },
    DictComp {
        key: Box<IrExpr>,
        value: Box<IrExpr>,
        variable: String,
        iterable: Box<IrExpr>,
        filter: Option<Box<IrExpr>>,
    },

    // List literal
    List(Vec<IrExpr>),

    // Dict literal
    Dict(Vec<(IrExpr, IrExpr)>),

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

    // serde_json::to_string(self).unwrap()
    SerdeToJson,

    // serde_json::from_str(s) - contains the target type name
    SerdeFromJson(String),
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
    /// `len(x)` → `x.len() as i64`
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
    /// `enumerate(x)` → `x.iter().enumerate()`
    Enumerate,
    /// `zip(a, b)` → `a.iter().zip(b.iter())`
    Zip,
    /// `sorted(xs)` → sorted copy
    Sorted,
    /// `read_file(path)` → `std::fs::read_to_string(path)`
    ReadFile,
    /// `write_file(path, content)` → `std::fs::write(path, content)`
    WriteFile,
    /// `json_stringify(x)` → `serde_json::to_string(&x).unwrap()`
    JsonStringify,
    /// `sleep(secs)` → `incan_stdlib::__private::tokio::time::sleep(...)`
    Sleep,
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
            BuiltinFnId::Sleep => Self::Sleep,
        })
    }
}

/// Known method kinds recognized by the Incan compiler.
///
/// These are methods that have special lowering or emit behavior. The emitter matches on this enum instead of string
/// names.
///
/// ## Adding a new method
///
/// 1. Add a variant here
/// 2. Update `MethodKind::from_name()` to map the string name
/// 3. Update `emit_known_method_call()` in `expressions/methods.rs` to emit the Rust code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    // ---- String methods ----
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

    // ---- Collection methods ----
    /// `x.contains(item)` → varies by type
    Contains,
    /// `x.get(key)` → `x.get(key)`
    Get,
    /// `x.insert(k, v)` → `x.insert(k, v)`
    Insert,
    /// `x.remove(key)` → `x.remove(key)`
    Remove,

    // ---- List methods ----
    /// `list.append(item)` → `list.push(item)`
    Append,
    /// `list.pop()` lowers via `Vec::pop()` without requiring `T: Default`. An empty list raises
    /// `IndexError: pop from empty list` through `incan_stdlib::errors::raise_list_pop_empty` (#194).
    Pop,
    /// `list.swap(i, j)` → `list.swap(i as usize, j as usize)`
    Swap,
    /// `list.reserve(n)` → `list.reserve(n as usize)`
    Reserve,
    /// `list.reserve_exact(n)` → `list.reserve_exact(n as usize)`
    ReserveExact,

    // ---- Internal/special methods ----
    /// `x.__slice__(start, end)` → `x[start..end]`
    Slice,
}

impl MethodKind {
    /// Try to resolve a method name to a known method kind.
    ///
    /// Returns `None` for unknown methods (which pass through as regular method calls).
    pub fn from_name(name: &str) -> Option<Self> {
        // Internal
        if incan_core::lang::magic_methods::from_str(name)
            == Some(incan_core::lang::magic_methods::MagicMethodId::Slice)
        {
            return Some(Self::Slice);
        }

        // List methods (includes a couple of generic collection methods we model explicitly).
        if let Some(id) = list_methods::from_str(name) {
            use list_methods::ListMethodId as L;
            return Some(match id {
                L::Append => Self::Append,
                L::Pop => Self::Pop,
                L::Swap => Self::Swap,
                L::Reserve => Self::Reserve,
                L::ReserveExact => Self::ReserveExact,
                L::Contains => Self::Contains,
                L::Remove => Self::Remove,
                // Not modeled as known IR methods yet:
                L::Count | L::Index => return None,
            });
        }

        // Dict methods.
        if let Some(id) = dict_methods::from_str(name) {
            use dict_methods::DictMethodId as D;
            return Some(match id {
                D::Get => Self::Get,
                D::Insert => Self::Insert,
                // keys/values are emitted as normal method calls.
                D::Keys | D::Values => return None,
            });
        }

        // Set methods.
        if set_methods::from_str(name).is_some() {
            return Some(Self::Contains);
        }

        // String methods.
        if let Some(id) = string_methods::from_str(name) {
            use string_methods::StringMethodId as S;
            return match id {
                S::Upper => Some(Self::Upper),
                S::Lower => Some(Self::Lower),
                S::Strip => Some(Self::Strip),
                S::Split => Some(Self::Split),
                S::Replace => Some(Self::Replace),
                S::Join => Some(Self::Join),
                S::StartsWith => Some(Self::StartsWith),
                S::EndsWith => Some(Self::EndsWith),
                S::Contains => Some(Self::Contains),
                // The rest are either typechecker-only (return types) or normal method calls:
                _ => None,
            };
        }

        None
    }
}
