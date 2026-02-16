//! Abstract Syntax Tree definitions for Incan
//!
//! This module defines all AST node types for the Incan language,
//! following the grammar defined in our RFCs.

use std::fmt;

/// Source location span (byte offsets)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// A node with source location
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

/// Identifier (interned string index in practice, String for simplicity here)
pub type Ident = String;

/// A program is a sequence of declarations, optionally with a `rust.module()` directive (RFC 023).
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub declarations: Vec<Spanned<Declaration>>,
    /// The `rust.module("path::to::module")` directive, if present.
    ///
    /// Declares that `@rust.extern` items in this module are backed by Rust functions at the given Rust module path.
    /// See RFC 023 for the full semantic design.
    pub rust_module_path: Option<Spanned<String>>,
}

/// Top-level declarations
#[derive(Debug, Clone, PartialEq)]
pub enum Declaration {
    Import(ImportDecl),
    Const(ConstDecl),
    Model(ModelDecl),
    Class(ClassDecl),
    Trait(TraitDecl),
    Newtype(NewtypeDecl),
    Enum(EnumDecl),
    Function(FunctionDecl),
    Docstring(String), // Module-level docstring
}

// ============================================================================
// Const bindings (module-level)
// ============================================================================

/// Visibility modifier for module-level items.
///
/// This is intentionally minimal for now; only `pub` is supported for consts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub visibility: Visibility,
    pub name: Ident,
    pub ty: Option<Spanned<Type>>,
    pub value: Spanned<Expr>,
}

// ============================================================================
// Imports
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub kind: ImportKind,
    pub alias: Option<Ident>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportKind {
    /// `import foo::bar::baz` or `import crate::config` - Rust-style module path
    Module(ImportPath),
    /// `from module import item1, item2` or `from ..utils import x` - Python-style multi-import
    From { module: ImportPath, items: Vec<ImportItem> },
    /// `import python "module"` - Python interop  FIXME: this doesn't actually work yet
    Python(String),
    /// `import rust::serde_json` - Rust crate import (direct crate usage)
    RustCrate {
        crate_name: String,
        /// Optional path within crate: `import rust::serde_json::Value`
        path: Vec<Ident>,
        /// Optional version requirement string (Cargo semver syntax).
        version: Option<String>,
        /// Optional feature list (only valid when `version` is provided).
        features: Vec<String>,
    },
    /// `from rust::time import Instant, Duration` - Rust crate with specific items
    RustFrom {
        crate_name: String,
        /// Optional path within crate before items: `from rust::std::collections import HashMap`
        path: Vec<Ident>,
        /// Optional version requirement string (Cargo semver syntax).
        version: Option<String>,
        /// Optional feature list (only valid when `version` is provided).
        features: Vec<String>,
        items: Vec<ImportItem>,
    },
}

/// A path in an import statement, supporting:
/// - Simple paths: `models`, `utils::helpers`
/// - Relative paths: `..common`, `super::utils`
/// - Absolute paths: `crate::config`
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPath {
    /// How many parent levels to go up (0 = current/absolute, 1 = parent, 2 = grandparent, etc.)
    pub parent_levels: usize,
    /// Whether this is an absolute path from project root (crate::...)
    pub is_absolute: bool,
    /// The path segments (module names)
    pub segments: Vec<Ident>,
}

impl ImportPath {
    pub fn simple(segments: Vec<Ident>) -> Self {
        Self {
            parent_levels: 0,
            is_absolute: false,
            segments,
        }
    }

    pub fn relative(parent_levels: usize, segments: Vec<Ident>) -> Self {
        Self {
            parent_levels,
            is_absolute: false,
            segments,
        }
    }

    pub fn absolute(segments: Vec<Ident>) -> Self {
        Self {
            parent_levels: 0,
            is_absolute: true,
            segments,
        }
    }

