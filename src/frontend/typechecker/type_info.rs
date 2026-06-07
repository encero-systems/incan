//! Lowering-facing typechecker artifact snapshots.
//!
//! This module contains the reusable semantic metadata that later compiler stages consume after typechecking. It keeps
//! the cross-phase snapshot surface separate from the main [`TypeChecker`](super::TypeChecker) orchestration state.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::{Expr, ParamKind, Span, Spanned};
use crate::frontend::symbols::{
    CallableParam, FunctionOverloadInfo, NewtypePrimitiveConstraint, ResolvedType, TypeBoundInfo,
};
use crate::frontend::testing_markers::TestingFixtureScope;
use incan_core::interop::{CoercionPolicy, RustFunctionSig};

use super::{ConstValue, const_eval};

/// Capture reusable typechecking output for later compiler stages.
///
/// This struct is the bridge that lets backend lowering/codegen consume the typechecker’s view of the program rather
/// than re-deriving types and semantics from the AST. The bridge is intentionally grouped by consumer contract: each
/// field names a semantic artifact family instead of exposing one flat collection of unrelated side channels.
///
/// ## Notes
/// - Expression types are keyed by `(span.start, span.end)` so downstream code can look them up without holding AST
///   node identities.
/// - Const classification is recorded to support RFC 008 “Rust-native vs Frozen” const emission.
///
/// ## Examples
/// ```ignore
/// use incan::frontend::{lexer, parser, typechecker};
///
/// let tokens = lexer::lex("def foo() -> int: return 1")?;
/// let ast = parser::parse(&tokens)?;
/// let mut tc = typechecker::TypeChecker::new();
/// tc.check_program(&ast)?;
/// let info = tc.type_info();
/// // info.expr_type(...) can now be queried by spans.
/// ```
#[derive(Debug, Default, Clone)]
pub struct TypeCheckInfo {
    /// Trait hierarchy metadata consumed by trait impl and default-method lowering.
    pub traits: TraitArtifacts,
    /// Derive expansion metadata imported from dependency modules and manifests.
    pub derivations: DerivationArtifacts,
    /// Expression-local resolution facts keyed by source spans.
    pub expressions: ExpressionArtifacts,
    /// Const evaluation facts needed by runtime and emission boundaries.
    pub consts: ConstArtifacts,
    /// Rust interop decisions that must be preserved exactly across lowering.
    pub rust: RustInteropArtifacts,
    /// Declaration-level binding rewrites and visibility facts consumed by lowering.
    pub declarations: DeclarationArtifacts,
    /// Call-site semantic decisions selected by the typechecker.
    pub calls: CallArtifacts,
    /// Test-runner and fixture metadata extracted during typechecking.
    pub testing: TestingArtifacts,
    /// Custom protocol decisions that lower into explicit runtime calls.
    pub protocols: ProtocolArtifacts,
}

/// Trait hierarchy metadata consumed by trait impl and default-method lowering.
#[derive(Debug, Default, Clone)]
pub struct TraitArtifacts {
    /// RFC 042: Direct supertraits per trait name, copied from
    /// [`TraitInfo::supertraits`](crate::frontend::symbols::TraitInfo::supertraits) for IR lowering.
    ///
    /// Lowering does not retain the typechecker symbol table; this snapshot supplies resolved supertrait type
    /// arguments after a successful check.
    pub direct_supertraits: HashMap<String, Vec<(String, Vec<ResolvedType>)>>,
    /// RFC 042: Trait type parameter names keyed by trait name for lowering-time generic substitution.
    ///
    /// Includes locally-declared and imported traits so backend lowering can handle cross-module trait hierarchies
    /// without relying on local AST declarations.
    pub type_params: HashMap<String, Vec<String>>,
}

/// Derive expansion metadata imported from dependency modules and manifests.
#[derive(Debug, Default, Clone)]
pub struct DerivationArtifacts {
    /// RFC 024: Imported derivable modules keyed by source module path, such as `yaml` or `formats.yaml`.
    ///
    /// Values are the trait names listed in the module's `__derives__` metadata. Lowering consumes this so
    /// user-authored derivable modules participate in the same derive expansion path as stdlib modules.
    pub derivable_modules: HashMap<String, Vec<String>>,
    /// RFC 024: Trait-level Rust derive paths keyed by module-qualified trait name, such as `yaml.Serialize`.
    ///
    /// The typechecker owns this because dependency modules are already imported and validated there. Lowering should
    /// not re-run module resolution or assume RFC 024 metadata only exists in stdlib source.
    pub trait_rust_derive_paths: HashMap<String, Vec<String>>,
}

