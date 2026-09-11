//! Canonical handling for checked Rust type displays that cross compiled-library boundaries.

use std::collections::BTreeSet;

use quote::ToTokens;
use syn::{GenericArgument, PathArguments, ReturnType, Type, TypeParamBound};

use crate::frontend::symbols::ResolvedType;
use crate::frontend::typechecker::split_canonical_public_library_type_name;

pub(crate) const PROVIDER_RUST_BRIDGE_MODULE: &str = "__incan_provider_rust";

/// Build the physical module route from an importing library through each admitted bridge hop to an owner.
///
/// One bridge segment precedes each dependency crossed, so a transitive facade renders every hop it actually went
/// through rather than collapsing to the final owner. Callers differ only in the prefix they put in front --
/// `::` for a Rust path, `pub::` for a typechecker binding key -- so the segments are built here and the prefix
/// stays at the call site. Two callers previously grew their own copy of this loop and were reconciled afterwards
/// by stripping one prefix to produce the other, which meant the convention lived in three places at once.
pub(crate) fn provider_bridge_route(library: &str, dependencies: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut route = vec![library.to_string()];
    for dependency in dependencies {
        route.push(PROVIDER_RUST_BRIDGE_MODULE.to_string());
        route.push(dependency);
    }
    route
}

#[derive(Clone, Copy)]
enum Qualification<'a> {
    Emission { provider: Option<&'a str> },
    Manifest { provider: &'a str },
}

/// Normalize every Rust display nested in one resolved type for generated declaration emission.
pub(crate) fn normalize_for_emission(
    ty: &mut ResolvedType,
    provider: Option<&str>,
    type_params: &[String],
) -> Result<(), String> {
    transform_resolved_type(ty, Qualification::Emission { provider }, type_params)
}

/// Qualify provider-owned Rust displays so a compiled subclass can republish them through its own dependency bridge.
pub(crate) fn qualify_for_manifest(
    ty: &ResolvedType,
    provider: Option<&str>,
    type_params: &[String],
) -> Result<ResolvedType, String> {
    let mut qualified = ty.clone();
    if let Some(provider) = provider {
        transform_resolved_type(&mut qualified, Qualification::Manifest { provider }, type_params)?;
    }
    Ok(qualified)
}

/// Return direct crate roots required by Rust displays in one local public class field.
pub(crate) fn public_bridge_roots(ty: &ResolvedType, type_params: &[String]) -> Result<BTreeSet<String>, String> {
    let mut roots = BTreeSet::new();
    collect_resolved_type_roots(ty, type_params, &mut roots)?;
    Ok(roots)
}

/// Apply one emission or manifest qualification policy recursively to a checked semantic type.
fn transform_resolved_type(
    ty: &mut ResolvedType,
    qualification: Qualification<'_>,
    type_params: &[String],
) -> Result<(), String> {
    match ty {
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => transform_resolved_type(inner, qualification, type_params),
        ResolvedType::FrozenDict(key, value) => {
            transform_resolved_type(key, qualification, type_params)?;
            transform_resolved_type(value, qualification, type_params)
        }
        ResolvedType::Generic(name, args) => {
            qualify_nominal_name(name, qualification);
            for arg in args {
                transform_resolved_type(arg, qualification, type_params)?;
            }
            Ok(())
        }
        ResolvedType::Tuple(args) => {
            for arg in args {
                transform_resolved_type(arg, qualification, type_params)?;
            }
            Ok(())
        }
        ResolvedType::Function(params, ret) => {
            for param in params {
                transform_resolved_type(&mut param.ty, qualification, type_params)?;
            }
            transform_resolved_type(ret, qualification, type_params)
        }
        ResolvedType::RustPath(display) => {
            *display = transform_display(display, qualification, type_params)?;
            Ok(())
        }
        ResolvedType::Named(name) => {
            qualify_nominal_name(name, qualification);
            Ok(())
        }
        ResolvedType::Never
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Numeric(_)
        | ResolvedType::Bool
        | ResolvedType::Str
        | ResolvedType::Bytes
        | ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes
        | ResolvedType::Unit
        | ResolvedType::TypeVar(_)
        | ResolvedType::SelfType
        | ResolvedType::CallSiteInfer
        | ResolvedType::Unknown => Ok(()),
    }
}

/// Rebase one structured provider-owned nominal name for the selected artifact boundary.
fn qualify_nominal_name(name: &mut String, qualification: Qualification<'_>) {
    if let Some(path) = name
        .strip_prefix(PROVIDER_RUST_BRIDGE_MODULE)
        .and_then(|path| path.strip_prefix("::"))
    {
        *name = match qualification {
            Qualification::Emission { provider: None } => return,
            Qualification::Emission {
                provider: Some(provider),
            } => format!(
                "::{}::{PROVIDER_RUST_BRIDGE_MODULE}::{path}",
                escaped_crate_name(provider)
            ),
            Qualification::Manifest { provider } => format!(
                "{PROVIDER_RUST_BRIDGE_MODULE}::{}::{PROVIDER_RUST_BRIDGE_MODULE}::{path}",
                escaped_crate_name(provider)
            ),
        };
        return;
    }
    let Some((dependency, public_name)) = split_canonical_public_library_type_name(name) else {
        return;
    };
    let dependency_matches_provider = match qualification {
        Qualification::Emission { provider } => provider == Some(dependency),
        Qualification::Manifest { provider } => provider == dependency,
    };
    let dependency = escaped_crate_name(dependency);
    *name = match qualification {
        Qualification::Emission { provider: None } => return,
        Qualification::Emission {
            provider: Some(provider),
        } if dependency_matches_provider => format!("::{}::{public_name}", escaped_crate_name(provider)),
        Qualification::Emission {
            provider: Some(provider),
        } => format!(
            "::{}::{PROVIDER_RUST_BRIDGE_MODULE}::{dependency}::{public_name}",
            escaped_crate_name(provider)
        ),
        Qualification::Manifest { provider } if dependency_matches_provider => {
            format!(
                "{PROVIDER_RUST_BRIDGE_MODULE}::{}::{public_name}",
                escaped_crate_name(provider)
            )
        }
        Qualification::Manifest { provider } => format!(
            "{PROVIDER_RUST_BRIDGE_MODULE}::{}::{PROVIDER_RUST_BRIDGE_MODULE}::{dependency}::{public_name}",
            escaped_crate_name(provider)
        ),
    };
}

/// Parse, qualify, and render one complete checked Rust type display.
fn transform_display(
    display: &str,
    qualification: Qualification<'_>,
    type_params: &[String],
) -> Result<String, String> {
    let canonical = display.strip_prefix("rust::").unwrap_or(display);
    let mut ty = syn::parse_str::<Type>(canonical)
        .map_err(|error| format!("invalid checked Rust type display `{display}`: {error}"))?;
    transform_type_paths(&mut ty, qualification, type_params)?;
    Ok(ty.to_token_stream().to_string())
}

/// Rebase every path reachable through one parsed Rust type while preserving its complete syntax.
fn transform_type_paths(ty: &mut Type, qualification: Qualification<'_>, type_params: &[String]) -> Result<(), String> {
    match ty {
        Type::Array(array) => transform_type_paths(&mut array.elem, qualification, type_params),
        Type::BareFn(function) => {
            for input in &mut function.inputs {
                transform_type_paths(&mut input.ty, qualification, type_params)?;
            }
            if let ReturnType::Type(_, output) = &mut function.output {
                transform_type_paths(output, qualification, type_params)?;
            }
            Ok(())
        }
        Type::Group(group) => transform_type_paths(&mut group.elem, qualification, type_params),
        Type::ImplTrait(implementation) => {
            for bound in &mut implementation.bounds {
                transform_type_bound(bound, qualification, type_params)?;
            }
            Ok(())
        }
        Type::Paren(paren) => transform_type_paths(&mut paren.elem, qualification, type_params),
        Type::Path(path) => {
            if let Some(qualified) = &mut path.qself {
                transform_type_paths(&mut qualified.ty, qualification, type_params)?;
                let old_len = path.path.segments.len();
                transform_path(&mut path.path, qualification, type_params)?;
                let new_len = path.path.segments.len();
                qualified.position = qualified.position + new_len - old_len;
            } else {
                transform_path(&mut path.path, qualification, type_params)?;
            }
            transform_path_arguments(&mut path.path, qualification, type_params)
        }
        Type::Ptr(pointer) => transform_type_paths(&mut pointer.elem, qualification, type_params),
        Type::Reference(reference) => transform_type_paths(&mut reference.elem, qualification, type_params),
        Type::Slice(slice) => transform_type_paths(&mut slice.elem, qualification, type_params),
        Type::TraitObject(object) => {
            for bound in &mut object.bounds {
                transform_type_bound(bound, qualification, type_params)?;
            }
            Ok(())
        }
        Type::Tuple(tuple) => {
            for element in &mut tuple.elems {
                transform_type_paths(element, qualification, type_params)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Rebase one parsed Rust path according to its provider ownership and artifact destination.
fn transform_path(
    path: &mut syn::Path,
    qualification: Qualification<'_>,
    type_params: &[String],
) -> Result<(), String> {
    let Some(first) = path.segments.first().map(|segment| segment.ident.to_string()) else {
        return Ok(());
    };
    if matches!(first.as_str(), "Self") || type_params.iter().any(|param| param == &first) {
        return Ok(());
    }
    if matches!(qualification, Qualification::Emission { .. }) && path.leading_colon.is_some() {
        return Ok(());
    }
    if matches!(qualification, Qualification::Manifest { .. }) {
        path.leading_colon = None;
    }
    if is_shared_rust_crate(&first) || path.segments.len() == 1 && is_rust_prelude_type(&first) {
        if matches!(qualification, Qualification::Emission { .. }) && path.segments.len() > 1 {
            path.leading_colon = Some(Default::default());
        }
        return Ok(());
    }

    match qualification {
        Qualification::Emission { provider: None } => {
            if path.segments.len() > 1 && !matches!(first.as_str(), "crate" | "self" | "super") {
                path.leading_colon = Some(Default::default());
            }
        }
        Qualification::Emission {
            provider: Some(provider),
        } => {
            if matches!(first.as_str(), "self" | "super") {
                return Err(format!(
                    "provider-owned Rust type display cannot retain relative `{first}::` paths"
                ));
            }
            let original = std::mem::take(&mut path.segments);
            let include_nested_bridge = first != "crate" && first != PROVIDER_RUST_BRIDGE_MODULE && original.len() > 1;
            let prefix = provider_prefix(provider, include_nested_bridge)?;
            path.segments = prefix.segments;
            if first == "crate" {
                path.segments.extend(original.into_iter().skip(1));
            } else {
                path.segments.extend(original);
            }
            path.leading_colon = Some(Default::default());
        }
        Qualification::Manifest { provider } => {
            if matches!(first.as_str(), "self" | "super") {
                return Err(format!(
                    "provider-owned Rust type display cannot retain relative `{first}::` paths"
                ));
            }
            let original = std::mem::take(&mut path.segments);
            let include_nested_bridge = first != "crate" && first != PROVIDER_RUST_BRIDGE_MODULE && original.len() > 1;
            let prefix = manifest_prefix(provider, include_nested_bridge)?;
            path.segments = prefix.segments;
            if first == "crate" {
                path.segments.extend(original.into_iter().skip(1));
            } else {
                path.segments.extend(original);
            }
        }
    }
    Ok(())
}

/// Build the absolute generated-Rust prefix that reaches one provider-owned type.
fn provider_prefix(provider: &str, include_bridge: bool) -> Result<syn::Path, String> {
    let provider = escaped_crate_name(provider);
    let display = if include_bridge {
        format!("{provider}::{PROVIDER_RUST_BRIDGE_MODULE}")
    } else {
        provider
    };
    syn::parse_str(&display).map_err(|error| format!("invalid provider dependency path `{display}`: {error}"))
}

/// Build the relocatable manifest prefix that republishes one provider-owned type.
fn manifest_prefix(provider: &str, include_nested_bridge: bool) -> Result<syn::Path, String> {
    let provider = escaped_crate_name(provider);
    let display = if include_nested_bridge {
        format!("{PROVIDER_RUST_BRIDGE_MODULE}::{provider}::{PROVIDER_RUST_BRIDGE_MODULE}")
    } else {
        format!("{PROVIDER_RUST_BRIDGE_MODULE}::{provider}")
    };
    syn::parse_str(&display).map_err(|error| format!("invalid provider manifest path `{display}`: {error}"))
}

/// Convert one Incan dependency key into its legal generated Rust crate identifier.
fn escaped_crate_name(name: &str) -> String {
    incan_core::lang::rust_keywords::escape_keyword(&name.replace('-', "_"))
}

/// Rebase type-bearing generic, associated, constrained, and parenthesized Rust path arguments.
fn transform_path_arguments(
    path: &mut syn::Path,
    qualification: Qualification<'_>,
    type_params: &[String],
) -> Result<(), String> {
    for segment in &mut path.segments {
        match &mut segment.arguments {
            PathArguments::AngleBracketed(arguments) => {
                for argument in &mut arguments.args {
                    match argument {
                        GenericArgument::Type(ty) => transform_type_paths(ty, qualification, type_params)?,
                        GenericArgument::AssocType(association) => {
                            transform_type_paths(&mut association.ty, qualification, type_params)?
                        }
                        GenericArgument::Constraint(constraint) => {
                            for bound in &mut constraint.bounds {
                                transform_type_bound(bound, qualification, type_params)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            PathArguments::Parenthesized(arguments) => {
                for input in &mut arguments.inputs {
                    transform_type_paths(input, qualification, type_params)?;
                }
                if let ReturnType::Type(_, output) = &mut arguments.output {
                    transform_type_paths(output, qualification, type_params)?;
                }
            }
            PathArguments::None => {}
        }
    }
    Ok(())
}

/// Rebase every trait path and nested argument carried by one parsed Rust type bound.
fn transform_type_bound(
    bound: &mut TypeParamBound,
    qualification: Qualification<'_>,
    type_params: &[String],
) -> Result<(), String> {
    if let TypeParamBound::Trait(trait_bound) = bound {
        transform_path(&mut trait_bound.path, qualification, type_params)?;
        transform_path_arguments(&mut trait_bound.path, qualification, type_params)?;
    }
    Ok(())
}

/// Collect direct dependency roots from every Rust-bearing position in one semantic type.
fn collect_resolved_type_roots(
    ty: &ResolvedType,
    type_params: &[String],
    roots: &mut BTreeSet<String>,
) -> Result<(), String> {
    match ty {
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => collect_resolved_type_roots(inner, type_params, roots),
        ResolvedType::FrozenDict(key, value) => {
            collect_resolved_type_roots(key, type_params, roots)?;
            collect_resolved_type_roots(value, type_params, roots)
        }
        ResolvedType::Generic(name, args) => {
            if let Some((dependency, _)) = split_canonical_public_library_type_name(name) {
                roots.insert(escaped_crate_name(dependency));
            }
            for arg in args {
                collect_resolved_type_roots(arg, type_params, roots)?;
            }
            Ok(())
        }
        ResolvedType::Tuple(args) => {
            for arg in args {
                collect_resolved_type_roots(arg, type_params, roots)?;
            }
            Ok(())
        }
        ResolvedType::Function(params, ret) => {
            for param in params {
                collect_resolved_type_roots(&param.ty, type_params, roots)?;
            }
            collect_resolved_type_roots(ret, type_params, roots)
        }
        ResolvedType::Named(name) => {
            if let Some((dependency, _)) = split_canonical_public_library_type_name(name) {
                roots.insert(escaped_crate_name(dependency));
            }
            Ok(())
        }
        ResolvedType::RustPath(display) => {
            let canonical = display.strip_prefix("rust::").unwrap_or(display);
            let parsed = syn::parse_str::<Type>(canonical)
                .map_err(|error| format!("invalid checked Rust type display `{display}`: {error}"))?;
            collect_type_roots(&parsed, type_params, roots);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Collect dependency roots recursively from one parsed Rust type.
fn collect_type_roots(ty: &Type, type_params: &[String], roots: &mut BTreeSet<String>) {
    match ty {
        Type::Array(array) => collect_type_roots(&array.elem, type_params, roots),
        Type::BareFn(function) => {
            for input in &function.inputs {
                collect_type_roots(&input.ty, type_params, roots);
            }
            if let ReturnType::Type(_, output) = &function.output {
                collect_type_roots(output, type_params, roots);
            }
        }
        Type::Group(group) => collect_type_roots(&group.elem, type_params, roots),
        Type::ImplTrait(implementation) => {
            for bound in &implementation.bounds {
                collect_type_bound_roots(bound, type_params, roots);
            }
        }
        Type::Paren(paren) => collect_type_roots(&paren.elem, type_params, roots),
        Type::Path(path) => {
            if let Some(qualified) = &path.qself {
                collect_type_roots(&qualified.ty, type_params, roots);
            }
            collect_path_root(&path.path, type_params, roots);
            collect_path_argument_roots(&path.path, type_params, roots);
        }
        Type::Ptr(pointer) => collect_type_roots(&pointer.elem, type_params, roots),
        Type::Reference(reference) => collect_type_roots(&reference.elem, type_params, roots),
        Type::Slice(slice) => collect_type_roots(&slice.elem, type_params, roots),
        Type::TraitObject(object) => {
            for bound in &object.bounds {
                collect_type_bound_roots(bound, type_params, roots);
            }
        }
        Type::Tuple(tuple) => {
            for element in &tuple.elems {
                collect_type_roots(element, type_params, roots);
            }
        }
        _ => {}
    }
}

/// Collect dependency roots from every type-bearing argument on one parsed Rust path.
fn collect_path_argument_roots(path: &syn::Path, type_params: &[String], roots: &mut BTreeSet<String>) {
    for segment in &path.segments {
        match &segment.arguments {
            PathArguments::AngleBracketed(arguments) => {
                for argument in &arguments.args {
                    match argument {
                        GenericArgument::Type(ty) => collect_type_roots(ty, type_params, roots),
                        GenericArgument::AssocType(association) => {
                            collect_type_roots(&association.ty, type_params, roots);
                        }
                        GenericArgument::Constraint(constraint) => {
                            for bound in &constraint.bounds {
                                collect_type_bound_roots(bound, type_params, roots);
                            }
                        }
                        _ => {}
                    }
                }
            }
            PathArguments::Parenthesized(arguments) => {
                for input in &arguments.inputs {
                    collect_type_roots(input, type_params, roots);
                }
                if let ReturnType::Type(_, output) = &arguments.output {
                    collect_type_roots(output, type_params, roots);
                }
            }
            PathArguments::None => {}
        }
    }
}

/// Collect dependency roots from one parsed Rust trait bound.
fn collect_type_bound_roots(bound: &TypeParamBound, type_params: &[String], roots: &mut BTreeSet<String>) {
    if let TypeParamBound::Trait(trait_bound) = bound {
        collect_path_root(&trait_bound.path, type_params, roots);
        collect_path_argument_roots(&trait_bound.path, type_params, roots);
    }
}

/// Record the dependency root represented by one parsed Rust path when it is not compiler-shared or generic.
fn collect_path_root(path: &syn::Path, type_params: &[String], roots: &mut BTreeSet<String>) {
    let Some(first) = path.segments.first().map(|segment| segment.ident.to_string()) else {
        return;
    };
    if is_shared_rust_crate(&first)
        || matches!(first.as_str(), "crate" | "self" | "super" | "Self")
        || type_params.iter().any(|param| param == &first)
        || path.segments.len() == 1
    {
        return;
    }
    if first == PROVIDER_RUST_BRIDGE_MODULE {
        if let Some(provider) = path.segments.iter().nth(1) {
            roots.insert(provider.ident.to_string());
        }
    } else {
        roots.insert(first);
    }
}

/// Return whether one Rust crate root is supplied by the compiler toolchain instead of a package dependency bridge.
pub(crate) fn is_shared_rust_crate(name: &str) -> bool {
    matches!(name, "std" | "core" | "alloc" | "incan_core" | "incan_stdlib")
}

/// Return whether one unqualified Rust type is supplied by the language prelude.
fn is_rust_prelude_type(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "char"
            | "str"
            | "String"
            | "Vec"
            | "Option"
            | "Result"
            | "Box"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    )
}

#[cfg(test)]
mod tests {
    use super::{Qualification, normalize_for_emission, public_bridge_roots, qualify_for_manifest, transform_display};
    use crate::frontend::symbols::ResolvedType;

    fn compact(display: &str) -> String {
        display.chars().filter(|character| !character.is_whitespace()).collect()
    }

    #[test]
    fn emission_routes_nested_provider_paths_and_nominals() {
        let normalized = transform_display(
            "rust_shadow::Envelope<Vec<Payload>>",
            Qualification::Emission {
                provider: Some("compiled_parent"),
            },
            &[],
        )
        .expect("valid Rust display");

        assert_eq!(
            compact(&normalized),
            "::compiled_parent::__incan_provider_rust::rust_shadow::Envelope<Vec<::compiled_parent::Payload>>"
        );
    }

    #[test]
    fn manifest_wraps_transitive_provider_paths() {
        let normalized = transform_display(
            "__incan_provider_rust::leaf::__incan_provider_rust::rust_shadow::Token",
            Qualification::Manifest { provider: "middle" },
            &[],
        )
        .expect("valid Rust display");

        assert_eq!(
            compact(&normalized),
            "__incan_provider_rust::middle::__incan_provider_rust::leaf::__incan_provider_rust::rust_shadow::Token"
        );
    }

    #[test]
    fn qualified_associated_paths_root_both_sides() {
        let normalized = transform_display(
            "<Payload as rust_traits::Carrier>::Item",
            Qualification::Emission {
                provider: Some("compiled_parent"),
            },
            &[],
        )
        .expect("valid Rust display");

        assert_eq!(
            compact(&normalized),
            "<::compiled_parent::Payloadas::compiled_parent::__incan_provider_rust::rust_traits::Carrier>::Item"
        );
    }

    #[test]
    fn bridge_roots_cover_trait_objects_impl_traits_and_associated_bounds() {
        let trait_object = ResolvedType::RustPath(
            "Box<dyn rust_traits::Carrier<Item = payload_crate::Payload> + support::Marker>".to_string(),
        );
        assert_eq!(
            public_bridge_roots(&trait_object, &[]).expect("valid trait-object display"),
            ["payload_crate", "rust_traits", "support"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );

        let implementation = ResolvedType::RustPath(
            "impl rust_traits::Carrier<Item: support::Marker> + callable::Factory(input::Value) -> output::Value"
                .to_string(),
        );
        assert_eq!(
            public_bridge_roots(&implementation, &[]).expect("valid impl-trait display"),
            ["callable", "input", "output", "rust_traits", "support"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    #[test]
    fn structured_box_retains_nested_provider_identity() {
        let mut ty = ResolvedType::Generic(
            "Box".to_string(),
            vec![ResolvedType::Named("pub::compiled_parent::Payload".to_string())],
        );
        assert_eq!(
            public_bridge_roots(&ty, &[]).expect("valid structured provider type"),
            ["compiled_parent"].into_iter().map(str::to_string).collect()
        );

        normalize_for_emission(&mut ty, Some("compiled_middle"), &[]).expect("provider path should normalize");
        assert_eq!(
            ty,
            ResolvedType::Generic(
                "Box".to_string(),
                vec![ResolvedType::Named(
                    "::compiled_middle::__incan_provider_rust::compiled_parent::Payload".to_string()
                )]
            )
        );
    }

    #[test]
    fn structured_provider_nominals_rebase_through_exact_owner() -> Result<(), String> {
        let provider_owned = ResolvedType::Generic(
            "Box".to_string(),
            vec![ResolvedType::Named("pub::compiled_parent::Payload".to_string())],
        );
        let manifest = qualify_for_manifest(&provider_owned, Some("compiled_parent"), &[])?;
        assert_eq!(
            manifest,
            ResolvedType::Generic(
                "Box".to_string(),
                vec![ResolvedType::Named(
                    "__incan_provider_rust::compiled_parent::Payload".to_string()
                )]
            )
        );

        let mut emission = manifest;
        normalize_for_emission(&mut emission, Some("compiled_middle"), &[])?;
        assert_eq!(
            emission,
            ResolvedType::Generic(
                "Box".to_string(),
                vec![ResolvedType::Named(
                    "::compiled_middle::__incan_provider_rust::compiled_parent::Payload".to_string()
                )]
            )
        );
        Ok(())
    }

    #[test]
    fn structured_transitive_nominals_rebase_through_nested_provider_bridge() -> Result<(), String> {
        let dependency_owned = ResolvedType::Generic(
            "Box".to_string(),
            vec![ResolvedType::Named("pub::compiled_leaf::ForeignPayload".to_string())],
        );
        let manifest = qualify_for_manifest(&dependency_owned, Some("compiled_parent"), &[])?;
        assert_eq!(
            manifest,
            ResolvedType::Generic(
                "Box".to_string(),
                vec![ResolvedType::Named(
                    "__incan_provider_rust::compiled_parent::__incan_provider_rust::compiled_leaf::ForeignPayload"
                        .to_string()
                )]
            )
        );

        let mut emission = manifest;
        normalize_for_emission(&mut emission, Some("compiled_middle"), &[])?;
        assert_eq!(
            emission,
            ResolvedType::Generic(
                "Box".to_string(),
                vec![ResolvedType::Named(
                    "::compiled_middle::__incan_provider_rust::compiled_parent::__incan_provider_rust::compiled_leaf::ForeignPayload"
                        .to_string()
                )]
            )
        );
        Ok(())
    }
}
