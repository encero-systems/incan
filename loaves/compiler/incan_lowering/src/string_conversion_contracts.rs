//! Consumer checks for one compilation's selected source conversion contract and exact raw chains.

use incan_semantics_core::body_ir::{BodyIrModule, StringConversionContract, loaf_item_path};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Only this invocation's entry can supply a fresh import union; package and dependency fragments supply none.
pub fn validate_conversion_placement(modules: &[BodyIrModule], entry: usize) -> Result<(), String> {
    for (index, module) in modules.iter().enumerate() {
        if index == entry {
            continue;
        }
        if !matches!(module.string_conversion_contract, StringConversionContract::Unselected) {
            return Err("unsupported Body IR misplaced string conversion contract".into());
        }
    }
    Ok(())
}

/// Absent and inactive states grant no conversion; a selected source trait keeps its encoded canonical identity.
pub fn conversion_trait(module: &BodyIrModule) -> Result<String, String> {
    let selection = match &module.string_conversion_contract {
        StringConversionContract::Unselected | StringConversionContract::Inactive => return Ok(String::new()),
        StringConversionContract::Selected(selection) => selection,
    };
    let refused = || "unsupported Body IR string conversion contract identity".to_owned();
    let owner = &selection.trait_identity;
    let method = &selection.method_identity;
    if !is_conversion_trait(owner) || !is_conversion_method(method) || !method_belongs_to_owner(method, owner) {
        return Err(refused());
    }
    if owner.scope_discriminant.is_some() || method.scope_discriminant.is_some() {
        return Err(refused());
    }
    match &owner.origin {
        SymbolOrigin::Package { library, module_path } => {
            if library != "incan_stdlib_primitives" || module_path != &["traits", "convert"] {
                return Err(refused());
            }
            loaf_item_path(owner).ok_or_else(|| "unsupported Body IR string conversion contract item".to_owned())
        }
        _ => Err(refused()),
    }
}

/// The selected lexical TryFrom trait rather than a same-name value or member.
fn is_conversion_trait(identity: &CanonicalSymbolId) -> bool {
    identity.kind == SemanticSourceTargetKind::Trait
        && identity.namespace == SymbolNamespace::OrdinaryLexical
        && identity.declaration_name == "TryFrom"
}

/// The selected member method, not a free function or differently named declaration.
fn is_conversion_method(identity: &CanonicalSymbolId) -> bool {
    identity.kind == SemanticSourceTargetKind::Method
        && identity.namespace == SymbolNamespace::Member
        && identity.declaration_name == "try_from"
}

/// Keep the method's origin and complete source span inside the selected trait's declaration.
fn method_belongs_to_owner(method: &CanonicalSymbolId, owner: &CanonicalSymbolId) -> bool {
    method.origin == owner.origin
        && method.declaration_span.start >= owner.declaration_span.start
        && method.declaration_span.end <= owner.declaration_span.end
}

/// A producer-attested automatic candidate becomes executable only under the selected invocation contract.
pub fn conversion_name(
    module: &BodyIrModule,
    declaration: &incan_semantics_core::body_ir::NominalDeclaration,
) -> Result<String, String> {
    if conversion_trait(module)?.is_empty() {
        return Ok(String::new());
    }
    if !matches!(declaration.canonical.origin, SymbolOrigin::Module(_)) {
        return Ok(String::new());
    }
    match declaration.newtype_string_conversion {
        incan_semantics_core::body_ir::NewtypeStringConversion::Unselected => Ok(String::new()),
        incan_semantics_core::body_ir::NewtypeStringConversion::Automatic => {
            conversion_chain(module, declaration, &[])?;
            Ok(format!(
                "__incan_string_try_from_{}",
                crate::symbols::symbol_component(&declaration.name)
            ))
        }
    }
}

