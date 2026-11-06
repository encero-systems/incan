//! Trait, derivation, expression, reference and const artifacts recorded for lowering.

use super::*;

/// Trait hierarchy metadata consumed by trait impl and default-method lowering.
#[derive(Debug, Default, Clone)]
pub struct TraitArtifacts {
    /// RFC 042: Direct supertraits per trait name, copied from
    /// [`TraitInfo::supertraits`](crate::symbols::TraitInfo::supertraits) for IR lowering.
    ///
    /// Lowering does not retain the typechecker symbol table; this snapshot supplies resolved supertrait type
    /// arguments after a successful check.
    pub direct_supertraits: HashMap<String, Vec<(String, Vec<ResolvedType>)>>,
    /// RFC 042: Trait type parameter names keyed by trait name for lowering-time generic substitution.
    ///
    /// Includes locally-declared and imported traits so backend lowering can handle cross-module trait hierarchies
    /// without relying on local AST declarations.
    pub type_params: HashMap<String, Vec<String>>,
    /// Exact source identities of visible trait methods, keyed by trait identity and method spelling.
    ///
    /// A source-visible trait uses its local binding as the trait key. A dependency-only trait uses its
    /// module-qualified source name. Imported default-method ASTs retain declaration spans from another source file,
    /// so lowering cannot look them up in the current module's span-keyed declaration table. This checked map carries
    /// the already-resolved identity across that boundary without reconstructing it from either spelling.
    pub method_identities: HashMap<(String, String), CanonicalSymbolId>,
    /// Visible trait slots, local or imported, whose checked declaration supplies a default body.
    ///
    /// A consumer still requires that body's executable representation; this fact never supplies a missing body.
    pub default_method_identities: HashSet<CanonicalSymbolId>,
    /// Visible trait slots the standard packages declare, including through aliases and facades.
    ///
    /// Their protocol contracts do not imply executable source bodies among the project's modules.
    pub standard_method_identities: HashSet<CanonicalSymbolId>,
    /// Checked trait slots selected by calls on an open `Self` receiver, keyed by the whole call span.
    ///
    /// Body IR retains these declaration identities for concrete specialization. They remain separate from ordinary
    /// resolved call identities because legacy default expansion selects each adopter's implementation later.
    pub self_method_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Declared result types of the trait slots called on an open `Self` receiver in the standard library's own trait
    /// defaults, keyed by the whole call span.
    ///
    /// The standard library's source leaves such a call's checked type open, because legacy expands those defaults by
    /// its own rules. Body IR, which publishes the defaults' executable representation, takes the slot's declared
    /// result from here instead; no legacy route reads it.
    pub self_method_result_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Declared parameters of the same slots, keyed by the whole call span, so Body IR binds the call's arguments to
    /// the slot's declaration as it binds any checked call; no legacy route reads it.
    pub self_method_params: HashMap<crate::ast::Span, Vec<CallableParam>>,
    /// Compiler-owned requirements inferred for otherwise unresolved calls on rigid declaration parameters. Only
    /// Body IR consumes these span-keyed targets; legacy call identities and explicit bound selections stay intact.
    pub inferred_method_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Checked type arguments of each source `with Trait[...]` adoption clause, keyed by the clause's span.
    ///
    /// Body IR instantiates an adopted trait's default methods for a concrete adopter from these arguments, so no
    /// later stage re-resolves the written clause.
    pub adoption_type_args: HashMap<crate::ast::Span, Vec<ResolvedType>>,
    /// Implicit arguments of bare source adoption clauses, retained only for Body IR by the clause's span. The
    /// existing semantic instantiation rule supplies owner parameter witnesses; legacy's explicit-only snapshot stays
    /// unchanged. Ambiguous or incomplete adoptions contribute no fact.
    pub body_ir_adoption_type_args: HashMap<crate::ast::Span, Vec<ResolvedType>>,
}

