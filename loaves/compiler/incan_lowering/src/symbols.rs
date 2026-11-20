//! Identifier-safe encoding of checked module names and concrete native type arguments.

use incan_lang::lang::types::numerics::as_str as numeric_name;
use incan_semantics_core::{
    IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType, IncanType, NumericTypeId,
    RuntimeBuiltinType,
};

/// Keep source carrier names and encode opaque Rust path keys as native identifiers.
pub fn model_symbol(name: &str) -> String {
    if !name.contains("::") {
        return name.to_owned();
    }
    format!("__incan_rust{}", symbol_component(name))
}

/// Encode one length-delimited UTF-8 component without admitting punctuation into a native identifier.
pub fn symbol_component(value: &str) -> String {
    let encoded = value.as_bytes();
    let mut component = format!("_{}", encoded.len());
    for byte in encoded {
        component.push('_');
        component.push_str(&byte.to_string());
    }
    component
}

/// Encode every semantic type variant and field; diagnostic Display never establishes instance identity.
pub fn type_component(ty: &IncanType) -> String {
    match ty {
        IncanType::Never => symbol_component("!"),
        IncanType::Primitive(primitive) => primitive_component(primitive),
        IncanType::Named(name) => format!("_named{}", symbol_component(name)),
        IncanType::Trait(identity) => format!("_trait{}", symbol_component(&identity.render_compact())),
        IncanType::Generic { base, args } => {
            type_sequence(format!("{}_args_{}", symbol_component(base), args.len()), args)
        }
        IncanType::Decimal { precision, scale } => format!("_decimal_{precision}_{scale}"),
        IncanType::Function { params, return_type } => function_component(params, return_type),
        IncanType::TypeToken(inner) => format!("_type_token{}", type_component(inner)),
        IncanType::Tuple(elements) => type_sequence(format!("_tuple_{}", elements.len()), elements),
        IncanType::TypeVar(name) => format!("_parameter{}", symbol_component(name)),
        IncanType::SelfType => symbol_component("Self"),
        IncanType::Ref(inner) => format!("_shared_ref{}", type_component(inner)),
        IncanType::RefMut(inner) => format!("_mutable_ref{}", type_component(inner)),
        IncanType::RustInteropPath(path) => format!("_rust_path{}", symbol_component(path)),
        IncanType::NativeBuiltin(builtin) => builtin_component(builtin),
        IncanType::Unsupported(reason) => format!("_unsupported{}", symbol_component(reason)),
        IncanType::Infer => symbol_component("_"),
        IncanType::Unknown => symbol_component("?"),
        IncanType::NativeArray { element, length } => format!("_native_array_{length}{}", type_component(element)),
        IncanType::NativeTraitObject { path, send, sync } => {
            format!("_trait_object{}{}{}", symbol_component(path), flag_component(*send), flag_component(*sync))
        }
        IncanType::NativePointer { mutable, pointee } => {
            format!("_native_pointer{}{}", flag_component(*mutable), type_component(pointee))
        }
        IncanType::NativeCChar => symbol_component("c.c_char"),
    }
}

fn type_sequence(prefix: String, types: &[IncanType]) -> String {
    let mut encoded = prefix;
    for ty in types {
        encoded.push_str(&type_component(ty));
    }
    encoded
}

/// Keep established scalar names while distinguishing numeric Bool from the ordinary Bool variant.
fn primitive_component(primitive: &IncanPrimitiveType) -> String {
    match primitive {
        IncanPrimitiveType::Int => symbol_component("int"),
        IncanPrimitiveType::Float => symbol_component("float"),
        IncanPrimitiveType::Numeric(numeric) => {
            if *numeric == NumericTypeId::Bool {
                return "_numeric_bool".to_owned();
            }
            symbol_component(numeric_name(*numeric))
        }
        IncanPrimitiveType::Bool => symbol_component("bool"),
        IncanPrimitiveType::Str => symbol_component("str"),
        IncanPrimitiveType::Bytes => symbol_component("bytes"),
        IncanPrimitiveType::FrozenStr => symbol_component("FrozenStr"),
        IncanPrimitiveType::FrozenBytes => symbol_component("FrozenBytes"),
        IncanPrimitiveType::Unit => symbol_component("Unit"),
    }
}

fn builtin_component(builtin: &RuntimeBuiltinType) -> String {
    match builtin {
        RuntimeBuiltinType::ValidationError => symbol_component("runtime::ValidationError"),
    }
}

/// Retain callable arity, parameter metadata and output; unequal checked signatures cannot share a symbol.
fn function_component(parameters: &[IncanCallableParam], result: &IncanType) -> String {
    let mut encoded = format!("_function_{}", parameters.len());
    for parameter in parameters {
        encoded.push_str(&parameter_component(parameter));
    }
    format!("{encoded}_returns{}", type_component(result))
}

/// Encode all equality-bearing parameter fields, including the collection ownership and partial-preset facts.
fn parameter_component(parameter: &IncanCallableParam) -> String {
    let mut encoded = format!("_param{}{}", type_component(&parameter.ty), parameter_kind_component(parameter.kind));
    match &parameter.name {
        None => encoded.push_str("_unnamed"),
        Some(name) => {
            encoded.push_str("_named");
            encoded.push_str(&symbol_component(name));
        }
    }
    format!(
        "{encoded}{}{}{}{}{}",
        flag_component(parameter.has_default),
        flag_component(parameter.is_partial_preset),
        flag_component(parameter.is_mut),
        flag_component(parameter.native_owned),
        input_access_component(parameter.input_access)
    )
}

fn input_access_component(access: IncanInputAccess) -> &'static str {
    match access {
        IncanInputAccess::Inferred => "_access_inferred",
        IncanInputAccess::InferredShared => "_access_inferred_shared",
        IncanInputAccess::Shared => "_access_shared",
        IncanInputAccess::Exclusive => "_access_exclusive",
        IncanInputAccess::Owned => "_access_owned",
    }
}

fn parameter_kind_component(kind: IncanCallableParamKind) -> &'static str {
    match kind {
        IncanCallableParamKind::Normal => "_normal",
        IncanCallableParamKind::RestPositional => "_rest_positional",
        IncanCallableParamKind::RestKeyword => "_rest_keyword",
    }
}

fn flag_component(value: bool) -> &'static str {
    if value { "_1" } else { "_0" }
}