/// Check each exact named underlying edge before scheduling roots; an unselected inner cannot be overwritten.
pub fn conversion_chain(
    module: &BodyIrModule,
    declaration: &incan_semantics_core::body_ir::NominalDeclaration,
    ancestors: &[String],
) -> Result<(), String> {
    use incan_semantics_core::body_ir::NewtypeStringConversion;
    use incan_semantics_core::{IncanPrimitiveType, IncanType, canonical_module_identity};
    if ancestors.contains(&declaration.name) {
        return Err("unsupported Body IR automatic string conversion cycle".into());
    }
    raw_conversion_owner(declaration)?;
    if matches!(
        declaration.newtype_string_conversion,
        NewtypeStringConversion::Unselected
    ) {
        return Err("unsupported Body IR automatic string conversion underlying selection".into());
    }
    match &declaration.field_types[0] {
        IncanType::Primitive(IncanPrimitiveType::Int) | IncanType::Primitive(IncanPrimitiveType::Str) => Ok(()),
        IncanType::Named(name) => {
            let Some(canonical) = declaration.named_type_identities.get(name).cloned() else {
                return Err("unsupported Body IR automatic string conversion underlying identity".into());
            };
            if canonical_module_identity(&canonical) != canonical_module_identity(&declaration.canonical) {
                return Err("unsupported Body IR automatic string conversion nonlocal underlying".into());
            }
            let mut selected = None;
            for candidate in &module.nominal_declarations {
                if candidate.canonical == canonical {
                    if selected.is_some() {
                        return Err("unsupported Body IR automatic string conversion ambiguous underlying".into());
                    }
                    selected = Some(candidate);
                }
            }
            let Some(inner) = selected else {
                return Err("unsupported Body IR automatic string conversion missing underlying".into());
            };
            if *name != canonical.declaration_name && *name != inner.carrier_name() {
                return Err("unsupported Body IR automatic string conversion underlying identity".into());
            }
            let mut seen = ancestors.to_vec();
            seen.push(declaration.name.clone());
            conversion_chain(module, inner, &seen)
        }
        _ => Err("unsupported Body IR automatic string conversion primitive".into()),
    }
}

/// Accept a source leaf or its exact retained carrier, never an unproven spelling fallback.
fn conversion_owner_name(declaration: &incan_semantics_core::body_ir::NominalDeclaration) -> bool {
    if declaration.name == declaration.canonical.declaration_name {
        return true;
    }
    let Some(key) = incan_semantics_core::body_ir::nominal_carrier_key(&declaration.canonical) else {
        return false;
    };
    if declaration.name != key {
        return false;
    }
    declaration
        .named_type_identities
        .get(&key)
        .is_some_and(|identity| *identity == declaration.canonical)
}

/// Require one public raw field with its exact owner identity, no binders, and no declared defaults.
fn raw_conversion_layout(declaration: &incan_semantics_core::body_ir::NominalDeclaration) -> bool {
    if !declaration.type_parameters.is_empty() || declaration.type_parameter_count != 0 {
        return false;
    }
    if declaration.fields != ["0"]
        || declaration.field_identities != [declaration.canonical.clone()]
        || declaration.field_types.len() != 1
    {
        return false;
    }
    declaration.field_public == [true]
        && !declaration.has_field_defaults
        && declaration.method_partial_defaults.is_empty()
}

/// Validate the canonical layout before or after project assembly gives it its exact retained carrier name.
fn raw_conversion_owner(declaration: &incan_semantics_core::body_ir::NominalDeclaration) -> Result<(), String> {
    use incan_semantics_core::{CompilerNodeId, canonical_module_identity};
    let refused = || "unsupported Body IR automatic string conversion owner".to_owned();
    let owner = &declaration.canonical;
    if owner.kind != SemanticSourceTargetKind::Newtype
        || owner.namespace != SymbolNamespace::OrdinaryLexical
        || !conversion_owner_name(declaration)
        || !raw_conversion_layout(declaration)
        || !matches!(owner.origin, SymbolOrigin::Module(_))
        || owner.scope_discriminant.is_some()
        || declaration.field_default_body.is_some()
        || declaration.field_aliases.iter().any(Option::is_some)
    {
        return Err(refused());
    }
    let Some(module) = canonical_module_identity(owner) else {
        return Err(refused());
    };
    let expected = CompilerNodeId::declaration_at(&module, owner.declaration_span.clone());
    if declaration.direct_declaration_id != expected {
        return Err(refused());
    }
    Ok(())
}
