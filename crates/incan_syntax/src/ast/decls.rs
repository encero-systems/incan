//! Declaration AST types: models, classes, traits, newtypes, enums, functions, methods, decorators, type parameters,
//! and trait bounds.

use incan_core::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;

use super::{Expr, Ident, ImportPath, IntLiteral, Spanned, Statement, Type, Visibility};

// ============================================================================
// Models (data containers with validation)
// ============================================================================

/// A model declaration: a named data container with optional fields, methods, and trait adoption.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    // Traits adopted by this model via `with TraitA, TraitB[T]`.
    pub traits: Vec<Spanned<TraitBound>>,
    /// Docstring at the start of the model body (surface `"""..."""`), when present.
    pub docstring: Option<String>,
    pub fields: Vec<Spanned<FieldDecl>>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

/// Optional metadata on a model/class field (alias, description).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldMetadata {
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// A field declaration within a model or class.
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

/// A class declaration: a named type with optional inheritance, trait adoption, fields, and methods.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub extends: Option<Ident>,
    pub traits: Vec<Spanned<TraitBound>>,
    /// Docstring at the start of the class body (surface `"""..."""`), when present.
    pub docstring: Option<String>,
    pub fields: Vec<Spanned<FieldDecl>>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

// ============================================================================
// Traits (behavior-only interfaces)
// ============================================================================

/// A trait declaration: a behavior-only interface with abstract and default methods.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    /// Supertraits adopted via `with TraitA, TraitB[T]` (RFC 042).
    pub traits: Vec<Spanned<TraitBound>>,
    /// Docstring at the start of the trait body (surface `"""..."""`), when present.
    pub docstring: Option<String>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

// ============================================================================
// Type aliases (transparent, documentation-bearing wrappers)
// ============================================================================

/// A type alias declaration: `pub type Query[T] = AxumQuery[T]`.
///
/// Compiles to a Rust `type` alias — no extra struct, no extra wrapping layer.
/// Useful for re-exporting external types under an Incan name while retaining full docstrings and IDE support.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasDecl {
    pub visibility: Visibility,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub target: Spanned<Type>,
}

// ============================================================================
// Inline Test Modules (RFC 018)
// ============================================================================

/// Reserved inline test-only scope introduced by `module tests:`.
#[derive(Debug, Clone, PartialEq)]
pub struct TestModuleDecl {
    /// The reserved module name. RFC 018 only permits `tests`.
    pub name: Ident,
    /// Top-level declarations scoped inside the inline test module.
    pub body: Vec<Spanned<super::Declaration>>,
}

// ============================================================================
// Newtypes (zero-cost wrappers with invariants)
// ============================================================================

/// A newtype declaration: a zero-cost wrapper with optional methods and invariants.
#[derive(Debug, Clone, PartialEq)]
pub struct NewtypeDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    /// `true` when declared as `type X = rusttype Y`, RFC 041.
    pub is_rusttype: bool,
    pub underlying: Spanned<Type>,
    pub docstring: Option<String>,
    /// Alias-style member rebinding entries inside a newtype/rusttype body.
    pub rebindings: Vec<Spanned<RebindingDecl>>,
    /// Optional `interop:` conversion edges (RFC 041).
    pub interop_edges: Vec<Spanned<InteropEdgeDecl>>,
    pub methods: Vec<Spanned<MethodDecl>>,
}

/// A short or qualified member rebinding declaration in a newtype/rusttype body.
#[derive(Debug, Clone, PartialEq)]
pub struct RebindingDecl {
    pub name: Ident,
    pub target: Spanned<Expr>,
}

/// Direction of a `interop:` edge declaration (RFC 041).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteropDirection {
    From,
    Into,
}

/// Adapter mode for an interop edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteropAdapterKind {
    Via,
    Try,
}

/// A single line in a `interop:` block.
#[derive(Debug, Clone, PartialEq)]
pub struct InteropEdgeDecl {
    pub direction: InteropDirection,
    pub ty: Spanned<Type>,
    pub adapter_kind: InteropAdapterKind,
    pub adapter: Spanned<Expr>,
}

// ============================================================================
// Enums (algebraic data types)
// ============================================================================

