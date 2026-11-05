//! Frontend bridge from typechecked AST function bodies into Body IR v0.
//!
//! Declaration-level HIR ([`crate::hir`]) does not model statements or expressions at all (see its module docs), so
//! Body IR v0 lowers directly from `ast::FunctionDecl` bodies plus [`TypeCheckInfo`], rather than from a hypothetical
//! body-shaped HIR that does not exist yet. Every [`Body`](incan_semantics_core::body_ir::Body) this module produces
//! carries a [`CompilerNodeId`] identical to the one [`crate::hir::build_hir_v0`] would assign the same function's
//! [`crate::hir`] declaration, so the two can be correlated by id without threading a [`crate::hir`] value through this
//! API.
//!
//! Body IR v0 lowers a representative, explicitly documented subset of the language surface (see
//! [`incan_semantics_core::body_ir`] module docs for the full rationale). Statements fully lowered: assignment
//! (inferred/let/mutable/reassignment), field/index assignment (including their pre-desugared compound `<op>=` forms),
//! compound assignment (`x <op>= y`), tuple unpacking, multi-target (lvalue) tuple assignment, chained assignment,
//! `return`, `if`/`elif`/`else`, `loop:`, `while`, `for` (both a `start..end` range and a general iterable -- builtin
//! collections or a resolved `__iter__`/`__next__` protocol, including the fallible `for item in iterable?:` form),
//! expression statements, statement-position `yield value` (see [`BodyBuilder::lower_stmt_into`] and
//! [`bir::Body::is_generator`]), all three RFC 018 `assert` forms -- plain condition, `assert value is P` (whose
//! bindings stay live for the rest of the enclosing block), and `assert call() raises E` (see
//! [`BodyBuilder::lower_assert`]) -- `pass`, `break` (including a value-producing `break` inside a `loop` expression),
//! `continue`. Expressions fully lowered: identifiers, literals (int/float/decimal/bool/string/bytes),
//! arithmetic/comparison/boolean binary operators and all three unary operators, calls and method calls (including
//! named, out-of-order, defaulted, and explicitly generic argument spellings -- see [`BodyBuilder::lower_call`]), field
//! access, indexing, slicing, parenthesization, range values, tuples, list/dict/set literals (list and dict spread
//! entries included; set literals have no spread spelling), `model`/`class` construction (named-only at the source
//! level, bound to declared field order -- see [`BodyBuilder::lower_nominal_construction`]), expression-position
//! `if`/`loop`, `try` (`?`), f-strings, list/dict comprehensions, lazy generator expressions, closure literals, partial
//! callables (see [`BodyBuilder::lower_closure`]/[`BodyBuilder::lower_partial`] for how captures are computed and
//! represented explicitly rather than left implicit), and `match` (see [`BodyBuilder::lower_match`] for how patterns
//! are lowered and their bindings scoped).
//!
//! Everything else lowers to an explicit `Statement::Unsupported` / `Operand::Unknown` node rather than panicking,
//! so the model stays total over real programs. That residue is now short, and every entry is a decided refusal
//! with a stated reason rather than pending work — #1101 and its sixteen children (#1158 through #1167, #1172,
//! and the #1244/#1246 follow-ons) are closed, so nothing here awaits a named owner. What still refuses: a spread
//! in a `model`/`class` construction, because the typechecker records no field binding for it and a construction
//! bound to declared field order cannot admit an unresolved layout; a spread with no statically proven shape
//! against a callee whose fixed signature *is* resolvable, whose arity no stage can establish; and a spread to a
//! locally held callable value, whose declared slots exist only in the callable's type. All three are the same
//! fact gap — no stage records how a spread maps onto declared slots — and closing them means representing that
//! mapping, not adding a lowering arm. `if let`/`while let`, destructuring comprehension and generator clauses,
//! `await`, `race for`, statement-position `loop:`, bytes literals, and range values, which earlier revisions of
//! this paragraph listed as residue, all lower now.
//!
//! Vocab and scoped-DSL surface nodes are deliberately *not* on that list. They are not unmodeled language
//! constructs awaiting a lowering; the desugar pass resolves them before this module runs, and one reaching here
//! means a caller skipped it. See [`build_body_ir_module`]'s input contract (#1166) for what a caller owes and
//! for how such a node is named when it arrives anyway.
//!
//! Source `unsafe:` regions retain an acknowledgment receipt, their source span, and the exact operations they
//! cover. They introduce no Incan scope. Consumers must validate and deliberately admit the acknowledgment before
//! lowering its operations; it neither grants provider capabilities nor bypasses another unsupported construct.
//! See [`BodyBuilder::lower_unsafe_region`].
//!
//! One coverage limit is silent rather than marked, and it is deliberate. Expression-position `yield` (the two-way
//! send/receive protocol) is a stub in the existing Rust-emission backend too, so there is no behavior to preserve;
//! the typechecker rejects a bare `yield` with no value before lowering runs. Model, class, trait-default, newtype,
//! and enum method bodies all lower through [`lower_owner_method_bodies`].

