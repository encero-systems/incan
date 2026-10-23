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

/// Select a compiled contract, governed body, or complete checked decorator replacement under one public identity.
pub(super) fn project_callable(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
    native_callables: &[&crate::body_ir::LoafCallable],
) -> Result<(ExecutableDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    if module.decorated_bindings.iter().any(|binding| {
        binding.original_body.canonical == source.canonical
            && binding.initialization == crate::body_ir::DecoratedInitialization::PerInstantiation
    }) {
        // No portable cached-template fragment is admitted; publishing only its wrapper would erase initialization.
        return Err(CoverageReason::UnsupportedConstruct);
    }
    if let Some(binding) = module.decorated_bindings.iter().find(|binding| {
        binding.original_body.canonical == source.canonical
            && binding.initialization == crate::body_ir::DecoratedInitialization::PerInvocation
    }) {
        return project_invocation_body(source, binding, module, library, eligible);
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
    if !module.body_requires_instantiation(source)
        && !source.is_generator()
        && crate::body_ir::LoafCallable::from_body(source).is_some()
    {
        return project_compiled_callable(source, module, library, eligible);
    }
    let value = project_body(source, module, library, eligible)?;
    let mut callables = Vec::new();
    for (identity, arguments) in value.native_calls {
        let callable = native_callables
            .iter()
            .copied()
            .find(|callable| callable.canonical == identity && callable.native_type_arguments == arguments)
            .cloned()
            .ok_or(CoverageReason::UnresolvedReference)?;
        if !callables.contains(&callable) {
            callables.push(callable);
        }
    }
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

/// Publish only the caller-visible frame of an already compiled item, retaining parameter defaults and their local
/// types. Implementation locals and Rust calls stay in the rlib; they cannot become consumer execution requirements.
fn project_compiled_callable(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<(ExecutableDeclaration, BTreeSet<CanonicalSymbolId>), CoverageReason> {
    let mut frame = source.clone();
    frame.block.stmts.clear();
    frame.panic_facts.clear();
    let mut audit = PublicationAudit::new(module, library, eligible, &source.named_type_identities);
    let identity = frame.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
    audit.identity(identity, false)?;
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
    for local in &mut frame.locals {
        local.identity = None;
        if audit.local_slots.contains(&local.id) {
            audit.ty(&local.ty)?;
        } else {
            // Keep slot indices stable for defaults without publishing implementation-only carriers.
            local.ty = IncanType::Tuple(Vec::new());
        }
    }
    frame.named_type_identities = audit.used_nominal_types;
    require_module_initialization(module, &mut audit.requirements);
    Ok((
        ExecutableDeclaration::CompiledCallable(Box::new(frame)),
        audit.requirements,
    ))
}

/// Publish a generic wrapper and original atomically without inventing a polymorphic replacement cell.
fn project_invocation_body(
    source: &Body,
    binding: &crate::body_ir::DecoratedBinding,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
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
    let wrapper = project_body(source, module, library, eligible)?;
    let original = project_body(&binding.original_body, module, library, eligible)?;
    let mut requirements = wrapper.requirements;
    requirements.extend(original.requirements);
    Ok((
        ExecutableDeclaration::InvocationDecoratedBody {
            body: Box::new(wrapper.body),
            binding: Box::new(crate::body_ir::DecoratedBinding {
                original_body: original.body,
                initializer: None,
                initialization: crate::body_ir::DecoratedInitialization::PerInvocation,
            }),
        },
        requirements,
    ))
}

/// Bind a declaration's physical fragment address to the already-published canonical owner and span.
pub(super) fn declaration_id(identity: &CanonicalSymbolId) -> Result<CompilerNodeId, CoverageReason> {
    let module = canonical_module_identity(identity).ok_or(CoverageReason::UnresolvedReference)?;
    Ok(CompilerNodeId::declaration_span(
        &module,
        identity.declaration_span.start,
        identity.declaration_span.end,
    ))
}

/// Project one eligible body onto package-owned addresses and audit every retained computation and type.
///
/// Local slots and scope ordinals are fragment-local, not compiler-session symbol IDs. Their optional checker
/// identities are removed because execution uses the slots and a local identity may contain a session discriminant.
/// Declaration references keep the canonical identity minted by the declaring compilation.
/// A nominal's actual retained default frame borrows that owner's binders; its own callable binder list stays empty.
pub(super) fn project_body(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    eligible: &BTreeSet<CanonicalSymbolId>,
) -> Result<ProjectedBody, CoverageReason> {
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
    body.decl_id = declaration_id(canonical)?;
    body.direct_call_id = body.decl_id.clone();
    audit.ty(&body.return_type)?;
    for local in &mut body.locals {
        local.identity = None;
        audit.ty(&local.ty)?;
    }
    audit.parameters(&mut body.params)?;
    audit.statements(&mut body.block.stmts)?;
    body.named_type_identities = audit.used_nominal_types;
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
        crate::body_ir::StaticInitializer::Computed(body) => body.named_type_identities.clone(),
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
    /// Only module-initialized replacements authorize reading storage under a callable's canonical identity.
    replacement_storage: BTreeSet<CanonicalSymbolId>,
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

impl<'a> PublicationAudit<'a> {
    /// Combine source-local declarations, retained checked references and the executable native storage inventory,
    /// keeping conflicting source identities separate and performing no new name resolution.
    fn new(
        module: &'a BodyIrModule,
        library: &'a str,
        eligible: &'a BTreeSet<CanonicalSymbolId>,
        checked_nominal_types: &'a BTreeMap<String, CanonicalSymbolId>,
    ) -> Self {
        // ---- Declarations: source-owned nominal identities ----
        let mut nominal_types = BTreeMap::new();
        for declaration in &module.nominal_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.fieldless_enum_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.value_enum_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.enum_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.trait_declarations {
            nominal_types.insert(declaration.declaration_name.clone(), declaration.clone());
        }

        // ---- Decorators: distinct original-callable and replacement-storage permissions ----
        let mut original_callables = BTreeSet::new();
        let mut replacement_storage = BTreeSet::new();
        for binding in &module.decorated_bindings {
            if !is_well_formed_source_binding(module, binding, library) {
                continue;
            }
            let Some(identity) = &binding.original_body.canonical else {
                continue;
            };
            original_callables.insert(identity.clone());
            if binding.initialization == crate::body_ir::DecoratedInitialization::ModuleStorage {
                replacement_storage.insert(identity.clone());
            }
        }

        // ---- Context: package and type auditing ----
        Self {
            current_callable: None,
            replacement_storage,
            original_callables,
            trait_self: false,
            type_parameters: BTreeSet::new(),
            library,
            eligible,
            nominal_types,
            checked_nominal_types,
            value_enum_declarations: &module.value_enum_declarations,
            nominal_declarations: &module.nominal_declarations,
            native_types: crate::body_ir::held_rust_items(std::slice::from_ref(module))
                .into_iter()
                .map(|declaration| (declaration.name, declaration.canonical))
                .collect(),
            used_nominal_types: BTreeMap::new(),
            requirements: BTreeSet::new(),
            native_calls: Vec::new(),
            local_slots: BTreeSet::new(),
        }
    }

    /// Rebase only producer-local origins; a foreign reexport retains its declaring package identity. Native items
    /// require adapter validation against the selected Rust closure, never an Incan executable fragment.
    fn identity(&mut self, identity: &mut CanonicalSymbolId, required: bool) -> Result<(), CoverageReason> {
        if let SymbolOrigin::Module(module_path) = &identity.origin {
            identity.origin = SymbolOrigin::Package {
                library: self.library.to_owned(),
                module_path: module_path.clone(),
            };
        }
        if identity.scope_discriminant.is_some() {
            return Err(CoverageReason::UnresolvedReference);
        }
        match &identity.origin {
            SymbolOrigin::Package { library, .. } => {
                if library == self.library && !self.eligible.contains(identity) {
                    if let Some(mut owner) =
                        generated_value_enum_member_owner(identity, self.value_enum_declarations, self.library)
                    {
                        self.identity(&mut owner, true)?;
                    } else {
                        return Err(CoverageReason::PrivateDependency);
                    }
                }
                if required {
                    self.requirements.insert(identity.clone());
                }
                Ok(())
            }
            SymbolOrigin::Builtin => Ok(()),
            // Native identities require no Incan fragment. Their exact storage and callable signatures remain
            // subject to the adapter's metadata validation against the selected native closure.
            SymbolOrigin::RustCrate(_) => Ok(()),
            SymbolOrigin::Module(_) => Err(CoverageReason::UnsupportedConstruct),
        }
    }

    /// Admit concrete types and declared binders, retaining the canonical declaration behind each named type.
    /// Native unsized text and slice referents remain structural types; this grants no owned frame for either.
    fn ty(&mut self, ty: &IncanType) -> Result<(), CoverageReason> {
        match ty {
            IncanType::SelfType if self.trait_self => Ok(()),
            IncanType::TypeVar(name) if self.type_parameters.contains(name) => Ok(()),
            IncanType::Primitive(_) | IncanType::Never | IncanType::Decimal { .. } | IncanType::NativeBuiltin(_) => {
                Ok(())
            }
            // The native unsized text and slice carriers have no nominal declaration. Their surrounding references
            // and type arguments retain the same storage distinction checked by executable layout planning.
            IncanType::RustInteropPath(path) if path == "str" => Ok(()),
            IncanType::Named(name) => self.named_type(name),
            IncanType::Generic { base, args } => {
                // Compiler collections have no declaration requirement; a generic nominal retains its checked base.
                if incan_lang::lang::types::collections::from_str(base).is_none()
                    && base != incan_lang::lang::types::UNION_TYPE_NAME
                    && base != "[]"
                {
                    self.named_type(base)?;
                }
                for arg in args {
                    self.ty(arg)?;
                }
                Ok(())
            }
            IncanType::Tuple(items) => {
                for item in items {
                    self.ty(item)?;
                }
                Ok(())
            }
            IncanType::Function { params, return_type } => {
                for param in params {
                    self.ty(&param.ty)?;
                }
                self.ty(return_type)
            }
            IncanType::Ref(inner)
            | IncanType::RefMut(inner)
            | IncanType::TypeToken(inner)
            | IncanType::NativeArray { element: inner, .. } => self.ty(inner),
            IncanType::TypeVar(_)
            | IncanType::SelfType
            | IncanType::RustInteropPath(_)
            | IncanType::Infer
            | IncanType::Unknown
            | IncanType::Unsupported(_) => Err(CoverageReason::UnresolvedReference),
        }
    }

    /// Retain a named model or trait only when local declarations and checker bindings agree on its identity, or
    /// select an exact native identity already present in the executable storage inventory.
    fn named_type(&mut self, name: &str) -> Result<(), CoverageReason> {
        let local = self.nominal_types.get(name);
        let checked = self.checked_nominal_types.get(name);
        if matches!((local, checked), (Some(local), Some(checked)) if local != checked) {
            return Err(CoverageReason::UnresolvedReference);
        }
        let mut identity = checked
            .or(local)
            .or_else(|| self.native_types.get(name))
            .cloned()
            .ok_or(CoverageReason::UnresolvedReference)?;
        self.identity(&mut identity, true)?;
        self.used_nominal_types.insert(name.to_owned(), identity);
        Ok(())
    }

    /// Audit declaration-owned defaults, including nested closure parameters, and retain every referenced frame slot.
    fn parameters(&mut self, parameters: &mut [CallableParam]) -> Result<(), CoverageReason> {
        for parameter in parameters {
            self.local_slots.insert(parameter.local);
            self.ty(&parameter.ty)?;
            match &mut parameter.default {
                CallableParamDefault::Source(value) => {
                    self.statements(&mut value.stmts)?;
                    self.operand(&mut value.result)?;
                }
                CallableParamDefault::Required => {}
                CallableParamDefault::PartialPreset { capture } => {
                    self.local_slots.insert(*capture);
                }
                CallableParamDefault::Unsupported { .. } => {
                    return Err(CoverageReason::UnsupportedConstruct);
                }
            }
        }
        Ok(())
    }

    /// Inspect every explicit operand rather than only the statements that happen to call functions.
    fn operand(&mut self, operand: &mut Operand) -> Result<(), CoverageReason> {
        match operand {
            Operand::Place(value) => self.place(&mut value.place),
            Operand::Constant(value) => self.constant(value),
        }
    }

    /// Scalar data keeps its existing contract; newtype values additionally require their exact retained owner.
    fn constant(&mut self, constant: &mut crate::body_ir::Constant) -> Result<(), CoverageReason> {
        match constant {
            crate::body_ir::Constant::NumericNewtype(value) => self.numeric_newtype_constant(value),
            crate::body_ir::Constant::FrozenAggregate(_, elements) => {
                for element in elements {
                    self.constant(element)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Validate the source physical address before projection; a foreign inline value lacks this local authority.
    /// Missing or duplicate owner layouts stay refused instead of reconstructing their ABI from a type name.
    fn numeric_newtype_constant(
        &mut self,
        value: &mut crate::body_ir::NumericNewtypeConstant,
    ) -> Result<(), CoverageReason> {
        let identity = value
            .constructor
            .canonical
            .as_ref()
            .ok_or(CoverageReason::UnresolvedReference)?;
        let mut owners = self
            .nominal_declarations
            .iter()
            .filter(|owner| &owner.canonical == identity);
        let owner = owners.next().ok_or(CoverageReason::UnresolvedReference)?;
        if owners.next().is_some() || !value.matches_layout(owner) {
            return Err(CoverageReason::UnresolvedReference);
        }
        let identity = value
            .constructor
            .canonical
            .as_mut()
            .ok_or(CoverageReason::UnresolvedReference)?;
        self.identity(identity, true)?;
        value.constructor.direct_declaration_id = Some(declaration_id(identity)?);
        Ok(())
    }

    /// Canonical fields and static storage require checked context; a requiring body cannot outlive missing
    /// initialization. Computed constants retain read-only authority. Callable replacement storage additionally
    /// requires its validated decorator binding.
    fn place(&mut self, place: &mut Place) -> Result<(), CoverageReason> {
        if let PlaceRoot::Local(local) = &place.root {
            self.local_slots.insert(*local);
        }
        if let PlaceRoot::Global(value) = &mut place.root {
            if value.identity.kind != crate::SemanticSourceTargetKind::Static
                && !(value.identity.kind == crate::SemanticSourceTargetKind::Const
                    && value.write_policy == crate::body_ir::GlobalWritePolicy::ReadOnly)
                && !self.replacement_storage.contains(&value.identity)
            {
                return Err(CoverageReason::UnsupportedConstruct);
            }
            self.identity(&mut value.identity, true)?;
            self.ty(&value.ty)?;
        }
        for projection in &mut place.projection {
            match projection {
                PlaceElem::Field {
                    canonical: Some(identity),
                    structural: false,
                    ..
                } => self.identity(identity, false)?,
                PlaceElem::Field {
                    canonical: None,
                    structural: true,
                    ..
                } => {}
                PlaceElem::Field { .. } => return Err(CoverageReason::UnresolvedReference),
                PlaceElem::UnionMember { ty } => self.ty(ty)?,
                PlaceElem::OptionPayload {
                    option_type,
                    payload_type,
                } => {
                    self.ty(option_type)?;
                    self.ty(payload_type)?;
                }
                PlaceElem::Index(value) => self.operand(value)?,
                PlaceElem::Slice { start, end, step } => {
                    for value in [start, end, step].into_iter().flatten() {
                        self.operand(value)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// All argument forms retain their source operand even when a runtime profile later refuses the shape.
    fn arguments(&mut self, arguments: &mut [ArgumentElement]) -> Result<(), CoverageReason> {
        for argument in arguments {
            match argument {
                ArgumentElement::One(value) | ArgumentElement::Named { operand: value, .. } => self.operand(value)?,
                ArgumentElement::Spread(value) => self.operand(&mut value.source)?,
            }
        }
        Ok(())
    }

    /// Keep enum and member addresses joined to checked canonical identities.
    fn fieldless_variant(&mut self, target: &mut FieldlessEnumVariantTarget) -> Result<(), CoverageReason> {
        self.identity(&mut target.enum_canonical, true)?;
        self.identity(&mut target.variant_canonical, false)?;
        target.enum_declaration_id = declaration_id(&target.enum_canonical)?;
        target.variant_declaration_id = declaration_id(&target.variant_canonical)?;
        Ok(())
    }

    /// Match and assert patterns can depend on types even when the matching branch is never entered.
    fn pattern(&mut self, pattern: &mut Pattern) -> Result<(), CoverageReason> {
        match pattern {
            Pattern::Wildcard | Pattern::Var(_) | Pattern::Literal(_) => {}
            Pattern::Tuple(items) | Pattern::Or(items) | Pattern::Result { fields: items, .. } => {
                for item in items {
                    self.pattern(item)?;
                }
            }
            Pattern::UnionMember { ty, fields } => {
                self.ty(ty)?;
                for field in fields {
                    self.pattern(field)?;
                }
            }
            Pattern::Nominal { target, fields } => {
                self.identity(&mut target.canonical, true)?;
                target.direct_declaration_id = declaration_id(&target.canonical)?;
                for (_, pattern) in fields {
                    self.pattern(pattern)?;
                }
            }
            Pattern::FieldlessEnumVariant(target) => self.fieldless_variant(target)?,
            Pattern::Enum { canonical, fields, .. } => {
                let identity = canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                self.identity(identity, true)?;
                for field in fields {
                    self.pattern(field)?;
                }
            }
            Pattern::Struct { .. } => return Err(CoverageReason::UnsupportedConstruct),
        }
        Ok(())
    }

    /// Every deferred or branching computation participates in the same conservative public closure.
    fn rvalue(&mut self, value: &mut Rvalue) -> Result<(), CoverageReason> {
        match value {
            Rvalue::Use(value) | Rvalue::UnaryOp(_, value) => self.operand(value)?,
            Rvalue::BinaryOp(_, left, right) => {
                self.operand(left)?;
                self.operand(right)?;
            }
            Rvalue::IsInstance {
                value,
                value_ty,
                target,
            } => {
                self.operand(value)?;
                self.ty(value_ty)?;
                self.ty(&target.ty)?;
                if let Some(identity) = &mut target.canonical {
                    self.identity(identity, true)?;
                }
            }
            Rvalue::Aggregate(kind, arguments) => {
                if let AggregateKind::Constructor(target) = kind {
                    let identity = target.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                    self.identity(identity, true)?;
                    target.direct_declaration_id = Some(declaration_id(identity)?);
                }
                if let AggregateKind::EnumVariant(target) = kind {
                    let owner = target
                        .enum_canonical
                        .as_mut()
                        .ok_or(CoverageReason::UnresolvedReference)?;
                    self.identity(owner, true)?;
                    self.identity(&mut target.variant_canonical, false)?;
                }
                self.arguments(arguments)?;
            }
            Rvalue::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.operand(key)?;
                            self.operand(value)?;
                        }
                        DictEntry::Spread(value) => self.operand(&mut value.source)?,
                    }
                }
            }
            Rvalue::ValueEnumVariant(target) => {
                self.identity(&mut target.enum_canonical, true)?;
                self.identity(&mut target.variant_canonical, false)?;
                target.enum_declaration_id = declaration_id(&target.enum_canonical)?;
                target.variant_declaration_id = declaration_id(&target.variant_canonical)?;
            }
            Rvalue::FieldlessEnumVariant(target) => self.fieldless_variant(target)?,
            Rvalue::ResultVariant(value) => {
                if !value.has_complete_canonical_types() {
                    return Err(CoverageReason::UnresolvedReference);
                }
                self.operand(&mut value.payload)?;
                for identity in value.canonical_types.values_mut() {
                    self.identity(identity, true)?;
                }
                self.ty(&value.ok_type)?;
                self.ty(&value.error_type)?;
            }
            Rvalue::Format(parts) => {
                for part in parts {
                    if let FormatPart::Expr { operand, .. } = part {
                        self.operand(operand)?;
                    }
                }
            }
            Rvalue::Closure {
                params,
                captured_operands,
                body,
            } => {
                if let Some(identity) = &mut body.original_callable {
                    if !self.original_callables.contains(identity) || self.current_callable.as_ref() != Some(identity) {
                        return Err(CoverageReason::UnresolvedReference);
                    }
                    self.identity(identity, true)?;
                }
                self.parameters(params)?;
                for value in captured_operands {
                    self.operand(value)?;
                }
                self.statements(&mut body.stmts)?;
                self.operand(&mut body.result)?;
            }
            Rvalue::Generator {
                source,
                captured_operands,
                body,
            } => {
                self.operand(source)?;
                for value in captured_operands {
                    self.operand(value)?;
                }
                self.statements(&mut body.stmts)?;
            }
            Rvalue::Match { scrutinee, arms } => {
                self.operand(scrutinee)?;
                for arm in arms {
                    self.pattern(&mut arm.pattern)?;
                    self.statements(&mut arm.guard_stmts)?;
                    if let Some(value) = &mut arm.guard {
                        self.operand(value)?;
                    }
                    self.statements(&mut arm.body_stmts)?;
                    self.operand(&mut arm.result)?;
                }
            }
        }
        Ok(())
    }

    /// Traverse the complete statement vocabulary; unsupported nodes cannot become coverage merely by encoding.
    fn statements(&mut self, statements: &mut [Statement]) -> Result<(), CoverageReason> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    /// Audit the call-site SDK claim under its own Self binder; unresolved leaves cannot cross publication.
    fn native_trait_selection(&mut self, target: &mut MethodTarget) -> Result<(), CoverageReason> {
        if !target.has_valid_native_trait_selection() {
            return Err(CoverageReason::UnresolvedReference);
        }
        let Some(selection) = &mut target.native_trait_delegation else {
            return Ok(());
        };
        self.identity(&mut selection.canonical, true)?;
        let inserted = self.type_parameters.insert("Self".to_owned());
        let result = selection
            .parameters
            .iter()
            .chain(std::iter::once(&selection.return_type))
            .chain(&selection.native_type_arguments)
            .try_for_each(|ty| self.ty(ty));
        if inserted {
            self.type_parameters.remove("Self");
        }
        result
    }

    /// Inspect one statement without hiding unsupported nodes behind successful codec serialization.
    fn statement(&mut self, statement: &mut Statement) -> Result<(), CoverageReason> {
        match &mut statement.kind {
            StatementKind::Assign { place, rvalue } => {
                self.place(place)?;
                self.rvalue(rvalue)?;
            }
            StatementKind::Call {
                destination,
                callee,
                args,
                ..
            } => {
                if let Some(place) = destination {
                    self.place(place)?;
                }
                self.arguments(args)?;
                match callee {
                    Callee::Function(CallableTarget::Named(target)) => {
                        for ty in &target.type_args {
                            self.ty(ty)?;
                        }
                        if target.builtin.is_none() {
                            let identity = target.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                            self.identity(identity, true)?;
                            if matches!(identity.origin, SymbolOrigin::RustCrate(_)) {
                                let selection = (identity.clone(), target.type_args.clone());
                                if !self.native_calls.contains(&selection) {
                                    self.native_calls.push(selection);
                                }
                            }
                            // Package dispatch is canonical throughout, including calls within the same fragment.
                            target.direct_call_id = None;
                        }
                    }
                    Callee::Function(CallableTarget::Local(target)) => self.place(&mut target.operand.place)?,
                    Callee::Method(target) => {
                        self.native_trait_selection(target)?;
                        for ty in &target.type_args {
                            self.ty(ty)?;
                        }
                        if let Some(identity) = &mut target.canonical {
                            self.identity(identity, true)?;
                        }
                    }
                    Callee::Helper(_) => {}
                    // The activation state is a producer-session fact. Until the portable host-requirement
                    // contract exists it must not be exported as if it were the consumer's provider plan.
                    Callee::ProviderOperation(_) => {
                        return Err(CoverageReason::UnsupportedConstruct);
                    }
                    Callee::TypeMetadata(_) => return Err(CoverageReason::UnsupportedConstruct),
                }
            }
            StatementKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.operand(cond)?;
                self.statements(&mut then_block.stmts)?;
                if let Some(block) = else_block {
                    self.statements(&mut block.stmts)?;
                }
            }
            StatementKind::Loop { body } | StatementKind::UnsafeRegion { body, .. } => {
                self.statements(&mut body.stmts)?
            }
            StatementKind::Break { value } | StatementKind::Return { value } => {
                if let Some(value) = value {
                    self.operand(value)?;
                }
            }
            StatementKind::Await { destination, awaited } => {
                if let Some(place) = destination {
                    self.place(place)?;
                }
                self.operand(awaited)?;
            }
            StatementKind::Race { destination, arms } => {
                if let Some(place) = destination {
                    self.place(place)?;
                }
                for arm in arms {
                    self.operand(&mut arm.awaitable)?;
                    self.statements(&mut arm.body.stmts)?;
                    self.operand(&mut arm.result)?;
                }
            }
            StatementKind::Yield { value } | StatementKind::Expr { value } => self.operand(value)?,
            StatementKind::Assert { kind, message, .. } => {
                if let Some(message) = message {
                    self.operand(message)?;
                }
                match kind {
                    AssertionKind::Condition { cond } => self.operand(cond)?,
                    AssertionKind::Pattern { scrutinee, pattern } => {
                        self.operand(scrutinee)?;
                        self.pattern(pattern)?;
                    }
                    AssertionKind::Raises { call, .. } => self.operand(call)?,
                }
            }
            StatementKind::TryPropagate {
                destination,
                operand,
                error_routing,
            } => {
                self.place(destination)?;
                self.operand(operand)?;
                match error_routing {
                    TryErrorRouting::SameType { error_type } => self.ty(error_type)?,
                    TryErrorRouting::ConversionRequired { .. } | TryErrorRouting::Unresolved => {
                        return Err(CoverageReason::UnsupportedConstruct);
                    }
                }
            }
            StatementKind::IterNext {
                destination,
                iterator,
                protocol,
            } => {
                self.place(destination)?;
                self.operand(iterator)?;
                if !matches!(protocol, crate::body_ir::IterProtocol::Builtin) {
                    return Err(CoverageReason::UnsupportedConstruct);
                }
            }
            StatementKind::Drop { .. } | StatementKind::Continue => {}
            StatementKind::Unsupported { .. } => return Err(CoverageReason::UnsupportedConstruct),
        }
        Ok(())
    }
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
    let source_address = CompilerNodeId::declaration_span(
        module.module_id.path(),
        identity.declaration_span.start,
        identity.declaration_span.end,
    );
    let mut roles = vec![&binding.original_body, wrapper];
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
