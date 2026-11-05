//! C binding record ids and records.

use super::*;

/// Construct the stable record identity shared by a checked C binding declaration and every raw call that selects it.
pub(super) fn c_binding_record_id(module_id: &str, binding: &str) -> String {
    format!("c_binding:{module_id}:{}", sanitize_record_label(binding))
}

/// Construct a deterministic identity for one checked raw C call.
pub(super) fn c_binding_call_record_id(module_id: &str, binding: &str, symbol: &str, span: Span) -> String {
    format!(
        "c_binding_call:{module_id}:{}:{}:{}:{}",
        sanitize_record_label(binding),
        sanitize_record_label(symbol),
        span.start,
        span.end
    )
}

/// Construct a deterministic identity for one compiler-proven facade-to-bridge relation.
pub(super) fn c_binding_facade_record_id(
    module_id: &str,
    facade: &incan_frontend::typechecker::CBindingRawCallOwner,
    bridge: &incan_frontend::typechecker::CBindingRawCallOwner,
    call_span: Span,
) -> String {
    format!(
        "c_binding_facade:{module_id}:{}:{}:{}:{}",
        sanitize_record_label(&facade.name),
        sanitize_record_label(&bridge.name),
        call_span.start,
        call_span.end
    )
}

/// Return the ordinary class declaration record that owns one compiler-checked C binding descriptor.
///
/// A descriptor is expected to originate from the vocabulary-desugared class declaration. When a partially parsed
/// module cannot supply that source declaration, omit the incomplete projection rather than crashing the codegraph
/// command or publishing a dangling declaration reference.
pub(super) fn c_binding_declaration_id(module: &ParsedModule, binding_name: &str) -> Option<String> {
    module
        .ast
        .declarations
        .iter()
        .enumerate()
        .find_map(|(index, declaration)| match &declaration.node {
            Declaration::Class(class) if class.name == binding_name => Some(declaration_id(module, declaration, index)),
            _ => None,
        })
}

/// Return the ordinary function declaration record that compiler typechecking retained as the owner of one raw call.
///
/// A missing record is represented as absent rather than guessed: methods and malformed tolerant-mode input gain no
/// fabricated bridge relationship.
pub(super) fn c_binding_raw_call_owner_declaration_id(
    module: &ParsedModule,
    owner: &incan_frontend::typechecker::CBindingRawCallOwner,
) -> Option<String> {
    module
        .ast
        .declarations
        .iter()
        .enumerate()
        .find_map(|(index, declaration)| match &declaration.node {
            Declaration::Function(function)
                if function.name == owner.name && declaration.span == owner.declaration_span =>
            {
                Some(declaration_id(module, declaration, index))
            }
            _ => None,
        })
}

/// Convert one opaque-resource declaration into the public codegraph vocabulary.
pub(super) fn c_binding_resource_record(resource: &CBindingResource) -> CodegraphCBindingResource {
    CodegraphCBindingResource {
        name: resource.name.clone(),
        native: resource.native.clone(),
        release: resource.release.clone(),
    }
}

/// Convert one checked native symbol contract into the public codegraph vocabulary.
pub(super) fn c_binding_symbol_record(symbol: &CBindingSymbol) -> CodegraphCBindingSymbol {
    CodegraphCBindingSymbol {
        name: symbol.name.clone(),
        native: symbol.native.clone(),
        parameters: symbol.parameters.iter().map(c_binding_parameter_record).collect(),
        return_type: c_binding_type_record(&symbol.return_type),
        buffers: symbol.buffers.iter().map(c_binding_buffer_record).collect(),
        outcomes: symbol.outcomes.iter().map(c_binding_outcome_record).collect(),
    }
}

/// Convert one compiler-owned pointer-to-length association without inferring it from names or generated Rust.
pub(super) fn c_binding_buffer_record(buffer: &incan_frontend::typechecker::CBindingBuffer) -> CodegraphCBindingBuffer {
    CodegraphCBindingBuffer {
        pointer_parameter: buffer.pointer_parameter.clone(),
        length_parameter: buffer.length_parameter.clone(),
        element: scalar_type_as_str(buffer.element).to_string(),
    }
}