impl TraitArtifacts {
    /// Whether a visible trait name resolves to a standard-package protocol: the checker recorded slots for it, and
    /// every one carries such an identity.
    pub fn is_standard_trait(&self, name: &str) -> bool {
        let mut slots = self
            .method_identities
            .iter()
            .filter(|((trait_name, _), _)| trait_name == name)
            .peekable();
        slots.peek().is_some() && slots.all(|(_, identity)| self.is_standard_slot(identity))
    }

    /// Whether a trait slot belongs to a standard-package protocol.
    pub fn is_standard_slot(&self, identity: &CanonicalSymbolId) -> bool {
        self.standard_method_identities.contains(identity)
    }

    /// Retain one visible slot's identity under its trait key, and whether its checked declaration has a default body.
    pub(crate) fn record_method(&mut self, trait_name: &str, method_name: &str, method: &MethodInfo) {
        let Some(identity) = &method.identity else {
            return;
        };
        self.method_identities
            .insert((trait_name.to_owned(), method_name.to_owned()), identity.clone());
        if method.has_body {
            self.default_method_identities.insert(identity.clone());
        }
    }
}

/// Derive expansion metadata imported from dependency modules and manifests.
#[derive(Debug, Default, Clone)]
pub struct DerivationArtifacts {
    /// Exact exported derive macro paths inspected from declared native dependencies. These authorize forwarding
    /// the macro to rustc, never an assumed layout change, generated method, or trait implementation.
    pub native_macro_paths: HashSet<String>,
    /// RFC 024: Trait-level Rust derive paths keyed by module-qualified trait name, such as `yaml.Serialize`.
    ///
    /// The typechecker owns this because dependency modules are already imported and validated there. Lowering should
    /// not re-run module resolution or assume RFC 024 metadata only exists in stdlib source.
    pub trait_rust_derive_paths: HashMap<String, Vec<String>>,
}

/// Transient requirements for one exact mutable empty-list initializer; only successful checked uses can close it.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrEmptyListRequirement {
    /// Source span of the syntactically empty initializer whose element is still unknown.
    pub(crate) initializer: crate::ast::Span,
    /// The declaration's only authorized write; any additional write prevents publication for this initializer.
    pub(crate) declaration_write: crate::ast::Span,
    /// Checked builtin collection name, preserved when the agreed element replaces its hole.
    pub(crate) list_name: String,
    /// Agreed fully closed append element, absent until a validated append supplies one.
    pub(crate) element: Option<ResolvedType>,
    /// A conflict, reassignment or unproven method prevents later appends from reinstating a grant.
    pub(crate) invalidated: bool,
    /// Exact receiver and length-argument reads whose checked contracts do not escape the candidate binding.
    pub(crate) permitted_reads: HashSet<crate::ast::Span>,
}