/// Expression-local resolution facts keyed by source spans.
#[derive(Debug, Default, Clone)]
pub struct ExpressionArtifacts {
    /// Map from expression span (start,end) -> resolved type.
    pub expr_types: HashMap<(usize, usize), ResolvedType>,
    /// Type names that implement `Awaitable[T]` by delegating to one concrete awaitable field.
    ///
    /// Lowering consumes this so `await wrapper` and `race for` arms can emit `wrapper.<field>.await` instead of
    /// trying to await the wrapper struct itself.
    pub awaitable_delegation_fields: HashMap<String, String>,
    /// RFC 046 computed property reads keyed by the full field-access expression span.
    ///
    /// Lowering/emission can use this to distinguish `obj.field` storage reads from `obj.property` getter calls while
    /// still consuming the same resolved expression type map for the property return type.
    pub computed_property_accesses: HashMap<(usize, usize), ComputedPropertyAccessInfo>,
    /// Map from identifier expression span (start,end) -> how it resolved (value vs type vs module).
    ///
    /// This exists so downstream stages (IR lowering/codegen) can reliably distinguish:
    /// - `x.method(...)` where `x` is a value binding, from
    /// - `Type.method(...)` where `Type` is a type name (emits `Type::method(...)` in Rust), and
    /// - imported placeholders (e.g. `from rust::... import Foo`) which are not value bindings.
    pub ident_kinds: HashMap<(usize, usize), IdentKind>,
    /// Identifier spans that resolved to the compiler-provided ambient `std.logging` logger binding.
    ///
    /// The binding is typechecked like an ordinary immutable `Logger` value, but lowering must materialize it as a
    /// module-local `std.logging.get_logger(...)` call so source metadata can become the logger name.
    pub ambient_logger_bindings: HashSet<(usize, usize)>,
    /// RFC 017 validated-newtype coercion decisions keyed by source expression span.
    ///
    /// Lowering consumes these decisions when an expression is used at an approved implicit-coercion site, such as a
    /// function argument, typed initializer, or model/class field initializer.
    pub validated_newtype_coercions: HashMap<(usize, usize), ValidatedNewtypeCoercionInfo>,
    /// Source-level codegraph targets proven during expression checking, keyed by call or reference expression span.
    ///
    /// The codegraph exporter consumes this instead of re-resolving names from syntax. Absence means the target is
    /// unsupported, ambiguous, degraded, or outside the current conservative source target set.
    pub source_targets: HashMap<(usize, usize), SourceTargetInfo>,
}

/// Const evaluation facts needed by runtime and emission boundaries.
#[derive(Debug, Default, Clone)]
pub struct ConstArtifacts {
    /// Const category classification (RFC 008): const name -> kind.
    pub const_kinds: HashMap<String, const_eval::ConstKind>,
    /// Computed const values (when available), keyed by const name.
    pub const_values: HashMap<String, ConstValue>,
}

/// Rust interop decisions that must be preserved exactly across lowering.
#[derive(Debug, Default, Clone)]
pub struct RustInteropArtifacts {
    /// `rusttype` Incan name → canonical Rust path string (`substrait::proto::type::Binary`), when the checker
    /// resolved the underlying type to [`ResolvedType::RustPath`]. Used by lowering so `m::T` spellings emit full
    /// paths without re-running import resolution.
    pub rusttype_canonical_paths: HashMap<String, String>,
    /// Rust-boundary coercion decisions keyed by argument expression span.
    pub arg_coercions: HashMap<(usize, usize), RustArgCoercionInfo>,
    /// Rust trait imports keyed by the source binding name with the trait path and method names they can place in
    /// scope.
    ///
    /// Lowering carries this into IR import items so codegen can retain extension-trait imports when Rust method
    /// lookup needs the trait in scope even though emitted call tokens do not otherwise mention the trait name.
    pub trait_imports: HashMap<String, RustTraitImportInfo>,
    /// Rust extension-trait import selected for one Rust method call.
    ///
    /// Keyed by the full method-call expression span. Lowering attaches the binding to the corresponding IR method
    /// call so generated-use analysis can retain the exact import instead of retaining every trait with the same
    /// method name.
    pub method_trait_import_uses: HashMap<(usize, usize), RustMethodTraitImportUse>,
    /// Body-less rusttype Rust-trait adoptions proven by metadata and therefore satisfied by the backing type alias.
    ///
    /// Lowering must not emit an `impl Trait for Alias` for these entries because Rust coherence treats the alias as
    /// the foreign backing type. The typechecker records only non-generic trait paths that metadata proved.
    pub rusttype_forwarded_trait_adoptions: HashSet<(String, String)>,
    /// Rust-boundary coercion decisions for method return values, keyed by the call expression span.
    ///
    /// Populated when metadata shows a `rusttype` method's actual Rust return type requires coercion to the
    /// Incan-declared type (e.g. `&str` → `String` for a method declared `-> str`).
    pub return_coercions: HashMap<(usize, usize), RustArgCoercionInfo>,
    /// Regular method calls whose arguments must keep Rust method-call lookup shape.
    ///
    /// Keyed by `(receiver_span.start, receiver_span.end, method_name)` so lowering can preserve borrow-sensitive
    /// lookup calls like `HashMap.get(key)` without re-querying rust-inspect metadata in the backend.
    pub regular_method_arg_shape_preserving_calls: HashSet<(usize, usize, String)>,
    /// Imported Rust named-field struct constructor calls keyed by full call-expression span.
    ///
    /// The frontend resolves positional source arguments against rust-inspect field metadata. Lowering consumes the
    /// resolved field names so `Range(1, 3)` can emit `Range { start: 1, end: 3 }` instead of an invalid tuple-style
    /// Rust constructor.
    pub named_field_constructor_fields: HashMap<(usize, usize), Vec<String>>,
    /// Imported Rust field accesses keyed by full field-expression span.
    ///
    /// The parser may use an Incan-safe source spelling such as `type_` for a Rust field whose metadata name is the
    /// Rust keyword `type`. Lowering consumes this resolved Rust field name so emission can use the real Rust field
    /// identifier rather than guessing from source text.
    pub field_access_names: HashMap<(usize, usize), String>,
    /// Rust closure parameter displays keyed by closure-expression span.
    ///
    /// This is populated when contextual Rust metadata proves a closure is being used as a Rust callable boundary
    /// whose parameter shape cannot be faithfully represented by ordinary Incan surface types, such as `&[T]`.
    /// Lowering/emission consumes the displays directly so generated closures keep Rust inference stable.
    pub closure_param_type_displays: HashMap<(usize, usize), Vec<String>>,
}

