//! Declaration records shared by the checker, lowering and compiler artifact adapters.

use crate::body_ir_values::CompilerBlock;
use crate::checked_types::CheckedType;
use crate::compiler_records::{CanonicalSymbolId, HirSourceSpan};
use incan_semantics_core::body_ir::{
    CallableParam, CallableParamDefault, CallableRepresentation, CheckedInputProfile, ClassInheritance,
    ConstantDeclaration, DecoratedBinding, EntrypointEffect, EnumDeclaration, ExplicitInputBinding, FieldMetadata,
    FieldlessEnumDeclaration, LocalDecl, MethodPartialDefaults, NativeAttribute, NativeCallableParameter,
    NativeUnionDeclaration, NewtypeDeserialize, NewtypeStringConversion, PanicFact, PrimitiveTraitAdapter,
    ReturnedView, RuntimeCallableBinding, ScopeInfo, StaticDeclaration, StringConversionContract, TraitImplementation,
    TypeAliasDeclaration, ValueEnumDeclaration,
};
use incan_semantics_core::record_identity::RecordPackageIdentity;
use incan_semantics_core::{AbiV0RuntimeRequirement, CompilerNodeId};

pub const BODY_IR_ADAPTER_PROFILE: &str = "incan.body-ir-records/1";
pub const BODY_IR_ADAPTER_SOURCE: &str = "incan_semantics_core Body IR at executable representation 53";
pub const BODY_IR_ADAPTER_GENERATION: i64 = 1;
pub const BODY_IR_ADAPTER_REMOVAL_GATE: &str = "remove native leaves only after their owning semantic families and every direct consumer accept the authored declarations";

/// One immutable SharedIdentities entry; allocation and pointer identity are deliberately absent.
#[derive(Clone)]
pub struct NamedTypeIdentity {
    pub name: String,
    pub identity: CanonicalSymbolId,
}
crate::incan_debug::incan_debug!(NamedTypeIdentity { name, identity });

/// One checked trait binder and its authored semantic argument.
#[derive(Clone)]
pub struct TraitTypeArgument {
    pub parameter: String,
    pub argument: CheckedType,
}
crate::incan_debug::incan_debug!(TraitTypeArgument { parameter, argument });

/// The selected adoption receiver and trait binders, kept separate from method binders.
#[derive(Clone)]
pub struct TraitDefaultInstantiation {
    pub receiver_type: CheckedType,
    pub arguments: Vec<TraitTypeArgument>,
}
crate::incan_debug::incan_debug!(TraitDefaultInstantiation {
    receiver_type,
    arguments
});

/// One required trait field and the exact adopter field that checking selected.
#[derive(Clone)]
pub struct TraitFieldWitness {
    pub requirement: CanonicalSymbolId,
    pub implementation: CanonicalSymbolId,
}
crate::incan_debug::incan_debug!(TraitFieldWitness {
    requirement,
    implementation
});

/// A checked adoption owned by one concrete adopter.  `methods=None` means coverage is unavailable; `Some([])` proves
/// an authenticated marker trait. Required-field witnesses live here rather than in a trailing side channel.
#[derive(Clone)]
pub struct CompilerTraitAdoption {
    pub adopter: CanonicalSymbolId,
    pub trait_identity: CanonicalSymbolId,
    pub arguments: Vec<CheckedType>,
    pub methods: Option<Vec<CanonicalSymbolId>>,
    pub fields: Vec<TraitFieldWitness>,
    pub primitive_adapter: Option<PrimitiveTraitAdapter>,
}
crate::incan_debug::incan_debug!(CompilerTraitAdoption {
    adopter,
    trait_identity,
    arguments,
    methods,
    fields,
    primitive_adapter
});

/// The exact derived constructor, validator and ordered required-field slots for one checked model.
#[derive(Clone)]
pub struct CompilerValidatedConstructor {
    pub constructor: CanonicalSymbolId,
    pub validator: CanonicalSymbolId,
    pub required_fields: Vec<i64>,
}
crate::incan_debug::incan_debug!(CompilerValidatedConstructor {
    constructor,
    validator,
    required_fields
});