use std::collections::{HashMap, HashSet};

pub use free_function_baseline::verify_module as verify_free_function_inputs;
pub(crate) use nominal_carriers::{retain_published_frame, retain_published_module};
pub use primitive_adapter_requirements::required as required_primitive_adapters;
pub use source_comparison::required_published_defaults as required_comparison_defaults;

use incan_lang::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanCallableParam,
    IncanCallableParamKind, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, rust_tuple_arity,
};

use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

use crate::ast;
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::{
    FixedUnpackPlan, IdentKind, ResolvedOperatorKind, TypeCheckInfo, ValidatedNewtypeCoercionInfo,
    semantic_type_from_resolved,
};

/// What one compilation lowers its modules against.
pub struct BodyIrContext<'a> {
    /// Body IR of the declarations a module reaches outside itself: the program's other modules and the declaration
    /// records of the Loaves the program links (DD-0007).
    pub modules: &'a [bir::BodyIrModule],
    /// The provider operations the linked Loaves admit.
    pub provider_operations: &'a ProviderOperationCatalog,
    /// The compilation's package identity, which explicit registry subjects are keyed by.
    pub registry_package: Option<&'a str>,
    /// The package a library unit's own declarations belong to; `None` for a program. A library's modules are its
    /// package's modules (`pub::<package>::<module>`), the identity its consumers read them under.
    pub library_package: Option<&'a str>,
}

/// Return every checked registry helper required by generated registrations and explicit subject materialization.
/// Dependency discovery uses the same selected helper identities as the Body IR producer, including function and
/// method descriptions, so loading cannot omit a checked call or select an unaccepted helper by spelling.
pub fn registry_helper_identities(info: &TypeCheckInfo) -> Vec<CanonicalSymbolId> {
    registry_effects::helper_identities(info)
}

/// Build Body IR v0 for every top-level function declaration and every non-abstract class/model/trait method in a
/// typechecked module.
///
/// `ast::Declaration::Function` items each produce one [`bir::Body`], matching the [`CompilerNodeId`]
/// [`crate::hir::build_hir_v0`] assigns the corresponding declaration (see that function's docs).
/// `ast::Declaration::Model`/`Class`/`Trait` items additionally contribute one [`bir::Body`] per non-abstract method
/// (#1102) — abstract methods (`body: None`, trait requirements with no implementation) contribute nothing, since there
/// is no body to lower. Method [`CompilerNodeId`]s are *not* assigned by [`crate::hir::build_hir_v0`] today
/// (declaration-level HIR only assigns ids to top-level declarations), so this function constructs its own method ids
/// by scoping the method name under its owning declaration's name — see [`lower_method_body`].
///
/// # Input contract
///
/// `program` must be a **desugared, feature-projected, typechecked** module, and `type_info` must be the checker
/// output for exactly that projected program. Every caller owes the preparation `CompilationSession::parse_source`
/// (`loaves/compiler/incan_driver/src/session.rs`) performs: parse, then
/// `vocab_desugar_pass::desugar_program_vocab_blocks_with_runtime`, then feature projection, and only then typecheck,
/// so a vocab-authored body means the same thing to every consumer (#1166).
///
/// What must **not** cross this boundary:
///
/// - Raw vocabulary syntax — `ast::Declaration::VocabBlock`, `ast::Statement::VocabBlock`,
///   `ast::Statement::VocabExpressionItem`, `ast::Expr::VocabBlock` — and scoped-DSL surface nodes
///   ([`SurfaceFeatureKey::ScopedDslSurface`], and the `LeadingDotPath`/`ScopedGlyph`/`ScopedSymbolCall` payloads of
///   `ast::Expr::Surface`). The desugar pass owns their meaning; lowering them here would give one source construct two
///   independent definitions.
/// - Declarations gated behind a package feature that is not active. Feature projection is part of the contract, not an
///   optimization: a body behind an inactive feature must not be lowered at all, because lowering it would put a body
///   into Body IR that the compilation does not contain.
///
/// What must cross it: ordinary declarations, statements, and expressions, including the async surface (`await`, `race
/// for`), which is genuine language surface no desugarer removes.
///
/// The contract is enforced rather than assumed. A vocab or scoped-DSL node that still arrives lowers to a
/// `bir::StatementKind::Unsupported` whose description names it as a caller contract violation (see the `refusals`
/// submodule), so a broken caller is a visible diagnostic at the original span instead of a silently missing body.
/// Those refusals are a safety net, never the normal path; the desugar pass keeps ownership of the real diagnostic
/// when a vocabulary's library manifest is unavailable.
///
/// The compilation session supplies an already desugared, feature-projected program and its checked facts; this
/// boundary applies no second projection and recreates no typecheck authority.
pub fn build_body_ir_module(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    context: &BodyIrContext<'_>,
) -> bir::BodyIrModule {
    let mut module = build_module_seed(program, module_path, type_info, context);
    retain_module_representations(&mut module, program, type_info, context.modules);
    module
}