/// Declaration-level binding rewrites and visibility facts consumed by lowering.
#[derive(Debug, Default, Clone)]
pub struct DeclarationArtifacts {
    /// Module-local function declarations keyed by source name after annotation resolution.
    ///
    /// Lowering consumes this instead of re-lowering raw AST annotations so aliases such as
    /// `type Expr = Union[...]` do not produce a different callable surface from typechecked call sites.
    pub function_bindings: HashMap<String, FunctionBindingInfo>,
    /// Module-local function declarations keyed by declaration span, preserving same-name overloads.
    pub function_bindings_by_span: HashMap<(usize, usize), FunctionBindingInfo>,
    /// Function declaration emitted names keyed by source declaration span.
    ///
    /// Present when top-level overloads need Rust-level name disambiguation while preserving one source name.
    pub function_emitted_names: HashMap<(usize, usize), String>,
    /// Overload candidates keyed by the source binding name visible in the current module.
    ///
    /// This includes declarations, imports, and aliases so call resolution, export metadata, and lowering all see one
    /// overload surface instead of rebuilding overload sets from syntax-specific paths.
    pub function_overloads: HashMap<String, Vec<FunctionOverloadInfo>>,
    /// Imported overload bindings keyed by local import name.
    ///
    /// Each value is the concrete Rust function name exported by the provider module for one overload candidate. IR
    /// import lowering consumes this so source-level overload names do not get re-exported as nonexistent Rust items.
    pub imported_function_emitted_names: HashMap<String, Vec<String>>,
    /// Module-visible partial projections keyed by their source binding name.
    ///
    /// Constructor partials can be materialized in const contexts without calling the generated wrapper. Keeping the
    /// target and preset expressions here lets const-eval and lowering consume one resolved projection surface.
    pub partial_projections: HashMap<String, PartialProjectionInfo>,
    /// Module-visible static bindings keyed by local name for lowering/runtime emission.
    pub static_bindings: HashMap<String, StaticBindingInfo>,
    /// Same-type method aliases keyed by nominal type name (`alias -> target_method`).
    ///
    /// This includes imported type metadata so lowering can rewrite calls through aliases such as
    /// `Path.__truediv__` or `OrdinalMap.nbytes` even when the alias was declared in stdlib or a dependency module
    /// rather than the current source file.
    pub type_method_rebindings: HashMap<String, HashMap<String, String>>,
    /// RFC 036: Module-visible function names whose declaration was rebound through a user-defined decorator chain.
    pub decorated_function_bindings: HashMap<String, DecoratedFunctionBindingInfo>,
    /// RFC 036: Decorated function bindings keyed by declaration span, preserving same-name overloads.
    pub decorated_function_bindings_by_span: HashMap<(usize, usize), DecoratedFunctionBindingInfo>,
    /// RFC 036: Method names whose declaration was rebound through a user-defined decorator chain.
    pub decorated_method_bindings: HashMap<(String, String), DecoratedMethodBindingInfo>,
}

/// Source-level partial projection metadata preserved after collection.
#[derive(Debug, Clone)]
pub struct PartialProjectionInfo {
    /// Source name of the partial declaration visible in the current module.
    pub name: String,
    /// Resolved source path to the projected target.
    pub target_path: Vec<String>,
    /// Semantic kind of the projected target when collection could classify it.
    pub target_kind: PartialProjectionTargetKind,
    /// Preset keyword expressions supplied by the partial declaration.
    pub presets: Vec<PartialProjectionPreset>,
}

/// One preset keyword/value pair from a partial declaration.
#[derive(Debug, Clone)]
pub struct PartialProjectionPreset {
    pub name: String,
    pub value: Spanned<Expr>,
}

/// Target kinds that matter to downstream partial projection consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialProjectionTargetKind {
    Function,
    ModelConstructor,
    ClassConstructor,
    NewtypeConstructor,
    Unknown,
}