/// A complete selected Body contract with all 24 live fields and explicit dispositions for the three skipped facts.
/// The retired facade field is retained only for validation: a record with a nonempty tombstone is refused and no
/// consumer may interpret it. Shared identities are copied as entries rather than sharing the native Arc map.
#[derive(Clone)]
pub struct CompilerBody {
    pub nominal_instantiation: Vec<TraitTypeArgument>,
    pub trait_default_instantiation: Option<TraitDefaultInstantiation>,
    pub type_parameters: Vec<String>,
    pub declaration_id: CompilerNodeId,
    pub direct_call_id: CompilerNodeId,
    pub canonical: Option<CanonicalSymbolId>,
    pub name: String,
    pub span: HirSourceSpan,
    pub return_type: CheckedType,
    pub callable_representation: Option<CallableRepresentation>,
    pub checked_input_profile: Option<CheckedInputProfile>,
    pub explicit_input_bindings: Vec<ExplicitInputBinding>,
    pub returned_view: Option<ReturnedView>,
    pub named_type_identities: Vec<NamedTypeIdentity>,
    pub locals: Vec<LocalDecl>,
    pub params: Vec<CallableParam>,
    pub param_locals: Vec<i64>,
    pub scopes: Vec<ScopeInfo>,
    pub block: CompilerBlock,
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    pub panic_facts: Vec<PanicFact>,
    pub is_async: bool,
    pub retired_rust_facade_wire: Option<(String, String)>,
    pub native_attributes: Vec<NativeAttribute>,
}
crate::incan_debug::incan_debug!(CompilerBody {
    nominal_instantiation,
    trait_default_instantiation,
    type_parameters,
    declaration_id,
    direct_call_id,
    canonical,
    name,
    span,
    return_type,
    callable_representation,
    checked_input_profile,
    explicit_input_bindings,
    returned_view,
    named_type_identities,
    locals,
    params,
    param_locals,
    scopes,
    block,
    runtime_requirements,
    panic_facts,
    is_async,
    retired_rust_facade_wire,
    native_attributes
});

/// All 29 live NominalDeclaration fields, including validation, adoption and owned-callable facts.
#[derive(Clone)]
pub struct CompilerNominalDeclaration {
    pub trait_adoptions: Vec<CompilerTraitAdoption>,
    pub native_item: String,
    pub class_inheritance: Option<ClassInheritance>,
    pub native_type_arguments: Vec<CheckedType>,
    pub direct_declaration_id: CompilerNodeId,
    pub canonical: CanonicalSymbolId,
    pub name: String,
    pub fields: Vec<String>,
    pub field_identities: Vec<CanonicalSymbolId>,
    pub field_types: Vec<CheckedType>,
    pub owned_callable_fields: Vec<i64>,
    pub field_public: Vec<bool>,
    pub field_aliases: Vec<Option<String>>,
    pub reflection_fields: Option<Vec<i64>>,
    pub reflection_metadata: Option<Vec<FieldMetadata>>,
    pub comparison_equality: Option<CanonicalSymbolId>,
    pub public: bool,
    pub has_field_defaults: bool,
    pub field_default_body: Vec<CompilerBody>,
    pub validated_constructor: Option<CompilerValidatedConstructor>,
    pub method_partial_defaults: Vec<MethodPartialDefaults>,
    pub display_method: Option<CanonicalSymbolId>,
    pub newtype_deserialize: NewtypeDeserialize,
    pub newtype_string_conversion: NewtypeStringConversion,
    pub derives: Vec<String>,
    pub named_type_identities: Vec<NamedTypeIdentity>,
    pub type_parameters: Vec<String>,
    pub type_parameter_count: i64,
    pub record_package_identity: Option<RecordPackageIdentity>,
}
crate::incan_debug::incan_debug!(CompilerNominalDeclaration {
    trait_adoptions,
    native_item,
    class_inheritance,
    native_type_arguments,
    direct_declaration_id,
    canonical,
    name,
    fields,
    field_identities,
    field_types,
    owned_callable_fields,
    field_public,
    field_aliases,
    reflection_fields,
    reflection_metadata,
    comparison_equality,
    public,
    has_field_defaults,
    field_default_body,
    validated_constructor,
    method_partial_defaults,
    display_method,
    newtype_deserialize,
    newtype_string_conversion,
    derives,
    named_type_identities,
    type_parameters,
    type_parameter_count,
    record_package_identity
});

/// All nine live LoafCallable fields, with an authenticated declaration-only frame rather than a second body.
#[derive(Clone)]
pub struct CompilerLoafCallable {
    pub canonical: CanonicalSymbolId,
    pub item: String,
    pub parameters: Vec<CheckedType>,
    pub return_type: CheckedType,
    pub type_parameters: Vec<String>,
    pub native_type_arguments: Vec<CheckedType>,
    pub native_callable_parameters: Vec<NativeCallableParameter>,
    pub parameter_defaults: Vec<CallableParamDefault>,
    pub declaration_frame: Vec<CompilerBody>,
}
crate::incan_debug::incan_debug!(CompilerLoafCallable {
    canonical,
    item,
    parameters,
    return_type,
    type_parameters,
    native_type_arguments,
    native_callable_parameters,
    parameter_defaults,
    declaration_frame
});

/// Declaration coverage that keeps a covered empty body set distinct from an uncovered declaration.
#[derive(Clone, Debug)]
pub enum CompilerBodyCoverage {
    Uncovered(String),
    Covered(Vec<CompilerBody>),
}

/// Public roots and their private implementation dependencies remain distinct declaration inventories.
#[derive(Clone)]
pub struct CompilerDeclarationSet {
    pub public_roots: Vec<CanonicalSymbolId>,
    pub private_implementations: Vec<CompilerBody>,
}
crate::incan_debug::incan_debug!(CompilerDeclarationSet {
    public_roots,
    private_implementations
});