    /// Convert to Rust-style path string (using ::)
    pub fn to_rust_path(&self) -> String {
        let mut parts = Vec::new();

        if self.is_absolute {
            parts.push("crate".to_string());
        } else {
            for _ in 0..self.parent_levels {
                parts.push("super".to_string());
            }
        }

        parts.extend(self.segments.clone());
        parts.join("::")
    }
}

impl fmt::Display for ImportPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_rust_path())
    }
}

/// An item in a `from ... import` statement
#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    pub name: Ident,
    pub alias: Option<Ident>,
}

// ============================================================================
// Models (data containers with validation)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct ModelDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    // Traits adopted by this model via `with TraitA, TraitB`.
    pub traits: Vec<Spanned<Ident>>,
    pub fields: Vec<Spanned<FieldDecl>>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldMetadata {
    pub alias: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub visibility: Visibility,
    pub name: Ident,
    pub metadata: FieldMetadata,
    pub ty: Spanned<Type>,
    pub default: Option<Spanned<Expr>>,
}

// ============================================================================
// Classes (general-purpose types with inheritance and traits)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub extends: Option<Ident>,
    pub traits: Vec<Spanned<Ident>>,
    pub fields: Vec<Spanned<FieldDecl>>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

// ============================================================================
// Traits (behavior-only interfaces)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

// ============================================================================
// Newtypes (zero-cost wrappers with invariants)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct NewtypeDecl {
    pub visibility: Visibility,
    pub name: Ident,
    pub underlying: Spanned<Type>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

// ============================================================================
// Enums (algebraic data types)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub variants: Vec<Spanned<VariantDecl>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantDecl {
    pub name: Ident,
    pub fields: Vec<Spanned<Type>>,
}

// ============================================================================
// Functions and Methods
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub is_async: bool,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Spanned<Param>>,
    pub return_type: Spanned<Type>,
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub decorators: Vec<Spanned<Decorator>>,
    pub is_async: bool,
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Spanned<Param>>,
    pub return_type: Spanned<Type>,
    pub body: Option<Vec<Spanned<Statement>>>, // None for abstract methods (...)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// `self` - immutable receiver
    Immutable,
    /// `mut self` - mutable receiver
    Mutable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub is_mut: bool,
    pub name: Ident,
    pub ty: Spanned<Type>,
    pub default: Option<Spanned<Expr>>,
}

// ============================================================================
// Decorators
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct Decorator {
    pub path: ImportPath,
    pub name: Ident,
    pub args: Vec<DecoratorArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorArg {
    /// Positional argument
    Positional(Spanned<Expr>),
    /// Named argument: `name: Type` or `name = value`
    Named(Ident, DecoratorArgValue),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorArgValue {
    Type(Spanned<Type>),
    Expr(Spanned<Expr>),
}

// ============================================================================
// Type Parameters and Trait Bounds (RFC 023)
// ============================================================================

/// A type parameter declaration with optional trait bounds.
///
/// RFC 023: Supports the `[T with (Eq, Debug)]` syntax.
///
/// ## Examples
/// - `T` — bare type parameter (no bounds)
/// - `T with Clone` — single bound
/// - `T with (Eq, Debug)` — multiple bounds
#[derive(Debug, Clone, PartialEq)]
pub struct TypeParam {
    pub name: Ident,
    pub bounds: Vec<TraitBound>,
}

impl TypeParam {
    /// Create a type parameter with no bounds (most common case).
    pub fn bare(name: Ident) -> Self {
        Self {
            name,
            bounds: Vec::new(),
        }
    }
}

/// A trait bound in a type parameter's `with` clause.
///
/// RFC 023: Maps to Rust trait bounds during emission.
///
/// ## Examples
/// - `Eq` — simple bound
/// - `From[U]` — bound with type arguments
#[derive(Debug, Clone, PartialEq)]
pub struct TraitBound {
    pub name: Ident,
    pub type_args: Vec<Spanned<Type>>,
}

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Simple type: `int`, `str`, `MyType`
    Simple(Ident),
    /// Generic type: `List[T]`, `Result[T, E]`
    Generic(Ident, Vec<Spanned<Type>>),
    /// Function type: `(int, str) -> bool`
    Function(Vec<Spanned<Type>>, Box<Spanned<Type>>),
    /// Unit type
    Unit,
    /// Tuple type: `(int, str)`
    Tuple(Vec<Spanned<Type>>),
    /// Self type - refers to the implementing type in traits
    SelfType,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Simple(name) => write!(f, "{}", name),
            Type::Generic(name, args) => {
                write!(f, "{}[", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg.node)?;
                }
                write!(f, "]")
            }
            Type::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p.node)?;
                }
                write!(f, ") -> {}", ret.node)
            }
            Type::Unit => write!(f, "Unit"),
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e.node)?;
                }
                write!(f, ")")
            }
            Type::SelfType => write!(f, "Self"),
        }
    }
}