/// An enum declaration: an algebraic data type with variants, optional methods, and trait adoption.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    /// Backing value type for RFC 032 value enums (`enum Name(str):` / `enum Name(int):`).
    pub value_type: Option<Spanned<ValueEnumType>>,
    /// Traits adopted by this enum via `with TraitA, TraitB[T]`.
    pub traits: Vec<Spanned<TraitBound>>,
    /// Docstring at the start of the enum body (surface `"""..."""`), when present.
    pub docstring: Option<String>,
    /// Variants declared before any enum methods.
    pub variants: Vec<Spanned<VariantDecl>>,
    /// Methods and associated functions declared after the variant list.
    pub methods: Vec<Spanned<MethodDecl>>,
}

/// The allowed backing literal types for a value enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueEnumType {
    Str,
    Int,
}

/// A literal assigned to a value enum variant.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueEnumLiteral {
    Str(String),
    Int(IntLiteral),
}

/// A single variant of an enum declaration, with optional tuple fields.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDecl {
    pub name: Ident,
    pub fields: Vec<Spanned<Type>>,
    pub value: Option<Spanned<ValueEnumLiteral>>,
}

// ============================================================================
// Functions and Methods
// ============================================================================

/// A top-level function declaration (`def`).
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub visibility: Visibility,
    pub decorators: Vec<Spanned<Decorator>>,
    pub surface_modifiers: Vec<SurfaceModifier>,
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Spanned<Param>>,
    pub return_type: Spanned<Type>,
    pub body: Vec<Spanned<Statement>>,
}

impl FunctionDecl {
    /// Returns `true` if this function has the given surface modifier.
    pub fn has_surface_modifier(&self, key: &SurfaceFeatureKey) -> bool {
        self.surface_modifiers.iter().any(|m| m.key == *key)
    }

    /// Returns `true` if this function was declared with the `async` soft keyword.
    pub fn is_async(&self) -> bool {
        self.has_surface_modifier(&SurfaceFeatureKey::SoftKeyword(KeywordId::Async))
    }
}

/// A method declaration within a model, class, or trait.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub decorators: Vec<Spanned<Decorator>>,
    pub surface_modifiers: Vec<SurfaceModifier>,
    pub name: Ident,
    /// Method-scoped generic parameters declared after the method name.
    pub type_params: Vec<TypeParam>,
    pub receiver: Option<Receiver>,
    pub params: Vec<Spanned<Param>>,
    pub return_type: Spanned<Type>,
    pub body: Option<Vec<Spanned<Statement>>>, // None for abstract methods (...)
}

impl MethodDecl {
    /// Returns `true` if this method has the given surface modifier.
    pub fn has_surface_modifier(&self, key: &SurfaceFeatureKey) -> bool {
        self.surface_modifiers.iter().any(|m| m.key == *key)
    }

    /// Returns `true` if this method was declared with the `async` soft keyword.
    pub fn is_async(&self) -> bool {
        self.has_surface_modifier(&SurfaceFeatureKey::SoftKeyword(KeywordId::Async))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// `self` - immutable receiver
    Immutable,
    /// `mut self` - mutable receiver
    Mutable,
}

/// Generic declaration-level surface modifier (e.g., soft keyword prefix before `def`).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceModifier {
    pub key: SurfaceFeatureKey,
}

/// A function or method parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub is_mut: bool,
    pub kind: ParamKind,
    pub name: Ident,
    pub ty: Spanned<Type>,
    pub default: Option<Spanned<Expr>>,
}

/// Surface parameter kind.
///
/// Rest parameters use the annotated element/value type at the source level, but downstream semantic layers bind the
/// local variable as the explicit container type described by RFC 038.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Ordinary fixed-arity parameter: `name: T`.
    Normal,
    /// Positional rest capture: `*name: T`, bound inside the function as `List[T]`.
    RestPositional,
    /// Keyword rest capture: `**name: T`, bound inside the function as `Dict[str, T]`.
    RestKeyword,
}

// ============================================================================
// Decorators
// ============================================================================

/// A decorator annotation (`@name(args)`).
#[derive(Debug, Clone, PartialEq)]
pub struct Decorator {
    pub path: ImportPath,
    pub name: Ident,
    pub args: Vec<DecoratorArg>,
}

/// A single argument to a decorator.
#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorArg {
    /// Positional argument
    Positional(Spanned<Expr>),
    /// Named argument: `name: Type` or `name = value`
    Named(Ident, DecoratorArgValue),
}

/// The value part of a named decorator argument (either a type or an expression).
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
