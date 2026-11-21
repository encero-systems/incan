//! Complete native fallible-iterator contracts selected from checked standard slots.

use incan_mir_plan_native::{EnumDeclaration, FallibleIteratorMethods, PlanType};
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

const SLOTS: [&str; 11] = [
    "__iter__",
    "__next__",
    "map",
    "filter",
    "flat_map",
    "take",
    "inspect",
    "map_err",
    "inspect_err",
    "collect",
    "fold",
];

/// Grant the native protocol only for source layouts occurring inside a retained SDK instantiation.
pub fn fallible_methods(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    owner: usize,
    native_names: &[String],
    intrinsics: &[EnumDeclaration],
) -> Result<Option<FallibleIteratorMethods>, String> {
    let declaration = &module.nominal_declarations[owner];
    if !native_names.contains(&declaration.name) || !library_item(declaration).is_empty() {
        return Ok(None);
    }
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
                return Err("unsupported Body IR ambiguous native fallible iterator slot".into());
            }
            slots.push(name.clone());
            if name == "__next__" {
                if implementation.implementation == implementation.method {
                    return Err("unsupported Body IR native fallible iterator without source poll".into());
                }
                next = Some(implementation.implementation.clone());
                continue;
            }
            if implementation.implementation != implementation.method {
                return Err(format!("unsupported Body IR native fallible iterator override {name}"));
            }
            if !published_default(module, &implementation.method) {
                return Err(format!(
                    "unsupported Body IR unavailable published native fallible iterator default {name}"
                ));
            }
            let current: Vec<IncanType> = implementation
                .type_arguments
                .iter()
                .map(|binding| binding.argument.clone())
                .collect();
            if current.len() != 2 {
                return Err("unsupported Body IR native fallible iterator default arguments".into());
            }
            if arguments.is_empty() {
                arguments = current;
            } else if arguments != current {
                return Err("unsupported Body IR inconsistent native fallible iterator arguments".into());
            }
        }
        if slots.is_empty() {
            continue;
        }
        for name in SLOTS {
            if !slots.iter().any(|slot| slot == name) {
                return Err(format!(
                    "unsupported Body IR incomplete native fallible iterator slot {name}"
                ));
            }
        }
        if slots.len() != 11 || arguments.len() != 2 {
            return Err("unsupported Body IR native fallible iterator contract shape".into());
        }
        let Some(identity) = next else {
            return Err("unsupported Body IR native fallible iterator without checked poll".into());
        };
        for body in &module.bodies {
            if body.canonical.as_ref() == Some(&identity)
                && poll_abi(constructors, module, body, owner, &arguments, intrinsics)?
            {
                let unions = collect_unions(module);
                let resolve = |ty: &IncanType| {
                    native_source_type(
                        constructors,
                        &module.nominal_declarations,
                        &module.enum_declarations,
                        &unions,
                        ty,
                        intrinsics,
                        "",
                    )
                };
                return Ok(Some(FallibleIteratorMethods {
                    item: resolve(&arguments[0])?,
                    error: resolve(&arguments[1])?,
                    next: function_name(module, body),
                }));
            }
        }
        return Err("unsupported Body IR unavailable native fallible iterator poll body".into());
    }
    Ok(None)
}

/// Require a complete published body; original owning modules were validated before their contexts were joined.
fn published_default(module: &BodyIrModule, identity: &CanonicalSymbolId) -> bool {
    for body in &module.bodies {
        if body.canonical.as_ref() != Some(identity) {
            continue;
        }
        return body.first_representation_gap().is_none();
    }
    false
}

/// Authenticate the declaring standard trait, including its enclosing span, rather than a matching member name.
pub fn standard_protocol(identity: &CanonicalSymbolId) -> bool {
    if identity.kind != SemanticSourceTargetKind::Trait
        || identity.namespace != SymbolNamespace::OrdinaryLexical
        || identity.declaration_name != "FallibleIterator"
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

/// Require the selected concrete mutable receiver and its exact `Result[Option[item], error]` contract.
fn poll_abi(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    owner: usize,
    arguments: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<bool, String> {
    if !body.type_parameters.is_empty() || body.params.len() != 1 {
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
    let expected = generic(
        "Result".into(),
        vec![
            generic("Option".into(), vec![arguments[0].clone()]),
            arguments[1].clone(),
        ],
    );
    Ok(body.return_type == expected)
}