/// Call-site semantic decisions selected by the typechecker.
#[derive(Debug, Default, Clone)]
pub struct CallArtifacts {
    /// RFC 038: unpack operands whose static shape has been proven by call binding.
    ///
    /// Lowering consumes these plans to rewrite fixed/static unpack operands into ordinary IR call arguments. This
    /// keeps backend emission from re-deriving the frontend's binding decision from raw IR shape.
    pub fixed_unpack_plans: HashMap<(usize, usize), FixedUnpackPlan>,
    /// RFC 054: For call expressions that used explicit bracketed type arguments, maps the **full call expression
    /// span** `(start, end)` to the final monomorphized type arguments in callee type-parameter order.
    ///
    /// Populated only after a successful generic function or method check when `[...]` was present; lowering prefers
    /// this over re-lowering AST type nodes so `_` placeholders never reach codegen as `IrType::Unknown`.
    ///
    /// ## Span stability
    ///
    /// Keys use the same `(start, end)` byte range the typechecker records for the call/`MethodCall` expression and
    /// that [`AstLowering::lower_expr`](crate::backend::ir::lower::AstLowering::lower_expr) receives as `expr_span`
    /// for those nodes, so lookup stays consistent across phases without holding AST node identities.
    pub call_site_monomorph_type_args: HashMap<(usize, usize), Vec<ResolvedType>>,
    /// RFC 038: Rest-aware callable signatures keyed by full call expression span.
    ///
    /// Function-value calls can recover this from the callee expression type, but method calls need a snapshot because
    /// lowering does not retain the frontend method table.
    pub call_site_callable_params: HashMap<(usize, usize), Vec<CallableParam>>,
    /// RFC 028: User-defined operator dispatch resolved by the typechecker.
    ///
    /// Lowering consumes this map so `a + b`, `-a`, and `a[b]` can become direct dunder method calls without
    /// re-running backend-side infix/index semantics. Primitive operators are intentionally absent from this map.
    pub resolved_operator_calls: HashMap<(usize, usize), ResolvedOperatorCall>,
    /// Trait-backed method dispatch selected by overload resolution.
    ///
    /// Lowering consumes this for calls whose selected method lives in a trait impl rather than an inherent Rust impl.
    /// This keeps codegen from re-deriving dispatch from method names or argument shapes.
    pub resolved_method_calls: HashMap<(usize, usize), ResolvedMethodCall>,
    /// Top-level overload callee emitted names selected by the typechecker, keyed by full call expression span.
    pub selected_function_emitted_names: HashMap<(usize, usize), String>,
}

/// Test-runner and fixture metadata extracted during typechecking.
#[derive(Debug, Default, Clone)]
pub struct TestingArtifacts {
    /// `std.testing.fixture` declarations resolved during typechecking.
    ///
    /// A successful typecheck guarantees async fixture entries have exactly one top-level `yield value` boundary.
    pub fixtures: HashMap<String, TestingFixtureInfo>,
}

/// Custom protocol decisions that lower into explicit runtime calls.
#[derive(Debug, Default, Clone)]
pub struct ProtocolArtifacts {
    /// RFC 068: Custom `for` iteration protocol choices keyed by iterable expression span.
    ///
    /// Lowering consumes this so a structural `__iter__` / `__next__` pair can become an explicit loop that calls the
    /// resolved hooks without relying on Rust's `IntoIterator`.
    pub iterations: HashMap<(usize, usize), ProtocolIterationInfo>,
}

/// A typechecker-resolved user-defined operator call consumed by IR lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOperatorCall {
    /// The concrete dunder method name selected by frontend method/trait dispatch.
    pub method: String,
    /// The AST operator shape this call replaces.
    pub kind: ResolvedOperatorKind,
}

/// Metadata for one imported Rust trait binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTraitImportInfo {
    /// Canonical import path used by Incan for this trait binding.
    pub trait_path: String,
    /// Resolved Rust definition path after re-export resolution, when available.
    pub definition_path: Option<String>,
    /// Method names this trait can place in Rust method-lookup scope.
    pub methods: HashSet<String>,
    /// Method signatures this trait metadata provided, keyed by method name.
    pub method_signatures: HashMap<String, RustFunctionSig>,
}

/// A typechecker-resolved Rust extension-trait import required by a method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustMethodTraitImportUse {
    /// Local import binding to retain in generated Rust.
    pub binding: String,
    /// Trait path selected for the call.
    pub trait_path: String,
    /// Method name observed at the call site.
    pub method: String,
    /// Trait method signature, when metadata supplied it.
    pub signature: Option<RustFunctionSig>,
}

/// A typechecker-resolved method call consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMethodCall {
    /// The concrete source-level method name selected by frontend method/trait dispatch.
    pub method: String,
    /// How the backend should emit this method call.
    pub dispatch: ResolvedMethodDispatch,
}