/// The declarations-only Body IR one member of an import cycle contributes to the other members' context before its
/// own bodies are lowered: its model, nominal and enum layouts under the module identity [`build_body_ir_module`]
/// gives it. A caller lowering a cycle adds every not-yet-lowered member's declarations to the context of each member,
/// so no member depends on the order the cycle's bodies are lowered in.
pub fn build_cycle_declarations(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    library_package: Option<&str>,
) -> bir::BodyIrModule {
    module_seed::build_cycle_declarations(program, module_path, type_info, library_package)
}

/// A type leaf whose retained canonical identity must survive Result-side settlement.
enum RetainedTypeLeaf {
    /// A source or package nominal binding.
    Named(String),
    /// A builtin runtime carrier with exact root binding proof.
    NativeBuiltin(incan_semantics_core::RuntimeBuiltinType),
}

/// Source-declared ordinary default expressions for each top-level function in this module.
///
/// Body-IR lowering needs this small source map only while it lowers a local `partial target(...)` into a forwarding
/// closure: the checked callable signature retains availability but not the executable default expression. The map
/// never leaves this frontend boundary; the resulting [`bir::CallableParamDefault::Source`] stores only Body IR.
type FunctionDefaultSources = HashMap<String, Vec<FunctionDefaultSource>>;

/// Exact spans of this source module's top-level function declarations, grouped by source spelling.
///
/// The typechecker exposes an intentionally wider overload surface that can include imports and aliases. Direct
/// Body-IR dispatch only admits a declaration physically represented by this module, so lowering retains this
/// small source-local map long enough to attach the chosen declaration identity to each named call.
type LocalFunctionDeclarations = HashMap<String, Vec<ast::Span>>;

/// Source-local checked models and classes with canonical layouts.
///
/// Unsupported inheritance and member binding forms never enter this constructor registry; checked field aliases select
/// canonical storage. Computed properties contribute getter bodies rather than layout fields.
type LocalNominalDeclarations = HashMap<String, bir::NominalDeclaration>;

/// Source-local fieldless normal enums whose canonical unit variants are retained for direct comparison.
///
/// This map exists only while lowering. The executor receives `BodyIrModule::fieldless_enum_declarations` and
/// revalidates exact enum/member identities there, so a source spelling never selects an imported or aliased enum.
type LocalFieldlessEnumDeclarations = HashMap<String, bir::FieldlessEnumDeclaration>;

/// Source-local RFC 032 value enums whose canonical scalar members are retained for direct execution.
///
/// This map is lowering-only. The executor receives `BodyIrModule::value_enum_declarations` and verifies retained
/// enum/member identities there, so imports, aliases, ordinary enums, and non-retained same-spelling forms never
/// become direct runtime targets.
type LocalValueEnumDeclarations = HashMap<String, bir::ValueEnumDeclaration>;

