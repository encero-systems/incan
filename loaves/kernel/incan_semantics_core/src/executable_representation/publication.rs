//! Producer-side execution-closure audit over checked Body IR. No source is checked or resolved here.

use std::collections::{BTreeMap, BTreeSet};

use crate::body_ir::{
    AggregateKind, ArgumentElement, AssertionKind, Body, BodyIrModule, CallableParam, CallableParamDefault,
    CallableTarget, Callee, DictEntry, FieldlessEnumVariantTarget, FormatPart, MethodTarget, Operand, Pattern, Place,
    PlaceElem, PlaceRoot, Rvalue, Statement, StatementKind, TryErrorRouting,
};
use crate::{CanonicalSymbolId, CompilerNodeId, IncanType, SymbolOrigin};

use super::{CoverageReason, ExecutableDeclaration};
use crate::canonical_module_identity;

mod audit;
#[cfg(test)]
mod tests;

/// One projected body together with the canonical identities its execution requires.
///
/// The body is the executable projection; requirements select already-checked dependencies without widening the
/// finalized public import surface or reconstructing unresolved source declarations.
pub(super) struct ProjectedBody {
    pub body: Body,
    pub requirements: BTreeSet<CanonicalSymbolId>,
    /// Exact native call selections retained independently of package declaration requirements.
    pub native_calls: Vec<(CanonicalSymbolId, Vec<IncanType>)>,
}