/// Expression-local resolution facts keyed by source spans.
#[derive(Debug, Default, Clone)]
pub struct ExpressionArtifacts {
    /// Unbounded source call binders awaiting constraints from their exact lexical result values.
    pub(crate) body_ir_open_calls: Vec<BodyIrOpenCall>,
    /// Symbolic results tied to one source call's binder scope, including tuple elements and method results.
    pub(crate) body_ir_open_values: HashMap<crate::ast::Span, BodyIrOpenValue>,
    /// Lexical aliases of a symbolic result; spelling never joins independent factory calls.
    pub(crate) body_ir_open_bindings: HashMap<CanonicalSymbolId, BodyIrOpenValue>,
    /// Candidate empty-list constraints collected during checking and consumed by successful final publication.
    /// Legacy never reads this state; overload transactions preserve it with the rest of TypeCheckInfo.
    pub(crate) body_ir_empty_list_requirements: HashMap<CanonicalSymbolId, BodyIrEmptyListRequirement>,
    /// Body IR-only callable-name reads selected by checking the receiver, keyed by the full field expression span.
    /// Ordinary source fields and metadata on type expressions do not enter this set; legacy member tables stay
    /// intact.
    pub(crate) body_ir_callable_name_reads: std::collections::BTreeSet<crate::ast::Span>,
    /// Executable expression types closed from checked value contracts, consumed only by Body IR. Legacy keeps its
    /// original contextual expression snapshot, including holes that Rust inference completes later.
    pub body_ir_expr_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Closed value contracts for exact checked lexical bindings; shadowed names never share an entry.
    pub body_ir_binding_types: HashMap<CanonicalSymbolId, ResolvedType>,
    /// Checked nominal pattern owners reached through an import alias when the legacy internal-name lookup has no
    /// lexical symbol. Only Body IR consumes these additive facts; missing identities remain unsupported patterns.
    pub body_ir_nominal_pattern_owners: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Agreed types required by validated function calls for exact parameter identities; `None` records a conflict.
    /// Only Body IR's implicit closure contracts consume these requirements; legacy parameter snapshots stay
    /// unchanged.
    pub body_ir_parameter_requirements: HashMap<CanonicalSymbolId, Option<ResolvedType>>,
    /// Resolved expression types keyed by original-source range and compiler-owned expansion context.
    pub expr_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Canonical named-type leaves of each checked expression type, addressed by generic/tuple child indices.
    ///
    /// The checker selects these identities while its accepted type bindings are available. Consumers retain the
    /// structural path and identity; an import alias is never a runtime type or layout key.
    pub expression_type_identities: HashMap<crate::ast::Span, BTreeMap<Vec<usize>, CanonicalSymbolId>>,
    /// Final checked type of an assignment binding, keyed by the assignment statement span.
    ///
    /// This differs from the initializer expression type when contextual numeric typing or a validated coercion
    /// selects the annotated destination type. Body IR consumes this fact instead of reconstructing annotations.
    pub assignment_binding_types: HashMap<crate::ast::Span, ResolvedType>,
    /// RFC 046 computed property reads keyed by the full field-access expression span.
    ///
    /// Lowering/emission can use this to distinguish `obj.field` storage reads from `obj.property` getter calls while
    /// still consuming the same resolved expression type map for the property return type.
    pub computed_property_accesses: HashMap<crate::ast::Span, ComputedPropertyAccessInfo>,
    /// Identifier resolution keyed by the complete checked span (value, type or module).
    ///
    /// This exists so downstream stages (IR lowering/codegen) can reliably distinguish:
    /// - `x.method(...)` where `x` is a value binding, from
    /// - `Type.method(...)` where `Type` is a type name (emits `Type::method(...)` in Rust), and
    /// - imported placeholders (e.g. `from rust::... import Foo`) which are not value bindings.
    pub ident_kinds: HashMap<crate::ast::Span, IdentKind>,
    /// The type of the place a value is written to, keyed by the value span, when the value needs adapting to it: a
    /// field or index assignment, a model or class constructor field, a `return`, a `yield`, or a `match` arm or
    /// `break` value of the construct's type (#1858, RFC 009).
    ///
    /// The checker accepts a value of an `Option`'s payload type there (`box.count = 5` for an `Option[int]` field),
    /// and a numeric value of a type that losslessly widens to the place's numeric type or union member
    /// (`box.count = small` for an `i8` value and an `int` field). Lowering wraps such a value in the `Some` layers
    /// the destination adds and widens it to the numeric type the destination holds.
    pub value_destination_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Explicitly admitted numeric conversions keyed by the complete boundary expression span. Unlike ordinary
    /// lossless destination adaptation, these facts authorize a source conversion that compatibility alone rejects.
    /// Body IR materializes the selected destination through its ordinary numeric assignment machinery.
    pub numeric_conversion_destinations: HashMap<crate::ast::Span, ResolvedType>,
    /// Spans of `dict.get(key)` calls whose result is only read, so no copy of the stored value is needed.
    ///
    /// `get` answers with `Option[V]` on every dict. A lookup whose result feeds a `match` or `if let` that only reads
    /// its bindings (see `check_expr/dict_lookups.rs`) is lowered to read the entry in place; every other lookup of a
    /// dict that is not static storage is completed with a copy of the entry.
    pub read_only_dict_lookups: HashSet<crate::ast::Span>,
    /// Values of chained assignments that each target gets its own evaluation of, keyed by the value span (#1806).
    pub chained_values_written_per_target: HashSet<crate::ast::Span>,
    /// RFC 017 validated-newtype coercion decisions keyed by source expression span.
    ///
    /// Lowering consumes these decisions when an expression is used at an approved implicit-coercion site, such as a
    /// function argument, typed initializer, or model/class field initializer.
    pub validated_newtype_coercions: HashMap<crate::ast::Span, ValidatedNewtypeCoercionInfo>,
    /// Source-level codegraph targets proven during expression checking, keyed by call or reference expression span.
    ///
    /// The codegraph exporter consumes this instead of re-resolving names from syntax. Absence means the target is
    /// unsupported, ambiguous, degraded, or outside the current conservative source target set.
    pub source_targets: HashMap<crate::ast::Span, SourceTargetInfo>,
    /// The method a local partial's target names on a value (`partial user.label(prefix="x")`), keyed by the target
    /// span, by the name its declaration has (after a method alias) (RFC 084).
    ///
    /// Lowering evaluates the target's receiver once, when the partial is built, and calls this method on it.
    pub local_partial_method_targets: HashMap<crate::ast::Span, String>,
}