/// Borrowed module facts shared by every body lowerer.
///
/// These facts are collected once from checked source and remain frontend-only: emitted Body IR carries only the
/// identities and representations a later direct executor needs. Keeping the bundle explicit avoids widening any
/// individual lowering helper's parameter surface as profiles add one bounded source-local fact at a time.
struct BodyIrLoweringFacts<'type_info, 'source> {
    /// Canonical provider-owned immutable method signatures available at the checked consumer boundary.
    published_methods: &'source [published_methods::PublishedMethod],
    /// Validated source-local model constant values, without static initialization semantics.
    model_constants: &'source HashMap<CanonicalSymbolId, model_constants::ModelConstant>,
    /// Checked local and published payload layouts indexed by their exact declaring identity.
    local_enum_declarations: &'source HashMap<CanonicalSymbolId, bir::EnumDeclaration>,
    /// Closed return carriers retained by a callable's providing module, keyed by its proven identity.
    published_callable_returns: &'source HashMap<CanonicalSymbolId, IncanType>,
    /// Parameter defaults of the linked Loaves' callables, in parameter order, keyed by declaration identity.
    published_callable_defaults: &'source HashMap<CanonicalSymbolId, Vec<bir::CallableParamDefault>>,
    /// Checked provider-method newtype results; conflicting declarations retain an explicit denial.
    published_newtype_returns: &'source HashMap<CanonicalSymbolId, Option<collect::PublishedNewtypeReturn>>,
    /// Exact checked provider storage types; a conflicting declaration records `None` and cannot authorize a global
    /// place.
    published_storage_types: &'source HashMap<CanonicalSymbolId, Option<IncanType>>,
    published_constants: &'source HashMap<CanonicalSymbolId, bir::Constant>,
    type_info: &'type_info TypeCheckInfo,
    /// Checked provider type names that cannot be distinguished from an ambient builtin by a bare signature leaf.
    runtime_type_conflicts: &'source std::collections::BTreeSet<String>,
    /// Project-aware entry points retain selected pattern carriers before source aliases are erased.
    retain_nominal_carriers: bool,
    function_default_sources: &'source FunctionDefaultSources,
    local_function_declarations: &'source LocalFunctionDeclarations,
    /// Physical function identities whose source declaration requires no type-argument instantiation.
    local_nongeneric_functions: &'source HashSet<CanonicalSymbolId>,
    local_receiver_methods: &'source [LocalReceiverMethod],
    /// Checked literal presets whose calls retain their complete arguments before source facts leave the frontend.
    local_trait_partials: &'source [trait_partials::LocalTraitPartial],
    local_nominal_declarations: &'source LocalNominalDeclarations,
    local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
    local_value_enum_declarations: &'source LocalValueEnumDeclarations,
    module_identity: &'source str,
    /// Invocation-owned package name for checked explicit subjects; absence remains a named refusal.
    registry_package: Option<&'source str>,
    /// Provider operations this compilation admits, keyed by canonical identity rather than by any spelling.
    provider_operations: &'source ProviderOperationCatalog,
    /// The file's checked `rust.module(...)` path, which an `@rust.extern` declaration delegates into (#2023).
    rust_module: Option<&'source str>,
}

/// Source facts a synthesized local partial needs for one target parameter.
#[derive(Clone)]
struct FunctionDefaultSource {
    /// The target parameter's original span.
    param_span: ast::Span,
    /// The target's ordinary source default, if it declared one.
    default: Option<ast::Spanned<ast::Expr>>,
}

/// One physical immutable instance method whose checked signature can close a caller-open receiver.
struct LocalReceiverMethod {
    /// Checked nominal declaration containing this method.
    owner: CanonicalSymbolId,
    /// Source slot spelling; alias projection uses the checker's existing table.
    name: String,
    /// Physical method span selecting its checked signature and canonical target.
    span: ast::Span,
}

/// Whether a compiled Loaf declares `identity`: its origin names a package, so a consumer names the Loaf's item
/// rather than declaring it (DD-0007).
pub(crate) fn is_loaf_identity(identity: &CanonicalSymbolId) -> bool {
    matches!(identity.origin, incan_semantics_core::SymbolOrigin::Package { .. })
}

/// Render a module path into the same module identity spelling [`crate::hir`] uses, so declaration ids line up between
/// the two representations.
fn body_ir_module_identity(module_path: &[String]) -> String {
    incan_semantics_core::module_identity_for_path(module_path)
}

/// Convert an AST byte-offset span into a Body IR source span.
const fn hir_span(span: ast::Span) -> HirSourceSpan {
    span
}

/// Source reads still available for one local, including whether projections shortened the legacy countdown.
#[derive(Clone, Copy)]
struct ReadCountdown {
    remaining: usize,
    projected: bool,
}