/// Backend-relevant dispatch target for a resolved method call.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedMethodDispatch {
    /// Emit as a fully-qualified trait call, e.g. `Trait::<T>::method(&receiver, ...)`.
    Trait {
        /// Rust-visible trait path. Local traits use their source name; imported stdlib traits use a fully-qualified
        /// `crate::__incan_std::...` path so callers do not need to import implementation traits explicitly.
        trait_path: String,
        /// Concrete trait type arguments selected by overload resolution.
        type_args: Vec<ResolvedType>,
    },
}

/// Typechecker-resolved custom iteration protocol consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolIterationInfo {
    /// Method selected on the iterable expression.
    pub iter_method: String,
    /// Concrete iterator object type returned from `__iter__`.
    pub iterator_type: ResolvedType,
    /// Method selected on the iterator object.
    pub next_method: String,
    /// Element type unwrapped from `__next__() -> Option[T]`.
    pub item_type: ResolvedType,
}

/// Lowering metadata for one RFC 046 computed property read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedPropertyAccessInfo {
    pub owner_type: String,
    pub property: String,
}

/// Operator expression shape for a resolved user-defined operator call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedOperatorKind {
    Binary,
    Unary,
    Index,
    IndexAssign,
    Truthiness,
    Len,
    Contains,
    Call,
}

/// Typechecker-proven call-unpack shape consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub enum FixedUnpackPlan {
    /// `*expr` has a statically known ordered shape with one type per contributed positional item.
    Positional(Vec<ResolvedType>),
    /// `**expr` has statically known string keys in source order.
    Keyword(Vec<String>),
}

/// How an identifier expression resolved in the symbol table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentKind {
    /// A value binding (variable/field), or a callable value (function).
    Value,
    /// A module static binding.
    Static,
    /// A type name (models/classes/enums/newtypes).
    TypeName,
    /// An enum variant constructor identifier.
    Variant,
    /// A module-like namespace (e.g. imported module placeholders).
    Module,
    /// A Rust import placeholder (`import rust::...` / `from rust::... import ...`).
    RustImport,
    /// A Rust value import, such as a public Rust constant.
    RustValue,
    /// A trait name (may be used as a type-like namespace).
    Trait,
}

/// Compiler-proven source declaration target for codegraph call/reference records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTargetInfo {
    /// Canonical source module path segments that own the target declaration.
    pub module_path: Vec<String>,
    /// Source declaration name in the owning module.
    pub name: String,
    /// Source declaration kind, matching the codegraph declaration `kind` spelling.
    pub kind: String,
}

/// Coercion category selected by the typechecker for a Rust-boundary call argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustArgCoercionKind {
    /// Builtin boundary matrix coercion (`i16 -> i64`, `str -> &str`, ...).
    Builtin(CoercionPolicy),
    /// Rusttype alias can flow to its backing Rust type without an explicit adapter call.
    RustTypeUnwrap,
    /// Rusttype alias uses a declared `interop:` adapter edge.
    RustTypeInterop,
}

/// Lowering metadata for one Rust-boundary call argument.
#[derive(Debug, Clone, PartialEq)]
pub struct RustArgCoercionInfo {
    /// Normalized Rust parameter type display from metadata (e.g. `f32`, `&str`).
    pub rust_target_type: String,
    /// Resolved target type for lowering IR typing.
    pub target_type: ResolvedType,
    /// Coercion strategy to apply.
    pub kind: RustArgCoercionKind,
}

/// One typechecker-approved validated-newtype coercion chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedNewtypeCoercionInfo {
    /// Ordered underlying-to-target conversion steps.
    pub steps: Vec<ValidatedNewtypeCoercionStep>,
    /// Final target type after all steps.
    pub target_type: ResolvedType,
    /// Runtime failure strategy selected for the coercion site.
    pub mode: ValidatedNewtypeCoercionMode,
}

/// Runtime failure behavior for one validated-newtype coercion site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedNewtypeCoercionMode {
    /// Ordinary sites panic on the first validation error.
    FailFast,
    /// Model/class constructor fields collect this field's validation error before the constructor fails.
    AggregateField { field_name: String },
}

/// One conversion step in a validated-newtype coercion chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNewtypeCoercionStep {
    /// Newtype being constructed by this step.
    pub newtype_name: String,
    /// Canonical validation hook to call. `None` means direct newtype wrapping is sufficient.
    pub ctor: Option<String>,
    /// Generated constrained-primitive predicates to enforce before direct wrapping.
    pub constraints: Vec<NewtypePrimitiveConstraint>,
}

/// Lowering metadata for a visible static binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticBindingInfo {
    /// `true` when this name came from `from pub::... import NAME`.
    pub is_imported: bool,
}

/// Lowering metadata for one source function declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBindingInfo {
    /// Typechecker-resolved source parameters, including default-presence markers.
    pub params: Vec<CallableParam>,
    /// Typechecker-resolved source return type.
    pub return_type: ResolvedType,
}