/// One accepted source call's independent inference scope; unresolved and conflicting groups grant no storage.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrOpenCall {
    pub span: Span,
    pub parameters: Vec<String>,
    pub result: ResolvedType,
    pub bindings: HashMap<String, ResolvedType>,
    /// This complete lexical group has passed explicit destination settlement.
    pub settled: bool,
    pub invalidated: bool,
}

/// A value template whose placeholders belong only to the indexed source call.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrOpenValue {
    pub group: usize,
    pub ty: ResolvedType,
}

/// Source-reference resolution facts keyed by source spans.
#[derive(Debug, Default, Clone)]
pub struct ReferenceArtifacts {
    /// RFC 120 canonical identities of resolved value and type references.
    ///
    /// A local, an import, an alias, and a re-export of one declaration all record the *same* value here, so a
    /// consumer can decide "do these two references mean the same thing" structurally without comparing spellings.
    /// [`ExpressionArtifacts::source_targets`] stays the string-shaped codegraph projection; it is never the
    /// identity. Absence means resolution did not prove an identity for that reference — consumers fail closed
    /// rather than reconstruct one.
    pub resolved_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// RFC 120 identities selected for statement-owned write targets at their exact authored identifier spans.
    ///
    /// Single, tuple-unpack, chained, and compound assignments retain one target span per written identifier. Keying
    /// by that exact span plus the target spelling preserves each target independently without asking Body IR lowering
    /// to repeat lexical lookup after the typechecker has exited the binding's scope. Compiler-generated assignments
    /// use their unique synthetic target spans through the same contract.
    pub resolved_write_identities: HashMap<(crate::ast::Span, String), CanonicalSymbolId>,
    /// Checked type of each statement-owned write target, keyed identically to [`Self::resolved_write_identities`].
    pub resolved_write_types: HashMap<(crate::ast::Span, String), ResolvedType>,
}

/// Const evaluation facts needed by runtime and emission boundaries.
/// Additive Body IR values do not replace legacy's classification or scalar evaluation tables.
#[derive(Debug, Default, Clone)]
pub struct ConstArtifacts {
    /// Exact immutable sized-integer constructor values selected by the const checker, keyed by initializer span.
    pub(crate) body_ir_newtype_values: HashMap<crate::ast::Span, BodyIrNewtypeConstantValue>,
    /// Const category classification (RFC 008): const name -> kind.
    pub const_kinds: HashMap<String, const_eval::ConstKind>,
    /// Computed const values (when available), keyed by const name.
    pub const_values: HashMap<String, ConstValue>,
}

/// A validated const expression's newtype owner and full numeric value, without a backend-selected layout.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrNewtypeConstantValue {
    /// Canonical lexical newtype binding selected during const validation.
    pub owner: CanonicalSymbolId,
    /// Complete checked numeric kind and value; source `int` and noninteger carriers are outside this slice.
    pub value: incan_semantics_core::body_ir::TypedNumericConstant,
}