impl ReadCountdown {
    /// Seed an ordinary local or a synthetic chained-assignment value before any source read is consumed.
    fn new(remaining: usize) -> Self {
        Self {
            remaining,
            projected: false,
        }
    }
}

/// A block that ends by returning, such as a match arm that refuses its input. Only its own statements follow a read
/// in it, so the reads it holds decide whether a read is an enclosing owner's last on this path. Inside a loop the
/// block qualifies only when no `break` or `continue` in it can leave for the statements after the loop.
struct TerminalPath {
    /// The block's scope; owners declared inside it keep their ordinary countdown.
    scope: bir::ScopeId,
    /// Conditional depth when the block was entered. A read in a branch nested inside the block can leave the owner
    /// live on a sibling path, and a read in a loop nested inside it can repeat, so only a read at this depth and
    /// this loop depth transfers.
    conditionals: usize,
    /// Loop depth when the block was entered. A read deeper than this is inside a loop nested in the block and can
    /// repeat; a `return` lowers its value outside every loop it leaves, so a shallower read is still on this path.
    loops: usize,
    /// Source reads the block still holds of each non-Copy owner declared before it.
    reads: HashMap<bir::LocalId, usize>,
}

/// Per-function lowering state: fresh local/scope allocation, current name bindings, and accumulated body-level
/// facts (runtime requirements, panic facts, which locals have been moved out of their declaring scope).
struct BodyBuilder<'type_info, 'source> {
    /// Owned native payload and iterator-item locals whose exact carrier replaces an opaque pattern placeholder.
    native_enum_payload_bindings: HashSet<bir::LocalId>,
    /// Provider-owned method records; the Incan consumer receives their selected identity rather than names to
    /// resolve.
    published_methods: &'source [published_methods::PublishedMethod],
    /// Checked model constant trees materialized afresh at each read.
    model_constants: &'source HashMap<CanonicalSymbolId, model_constants::ModelConstant>,
    /// The same retained layouts used to close constructor destinations and pattern payload facts.
    local_enum_declarations: &'source HashMap<CanonicalSymbolId, bir::EnumDeclaration>,
    /// Provider-owned closed return carriers; a private import alias is never a consumer-side layout key.
    published_callable_returns: &'source HashMap<CanonicalSymbolId, IncanType>,
    /// Parameter defaults of the linked Loaves' callables, in parameter order, keyed by declaration identity.
    published_callable_defaults: &'source HashMap<CanonicalSymbolId, Vec<bir::CallableParamDefault>>,
    /// Exact provider-method return layouts, selected only by a checked method identity.
    published_newtype_returns: &'source HashMap<CanonicalSymbolId, Option<collect::PublishedNewtypeReturn>>,
    /// Owning declaration types for imported storage, selected only by the checked global identity.
    published_storage_types: &'source HashMap<CanonicalSymbolId, Option<IncanType>>,
    /// Immutable values decoded from the selected package representation, indexed by canonical declaration.
    published_constants: &'source HashMap<CanonicalSymbolId, bir::Constant>,
    type_info: &'type_info TypeCheckInfo,
    /// A source ownership proof is retained only for this exact callable. Nested evaluators start without it;
    /// invalid or incomplete evidence refuses the body before ordinary lowering can choose repair copies.
    input_body: Result<Option<crate::typechecker::CheckedInputBody<'type_info>>, &'static str>,
    /// Canonical whole owners authenticated by complete source-access coverage before branch lowering.
    input_transfer_subjects: std::collections::BTreeSet<CanonicalSymbolId>,
    /// Complete source path coverage permits omitting a structurally unreachable statement suffix.
    input_paths_checked: bool,
    /// Actual whole non-Copy Owned initializer assignments retained for staged conditional cleanup.
    input_owned_initializers: Vec<bir::CheckedOwnedInitializer>,
    /// Explicit local bindings, retained on the body so they still require a profile if the profile is lost.
    input_local_bindings: Vec<bir::ExplicitInputBinding>,
    /// The declared view this body returns, once its parameter locals exist.
    returned_view: Option<returned_views::ReturnedViewSource>,
    /// Each lowered call carrying a view in its `Result` or `Option`, by call span: its substituted place, access and
    /// the payload carrying the view.
    carried_view_places: HashMap<ast::Span, (bir::Place, incan_semantics_core::IncanInputAccess, usize)>,
    /// Each lowered async call returning a direct view, by call span: the source place the awaited expression aliases
    /// after its physical reference result has resumed.
    awaited_view_places: HashMap<ast::Span, (bir::Place, incan_semantics_core::IncanInputAccess)>,
    /// Each local bound to such a call, by identity: the call's span, whose substituted place a match takes out.
    carried_view_locals: HashMap<incan_semantics_core::CanonicalSymbolId, ast::Span>,
    /// Statement-if scopes whose branch states are independently joined for authenticated whole owners.
    input_if_scopes: Vec<bir::ScopeId>,
    /// Statement loops whose exit states are joined for authenticated whole owners, innermost last.
    input_loops: Vec<conditional_inputs::InputLoop>,
    /// Canonical provider context that requires an explicit refusal for ambiguous bare builtin spellings.
    runtime_type_conflicts: &'source std::collections::BTreeSet<String>,
    /// Project-aware entry points retain selected pattern carriers before source aliases are erased.
    retain_nominal_carriers: bool,
    /// Source defaults for top-level partial targets, retained only until they lower into Body IR.
    function_default_sources: &'source FunctionDefaultSources,
    /// Exact declarations physically present in this module, used only to retain same-module call identities.
    local_function_declarations: &'source LocalFunctionDeclarations,
    /// Physical nongeneric functions eligible for a closed declaration-signature fallback at intrinsic references.
    local_nongeneric_functions: &'source HashSet<CanonicalSymbolId>,
    /// Physical source methods available when a retained payload closes a legacy caller-open receiver.
    local_receiver_methods: &'source [LocalReceiverMethod],
    /// Source-local checked trait targets and effect-free preset values, consumed only while producing call facts.
    local_trait_partials: &'source [trait_partials::LocalTraitPartial],
    /// Checked local and selected package models, used to retain exact constructor identity and layout.
    local_nominal_declarations: &'source LocalNominalDeclarations,
    /// Checked local and selected package fieldless enums, used to retain exact unit-member target identities.
    local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
    /// Checked local and selected package value enums, used to retain exact member target identity.
    local_value_enum_declarations: &'source LocalValueEnumDeclarations,
    /// Owning module identity used to construct a source-span declaration identity without consulting a backend.
    module_identity: &'source str,
    /// Invocation-owned package name, consumed only by checked registry subject materialization.
    registry_package: Option<&'source str>,
    /// Provider operations this compilation admits, consulted only by canonical identity (see `provider_ops`).
    provider_operations: &'source ProviderOperationCatalog,
    /// Checked return type of the function/method currently being lowered, used only to retain `?` error routing.
    owner_return_type: IncanType,
    /// Explicit owner and callable binders whose checked types remain placeholders at this boundary.
    type_parameters: Vec<String>,
    /// Checked newtype owner of the current method, authorizing legacy's raw construction within its implementation.
    newtype_implementation: Option<CanonicalSymbolId>,
    locals: Vec<bir::LocalDecl>,
    scopes: Vec<bir::ScopeInfo>,
    /// Current source-name -> local binding.
    ///
    /// A plain inferred assignment reuses an active binding; `let` and `mut` replace this map entry with an explicit
    /// shadow. Nested lowering paths snapshot and restore the map at their lexical boundary, so branch/loop/arm
    /// names remain available to their Body-IR statements without leaking into following source.
    bindings: HashMap<String, bir::LocalId>,
    /// Canonical source binding -> frame local. This is the semantic lookup used for every resolver-proven read or
    /// write; [`Self::bindings`] remains only as a lexical bookkeeping projection for lowering constructs that
    /// introduce and restore source names.
    identity_bindings: HashMap<CanonicalSymbolId, bir::LocalId>,
    /// Explicit local loans select existing places; they do not allocate reference-valued owners or drops.
    input_aliases: HashMap<CanonicalSymbolId, input_locals::LocalInputAlias>,
    /// Canonical storage currently selected by a direct local static binding; reads retain a global place rather than
    /// a snapshot.
    static_aliases: HashMap<bir::LocalId, bir::GlobalPlace>,
    /// Locals originally declared as storage bindings, which can detach to values and later select another static.
    static_binding_locals: HashSet<bir::LocalId>,
    /// Names lowering could not resolve to a tracked local (e.g. module-level `const`/`static`), reused across
    /// repeated reads instead of allocating a fresh external local per read.
    external_locals: HashMap<String, bir::LocalId>,
    /// Remaining textual reads for each tracked (non-temporary) local, seeded at declaration time by counting
    /// `Ident` occurrences of its name in the declaring scope's statement suffix (see [`count_reads_in_stmts`]).
    /// Source operands consume their root's occurrence; generated projections do not. A final whole-owner read
    /// selects [`bir::OwnershipFact::Move`] only when its control-flow and pending-operand guards also permit it.
    remaining_reads: HashMap<bir::LocalId, ReadCountdown>,
    /// Local owners whose earlier operands remain unread until a call, dictionary write, or comprehension insertion.
    /// Nested evaluation cannot move one of these owners out from under the pending outer operand.
    pending_argument_reads: Vec<bir::LocalId>,
    /// Active normalized loop scopes. Reads of enclosing owners must survive the next iteration even at their last
    /// textual use.
    loop_scopes: Vec<bir::ScopeId>,
    /// Active conditional bodies. An enclosing owner cannot be marked unconditionally moved by just one path;
    /// locals created within that body retain ordinary final transfers.
    conditional_scopes: Vec<bir::ScopeId>,
    /// Active blocks that end by returning, innermost last. Statements after such a block are not on its path, so
    /// an enclosing owner's last read in it transfers the owner although later statements still read it.
    terminal_paths: Vec<TerminalPath>,
    /// Assignment destination whose current value ends when its right-hand side completes, with the enclosing loop
    /// depth. A loop nested inside that expression still repeats reads of the current value.
    replacing_local: Option<(bir::LocalId, usize)>,
    /// Source-annotated function locals whose Rust representation is a function pointer rather than an inferred item.
    function_pointer_locals: Vec<bir::LocalId>,
    /// Local bindings the checker proved hold capturing callable values rather than function pointers.
    closure_holding_locals: Vec<bir::LocalId>,
    /// Locals whose value has been moved out via a full-value (non-projected) read, so scope-exit drop insertion
    /// skips them.
    moved_out: HashSet<bir::LocalId>,
    /// `mut` parameters of non-`Copy` type: the callee borrows the caller's value (#2022), so it never drops one.
    pub(super) borrowed_parameters: HashSet<bir::LocalId>,
    /// Rest packs and native collection value parameters own their storage, despite their builtin collection layout.
    pub(super) owned_parameters: HashSet<bir::LocalId>,
    /// Locals whose current value was built by `lower_range_value` or copied from another such local. A checked
    /// `Range[T]` spelling alone is not a layout contract: parameters, call results, imports, and user
    /// declarations can use it without the four-field `AggregateKind::Range` representation. Only this
    /// source-local provenance permits a later `for` loop to project range fields.
    materialized_range_locals: HashSet<bir::LocalId>,
    /// Stack of the innermost-to-outermost enclosing loop's `break`-value target, pushed/popped by every loop-
    /// lowering path (`while`, `for`, and value-producing `loop` expressions) around its own body. `Some(local)`
    /// means the innermost loop is a value-producing `loop:` expression (see [`Self::lower_loop_expr`]) whose
    /// `break value` statements should assign into `local` instead of carrying the value on the `Break` statement
    /// itself; `None` means the innermost loop does not produce a value (`while`/`for`, which never legally see a
    /// `break value` today, or a `loop:` expression's own synthetic exit checks). Always non-empty while lowering
    /// any loop body, so [`Self::lower_break`] can look up the innermost target with `.last()`.
    loop_break_targets: Vec<Option<bir::LocalId>>,
    /// Checked operations that run before `continue` reaches the innermost loop header. Counting ranges retain their
    /// increment here; ordinary loops retain an empty sequence.
    loop_continue_actions: Vec<Vec<bir::Statement>>,
    runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    panic_facts: Vec<bir::PanicFact>,
    /// Source-span types that replace the checker's recorded ones while one evaluation of a chained value written per
    /// target is lowered. The checker checks such a value once per target but records one type per span, so each
    /// evaluation installs its own target's types here and clears them afterwards; it is empty everywhere else.
    type_overrides: HashMap<crate::ast::Span, IncanType>,
    /// Canonical leaves consumed from an exactly selected constructor field, emitted beside this body's projected
    /// carriers.
    contextual_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Only a checker-planned shared method chain may rewrite its callable receiver signature positions.
    shared_method_chain: bool,
    /// Whether the pattern being lowered matches its scrutinee in place: an arm changes a part of a writable place
    /// through a name the pattern binds (#1561), so each such name borrows that part mutably instead of reading a
    /// copy.
    in_place_pattern: bool,
    /// Source-span types of `Ok(...)` and `Err(...)` values whose open `Result` side the checker settled where the
    /// value flows, on the call parameter, closure or comprehension it flows into rather than on the value's own span
    /// (see [`Self::settle_open_result_value`]). Canonical set conversions also retain the exact element their
    /// evaluated builtin iterable proves, so an inferred binding preserves that storage without changing legacy
    /// checker facts (see [`Self::retain_set_constructor_type`]).
    settled_types: HashMap<crate::ast::Span, IncanType>,
    /// Spans of values whose newtype coercion is being lowered, so lowering the wrapped value itself does not apply
    /// the coercion again (see `lower_newtype_coercion`).
    coercions_in_progress: HashSet<crate::ast::Span>,
    next_local: u32,
    next_scope: u32,
}

