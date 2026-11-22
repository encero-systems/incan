//! Exact checked serde hook contracts closed from retained nominal and Body facts.

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParamDefault, LocalOrigin, NewtypeDeserialize, NominalDeclaration,
};
use incan_semantics_core::{
    CanonicalSymbolId, IncanPrimitiveType, IncanType, RuntimeBuiltinType, SemanticSourceTargetKind,
};

use crate::nominal_instances::{NominalInstance, nominal_symbol};
use crate::symbols::symbol_component;
use crate::type_parameters::instantiate;

/// Name a root only for an explicit checked string reconstruction; absent selection never becomes ordinary serde.
pub fn checked_deserialize_name(declaration: &NominalDeclaration) -> Result<String, String> {
    let deserialize = declaration.derives.iter().any(|derive| derive == "serde::Deserialize");
    match &declaration.newtype_deserialize {
        NewtypeDeserialize::Unselected => {
            if deserialize {
                return Err("unsupported Body IR newtype Deserialize without retained reconstruction selection".into());
            }
            Ok(String::new())
        }
        NewtypeDeserialize::Ordinary => {
            if !deserialize {
                return Err("unsupported Body IR ordinary newtype Deserialize without derive selection".into());
            }
            Ok(String::new())
        }
        NewtypeDeserialize::Checked(_) => {
            if !deserialize
                || declaration.field_types.len() != 1
                || declaration.field_types[0] != IncanType::Primitive(IncanPrimitiveType::Str)
            {
                return Err("unsupported Body IR checked newtype Deserialize outside owned str".into());
            }
            Ok(format!(
                "__incan_checked_deserialize{}",
                symbol_component(&declaration.name)
            ))
        }
    }
}

/// The selected owner's arguments for scheduling, read from the exact discovery registry.
pub fn checked_owner_arguments(
    declaration: &NominalDeclaration,
    instances: &[NominalInstance],
) -> Result<Vec<IncanType>, String> {
    if declaration.type_parameters.is_empty() {
        return Ok(Vec::new());
    }
    for instance in instances {
        if instance.declaration.canonical == declaration.canonical
            && nominal_symbol(&instance.declaration.name, &instance.arguments) == declaration.name
        {
            if instance.declaration.type_parameters != declaration.type_parameters {
                return Err("unsupported Body IR checked Deserialize owner binder disagreement".into());
            }
            return Ok(instance.arguments.clone());
        }
    }
    Err("unsupported Body IR checked Deserialize without exact owner arguments".into())
}

/// Validate one exact source hook before closing its existing frame; never choose a body by conventional name.
pub fn selected_deserialize_hook(
    module: &BodyIrModule,
    declaration: &NominalDeclaration,
    arguments: &[IncanType],
) -> Result<(Body, Body), String> {
    let NewtypeDeserialize::Checked(hook) = &declaration.newtype_deserialize else {
        return Err("unsupported Body IR checked Deserialize without selected hook".into());
    };
    if hook.kind != SemanticSourceTargetKind::Method
        || hook.origin != declaration.canonical.origin
        || hook.declaration_span.start < declaration.canonical.declaration_span.start
        || hook.declaration_span.end > declaration.canonical.declaration_span.end
    {
        return Err("unsupported Body IR checked Deserialize hook owner provenance".into());
    }
    let mut found: Option<&Body> = None;
    for body in &module.bodies {
        if body.canonical.as_ref() == Some(hook) {
            if found.is_some() {
                return Err("unsupported Body IR checked Deserialize duplicate hook Body".into());
            }
            found = Some(body);
        }
    }
    let Some(body) = found else {
        return Err("unsupported Body IR checked Deserialize missing hook Body".into());
    };
    if !hook_declaration_agrees(body, hook, declaration) {
        return Err("unsupported Body IR checked Deserialize hook declaration disagreement".into());
    }
    let closed = instantiate(body, arguments, &[])?;
    require_closed_hook(&closed)?;
    Ok((body.clone(), closed))
}

fn hook_declaration_agrees(body: &Body, hook: &CanonicalSymbolId, declaration: &NominalDeclaration) -> bool {
    body.name == hook.declaration_name
        && body.span == hook.declaration_span
        && body.type_parameters == declaration.type_parameters
}

/// Require an owned String input and the canonical builtin error before constructing an executable serde root.
fn require_closed_hook(body: &Body) -> Result<(), String> {
    if body.is_async || body.is_generator() || body.params.len() != 1 || body.locals.is_empty() {
        return Err("unsupported Body IR checked Deserialize hook frame".into());
    }
    let parameter = &body.params[0];
    if parameter.mutable
        || parameter.ty != IncanType::Primitive(IncanPrimitiveType::Str)
        || parameter.local.index() != 0
        || body.locals[0].ty != parameter.ty
    {
        return Err("unsupported Body IR checked Deserialize hook owned str parameter".into());
    }
    if !matches!(body.locals[0].origin, LocalOrigin::Parameter) {
        return Err("unsupported Body IR checked Deserialize hook receiver".into());
    }
    if !matches!(parameter.default, CallableParamDefault::Required) {
        return Err("unsupported Body IR checked Deserialize hook parameter default".into());
    }
    if let IncanType::Generic { base, args } = &body.return_type
        && collection_type(base) == Some(CollectionTypeId::Result)
        && args.len() == 2
        && args[1] == IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError)
    {
        return Ok(());
    }
    Err("unsupported Body IR checked Deserialize hook canonical Result".into())
}
