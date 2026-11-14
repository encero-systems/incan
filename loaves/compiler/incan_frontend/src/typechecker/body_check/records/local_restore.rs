//! Restoration of one local record row into the native local maps it names.

use super::*;

/// Recreate every supported local map directly from the selected field's typed primitives.
pub(super) fn restore_local(
    positions: &Positions,
    local: &mut BodyLocal,
    record_identities: &mut BTreeMap<String, CanonicalSymbolId>,
    row: &[String],
) -> Result<(), NodeBindingError> {
    if call_artifacts_generated::restore(positions, &mut local.calls, row)? {
        return Ok(());
    }
    if rust_artifacts_generated::restore(positions, &mut local.rust, row)? {
        return Ok(());
    }
    if protocol_artifacts_generated::restore(positions, &mut local.protocols, row)? {
        return Ok(());
    }
    if matches!(
        row[0].as_str(),
        "input-call" | "input-local" | "input-flow" | "input-copy"
    ) {
        return inputs::restore(positions, local, row);
    }
    let span = positions.span(&row[1])?;
    let mut values = row[3..].iter();
    match row[0].as_str() {
        "type" => {
            let ty = read_type(&mut values)?;
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
            let binding = positions.canonical(next(&mut values)?)?;
            if span_of(&binding) != span {
                return Err(NodeBindingError::WrongOwner);
            }
            if local
                .expressions
                .body_ir_binding_types
                .insert(binding, read_type(&mut values)?)
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "binding-type" if row[2] == "parameter-requirement" => {
            let binding = positions.canonical(next(&mut values)?)?;
            if span_of(&binding) != span || binding.kind != incan_semantics_core::SemanticSourceTargetKind::Parameter {
                return Err(NodeBindingError::WrongOwner);
            }
            let requirement = match next(&mut values)? {
                "none" => None,
                "some" => Some(read_type(&mut values)?),
                _ => return Err(NodeBindingError::MissingNode),
            };
            if local.expressions.body_ir_parameter_requirements.contains_key(&binding) {
                return Err(NodeBindingError::MissingNode);
            }
            local
                .expressions
                .body_ir_parameter_requirements
                .insert(binding, requirement);
        }
        "parameter" => {
            local
                .references
                .resolved_write_types
                .insert((span, row[2].clone()), read_type(&mut values)?);
        }
        "native-value" if row[2] == "type" => {
            let identity = positions.canonical(next(&mut values)?)?;
            if span_of(&identity) != span
                || local
                    .rust
                    .value_native_types
                    .insert(identity, read_type(&mut values)?)
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
            if map.insert(span, read_type(&mut values)?).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "identity-context" if row[2] == "reference" => {
            restore_identity_context(positions, local, record_identities, span, &mut values)?;
        }
        "reference" if row[2] == "read" => {
            let identity = next(&mut values)?;
            let canonical = restored_identity(positions, record_identities, identity)?;
            local.references.resolved_identities.insert(span, canonical);
        }
        "write" => {
            local
                .references
                .resolved_write_identities
                .insert((span, row[2].clone()), positions.canonical(next(&mut values)?)?);
        }
        "binding" if row[2] == "declaration" => {
            local.nested.insert(span, positions.canonical(next(&mut values)?)?);
        }
        "identifier" if row[2] == "kind" => {
            let kind = match next(&mut values)? {
                "value" => super::super::super::type_info::IdentKind::Value,
                "static" => super::super::super::type_info::IdentKind::Static,
                "type-name" => super::super::super::type_info::IdentKind::TypeName,
                "variant" => super::super::super::type_info::IdentKind::Variant,
                "module" => super::super::super::type_info::IdentKind::Module,
                "rust-import" => super::super::super::type_info::IdentKind::RustImport,
                "rust-value" => super::super::super::type_info::IdentKind::RustValue,
                "trait" => super::super::super::type_info::IdentKind::Trait,
                _ => return Err(NodeBindingError::MissingNode),
            };
            if local.expressions.ident_kinds.insert(span, kind).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "type-identity" if row.len() == 4 => {
            let path = read_type_identity_path(&row[2])?;
            let identity = positions.canonical_type(next(&mut values)?)?;
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
        }
        "constructor" if row[2] == "field-binding" => {
            let field_count = read_number(&mut values)?;
            let argument_count = read_number(&mut values)?;
            if field_count > row.len() || argument_count > values.len() {
                return Err(NodeBindingError::MissingNode);
            }
            let argument_slots = (0..argument_count)
                .map(|_| read_number(&mut values))
                .collect::<Result<Vec<_>, _>>()?;
            let default_count = read_number(&mut values)?;
            if default_count > values.len() {
                return Err(NodeBindingError::MissingNode);
            }
            let defaulted_slots = (0..default_count)
                .map(|_| read_number(&mut values))
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
        }
        "requirement" if row[2] == "constructor-field" => {
            let field = positions.canonical(next(&mut values)?)?;
            let owner = positions.canonical_type(next(&mut values)?)?;
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
        }
        "builtin" if row[2] == "call" => {
            let builtin =
                incan_lang::lang::builtins::from_str(next(&mut values)?).ok_or(NodeBindingError::MissingNode)?;
            local.calls.resolved_builtin_calls.insert(span, builtin);
        }
        "builtin" if row[2] == "method" => {
            let builtin = incan_lang::lang::builtins::method_from_parts(next(&mut values)?, next(&mut values)?)
                .ok_or(NodeBindingError::MissingNode)?;
            retain_builtin_method_identity(record_identities, builtin)?;
            local.calls.resolved_builtin_method_calls.insert(span, builtin);
        }
        "signature" if matches!(row[2].as_str(), "monomorphs" | "type-arguments") => {
            let count = next(&mut values)?
                .parse::<usize>()
                .map_err(|_| NodeBindingError::MissingNode)?;
            if count > values.len() {
                return Err(NodeBindingError::MissingNode);
            }
            let arguments = (0..count)
                .map(|_| read_type(&mut values))
                .collect::<Result<Vec<_>, _>>()?;
            let map = if row[2] == "monomorphs" {
                &mut local.calls.call_site_monomorph_type_args
            } else {
                &mut local.calls.body_ir_call_type_arguments
            };
            map.insert(span, arguments);
        }
        "signature" => {
            let parameters = read_parameters(&mut values)?;
            let map = match row[2].as_str() {
                "call" => &mut local.calls.call_site_callable_params,
                "body-call" => &mut local.calls.body_ir_call_parameter_types,
                _ => return Err(NodeBindingError::MissingNode),
            };
            map.insert(span, parameters);
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