// ============================================================================
// Comprehension desugaring helpers
// ============================================================================

// ============================================================================
// Free helper functions
// ============================================================================

/// Map a surface unary operator to Body IR's unary operator set. Exhaustive: all three surface unary operators have
/// a direct Body IR equivalent.
const fn lower_unary_op(op: ast::UnaryOp) -> bir::UnOp {
    match op {
        ast::UnaryOp::Neg => bir::UnOp::Neg,
        ast::UnaryOp::Not => bir::UnOp::Not,
        ast::UnaryOp::Invert => bir::UnOp::Invert,
    }
}

mod match_;

mod checked_json;
mod newtype_string_conversion;
pub use newtype_string_conversion::compilation_string_conversion_contract;

mod newtype_constraints;
mod newtype_field_ingress;
mod newtype_hooks;
mod newtype_ingress;
mod newtype_sum;
mod validation_errors;

mod closures;
mod conditional_inputs;
mod input_access;
mod input_arguments;
mod input_locals;
mod local_partials;
mod trait_partials;

mod async_;

mod literals;
mod nominal_carriers;

mod calls;
mod checked_bridge;
mod checked_foreign;
mod constructor_carriers;
mod primitive_adapter_requirements;
mod primitive_adapters;
pub(crate) mod runtime_nominals;
mod runtime_storage;
mod rust_calls;
mod rust_values;
mod source_comparison;
mod type_metadata;