/// Lowering metadata for one RFC 036 decorated function binding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoratedFunctionBindingInfo {
    /// Final type of the module-visible binding after applying all user-defined decorators.
    pub ty: ResolvedType,
    /// Original callable type before decorators are applied.
    pub original_ty: ResolvedType,
    /// Source-declared type parameters preserved for explicit call-site generic arguments.
    pub type_params: Vec<String>,
    /// Explicit source-declared bounds per type parameter.
    pub type_param_bounds: HashMap<String, Vec<String>>,
    /// Resolved source-declared bounds, preserving generic type arguments.
    pub type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>>,
    /// Whether the original declaration is async.
    pub is_async: bool,
}

/// Lowering metadata for one RFC 036 decorated method binding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoratedMethodBindingInfo {
    /// Final unbound callable type after applying all user-defined decorators. The receiver is the first parameter.
    pub unbound_ty: ResolvedType,
    /// Original unbound callable type before decorators are applied. The receiver is the first parameter.
    pub original_unbound_ty: ResolvedType,
}

/// Lowering and test-runner metadata for one `std.testing.fixture` function.
///
/// This is the frontend handoff for test-runner and lowering-adjacent code that needs to know fixture shape without
/// re-resolving decorators from raw AST nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestingFixtureInfo {
    /// Fixture scope selected by `@fixture(scope=...)`, defaulting to function scope.
    pub scope: TestingFixtureScope,
    /// Whether `@fixture(autouse=true)` was set.
    pub autouse: bool,
    /// Whether the fixture declaration used `async def`.
    pub is_async: bool,
    /// Whether the fixture has teardown work after its yielded value.
    pub has_teardown: bool,
    /// Fixture dependencies named by parameters that resolve to other fixture functions.
    pub dependencies: Vec<String>,
}

