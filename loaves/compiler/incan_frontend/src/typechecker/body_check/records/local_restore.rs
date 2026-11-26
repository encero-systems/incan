//! Restoration of one local record row into the native local maps it names.

use super::*;
use crate::symbols::ResolvedType;

/// The remaining typed fields of one local record row, after its kind, span and subkind.
type Values<'row> = std::slice::Iter<'row, String>;

/// Recreate every supported local map directly from the selected field's typed primitives.
pub(super) fn restore_local(
    checker: &TypeChecker,
    positions: &Positions,
    local: &mut BodyLocal,
    record_identities: &mut BTreeMap<String, CanonicalSymbolId>,
    row: &[String],
) -> Result<(), NodeBindingError> {
    if restore_delegated_row(positions, local, row)? {
        return Ok(());
    }
    let span = positions.span(&row[1])?;
    let mut values = row[3..].iter();
    match row[0].as_str() {
        "identity-context" if row[2] == "reference" => {
            restore_identity_context(positions, local, record_identities, span, &mut values)?;
        }
        "reference" if row[2] == "read" => {
            let identity = next(&mut values)?;
            let canonical = match restore_rust_field_reference(checker, local, span, identity) {
                Some(canonical) => canonical,
                None => restored_identity(positions, record_identities, identity)?,
            };
            local.references.resolved_identities.insert(span, canonical);
        }
        "write" => {
            local
                .references
                .resolved_write_identities
                .insert((span, row[2].clone()), positions.canonical(next(&mut values)?)?);
        }
        "binding" if row[2] == "declaration" => restore_binding_declaration(positions, local, span, &mut values)?,
        "identifier" if row[2] == "kind" => {
            let kind = read_ident_kind(&mut values)?;
            if local.expressions.ident_kinds.insert(span, kind).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "type-identity" if row.len() == 4 => {
            restore_type_identity(checker, positions, local, span, &row[2], &mut values)?;
        }
        "constructor" if row[2] == "field-binding" => {
            restore_constructor_field_binding(local, span, row.len(), &mut values)?;
        }
        "requirement" if row[2] == "constructor-field" => {
            restore_constructor_field_requirement(positions, local, span, &mut values)?;
        }
        "builtin" if row[2] == "call" => {
            let builtin =
                incan_lang::lang::builtins::from_str(next(&mut values)?).ok_or(NodeBindingError::MissingNode)?;
            let canonical =
                crate::symbols::canonical_builtin_function_identity(builtin).ok_or(NodeBindingError::MissingNode)?;
            retain_registry_identity(record_identities, canonical)?;
            local.calls.resolved_builtin_calls.insert(span, builtin);
        }
        "builtin" if row[2] == "method" => {
            let builtin = incan_lang::lang::builtins::method_from_parts(next(&mut values)?, next(&mut values)?)
                .ok_or(NodeBindingError::MissingNode)?;
            retain_builtin_method_identity(record_identities, builtin)?;
            local.calls.resolved_builtin_method_calls.insert(span, builtin);
        }
        "type" | "binding-type" | "parameter" | "native-value" | "native-call" | "signature" => {
            restore_type_fact(positions, local, span, row, &mut values)?;
        }
        "target" if row[2] == "source" => {
            let name = next(&mut values)?.to_owned();
            let kind = next(&mut values)?.to_owned();
            let module_path = values.by_ref().cloned().collect();
            local.expressions.source_targets.insert(
                span,
                super::super::super::type_info::SourceTargetInfo {
                    name,
                    kind,
                    module_path,
                },
            );
        }
        _ => return Err(NodeBindingError::Unsupported("canonical field")),
    }
    if values.next().is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Restore one checked type fact of the local at `span`: an expression, binding, parameter or native type, or a
/// call signature. Facts the local already holds for a binding, native value or native call are refused.
fn restore_type_fact(
    positions: &Positions,
    local: &mut BodyLocal,
    span: Span,
    row: &[String],
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    match row[0].as_str() {
        "type" => {
            let ty = read_type(values)?;
            let map = match row[2].as_str() {
                "expr" => &mut local.expressions.expr_types,
                "body-expr" => &mut local.expressions.body_ir_expr_types,
                "assignment" => &mut local.expressions.assignment_binding_types,
                "destination" => &mut local.expressions.value_destination_types,
                _ => return Err(NodeBindingError::MissingNode),
            };
            map.insert(span, ty);
        }
        "binding-type" if row[2] == "body" => {
            let binding = positions.canonical(next(values)?)?;
            if span_of(&binding) != span {
                return Err(NodeBindingError::WrongOwner);
            }
            if local
                .expressions
                .body_ir_binding_types
                .insert(binding, read_type(values)?)
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "binding-type" if row[2] == "parameter-requirement" => {
            restore_parameter_requirement(positions, local, span, values)?;
        }
        "parameter" => {
            local
                .references
                .resolved_write_types
                .insert((span, row[2].clone()), read_type(values)?);
        }
        "native-value" if row[2] == "type" => {
            let identity = positions.canonical(next(values)?)?;
            if span_of(&identity) != span
                || local
                    .rust
                    .value_native_types
                    .insert(identity, read_type(values)?)
                    .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "native-call" if matches!(row[2].as_str(), "argument-type" | "result-type") => {
            let map = if row[2] == "argument-type" {
                &mut local.calls.body_ir_native_argument_types
            } else {
                &mut local.calls.body_ir_call_result_types
            };
            if map.insert(span, read_type(values)?).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "signature" => restore_signature(local, span, &row[2], values)?,
        _ => return Err(NodeBindingError::Unsupported("canonical field")),
    }
    Ok(())
}

/// Restore a row whose kind has its own restorer: the generated call, Rust and protocol artifact codecs, the input
/// rows, and the scope allocation, which must equal the selected local's. Returns whether the row was one of them.
fn restore_delegated_row(
    positions: &Positions,
    local: &mut BodyLocal,
    row: &[String],
) -> Result<bool, NodeBindingError> {
    if call_artifacts_generated::restore(positions, &mut local.calls, row)? {
        return Ok(true);
    }
    if rust_artifacts_generated::restore(positions, &mut local.rust, row)? {
        return Ok(true);
    }
    if protocol_artifacts_generated::restore(positions, &mut local.protocols, row)? {
        return Ok(true);
    }
    if matches!(
        row[0].as_str(),
        "input-call" | "input-local" | "input-flow" | "input-copy"
    ) {
        inputs::restore(positions, local, row)?;
        return Ok(true);
    }
    if row[0] == "scope" {
        if positions.scopes.as_ref().is_none_or(|layout| layout.row() != row) {
            return Err(NodeBindingError::InvalidField(
                "scope allocation differs from selected local".into(),
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

/// Restore the checked type requirement of one parameter binding declared at `span`, or its recorded absence. A
/// parameter has at most one requirement.
fn restore_parameter_requirement(
    positions: &Positions,
    local: &mut BodyLocal,
    span: Span,
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    let binding = positions.canonical(next(values)?)?;
    if span_of(&binding) != span || binding.kind != incan_semantics_core::SemanticSourceTargetKind::Parameter {
        return Err(NodeBindingError::WrongOwner);
    }
    let requirement = match next(values)? {
        "none" => None,
        "some" => Some(read_type(values)?),
        _ => return Err(NodeBindingError::MissingNode),
    };
    if local.expressions.body_ir_parameter_requirements.contains_key(&binding) {
        return Err(NodeBindingError::MissingNode);
    }
    local
        .expressions
        .body_ir_parameter_requirements
        .insert(binding, requirement);
    Ok(())
}

/// Restore a nested binding declared at `span`. When the row names a scope ordinal, the binding's physical scope must
/// be the one the selected local allocated for it.
fn restore_binding_declaration(
    positions: &Positions,
    local: &mut BodyLocal,
    span: Span,
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    let canonical = positions.canonical(next(values)?)?;
    if let Some(ordinal) = values.next() {
        let scope = positions
            .scopes
            .as_ref()
            .ok_or(NodeBindingError::MissingOwner)?
            .binding_scope(ordinal)?;
        if canonical.scope_discriminant.map(|physical| physical.0) != Some(scope) {
            return Err(NodeBindingError::WrongOwner);
        }
    }
    local.nested.insert(span, canonical);
    Ok(())
}

/// Read the checked kind of an identifier from its record spelling.
fn read_ident_kind(values: &mut Values<'_>) -> Result<super::super::super::type_info::IdentKind, NodeBindingError> {
    use super::super::super::type_info::IdentKind;

    Ok(match next(values)? {
        "value" => IdentKind::Value,
        "static" => IdentKind::Static,
        "type-name" => IdentKind::TypeName,
        "variant" => IdentKind::Variant,
        "module" => IdentKind::Module,
        "rust-import" => IdentKind::RustImport,
        "rust-value" => IdentKind::RustValue,
        "trait" => IdentKind::Trait,
        _ => return Err(NodeBindingError::MissingNode),
    })
}

/// Restore the identity of one type component of the expression at `span`. A stable identity the record positions do
/// not hold may still name an unbound Rust item the current ABI validates. Each component is restored once.
fn restore_type_identity(
    checker: &TypeChecker,
    positions: &Positions,
    local: &mut BodyLocal,
    span: Span,
    path: &str,
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    let path = read_type_identity_path(path)?;
    let stable = next(values)?;
    let identity = positions
        .canonical_type(stable)
        .or_else(|error| restore_native_expression_identity(checker, local, span, &path, stable).ok_or(error))?;
    if local
        .expressions
        .expression_type_identities
        .entry(span)
        .or_default()
        .insert(path, identity)
        .is_some()
    {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Restore how a constructor call at `span` binds its fields. Argument slots and strictly ascending defaulted slots
/// must together cover every field exactly once, and each constructor call has one binding.
fn restore_constructor_field_binding(
    local: &mut BodyLocal,
    span: Span,
    row_len: usize,
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    let field_count = read_number(values)?;
    let argument_count = read_number(values)?;
    if field_count > row_len || argument_count > values.len() {
        return Err(NodeBindingError::MissingNode);
    }
    let argument_slots = (0..argument_count)
        .map(|_| read_number(values))
        .collect::<Result<Vec<_>, _>>()?;
    let default_count = read_number(values)?;
    if default_count > values.len() {
        return Err(NodeBindingError::MissingNode);
    }
    let defaulted_slots = (0..default_count)
        .map(|_| read_number(values))
        .collect::<Result<Vec<_>, _>>()?;
    let mut occupied = argument_slots.clone();
    occupied.extend(defaulted_slots.iter().copied());
    occupied.sort_unstable();
    if occupied != (0..field_count).collect::<Vec<_>>()
        || !defaulted_slots.windows(2).all(|pair| pair[0] < pair[1])
        || local
            .calls
            .constructor_field_bindings
            .insert(
                span,
                super::super::super::type_info::ConstructorFieldBinding {
                    argument_slots,
                    defaulted_slots,
                    field_count,
                },
            )
            .is_some()
    {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Restore the field a constructor call at `span` requires and the model or class that owns it.
fn restore_constructor_field_requirement(
    positions: &Positions,
    local: &mut BodyLocal,
    span: Span,
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    let field = positions.canonical(next(values)?)?;
    let owner = positions.canonical_type(next(values)?)?;
    if field.kind != SemanticSourceTargetKind::Field
        || !matches!(
            owner.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
        )
        || local
            .calls
            .body_ir_constructor_field_requirements
            .insert(span, (field, owner))
            .is_some()
    {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Restore one signature fact of the call at `span`: its monomorph or Body IR type arguments, or its call-site or
/// Body IR parameter types.
fn restore_signature(
    local: &mut BodyLocal,
    span: Span,
    subkind: &str,
    values: &mut Values<'_>,
) -> Result<(), NodeBindingError> {
    if matches!(subkind, "monomorphs" | "type-arguments") {
        let count = next(values)?
            .parse::<usize>()
            .map_err(|_| NodeBindingError::MissingNode)?;
        if count > values.len() {
            return Err(NodeBindingError::MissingNode);
        }
        let arguments = (0..count).map(|_| read_type(values)).collect::<Result<Vec<_>, _>>()?;
        let map = if subkind == "monomorphs" {
            &mut local.calls.call_site_monomorph_type_args
        } else {
            &mut local.calls.body_ir_call_type_arguments
        };
        map.insert(span, arguments);
        return Ok(());
    }
    let parameters = read_parameters(values)?;
    let map = match subkind {
        "call" => &mut local.calls.call_site_callable_params,
        "body-call" => &mut local.calls.body_ir_call_parameter_types,
        _ => return Err(NodeBindingError::MissingNode),
    };
    map.insert(span, parameters);
    Ok(())
}

/// Rebind one checked field occurrence against the public field of its currently admitted Rust ABI owner.
/// Keep its native owner layout in the pending restoration until the entire record is admitted.
fn restore_rust_field_reference(
    checker: &TypeChecker,
    local: &mut BodyLocal,
    span: Span,
    identity: &str,
) -> Option<CanonicalSymbolId> {
    use incan_lang::interop::RustItemKind;

    let (path, name) = identity.strip_prefix("member/field:rust::")?.rsplit_once("::")?;
    local.rust.field_values.get(&span)?;
    let owner = incan_semantics_core::body_ir::rust_item_identity(path)?;
    let metadata = checker.rust_item_metadata_for_path(path)?;
    let RustItemKind::Type(info) = &metadata.kind else {
        return None;
    };
    let field = TypeChecker::rust_field_for_source_name(info, name)?;
    let canonical = owner.generated_member(&field.name, SemanticSourceTargetKind::Field);
    let stable = StableDeclarationId::from_canonical(&canonical, None, None)?;
    if stable.render_compact() != identity {
        return None;
    }
    let existing = local
        .rust
        .checked_layouts
        .get(path)
        .or_else(|| checker.type_info.rust.checked_layouts.get(path));
    if let Some(layout) = checker.rust_layout_from_metadata(path, info, existing) {
        local.rust.checked_layouts.insert(path.to_owned(), layout);
    }
    Some(canonical)
}

/// Restore an unbound Rust item only at the decoded expression's exact type component. The current ABI must
/// validate its identity. The root of a checked native call can also use the result item that the call witnessed.
fn restore_native_expression_identity(
    checker: &TypeChecker,
    local: &BodyLocal,
    span: Span,
    path: &[usize],
    stable: &str,
) -> Option<CanonicalSymbolId> {
    let agrees = |canonical: &CanonicalSymbolId| {
        StableDeclarationId::from_canonical(canonical, None, None)
            .is_some_and(|candidate| candidate.render_compact() == stable)
    };
    if path.is_empty()
        && let Some(item) = local
            .rust
            .checked_calls
            .get(&span)
            .and_then(|call| call.result_item_path.as_deref())
            .and_then(incan_semantics_core::body_ir::rust_item_identity)
            .filter(agrees)
    {
        return Some(item);
    }

    let ty = local.expressions.expr_types.get(&span)?;
    let leaf = path.iter().try_fold(ty, |ty, &component| match ty {
        ResolvedType::Generic(_, arguments) | ResolvedType::Tuple(arguments) => arguments.get(component),
        ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
            if component == 0 =>
        {
            Some(inner.as_ref())
        }
        ResolvedType::FrozenDict(key, value) => [key.as_ref(), value.as_ref()].get(component).copied(),
        _ => None,
    })?;
    let ResolvedType::RustPath(native) = leaf else {
        return None;
    };
    checker.native_expression_leaf_identity(native).filter(agrees)
}