mod operators;

mod expr;

mod defaults;

mod assertions;

mod comprehensions;

mod control_flow;
mod iteration_protocol;

mod stmt;

pub(crate) mod free_vars;
mod properties;
mod provider_ops;
mod refusals;

mod reads;
mod shared_reads;

mod carrier_projection;
mod collect;
mod collection_context;
mod decorated_bindings;
mod entrypoint_effects;
mod free_function_baseline;
mod model_constants;
mod published_methods;
mod registry_descriptions;
mod registry_effects;
mod representation_facts;
mod result_channels;
mod returned_views;
mod static_aliases;
mod union_layouts;
pub use union_layouts::reaches_union;

mod bodies;
mod enum_types;
mod opaque_returns;
mod parameter_types;
mod partial_closures;

mod direct_replacement;
mod locals_and_types;
mod module_seed;
mod partials;
mod places_and_ownership;
mod text_codecs;
mod trait_returns;
mod trait_values;

mod primitives;

mod args;
mod fixed_spreads;
mod numeric_destinations;
mod rest_args;
mod variadics;

use bodies::*;
use collect::*;
pub use direct_replacement::*;
use module_seed::*;
pub use provider_ops::ProviderOperationCatalog;
use provider_ops::ProviderOperationRecord;
use reads::*;

#[cfg(test)]
mod representation_tests;

#[cfg(test)]
#[path = "body_ir/baselines/union_layouts.rs"]
mod legacy_union_layouts;

#[cfg(test)]
#[path = "body_ir/baselines/opaque_returns.rs"]
mod legacy_opaque_returns;

#[cfg(test)]
#[path = "body_ir/baselines/shared_reads.rs"]
mod legacy_shared_reads;

#[cfg(test)]
#[path = "body_ir/baselines/source_comparison.rs"]
mod legacy_source_comparison;

#[cfg(test)]
#[path = "body_ir/baselines/trait_returns.rs"]
mod legacy_trait_returns;

#[cfg(test)]
#[path = "body_ir/baselines/trait_values.rs"]
mod legacy_trait_values;

#[cfg(test)]
mod tuple_destructure_interop_tests;