impl TypeCheckInfo {
    /// Return the resolved type recorded for the expression at `span`, if any.
    pub fn expr_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.expr_types.get(&(span.start, span.end))
    }

    /// Return exact Rust parameter displays recorded for a closure expression, if any.
    pub fn closure_param_type_displays(&self, span: Span) -> Option<&[String]> {
        self.rust
            .closure_param_type_displays
            .get(&(span.start, span.end))
            .map(Vec::as_slice)
    }

    /// Return computed-property metadata for a field-access expression, if that access resolved to a property.
    pub fn computed_property_access(&self, span: Span) -> Option<&ComputedPropertyAccessInfo> {
        self.expressions.computed_property_accesses.get(&(span.start, span.end))
    }

    /// Record that a field-access expression resolved to a computed property read.
    pub(crate) fn record_computed_property_access(&mut self, span: Span, owner_type: &str, property: &str) {
        self.expressions.computed_property_accesses.insert(
            (span.start, span.end),
            ComputedPropertyAccessInfo {
                owner_type: owner_type.to_string(),
                property: property.to_string(),
            },
        );
    }

    /// Return the RFC 038 fixed/static unpack plan recorded for an unpack operand, if any.
    pub fn fixed_unpack_plan(&self, span: Span) -> Option<&FixedUnpackPlan> {
        self.calls.fixed_unpack_plans.get(&(span.start, span.end))
    }

    /// Return how the identifier expression at `span` resolved in the symbol table.
    pub fn ident_kind(&self, span: Span) -> Option<IdentKind> {
        self.expressions.ident_kinds.get(&(span.start, span.end)).copied()
    }

    /// Return a compiler-proven source target for the expression at `span`, if one was recorded.
    pub fn source_target(&self, span: Span) -> Option<&SourceTargetInfo> {
        self.expressions.source_targets.get(&(span.start, span.end))
    }

    /// Return whether the identifier at `span` resolved to the ambient `std.logging` logger binding.
    pub fn is_ambient_logger_binding(&self, span: Span) -> bool {
        self.expressions
            .ambient_logger_bindings
            .contains(&(span.start, span.end))
    }

    /// Record that an identifier resolved to the ambient `std.logging` logger binding.
    pub(crate) fn record_ambient_logger_binding(&mut self, span: Span) {
        self.expressions.ambient_logger_bindings.insert((span.start, span.end));
    }

    /// Return static-binding metadata for `name`, if the checker recorded one.
    pub fn static_binding(&self, name: &str) -> Option<&StaticBindingInfo> {
        self.declarations.static_bindings.get(name)
    }

    /// Return the Rust emitted name selected for a function declaration span, if overloads renamed it.
    pub fn function_emitted_name(&self, span: Span) -> Option<&str> {
        self.declarations
            .function_emitted_names
            .get(&(span.start, span.end))
            .map(String::as_str)
    }

    /// Record a Rust emitted name for an overloaded function declaration.
    pub(crate) fn record_function_emitted_name(&mut self, span: Span, emitted_name: String) {
        self.declarations
            .function_emitted_names
            .insert((span.start, span.end), emitted_name);
    }

    /// Return emitted provider function names for an imported overload binding, if any.
    pub fn imported_function_emitted_names(&self, local_name: &str) -> Option<&[String]> {
        self.declarations
            .imported_function_emitted_names
            .get(local_name)
            .map(Vec::as_slice)
    }

    /// Record emitted provider function names for an imported overload binding.
    pub(crate) fn record_imported_function_emitted_names(&mut self, local_name: String, emitted_names: Vec<String>) {
        self.declarations
            .imported_function_emitted_names
            .insert(local_name, emitted_names);
    }

    /// Return partial projection metadata for a visible partial binding, if collection recorded one.
    pub fn partial_projection(&self, local_name: &str) -> Option<&PartialProjectionInfo> {
        self.declarations.partial_projections.get(local_name)
    }

    /// Record partial projection metadata for a visible partial binding.
    pub(crate) fn record_partial_projection(&mut self, projection: PartialProjectionInfo) {
        self.declarations
            .partial_projections
            .insert(projection.name.clone(), projection);
    }

    /// Return overload candidates for one source binding, if any.
    pub fn function_overloads(&self, local_name: &str) -> Option<&[FunctionOverloadInfo]> {
        self.declarations.function_overloads.get(local_name).map(Vec::as_slice)
    }

    /// Record overload candidates for one source binding.
    pub(crate) fn record_function_overloads(&mut self, local_name: String, overloads: Vec<FunctionOverloadInfo>) {
        self.declarations.function_overloads.insert(local_name, overloads);
    }

    /// Return frontend fixture metadata for `name`, if the declaration was marked with `@fixture`.
    pub fn testing_fixture(&self, name: &str) -> Option<&TestingFixtureInfo> {
        self.testing.fixtures.get(name)
    }

    /// Return the computed const value for `name`, when const evaluation succeeded.
    pub fn const_value(&self, name: &str) -> Option<&ConstValue> {
        self.consts.const_values.get(name)
    }

    /// Return the recorded Rust-boundary argument coercion for the expression at `span`, if any.
    pub fn rust_arg_coercion(&self, span: Span) -> Option<&RustArgCoercionInfo> {
        self.rust.arg_coercions.get(&(span.start, span.end))
    }

    /// Return the validated-newtype coercion recorded for the expression at `span`, if any.
    pub fn validated_newtype_coercion(&self, span: Span) -> Option<&ValidatedNewtypeCoercionInfo> {
        self.expressions
            .validated_newtype_coercions
            .get(&(span.start, span.end))
    }

    /// Record a typechecker-approved validated-newtype coercion for a source expression span.
    pub(crate) fn record_validated_newtype_coercion(&mut self, span: Span, info: ValidatedNewtypeCoercionInfo) {
        self.expressions
            .validated_newtype_coercions
            .insert((span.start, span.end), info);
    }

    /// Return the recorded return coercion for the call expression at `span`, if any.
    pub fn rust_return_coercion(&self, span: Span) -> Option<&RustArgCoercionInfo> {
        self.rust.return_coercions.get(&(span.start, span.end))
    }

    /// Whether lowering should preserve Rust method-call lookup argument shape for this receiver/method pair.
    pub fn preserves_regular_method_arg_shape(&self, receiver_span: Span, method: &str) -> bool {
        self.rust.regular_method_arg_shape_preserving_calls.contains(&(
            receiver_span.start,
            receiver_span.end,
            method.to_string(),
        ))
    }

    /// Record that lowering should preserve Rust method-call lookup argument shape for this receiver/method pair.
    pub(crate) fn record_regular_method_arg_shape(&mut self, receiver_span: Span, method: &str) {
        self.rust.regular_method_arg_shape_preserving_calls.insert((
            receiver_span.start,
            receiver_span.end,
            method.to_string(),
        ));
    }

    /// Return the Rust struct field names selected for this named-field constructor call, if any.
    pub fn rust_named_field_constructor_fields(&self, span: Span) -> Option<&[String]> {
        self.rust
            .named_field_constructor_fields
            .get(&(span.start, span.end))
            .map(Vec::as_slice)
    }

    /// Record the Rust struct field names selected for a named-field constructor call.
    pub(crate) fn record_rust_named_field_constructor_fields(&mut self, span: Span, fields: Vec<String>) {
        self.rust
            .named_field_constructor_fields
            .insert((span.start, span.end), fields);
    }

    /// Return the Rust field name resolved for one Rust field-access expression, if one was recorded.
    pub fn rust_field_access_name(&self, span: Span) -> Option<&str> {
        self.rust
            .field_access_names
            .get(&(span.start, span.end))
            .map(String::as_str)
    }

    /// Record the Rust field name resolved for one Rust field-access expression.
    pub(crate) fn record_rust_field_access_name(&mut self, span: Span, field: String) {
        self.rust.field_access_names.insert((span.start, span.end), field);
    }

    /// Return rest-aware callable metadata recorded for the full call expression span, if any.
    pub fn call_site_callable_params(&self, span: Span) -> Option<&[CallableParam]> {
        self.calls
            .call_site_callable_params
            .get(&(span.start, span.end))
            .map(Vec::as_slice)
    }

    /// Return the overloaded Rust emitted callee selected for one source call expression.
    pub fn selected_function_emitted_name(&self, span: Span) -> Option<&str> {
        self.calls
            .selected_function_emitted_names
            .get(&(span.start, span.end))
            .map(String::as_str)
    }

    /// Record the overloaded Rust emitted callee selected for one source call expression.
    pub(crate) fn record_selected_function_emitted_name(&mut self, span: Span, emitted_name: String) {
        self.calls
            .selected_function_emitted_names
            .insert((span.start, span.end), emitted_name);
    }

    /// Record callable metadata needed by lowering when the callee expression alone cannot carry it.
    pub(crate) fn record_call_site_callable_params(&mut self, span: Span, params: &[CallableParam]) {
        if params
            .iter()
            .any(|param| param.kind != ParamKind::Normal || callable_param_needs_boundary_snapshot(&param.ty))
        {
            self.calls
                .call_site_callable_params
                .insert((span.start, span.end), params.to_vec());
        }
    }

    /// Record exact callable metadata when overload/source-method resolution selected a concrete callable.
    pub(crate) fn record_call_site_callable_params_exact(&mut self, span: Span, params: &[CallableParam]) {
        self.calls
            .call_site_callable_params
            .insert((span.start, span.end), params.to_vec());
    }

    /// Record callable metadata required by an explicit lowered dispatch path.
    pub(crate) fn record_call_site_callable_params_for_dispatch(&mut self, span: Span, params: &[CallableParam]) {
        self.calls
            .call_site_callable_params
            .insert((span.start, span.end), params.to_vec());
    }

    /// Return a typechecker-resolved user-defined operator call for `span`, if any.
    pub fn resolved_operator_call(&self, span: Span) -> Option<&ResolvedOperatorCall> {
        self.calls.resolved_operator_calls.get(&(span.start, span.end))
    }

    /// Return a typechecker-resolved method call for `span`, if any.
    pub fn resolved_method_call(&self, span: Span) -> Option<&ResolvedMethodCall> {
        self.calls.resolved_method_calls.get(&(span.start, span.end))
    }

    /// Return the Rust extension-trait import selected for the method call at `span`, if any.
    pub fn rust_method_trait_import_use(&self, span: Span) -> Option<&RustMethodTraitImportUse> {
        self.rust.method_trait_import_uses.get(&(span.start, span.end))
    }

    /// Return custom iteration protocol metadata for `span`, if any.
    pub fn protocol_iteration(&self, span: Span) -> Option<&ProtocolIterationInfo> {
        self.protocols.iterations.get(&(span.start, span.end))
    }

    /// Record a user-defined operator call that lowering should emit as a direct dunder method call.
    pub(crate) fn record_resolved_operator_call(
        &mut self,
        span: Span,
        method: impl Into<String>,
        kind: ResolvedOperatorKind,
    ) {
        self.calls.resolved_operator_calls.insert(
            (span.start, span.end),
            ResolvedOperatorCall {
                method: method.into(),
                kind,
            },
        );
    }

    /// Record a resolved method dispatch that lowering should preserve explicitly.
    pub(crate) fn record_resolved_method_call(
        &mut self,
        span: Span,
        method: impl Into<String>,
        dispatch: ResolvedMethodDispatch,
    ) {
        self.calls.resolved_method_calls.insert(
            (span.start, span.end),
            ResolvedMethodCall {
                method: method.into(),
                dispatch,
            },
        );
    }

    /// Record that a Rust method call requires a specific imported extension trait in generated Rust scope.
    pub(crate) fn record_rust_method_trait_import_use(&mut self, span: Span, import_use: RustMethodTraitImportUse) {
        self.rust
            .method_trait_import_uses
            .insert((span.start, span.end), import_use);
    }

    /// Record a custom `for` iteration protocol route.
    pub(crate) fn record_protocol_iteration(&mut self, span: Span, info: ProtocolIterationInfo) {
        self.protocols.iterations.insert((span.start, span.end), info);
    }
}

/// Return whether a callable parameter type carries borrow shape that lowering cannot recover from the callee alone.
fn callable_param_needs_boundary_snapshot(ty: &ResolvedType) -> bool {
    if ty.is_union() {
        return true;
    }
    match ty {
        ResolvedType::Ref(_) | ResolvedType::RefMut(_) | ResolvedType::FrozenStr | ResolvedType::FrozenBytes => true,
        ResolvedType::Function(params, ret) => {
            params
                .iter()
                .any(|param| callable_param_needs_boundary_snapshot(&param.ty))
                || callable_param_needs_boundary_snapshot(ret)
        }
        ResolvedType::Generic(_, args) => args.iter().any(callable_param_needs_boundary_snapshot),
        ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
            callable_param_needs_boundary_snapshot(inner)
        }
        ResolvedType::FrozenDict(key, value) => {
            callable_param_needs_boundary_snapshot(key) || callable_param_needs_boundary_snapshot(value)
        }
        ResolvedType::Tuple(items) => items.iter().any(callable_param_needs_boundary_snapshot),
        _ => false,
    }
}