/// Convert one named C parameter contract into the public codegraph vocabulary.
pub(super) fn c_binding_parameter_record(parameter: &CBindingParameter) -> CodegraphCBindingParameter {
    CodegraphCBindingParameter {
        name: parameter.name.clone(),
        ty: c_binding_type_record(&parameter.ty),
    }
}

/// Convert one declared output-state transition into the public codegraph vocabulary.
pub(super) fn c_binding_outcome_record(outcome: &CBindingOutcome) -> CodegraphCBindingOutcome {
    CodegraphCBindingOutcome {
        result: outcome.result.clone(),
        initializes: outcome.initializes.clone(),
        updates: outcome.updates.clone(),
        invalidates: outcome.invalidates.clone(),
    }
}

/// Convert a checked C type structurally without leaking generated Rust spelling into the public graph.
pub(super) fn c_binding_type_record(ty: &CBindingType) -> CodegraphCBindingType {
    match ty {
        CBindingType::Scalar(scalar) => CodegraphCBindingType::Scalar {
            spelling: scalar_type_as_str(*scalar).to_string(),
        },
        CBindingType::Pointer { mutable, pointee } => CodegraphCBindingType::Pointer {
            mutable: *mutable,
            pointee: Box::new(c_binding_type_record(pointee)),
        },
        CBindingType::Struct(name) => CodegraphCBindingType::Struct { name: name.clone() },
        CBindingType::Resource { access, resource } => CodegraphCBindingType::Resource {
            access: c_resource_access_spelling(*access).to_string(),
            resource: resource.clone(),
        },
        CBindingType::Output { mode, value } => CodegraphCBindingType::Output {
            mode: c_output_mode_spelling(*mode).to_string(),
            value: Box::new(c_binding_type_record(value)),
        },
        CBindingType::Nullable(value) => CodegraphCBindingType::Nullable {
            value: Box::new(c_binding_type_record(value)),
        },
        CBindingType::Void => CodegraphCBindingType::Void,
    }
}

/// Return the stable codegraph spelling for a checked opaque-resource access mode.
pub(super) fn c_resource_access_spelling(access: CResourceAccess) -> &'static str {
    match access {
        CResourceAccess::Owned => "owned",
        CResourceAccess::Borrowed => "borrowed",
        CResourceAccess::BorrowedMut => "borrowed_mut",
    }
}

/// Return the stable codegraph spelling for a checked compiler-managed output mode.
pub(super) fn c_output_mode_spelling(mode: COutputMode) -> &'static str {
    match mode {
        COutputMode::Out => "out",
        COutputMode::InOut => "in_out",
    }
}

/// Convert one target-verified C enum declaration into the public codegraph vocabulary.
pub(super) fn c_binding_enum_record(enumeration: &CBindingEnum) -> CodegraphCBindingEnum {
    CodegraphCBindingEnum {
        name: enumeration.name.clone(),
        carrier: scalar_type_as_str(enumeration.carrier).to_string(),
        variants: enumeration.variants.iter().map(c_binding_enum_variant_record).collect(),
    }
}

/// Convert one target-verified native C enum constant into the public codegraph vocabulary.
pub(super) fn c_binding_enum_variant_record(variant: &CBindingEnumVariant) -> CodegraphCBindingEnumVariant {
    CodegraphCBindingEnumVariant {
        name: variant.name.clone(),
        native: variant.native.clone(),
    }
}

/// Convert one checked plain C structure declaration into the public codegraph vocabulary.
pub(super) fn c_binding_struct_record(structure: &CBindingStruct) -> CodegraphCBindingStruct {
    CodegraphCBindingStruct {
        name: structure.name.clone(),
        native: structure.native.clone(),
        fields: structure.fields.iter().map(c_binding_struct_field_record).collect(),
    }
}

/// Convert one checked plain C structure field into the public codegraph vocabulary.
pub(super) fn c_binding_struct_field_record(field: &CBindingStructField) -> CodegraphCBindingStructField {
    CodegraphCBindingStructField {
        name: field.name.clone(),
        ty: c_binding_type_record(&field.ty),
    }
}