/// All live BodyIrModule fields, including invocation-only facts that publication must not import.
/// `runtime_binding_scopes` defaults to empty.
#[derive(Clone)]
pub struct CompilerBodyIrModule {
    pub runtime_callable_bindings: Vec<RuntimeCallableBinding>,
    pub decorated_bindings: Vec<DecoratedBinding>,
    pub entrypoint_effects: Vec<EntrypointEffect>,
    pub program_entrypoints: Vec<CanonicalSymbolId>,
    pub string_conversion_contract: StringConversionContract,
    pub constant_declarations: Vec<ConstantDeclaration>,
    pub type_alias_declarations: Vec<TypeAliasDeclaration>,
    pub enum_declarations: Vec<EnumDeclaration>,
    pub loaf_callables: Vec<CompilerLoafCallable>,
    pub native_union_declarations: Vec<NativeUnionDeclaration>,
    pub static_declarations: Vec<StaticDeclaration>,
    pub module_id: CompilerNodeId,
    pub nominal_declarations: Vec<CompilerNominalDeclaration>,
    pub fieldless_enum_declarations: Vec<FieldlessEnumDeclaration>,
    pub value_enum_declarations: Vec<ValueEnumDeclaration>,
    pub trait_declarations: Vec<CanonicalSymbolId>,
    pub trait_implementations: Vec<TraitImplementation>,
    pub bodies: Vec<CompilerBody>,
    pub runtime_binding_scopes: Vec<(String, Vec<String>)>,
}
crate::incan_debug::incan_debug!(CompilerBodyIrModule {
    runtime_callable_bindings,
    decorated_bindings,
    entrypoint_effects,
    program_entrypoints,
    string_conversion_contract,
    constant_declarations,
    type_alias_declarations,
    enum_declarations,
    loaf_callables,
    native_union_declarations,
    static_declarations,
    module_id,
    nominal_declarations,
    fieldless_enum_declarations,
    value_enum_declarations,
    trait_declarations,
    trait_implementations,
    bodies,
    runtime_binding_scopes
});

/// A retired facade payload makes the body inadmissible.
pub fn valid_compiler_body(body: &CompilerBody) -> bool {
    body.retired_rust_facade_wire.is_none()
}

/// The recursive optional-body representation admits at most one element.
pub fn valid_optional_body(bodies: &[CompilerBody]) -> bool {
    bodies.len() <= 1
}

/// Validate aligned field tables, ordered constructor slots and ownership of each adoption.
pub fn valid_nominal_declaration(decl: &CompilerNominalDeclaration) -> bool {
    let count = decl.fields.len();
    let in_range = |slot: &i64| usize::try_from(*slot).is_ok_and(|index| index < count);
    decl.field_identities.len() == count
        && decl.field_types.len() == count
        && decl.field_public.len() == count
        && decl.field_aliases.len() == count
        && valid_optional_body(&decl.field_default_body)
        && (!decl.has_field_defaults || decl.field_default_body.len() == 1)
        && decl.owned_callable_fields.iter().all(in_range)
        && decl.validated_constructor.as_ref().is_none_or(|c| {
            c.required_fields.iter().all(in_range) && c.required_fields.windows(2).all(|pair| pair[0] < pair[1])
        })
        && decl.trait_adoptions.iter().all(|a| a.adopter == decl.canonical)
}

/// A declaration-only frame has the selected callable identity and an empty statement block.
pub fn valid_loaf_callable(callable: &CompilerLoafCallable) -> bool {
    callable.parameter_defaults.len() == callable.parameters.len()
        && match callable.declaration_frame.as_slice() {
            [] => true,
            [frame] => frame.canonical.as_ref() == Some(&callable.canonical) && frame.block.statements.is_empty(),
            _ => false,
        }
}

/// Preserve covered-empty as a zero count and uncovered as absence.
pub fn covered_body_count(coverage: &CompilerBodyCoverage) -> Option<i64> {
    match coverage {
        CompilerBodyCoverage::Uncovered(_) => None,
        CompilerBodyCoverage::Covered(bodies) => i64::try_from(bodies.len()).ok(),
    }
}

/// Invocation-only facts are present if any binding inventory is nonempty.
pub fn valid_invocation_view(module: &CompilerBodyIrModule) -> bool {
    !module.runtime_callable_bindings.is_empty()
        || !module.runtime_binding_scopes.is_empty()
        || !module.program_entrypoints.is_empty()
}

/// A published expanded body cannot carry native macro inputs or a retired facade payload.
pub fn valid_published_expanded_body(body: &CompilerBody) -> bool {
    body.native_attributes.is_empty() && valid_compiler_body(body)
}

/// Clear the caller's derived indexes before field type substitution. The returned declaration owns its fields.
pub fn invalidate_owned_callable_fields(decl: &mut CompilerNominalDeclaration) -> CompilerNominalDeclaration {
    decl.owned_callable_fields.clear();
    // The Incan mut parameter exposes the change to the caller and also returns an independent owned record.
    decl.clone()
}