/// Publish one retained immutable value and its exact layout requirements without interpreting source expressions.
pub(super) fn project_constant(
    source: &crate::body_ir::ConstantDeclaration,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(crate::body_ir::ConstantDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    let mut value = source.clone();
    let named_types = BTreeMap::new();
    let mut audit = PublicationAudit::new(module, library, eligible, &named_types);
    audit.identity(&mut value.canonical, false)?;
    audit.constant(&mut value.value)?;
    Ok((value, audit.requirements))
}

/// Select a governed body or complete checked decorator replacement under one public identity. Compiled exports
/// reach `project_compiled_frame` directly at the owning publication boundary.
pub(super) fn project_callable(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
    native_callables: &[&crate::body_ir::LoafCallable],
) -> Result<(ExecutableDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    if let Some(binding) = module.decorated_bindings.iter().find(|binding| {
        binding.original_body.canonical == source.canonical
            && binding.initialization == crate::body_ir::DecoratedInitialization::PerInstantiation
    }) {
        return project_generic_decorated_body(source, binding, module, library, eligible, native_callables);
    }
    let decorated = module
        .static_declarations
        .iter()
        .any(|storage| Some(&storage.canonical) == source.canonical.as_ref());
    if decorated {
        let value = project_decorated_body(source, module, library, eligible)?;
        return Ok((
            ExecutableDeclaration::DecoratedBody {
                body: Box::new(value.wrapper),
                binding: Box::new(value.binding),
                storage: Box::new(value.storage),
            },
            value.requirements,
        ));
    }
    let value = project_body(source, module, library, eligible)?;
    let callables = selected_native_calls(&value.native_calls, native_callables)?;
    let declaration = if callables.is_empty() {
        ExecutableDeclaration::Body(Box::new(value.body))
    } else {
        ExecutableDeclaration::NativeBody {
            body: Box::new(value.body),
            callables,
        }
    };
    Ok((declaration, value.requirements))
}

/// Publish a generic replacement atomically with its original, initializer and exact native call dependencies.
/// The initializer remains a template under the source binders; consumers close all frames through the ordinary
/// per-instantiation binding policy rather than publishing a polymorphic module cell.
fn project_generic_decorated_body(
    source: &Body,
    binding: &crate::body_ir::DecoratedBinding,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
    native_callables: &[&crate::body_ir::LoafCallable],
) -> Result<(ExecutableDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    if module
        .decorated_bindings
        .iter()
        .filter(|candidate| candidate.original_body.canonical == source.canonical)
        .count()
        != 1
        || !is_well_formed_source_binding(module, binding, library)
    {
        return Err(CoverageReason::UnresolvedReference);
    }
    let initializer = binding
        .initializer
        .as_ref()
        .ok_or(CoverageReason::UnresolvedReference)?;
    let wrapper = project_body(source, module, library, eligible)?;
    let original = project_body(&binding.original_body, module, library, eligible)?;
    let initializer = project_body(initializer, module, library, eligible)?;
    let mut requirements = wrapper.requirements;
    requirements.extend(original.requirements);
    requirements.extend(initializer.requirements);
    let mut native_calls = wrapper.native_calls;
    native_calls.extend(original.native_calls);
    native_calls.extend(initializer.native_calls);
    let callables = selected_native_calls(&native_calls, native_callables)?;
    Ok((
        ExecutableDeclaration::GenericDecoratedBody {
            body: Box::new(wrapper.body),
            binding: Box::new(crate::body_ir::DecoratedBinding {
                original_body: original.body,
                initializer: Some(Box::new(initializer.body)),
                initialization: crate::body_ir::DecoratedInitialization::PerInstantiation,
            }),
            callables,
        },
        requirements,
    ))
}

/// Publish only a compiled export's governed signature, independently of whether its implementation is portable.
/// Parameter passing and default computations retain their real locals; unused implementation slots are inert unit
/// placeholders so declaration-local addresses remain stable without publishing private implementation types.
/// A conditional profile publishes like a straight-line one: the frame's signature is published, not its statements,
/// so the initializer transfers its compiled body performed are not published either.
pub(super) fn project_compiled_frame(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(ExecutableDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    if source.input_profile_gap().is_some() {
        return Err(CoverageReason::UnsupportedConstruct);
    }
    let mut frame = source.clone();
    frame.block.stmts.clear();
    frame.panic_facts.clear();
    let mut audit = PublicationAudit::new(module, library, eligible, &source.named_type_identities);
    let identity = frame.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
    audit.identity(identity, false)?;
    if let Some(profile) = &mut frame.checked_input_profile {
        audit.identity(&mut profile.owner, false)?;
        profile.owned_initializers.clear();
    }
    frame.decl_id = declaration_id(identity)?;
    frame.direct_call_id = frame.decl_id.clone();
    audit.ty(&frame.return_type)?;
    audit.parameters(&mut frame.params)?;
    let retained_slots = audit
        .local_slots
        .iter()
        .next_back()
        .map_or(0, |local| local.index() + 1);
    frame.locals.truncate(retained_slots);
    // Explicit binding obligations stay; their session-local checker identities do not.
    for binding in &mut frame.explicit_input_bindings {
        binding.identity = None;
    }
    for local in &mut frame.locals {
        local.identity = None;
        if audit.local_slots.contains(&local.id) {
            audit.ty(&local.ty)?;
        } else {
            // Keep slot indices stable for defaults without publishing implementation-only carriers.
            local.ty = IncanType::Tuple(Vec::new());
        }
    }
    frame.runtime_requirements.clear();
    frame.extern_delegation = None;
    frame.named_type_identities = std::sync::Arc::new(audit.used_nominal_types);
    require_module_initialization(module, &mut audit.requirements);
    let native_callables = module.loaf_callables.iter().collect::<Vec<_>>();
    let callables = selected_native_calls(&audit.native_calls, &native_callables)?;
    Ok((
        ExecutableDeclaration::CompiledCallableFrame {
            frame: Box::new(frame),
            callables,
        },
        audit.requirements,
    ))
}

/// Recover checker-authenticated native selections for executable bodies and declaration defaults. Open argument
/// spellings can belong to different generic frames, so preserve every checked signature sharing that selection;
/// the closed consumer selects against its own checked result rather than losing a frame to first-match ordering.
/// Borrowed structured keys index the inventory once; duplicate records and repeated selections retain their first
/// publication position without cloning identities or type arguments for lookup.
fn selected_native_calls(
    selections: &[(CanonicalSymbolId, Vec<IncanType>)],
    native_callables: &[&crate::body_ir::LoafCallable],
) -> Result<Vec<crate::body_ir::LoafCallable>, CoverageReason> {
    if selections.is_empty() {
        return Ok(Vec::new());
    }

    // ---- Inventory: retain all distinct signatures under their checked selection ----
    let requested = selections
        .iter()
        .map(|(identity, arguments)| (identity, arguments.as_slice()))
        .collect::<BTreeSet<_>>();
    let mut indexed: BTreeMap<_, Vec<(usize, &crate::body_ir::LoafCallable)>> = BTreeMap::new();
    for (position, callable) in native_callables.iter().copied().enumerate() {
        let selection = (&callable.canonical, callable.native_type_arguments.as_slice());
        if !requested.contains(&selection) {
            continue;
        }
        let candidates = indexed.entry(selection).or_default();
        if !candidates.iter().any(|(_, retained)| *retained == callable) {
            candidates.push((position, callable));
        }
    }

    // ---- Selection: preserve request order and the inventory order of each matching frame ----
    let mut callables = Vec::new();
    let mut emitted = BTreeSet::new();
    for (identity, arguments) in selections {
        let candidates = indexed
            .get(&(identity, arguments.as_slice()))
            .ok_or(CoverageReason::UnresolvedReference)?;
        for (position, callable) in candidates {
            if emitted.insert(*position) {
                callables.push((*callable).clone());
            }
        }
    }
    Ok(callables)
}

/// Bind a declaration's physical fragment address to the already-published canonical owner and span.
pub(super) fn declaration_id(identity: &CanonicalSymbolId) -> Result<CompilerNodeId, CoverageReason> {
    let module = canonical_module_identity(identity).ok_or(CoverageReason::UnresolvedReference)?;
    Ok(CompilerNodeId::declaration_at(&module, identity.declaration_span))
}

/// Project one eligible body onto package-owned addresses and audit every retained computation and type.
///
/// Local slots and scope ordinals are fragment-local, not compiler-session symbol IDs. Their optional checker
/// identities are removed because execution uses the slots and a local identity may contain a session discriminant.
/// Declaration references keep the canonical identity minted by the declaring compilation.
/// Generated generic metadata retains its authenticated binder ancestry inside this fragment; it is never an
/// independently addressable package declaration.
/// A nominal's actual retained default frame borrows that owner's binders; its own callable binder list stays empty.
/// Conditional cleanup profiles require a separate canonical-storage publication contract before these local
/// identities can be erased. Keep that refusal independent of source execution admission.
pub(super) fn project_body(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<ProjectedBody, CoverageReason> {
    if source.input_profile_gap().is_some()
        || source
            .checked_input_profile
            .as_ref()
            .is_some_and(|profile| profile.version != 1)
    {
        return Err(CoverageReason::UnsupportedConstruct);
    }
    let mut body = source.clone();
    let mut audit = PublicationAudit::new(module, library, eligible, &source.named_type_identities);
    audit.current_callable = source.canonical.clone();
    audit.trait_self = source.locals.first().is_some_and(|local| {
        local.ty == IncanType::SelfType && matches!(local.origin, crate::body_ir::LocalOrigin::Receiver { .. })
    });
    audit.type_parameters.extend(source.type_parameters.iter().cloned());
    if let Some(member) = &source.canonical {
        for owner in &module.nominal_declarations {
            if encloses(&owner.canonical, member) || owns_default_frame(owner, source) {
                audit.type_parameters.extend(owner.type_parameters.iter().cloned());
            }
        }
        for owner in &module.trait_declarations {
            if encloses(owner, member) {
                let mut owner = owner.clone();
                audit.identity(&mut owner, true)?;
            }
        }
    }
    let canonical = body.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
    audit.identity(canonical, false)?;
    if let Some(profile) = &mut body.checked_input_profile {
        audit.identity(&mut profile.owner, false)?;
    }
    body.decl_id = declaration_id(canonical)?;
    body.direct_call_id = body.decl_id.clone();
    audit.ty(&body.return_type)?;
    for local in &mut body.locals {
        local.identity = None;
        audit.ty(&local.ty)?;
    }
    for binding in &mut body.explicit_input_bindings {
        binding.identity = None;
    }
    audit.parameters(&mut body.params)?;
    audit.statements(&mut body.block.stmts)?;
    body.named_type_identities = std::sync::Arc::new(audit.used_nominal_types);
    require_module_initialization(module, &mut audit.requirements);
    Ok(ProjectedBody {
        body,
        requirements: audit.requirements,
        native_calls: audit.native_calls,
    })
}

/// Audit a checked plain-model layout with the same type/reference rules used for executable bodies.
///
/// Every field, including private storage, retains its checked identity and type in the required execution layout.
/// A generic layout also retains its own template binding when none of its field types names the owner.
pub(super) fn project_nominal(
    source: &crate::body_ir::NominalDeclaration,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(crate::body_ir::NominalDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    // Local inheritance is executable only with the validated full source module; package closure is not admitted.
    if source.class_inheritance.is_some() {
        return Err(CoverageReason::UnsupportedConstruct);
    }
    if source.fields.len() != source.field_types.len() || source.fields.len() != source.field_identities.len() {
        return Err(CoverageReason::UnresolvedReference);
    }
    if !source.field_identities.iter().all(|field| eligible.contains(field)) {
        return Err(CoverageReason::UnresolvedReference);
    }
    let mut nominal = source.clone();
    // Automatic conversion is invocation-local and cannot become a published package obligation.
    nominal.newtype_string_conversion = crate::body_ir::NewtypeStringConversion::Unselected;
    let mut audit = PublicationAudit::new(module, library, eligible, &source.named_type_identities);
    audit.type_parameters.extend(source.type_parameters.iter().cloned());
    audit.identity(&mut nominal.canonical, false)?;
    nominal.direct_declaration_id = declaration_id(&nominal.canonical)?;
    for ty in &nominal.field_types {
        audit.ty(ty)?;
    }
    nominal.trait_adoptions.clear();
    for source_adoption in &source.trait_adoptions {
        let mut adoption = source_adoption.clone();
        audit.identity(&mut adoption.trait_identity, false)?;
        for argument in &adoption.arguments {
            audit.ty(argument)?;
        }
        if let Some(methods) = &mut adoption.methods {
            for method in methods {
                audit.identity(method, false)?;
            }
        }
        if let Some(adapter) = &mut adoption.primitive_adapter {
            audit.ty(&adapter.receiver_type)?;
            audit.identity(&mut adapter.payload_field, false)?;
            audit.identity(&mut adapter.decode_slot, false)?;
            audit.identity(&mut adapter.decoded_value, true)?;
        }
        nominal.trait_adoptions.push(adoption);
    }
    if let Some(method) = &mut nominal.display_method {
        audit.identity(method, true)?;
    }
    if let Some(defaults) = &source.field_default_body {
        let projected = project_body(defaults, module, library, eligible)?;
        nominal.field_default_body = Some(Box::new(projected.body));
        audit.requirements.extend(projected.requirements);
    }
    for binding in &mut nominal.method_partial_defaults {
        audit.identity(&mut binding.target, true)?;
        let projected = project_body(&binding.frame, module, library, eligible)?;
        *binding.frame = projected.body;
        audit.requirements.extend(projected.requirements);
    }
    let carrier = if source.type_parameters.is_empty() {
        source.carrier_name()
    } else {
        source.template_carrier_name()
    };
    if carrier != source.name {
        if crate::body_ir::nominal_carrier_key(&nominal.canonical).as_ref() != Some(&carrier) {
            return Err(CoverageReason::UnresolvedReference);
        }
        audit.used_nominal_types.insert(carrier, nominal.canonical.clone());
    }
    nominal.named_type_identities = audit.used_nominal_types;
    Ok((nominal, audit.requirements))
}

/// Project a checked normal enum, retaining canonical variant addresses and auditing every payload type.
pub(super) fn project_enum(
    source: &crate::body_ir::EnumDeclaration,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(crate::body_ir::EnumDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    let mut value = source.clone();
    let mut audit = PublicationAudit::new(module, library, eligible, &source.named_type_identities);
    audit.type_parameters.extend(source.type_parameters.iter().cloned());
    audit.identity(&mut value.canonical, false)?;
    value.direct_declaration_id = declaration_id(&value.canonical)?;
    for variant in &mut value.variants {
        audit.identity(&mut variant.canonical, false)?;
        variant.direct_declaration_id = declaration_id(&variant.canonical)?;
        for ty in &variant.fields {
            audit.ty(ty)?;
        }
    }
    value.named_type_identities = audit.used_nominal_types;
    Ok((value, audit.requirements))
}

/// Project literal or computed storage, including read-only constants, under its checked canonical identity.
/// Computed initialization receives the same portable body audit as a callable; its dependencies remain required.
pub(super) fn project_static(
    source: &crate::body_ir::StaticDeclaration,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(crate::body_ir::StaticDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    if source.canonical.namespace != crate::SymbolNamespace::OrdinaryLexical
        || !matches!(
            source.canonical.kind,
            crate::SemanticSourceTargetKind::Static | crate::SemanticSourceTargetKind::Const
        )
        || !module.is_well_formed_static_declaration(source)
    {
        return Err(CoverageReason::UnresolvedReference);
    }
    let mut value = source.clone();
    let checked_types = match &source.initial {
        crate::body_ir::StaticInitializer::Computed(body) => (*body.named_type_identities).clone(),
        _ => BTreeMap::new(),
    };
    let mut audit = PublicationAudit::new(module, library, eligible, &checked_types);
    audit.identity(&mut value.canonical, false)?;
    audit.ty(&value.ty)?;
    if let crate::body_ir::StaticInitializer::Computed(body) = &source.initial {
        let projected = project_body(body, module, library, eligible)?;
        value.initial = crate::body_ir::StaticInitializer::Computed(Box::new(projected.body));
        audit.requirements.extend(projected.requirements);
    }
    Ok((value, audit.requirements))
}

/// Audit a checked erased alias and retain only the canonical nominal context its expanded target requires.
pub(super) fn project_type_alias(
    source: &crate::body_ir::TypeAliasDeclaration,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(crate::body_ir::TypeAliasDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    let mut value = source.clone();
    let mut audit = PublicationAudit::new(module, library, eligible, &source.named_type_identities);
    audit.identity(&mut value.canonical, false)?;
    audit.ty(&value.ty)?;
    value.named_type_identities = audit.used_nominal_types;
    Ok((value, audit.requirements))
}

/// Authenticate an already checked generated member through one exact retained value-enum owner. The member is
/// addressed through its owner's layout; this does not invent a callable body or admit private source methods.
pub(super) fn generated_value_enum_member_owner(
    identity: &CanonicalSymbolId,
    declarations: &[crate::body_ir::ValueEnumDeclaration],
    library: &str,
) -> Option<CanonicalSymbolId> {
    let mut owners = declarations.iter().filter_map(|declaration| {
        let mut owner = declaration.canonical.clone();
        if let SymbolOrigin::Module(path) = &owner.origin {
            owner.origin = SymbolOrigin::Package {
                library: library.to_owned(),
                module_path: path.clone(),
            };
        }
        (owner.declaration_name == declaration.name && is_generated_value_enum_member(identity, &owner))
            .then_some(owner)
    });
    let owner = owners.next()?;
    owners.next().is_none().then_some(owner)
}

/// Authenticate generated scalar-enum members by their exact declaring identity at publication and index validation.
/// A matching spelling alone never grants an enum's executable layout to an ordinary source method.
pub(super) fn is_generated_value_enum_member(identity: &CanonicalSymbolId, owner: &CanonicalSymbolId) -> bool {
    identity.namespace == crate::SymbolNamespace::Member
        && identity.kind == crate::SemanticSourceTargetKind::Method
        && matches!(identity.declaration_name.as_str(), "value" | "from_value")
        && owner.namespace == crate::SymbolNamespace::OrdinaryLexical
        && owner.kind == crate::SemanticSourceTargetKind::Enum
        && owner.declaration_span.end > owner.declaration_span.start
        && owner.origin == identity.origin
        && owner.scope_discriminant == identity.scope_discriminant
        && owner.declaration_span == identity.declaration_span
}

/// Require the same canonical origin and enclosing declaration span before borrowing an owner's binders.
fn encloses(owner: &CanonicalSymbolId, member: &CanonicalSymbolId) -> bool {
    owner != member
        && owner.origin == member.origin
        && owner.declaration_span.start <= member.declaration_span.start
        && member.declaration_span.end <= owner.declaration_span.end
}

/// Bind a generated metadata operation to a declared parameter of this callable or its checked nominal owner.
/// Matching source spans and expansion ancestry authenticate the binder; a same-spelled method or a binder from
/// another frame cannot gain reflection publication through a scoped identity alone.
fn generic_metadata_belongs_to(
    target: &MethodTarget,
    callable: &CanonicalSymbolId,
    parameters: &BTreeSet<String>,
    nominals: &[crate::body_ir::NominalDeclaration],
) -> bool {
    if !target.has_valid_generic_metadata() {
        return false;
    }
    let Some(crate::body_ir::ReflectionOperation::GenericMetadata(binder)) = &target.reflection else {
        return false;
    };
    if !parameters.contains(&binder.declaration_name) {
        return false;
    }
    let contains_binder = |owner: &CanonicalSymbolId| {
        encloses(owner, binder) && owner.declaration_span.expansion == binder.declaration_span.expansion
    };
    contains_binder(callable)
        || nominals.iter().any(|owner| {
            encloses(&owner.canonical, callable)
                && owner.type_parameters.contains(&binder.declaration_name)
                && contains_binder(&owner.canonical)
        })
}

/// Borrow nominal binders only for the actual retained constructor default frame, whose identity is the owner's
/// rather than an enclosed member's. A matching name or span alone cannot grant this declaration context.
fn owns_default_frame(owner: &crate::body_ir::NominalDeclaration, frame: &Body) -> bool {
    frame.canonical.as_ref() == Some(&owner.canonical) && owner.field_default_body.as_deref() == Some(frame)
}

/// One exhaustive pass owns publication's type/reference checks and artifact-local address projection.
struct PublicationAudit<'a> {
    /// Validated declarations whose explicit original-callable reads can retain their source implementation.
    original_callables: BTreeSet<CanonicalSymbolId>,
    /// The audited source declaration before package projection; original reads must belong to this exact frame.
    current_callable: Option<CanonicalSymbolId>,
    /// Validated bindings authorize replacement reads; the binding retains its module or per-instance lifetime.
    replacement_bindings: BTreeSet<CanonicalSymbolId>,
    /// Trait-default receiver placeholders remain abstract until concrete checked dispatch specialization.
    trait_self: bool,
    /// Only binders retained by the declaration or its canonical nominal owner may remain open in publication.
    type_parameters: BTreeSet<String>,
    library: &'a str,
    eligible: &'a BTreeSet<CanonicalSymbolId>,
    nominal_types: BTreeMap<String, CanonicalSymbolId>,
    checked_nominal_types: &'a BTreeMap<String, CanonicalSymbolId>,
    /// Checked raw-value enum layouts authenticate compiler-generated getters without widening private publication.
    value_enum_declarations: &'a [crate::body_ir::ValueEnumDeclaration],
    nominal_declarations: &'a [crate::body_ir::NominalDeclaration],
    /// Metadata-selected native storage uses the same item inventory as executable layout planning.
    native_types: BTreeMap<String, CanonicalSymbolId>,
    used_nominal_types: BTreeMap<String, CanonicalSymbolId>,
    requirements: BTreeSet<CanonicalSymbolId>,
    /// Native references need their checker-produced callable records rather than an Incan fragment.
    native_calls: Vec<(CanonicalSymbolId, Vec<IncanType>)>,
    /// Local storage referenced by retained parameter defaults, independent of implementation-only locals.
    local_slots: BTreeSet<crate::body_ir::LocalId>,
}
/// A decorated declaration retains all three runtime roles and one checked execution dependency set.
pub(super) struct ProjectedDecoratedBody {
    pub wrapper: Body,
    pub binding: crate::body_ir::DecoratedBinding,
    pub storage: crate::body_ir::StaticDeclaration,
    pub requirements: BTreeSet<CanonicalSymbolId>,
}

/// Retain the visible wrapper, original callable and checked replacement as one indivisible execution fragment.
/// The caller publishes this only under the visible canonical identity; the original never becomes a public root.
pub(super) fn project_decorated_body(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<ProjectedDecoratedBody, CoverageReason> {
    let identity = source.canonical.as_ref().ok_or(CoverageReason::UnresolvedReference)?;
    let mut bindings = module
        .decorated_bindings
        .iter()
        .filter(|binding| binding.original_body.canonical.as_ref() == Some(identity));
    let binding = bindings.next().ok_or(CoverageReason::UnresolvedReference)?;
    if bindings.next().is_some() || !is_well_formed_source_binding(module, binding, library) {
        return Err(CoverageReason::UnresolvedReference);
    }
    let storage = module
        .static_declarations
        .iter()
        .find(|storage| &storage.canonical == identity)
        .ok_or(CoverageReason::UnresolvedReference)?;
    let crate::body_ir::StaticInitializer::Computed(initializer) = &storage.initial else {
        return Err(CoverageReason::UnresolvedReference);
    };
    let wrapper = project_body(source, module, library, eligible)?;
    let original = project_body(&binding.original_body, module, library, eligible)?;
    let replacement = project_body(initializer, module, library, eligible)?;
    let mut storage = storage.clone();
    storage.canonical = wrapper
        .body
        .canonical
        .clone()
        .ok_or(CoverageReason::UnresolvedReference)?;
    storage.initial = crate::body_ir::StaticInitializer::Computed(Box::new(replacement.body));
    let mut requirements = wrapper.requirements;
    requirements.extend(original.requirements);
    requirements.extend(replacement.requirements);
    Ok(ProjectedDecoratedBody {
        wrapper: wrapper.body,
        binding: crate::body_ir::DecoratedBinding {
            original_body: original.body,
            initializer: None,
            initialization: crate::body_ir::DecoratedInitialization::ModuleStorage,
        },
        storage,
        requirements,
    })
}

/// Every entry into a declaring module must preserve its checked computed initialization, including private effects.
/// Satisfiability subsequently refuses the caller if any required initializer cannot be represented.
fn require_module_initialization(module: &BodyIrModule, requirements: &mut BTreeSet<CanonicalSymbolId>) {
    requirements.extend(
        module
            .static_declarations
            .iter()
            .filter(|storage| matches!(storage.initial, crate::body_ir::StaticInitializer::Computed(_)))
            .map(|storage| storage.canonical.clone()),
    );
}

/// Validate source-local function frames before projecting their addresses into a package-owned consumer view.
/// The source module is unchanged. A package identity must name this exact source module and library; each role must
/// already carry its source declaration's physical address. Methods retain refusal until their owner context can be
/// projected with the same proof. Already-projected modules use the consumer validator directly.
fn is_well_formed_source_binding(
    module: &BodyIrModule,
    binding: &crate::body_ir::DecoratedBinding,
    library: &str,
) -> bool {
    // ---- Source: canonical owner and physical declaring module ----
    let Some(identity) = binding.original_body.canonical.as_ref() else {
        return false;
    };
    let Some(owner) = canonical_module_identity(identity) else {
        return false;
    };
    if owner == module.module_id.path() {
        return module.is_well_formed_decorated_binding(binding);
    }
    let SymbolOrigin::Package {
        library: declared_library,
        module_path,
    } = &identity.origin
    else {
        return false;
    };
    if declared_library != library
        || crate::module_identity_for_path(module_path) != module.module_id.path()
        || identity.kind != crate::SemanticSourceTargetKind::Function
    {
        return false;
    }

    // ---- Roles: wrapper, original and replacement share the source address ----
    let Some(wrapper) = module
        .bodies
        .iter()
        .find(|body| body.canonical.as_ref() == Some(identity))
    else {
        return false;
    };
    let source_address = CompilerNodeId::declaration_at(module.module_id.path(), identity.declaration_span);
    let mut roles = vec![&binding.original_body, wrapper];
    if let Some(initializer) = &binding.initializer {
        roles.push(initializer);
    }
    if binding.initialization == crate::body_ir::DecoratedInitialization::ModuleStorage {
        let Some(storage) = module
            .static_declarations
            .iter()
            .find(|storage| &storage.canonical == identity)
        else {
            return false;
        };
        let crate::body_ir::StaticInitializer::Computed(initializer) = &storage.initial else {
            return false;
        };
        roles.push(initializer);
    }
    if roles
        .iter()
        .any(|body| body.decl_id != source_address || body.direct_call_id != source_address)
    {
        return false;
    }
    let Ok(projected_address) = declaration_id(identity) else {
        return false;
    };

    // ---- Consumer: validate an isolated projection without relaxing package scope ----
    let mut view = module.clone();
    view.module_id = CompilerNodeId::module(owner);
    for body in &mut view.bodies {
        if body.canonical.as_ref() == Some(identity) {
            body.decl_id = projected_address.clone();
            body.direct_call_id = projected_address.clone();
        }
    }
    for retained in &mut view.decorated_bindings {
        if retained.original_body.canonical.as_ref() == Some(identity) {
            retained.original_body.decl_id = projected_address.clone();
            retained.original_body.direct_call_id = projected_address.clone();
            if let Some(initializer) = &mut retained.initializer {
                initializer.decl_id = projected_address.clone();
                initializer.direct_call_id = projected_address.clone();
            }
        }
    }
    for retained in &mut view.static_declarations {
        if &retained.canonical == identity
            && let crate::body_ir::StaticInitializer::Computed(body) = &mut retained.initial
        {
            body.decl_id = projected_address.clone();
            body.direct_call_id = projected_address.clone();
        }
    }
    view.decorated_bindings
        .iter()
        .find(|retained| retained.original_body.canonical.as_ref() == Some(identity))
        .is_some_and(|retained| view.is_well_formed_decorated_binding(retained))
}