// ============================================================================
// Statements
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// `x = value` or `let x = value` or `mut x = value`
    Assignment(AssignmentStmt),
    /// `obj.field = value` or `self.field = value`
    FieldAssignment(FieldAssignmentStmt),
    /// `obj[index] = value`
    IndexAssignment(IndexAssignmentStmt),
    /// `return expr`
    Return(Option<Spanned<Expr>>),
    /// `if expr: ... [else: ...]`
    If(IfStmt),
    /// `while expr: ...`
    While(WhileStmt),
    /// `for x in expr: ...`
    For(ForStmt),
    /// `assert cond` or `assert cond, msg`
    Assert(AssertStmt),
    /// Expression statement
    Expr(Spanned<Expr>),
    /// `pass` or `...`
    Pass,
    /// `break` - exit the innermost loop
    Break,
    /// `continue` - skip to next iteration
    Continue,
    /// Compound assignment: `x += value`, `x -= value`, etc.
    CompoundAssignment(CompoundAssignmentStmt),
    /// Tuple unpacking: `a, b = expr` or `let a, b = expr`
    TupleUnpack(TupleUnpackStmt),
    /// Tuple assignment to lvalues: `arr[i], arr[j] = arr[j], arr[i]`
    TupleAssign(TupleAssignStmt),
    /// Chained assignment: `x = y = z = value`
    ChainedAssignment(ChainedAssignmentStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssignmentStmt {
    pub binding: BindingKind,
    pub name: Ident,
    pub ty: Option<Spanned<Type>>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldAssignmentStmt {
    /// Span of the assignment target (e.g. `self.field`).
    pub target_span: Span,
    pub object: Spanned<Expr>,
    pub field: Ident,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexAssignmentStmt {
    pub object: Spanned<Expr>,
    pub index: Spanned<Expr>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// Plain `x = value` - first assignment (becomes `let` in Rust)
    Inferred,
    /// `let x = value`
    Let,
    /// `mut x = value`
    Mutable,
    /// Reassignment to existing mutable variable (no `let` in Rust)
    Reassign,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompoundAssignmentStmt {
    pub name: Ident,
    pub op: CompoundOp,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainedAssignmentStmt {
    pub binding: BindingKind,
    pub targets: Vec<Ident>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TupleUnpackStmt {
    pub binding: BindingKind,
    pub names: Vec<Ident>,
    pub value: Spanned<Expr>,
}

/// Tuple assignment to lvalue expressions: `arr[i], arr[j] = arr[j], arr[i]`
/// Used for swaps and multi-target assignments where targets are not new bindings.
#[derive(Debug, Clone, PartialEq)]
pub struct TupleAssignStmt {
    /// Left-hand side lvalue expressions (index, field, or identifier references)
    pub targets: Vec<Spanned<Expr>>,
    /// Right-hand side expression (typically a tuple)
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    Add,      // +=
    Sub,      // -=
    Mul,      // *=
    Div,      // /=
    FloorDiv, // //=
    Mod,      // %=
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    pub condition: Spanned<Expr>,
    pub then_body: Vec<Spanned<Statement>>,
    pub elif_branches: Vec<(Spanned<Expr>, Vec<Spanned<Statement>>)>,
    pub else_body: Option<Vec<Spanned<Statement>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileStmt {
    pub condition: Spanned<Expr>,
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForStmt {
    pub var: Ident,
    pub iter: Spanned<Expr>,
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertStmt {
    pub condition: Spanned<Expr>,
    pub message: Option<Spanned<Expr>>,
}

// ============================================================================
// Expressions
// ============================================================================

/// Slice expression: represents `start:end` or `start:end:step`
/// All components are optional, e.g., `[:5]`, `[2:]`, `[::2]`
#[derive(Debug, Clone, PartialEq)]
pub struct SliceExpr {
    pub start: Option<Box<Spanned<Expr>>>,
    pub end: Option<Box<Spanned<Expr>>>,
    pub step: Option<Box<Spanned<Expr>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Identifier
    Ident(Ident),
    /// Literal
    Literal(Literal),
    /// `self`
    SelfExpr,
    /// Binary operation: `a + b`
    Binary(Box<Spanned<Expr>>, BinaryOp, Box<Spanned<Expr>>),
    /// Unary operation: `-x`, `not x`
    Unary(UnaryOp, Box<Spanned<Expr>>),
    /// Function/method call: `f(a, b)`
    Call(Box<Spanned<Expr>>, Vec<CallArg>),
    /// Index: `x[i]`
    Index(Box<Spanned<Expr>>, Box<Spanned<Expr>>),
    /// Slice: `x[start:end]` or `x[start:end:step]`
    Slice(Box<Spanned<Expr>>, SliceExpr),
    /// Field access: `x.field`
    Field(Box<Spanned<Expr>>, Ident),
    /// Method call: `x.method(args)`
    MethodCall(Box<Spanned<Expr>>, Ident, Vec<CallArg>),
    /// `await expr`
    Await(Box<Spanned<Expr>>),
    /// `expr?` (try/propagate)
    Try(Box<Spanned<Expr>>),
    /// Match expression
    Match(Box<Spanned<Expr>>, Vec<Spanned<MatchArm>>),
    /// If expression
    If(Box<IfExpr>),
    /// List comprehension: `[expr for x in iter if cond]`
    ListComp(Box<ListComp>),
    /// Dict comprehension: `{k: v for x in iter if cond}`
    DictComp(Box<DictComp>),
    /// Closure: `(x, y) => expr` (a lot like python's lambda)
    Closure(Vec<Spanned<Param>>, Box<Spanned<Expr>>),
    /// Tuple: `(a, b)`
    Tuple(Vec<Spanned<Expr>>),
    /// List literal: `[a, b, c]`
    List(Vec<Spanned<Expr>>),
    /// Dict literal: `{k: v, ...}`
    Dict(Vec<(Spanned<Expr>, Spanned<Expr>)>),
    /// Set literal: `{a, b, c}`
    Set(Vec<Spanned<Expr>>),
    /// Parenthesized expression
    Paren(Box<Spanned<Expr>>),
    /// Type constructor: `Some(x)`, `Ok(x)`, `User(id=1, name="Ada")`
    Constructor(Ident, Vec<CallArg>),
    /// f-string: `f"Hello {name}"`
    FString(Vec<FStringPart>),
    /// `yield expr` (for fixtures/generators)
    Yield(Option<Box<Spanned<Expr>>>),
    /// Range expression: `start..end` (exclusive) or `start..=end` (inclusive)
    Range {
        start: Box<Spanned<Expr>>,
        end: Box<Spanned<Expr>>,
        inclusive: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Expr(Spanned<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv, // // (Python-style floor division)
    Mod,
    Pow,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    In,
    NotIn,
    Is,
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::FloorDiv => write!(f, "//"),
            BinaryOp::Mod => write!(f, "%"),
            BinaryOp::Pow => write!(f, "**"),
            BinaryOp::Eq => write!(f, "=="),
            BinaryOp::NotEq => write!(f, "!="),
            BinaryOp::Lt => write!(f, "<"),
            BinaryOp::Gt => write!(f, ">"),
            BinaryOp::LtEq => write!(f, "<="),
            BinaryOp::GtEq => write!(f, ">="),
            BinaryOp::And => write!(f, "and"),
            BinaryOp::Or => write!(f, "or"),
            BinaryOp::In => write!(f, "in"),
            BinaryOp::NotIn => write!(f, "not in"),
            BinaryOp::Is => write!(f, "is"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    /// Positional argument
    Positional(Spanned<Expr>),
    /// Named argument: `name=value`
    Named(Ident, Spanned<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternArg {
    /// Positional pattern: `Type(x)`
    Positional(Spanned<Pattern>),
    /// Named pattern: `Type(name=pat)`
    Named(Ident, Spanned<Pattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    pub guard: Option<Spanned<Expr>>, // `if condition` guard
    pub body: MatchBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchBody {
    /// `=> expr` (single expression)
    Expr(Spanned<Expr>),
    /// Block of statements
    Block(Vec<Spanned<Statement>>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Wildcard: `_`
    Wildcard,
    /// Binding: `x`
    Binding(Ident),
    /// Literal: `42`, `"hello"`, `true`
    Literal(Literal),
    /// Constructor: `Some(x)`, `Ok(value)`, `Type(name=pat)`
    Constructor(Ident, Vec<PatternArg>),
    /// Tuple: `(a, b)`
    Tuple(Vec<Spanned<Pattern>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfExpr {
    pub condition: Spanned<Expr>,
    pub then_body: Vec<Spanned<Statement>>,
    pub else_body: Option<Vec<Spanned<Statement>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListComp {
    pub expr: Spanned<Expr>,
    pub var: Ident,
    pub iter: Spanned<Expr>,
    pub filter: Option<Spanned<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DictComp {
    pub key: Spanned<Expr>,
    pub value: Spanned<Expr>,
    pub var: Ident,
    pub iter: Spanned<Expr>,
    pub filter: Option<Spanned<Expr>>,
}

// ============================================================================
// Visitor trait for AST traversal
// ============================================================================

pub trait Visitor {
    fn visit_program(&mut self, program: &Program) {
        for decl in &program.declarations {
            self.visit_declaration(decl);
        }
    }

    fn visit_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(i) => self.visit_import(i),
            Declaration::Const(c) => self.visit_const(c),
            Declaration::Model(m) => self.visit_model(m),
            Declaration::Class(c) => self.visit_class(c),
            Declaration::Trait(t) => self.visit_trait(t),
            Declaration::Newtype(n) => self.visit_newtype(n),
            Declaration::Enum(e) => self.visit_enum(e),
            Declaration::Function(f) => self.visit_function(f),
            Declaration::Docstring(d) => self.visit_docstring(d),
        }
    }

    fn visit_import(&mut self, _import: &ImportDecl) {}
    fn visit_const(&mut self, _const_decl: &ConstDecl) {}
    fn visit_docstring(&mut self, _doc: &str) {}
    fn visit_model(&mut self, _model: &ModelDecl) {}
    fn visit_class(&mut self, _class: &ClassDecl) {}
    fn visit_trait(&mut self, _trait: &TraitDecl) {}
    fn visit_newtype(&mut self, _newtype: &NewtypeDecl) {}
    fn visit_enum(&mut self, _enum: &EnumDecl) {}
    fn visit_function(&mut self, _func: &FunctionDecl) {}
    fn visit_statement(&mut self, _stmt: &Spanned<Statement>) {}
    fn visit_expr(&mut self, _expr: &Spanned<Expr>) {}
    fn visit_type(&mut self, _ty: &Spanned<Type>) {}
    fn visit_pattern(&mut self, _pat: &Spanned<Pattern>) {}
}
