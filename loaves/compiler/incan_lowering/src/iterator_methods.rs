//! Checked source polling authority for ordinary SDK iterator storage bounds.

use incan_mir_plan_native::{EnumDeclaration, IteratorMethods, PlanType};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{
    CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin, generic,
};

use crate::classes::{frame_type, function_name, parameter_local_index};
use crate::decls::library_item;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::trait_links::contains;
use crate::unions::collect_unions;

/// Authorize only a concrete source adopter actually retained as an SDK storage argument.
pub fn iterator_methods(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    owner: usize,
    native_names: &[String],
    intrinsics: &[EnumDeclaration],
) -> Result<Option<IteratorMethods>, String> {
    let declaration = &module.nominal_declarations[owner];
    if !native_names.contains(&declaration.name) || !library_item(declaration).is_empty() {
        return Ok(None);
    }
    let mut selected: Option<IteratorMethods> = None;
    for protocol in &module.trait_declarations {
        if !standard_protocol(protocol) {
            continue;
        }
        let mut arguments: Vec<IncanType> = Vec::new();
        let mut next: Option<CanonicalSymbolId> = None;
        let mut slots: Vec<String> = Vec::new();
        for implementation in &module.trait_implementations {
            if implementation.owner != declaration.canonical
                || implementation.method.origin != protocol.origin
                || !contains(protocol, &implementation.method)
            {
                continue;
            }
            let name = implementation.method.declaration_name.clone();
            if slots.contains(&name) {
                return Err("unsupported Body IR ambiguous native iterator slot".into());
            }
            slots.push(name.clone());
            if name == "__next__" {
                let identity = &implementation.implementation;
                if *identity == implementation.method
                    || identity.origin != declaration.canonical.origin
                    || !contains(&declaration.canonical, identity)
                {
                    return Err("unsupported Body IR native iterator without source poll".into());
                }
                next = Some(identity.clone());
                continue;
            }
            // The native SDK trait owns its sum default. An override must not be silently replaced by that default.
            if name == "sum" && implementation.implementation != implementation.method {
                return Err("unsupported Body IR native iterator sum override".into());
            }
            if implementation.implementation != implementation.method {
                continue;
            }
            let current: Vec<IncanType> = implementation
                .type_arguments
                .iter()
                .map(|binding| binding.argument.clone())
                .collect();
            if current.len() != 1 {
                return Err("unsupported Body IR native iterator default arguments".into());
            }
            if arguments.is_empty() {
                arguments = current;
            } else if arguments != current {
                return Err("unsupported Body IR inconsistent native iterator arguments".into());
            }
        }
        if slots.is_empty() {
            continue;
        }
        if arguments.len() != 1 || !slots.iter().any(|slot| slot == "sum") {
            return Err("unsupported Body IR incomplete native iterator contract".into());
        }
        let Some(identity) = next else {
            return Err("unsupported Body IR native iterator without checked poll".into());
        };
        let bodies: Vec<&Body> = module
            .bodies
            .iter()
            .filter(|body| body.canonical.as_ref() == Some(&identity))
            .collect();
        if bodies.len() != 1 || !poll_abi(constructors, module, bodies[0], owner, &arguments[0], intrinsics)? {
            return Err("unsupported Body IR unavailable or invalid native iterator poll body".into());
        }
        if selected.is_some() {
            return Err("unsupported Body IR ambiguous native iterator declaration".into());
        }
        let unions = collect_unions(module);
        selected = Some(IteratorMethods {
            item: native_source_type(
                constructors,
                &module.nominal_declarations,
                &module.enum_declarations,
                &unions,
                &arguments[0],
                intrinsics,
                "",
            )?,
            next: function_name(module, bodies[0]),
        });
    }
    Ok(selected)
}

/// Authenticate the standard declaring trait rather than a source trait with the same spelling.
pub fn standard_protocol(identity: &CanonicalSymbolId) -> bool {
    if identity.kind != SemanticSourceTargetKind::Trait
        || identity.namespace != SymbolNamespace::OrdinaryLexical
        || identity.declaration_name != "Iterator"
    {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_primitives" && module_path == &["derives", "collection"]
        }
        SymbolOrigin::Module(path) => path == &["std", "derives", "collection"],
        _ => false,
    }
}

/// Require the unique concrete mutable receiver, exact `Option[item]` result, and a fully retained poll body.
fn poll_abi(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    owner: usize,
    item: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Result<bool, String> {
    if !body.type_parameters.is_empty() || body.params.len() != 1 || body.first_representation_gap().is_some() {
        return Ok(false);
    }
    let reference = PlanType::ModelMutRef(owner as i64, module.nominal_declarations[owner].name.clone());
    let local = &body.locals[parameter_local_index(body, 0)];
    if frame_type(
        constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        local,
        intrinsics,
        "",
    )? != reference
    {
        return Ok(false);
    }
    Ok(body.return_type == generic("Option".into(), vec![item.clone()]))
}
