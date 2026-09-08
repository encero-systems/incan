//! Map rust-analyzer `hir` definitions into [`incan_core::interop::RustItemMetadata`].

use std::collections::{BTreeMap, HashSet};

use incan_core::interop::{
    RustAssociatedTypeBinding, RustAssociatedTypeRequirement, RustExpandedDeriveTrait, RustFieldInfo, RustFunctionSig,
    RustImplementedTrait, RustItemKind, RustItemMetadata, RustMacroInfo, RustMethodSig, RustModuleChild,
    RustModuleChildKind, RustModuleInfo, RustMutableReferenceCandidate, RustMutableReferenceTypeParam, RustParam,
    RustPayloadCarrier, RustTraitAssoc, RustTraitInfo, RustTypeInfo, RustTypeShape, RustTypeShapePathFallback,
    RustVariantInfo, RustVisibility, is_box_carrier_path, parse_rust_type_shape_text, render_rust_type_shape,
    rust_source_borrowed_type_param_bound_display, rust_source_callable_bound_for_type_param,
    rust_source_type_param_has_as_fd_bound, split_top_level_rust_args,
};
use ra_ap_hir::{
    Adt, AssocItem, Const as HirConst, Crate, DisplayTarget, Enum, FieldSource, Function, GenericDef, GenericParam,
    HasSource, HasVisibility, HirDisplay, Impl, Module, ModuleDef, Mutability, Name, ScopeDef, Semantics,
    Static as HirStatic, Trait, Type, Variant, VariantDef, Visibility, attach_db,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::{
    AstNode, SyntaxNode,
    ast::{self, HasAttrs, HasGenericArgs, HasGenericParams, HasModuleItem, HasName, HasTypeBounds},
};

use super::error::RustMetadataError;
use super::generic_params::{SourceOwnerGenerics, source_owner_generics};
use super::loader::RustWorkspace;

const INCAN_DERIVE_PROBE_PREFIX: &str = "__IncanDeriveProbe";

fn map_visibility(vis: Visibility) -> RustVisibility {
    match vis {
        Visibility::Public => RustVisibility::Public,
        Visibility::Module(_, _) | Visibility::PubCrate(_) => RustVisibility::Restricted,
    }
}

fn is_exported_rust_api(vis: Visibility) -> bool {
    matches!(vis, Visibility::Public)
}

fn format_ty(ty: &Type<'_>, db: &RootDatabase, dt: DisplayTarget) -> String {
    format!("{}", ty.display(db, dt))
}

fn normalize_display_path(display: &str) -> String {
    display.trim().trim_start_matches("::").to_string()
}

fn split_display_base(display: &str) -> &str {
    display.split('<').next().unwrap_or(display)
}

fn display_looks_like_type_param(display: &str) -> bool {
    !display.is_empty()
        && !display.contains("::")
        && !display.contains(['<', '>', '(', ')', '[', ']', '&', ' '])
        && display.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn module_path_segments(module: Module, db: &RootDatabase) -> Vec<String> {
    module
        .path_to_root(db)
        .into_iter()
        .rev()
        .filter_map(|module| module.name(db).map(|name| name.as_str().to_owned()))
        .collect()
}

fn field_module(field: &ra_ap_hir::Field, db: &RootDatabase) -> Module {
    match field.parent_def(db) {
        VariantDef::Struct(strukt) => strukt.module(db),
        VariantDef::Union(union) => union.module(db),
        VariantDef::Variant(variant) => variant.module(db),
    }
}

/// Resolve a written path relative to the module that declares it: `self::` and `super::` markers walk the module
/// tree, and a bare path joins onto the declaring module. An absolute `::` path is not owner-relative and returns
/// unchanged rather than being joined onto the module; `resolve_source_path` handles that form.
fn resolve_relative_source_path(text: &str, crate_name: &str, module: Module, db: &RootDatabase) -> Option<String> {
    let text = text.trim();
    if let Some(absolute) = text.strip_prefix("::") {
        // Absolute paths are not this function's business; see `resolve_source_path`. Fail closed rather than
        // join a global path onto the owning module.
        return (!absolute.is_empty()).then(|| absolute.to_string());
    }
    let mut text = text;
    if text.is_empty() {
        return None;
    }

    let mut module_segments = module_path_segments(module, db);
    if let Some(rest) = text.strip_prefix("crate::") {
        text = rest;
        module_segments.clear();
    } else if let Some(rest) = text.strip_prefix("self::") {
        text = rest;
    } else {
        while let Some(rest) = text.strip_prefix("super::") {
            text = rest;
            module_segments.pop();
        }
    }

    let mut canonical = vec![crate_name.to_string()];
    canonical.extend(module_segments);
    canonical.extend(
        text.split("::")
            .filter(|segment| !segment.is_empty())
            .map(ToOwned::to_owned),
    );
    Some(canonical.join("::"))
}

fn canonical_module_def_path(def: ModuleDef, db: &RootDatabase) -> Option<String> {
    let local_path = match def {
        ModuleDef::BuiltinType(builtin) => builtin.name().as_str().to_owned(),
        _ => def.canonical_path(db, def.module(db)?.krate(db).edition(db))?,
    };
    let crate_name = def
        .module(db)
        .and_then(|module| module.krate(db).display_name(db))
        .map(|name| name.canonical_name().as_str().to_owned());

    match crate_name {
        Some(crate_name) if !local_path.starts_with(crate_name.as_str()) => Some(format!("{crate_name}::{local_path}")),
        Some(_) | None => Some(local_path),
    }
}

fn canonical_adt_path(adt: Adt, db: &RootDatabase) -> Option<String> {
    canonical_module_def_path(ModuleDef::Adt(adt), db)
}

/// Return whether a Rust display type is an exact numeric primitive.
fn is_exact_numeric_display(text: &str) -> bool {
    matches!(
        text,
        "f32"
            | "f64"
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
    )
}

/// Return the canonical Rust numeric display when `text` is exactly a primitive numeric type or reference.
fn exact_numeric_boundary_display(text: &str) -> Option<String> {
    let normalized = normalize_display_path(text)
        .replace("'static ", "")
        .replace("'_", "")
        .replace(' ', "");
    if is_exact_numeric_display(normalized.as_str()) {
        return Some(normalized);
    }
    if let Some(inner) = normalized.strip_prefix('&') {
        let inner = inner.strip_prefix("mut").unwrap_or(inner).trim();
        if is_exact_numeric_display(inner) {
            return Some(format!("&{inner}"));
        }
    }
    None
}

/// Resolve one source-level type path to the canonical Rust namespace used by interop metadata.
///
/// Structural type shapes intentionally erase numeric widths and rust-analyzer displays include defaulted generic
/// arguments. Neither representation is a valid nominal identity: for example, `Vec<u8>` can otherwise become
/// `alloc::vec::Vec<i64, alloc::alloc::Global>`. Written Rust signatures retain the exact scalar and omitted-default
/// contract, while HIR resolves their paths to defining modules. Combine those two authorities here.
fn source_module_type_path_identity_display(module: Module, text: &str, db: &RootDatabase) -> Option<String> {
    let text = text.trim().trim_start_matches("::").replace(' ', "");
    if text.is_empty() {
        return None;
    }
    match text.as_str() {
        "Self" | "bool" | "f32" | "f64" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32"
        | "u64" | "u128" | "usize" | "str" | "String" | "Vec" | "Option" | "Result" | "Box" | "()" | "[u8]" => {
            return Some(text);
        }
        _ => {}
    }

    let crate_name = module
        .krate(db)
        .display_name(db)
        .map(|name| name.canonical_name().as_str().to_owned())?;
    resolve_source_path(text.as_str(), crate_name.as_str(), module, db)
}

/// Project written Rust type syntax through one canonical path resolver without losing scalar width or adding
/// defaulted generic arguments.
fn source_type_identity_display_with_resolver(
    text: &str,
    resolve_path: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let original = text.trim();
    if original.is_empty() {
        return None;
    }
    if original.starts_with("impl ") {
        return Some(original.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    if let Some(rest) = original.strip_prefix("dyn ") {
        return Some(format!(
            "dyn {}",
            source_type_identity_display_with_resolver(rest, resolve_path)?
        ));
    }
    if let Some((is_mut, inner)) = source_borrow_kind(original) {
        let inner = source_type_identity_display_with_resolver(inner, resolve_path)?;
        return Some(if is_mut {
            format!("&mut {inner}")
        } else {
            format!("&{inner}")
        });
    }
    if original.starts_with('[') && original.ends_with(']') {
        let inner = &original[1..original.len() - 1];
        // Preserve slices as structural syntax. Arrays include a top-level semicolon and need const-expression
        // metadata that this source identity projection does not claim to provide.
        if !inner.contains(';') {
            return Some(format!(
                "[{}]",
                source_type_identity_display_with_resolver(inner, resolve_path)?
            ));
        }
    }
    if original.starts_with('(') && original.ends_with(')') {
        let inner = &original[1..original.len() - 1];
        if inner.trim().is_empty() {
            return Some("()".to_string());
        }
        let items = split_top_level_rust_args(inner)
            .into_iter()
            .map(|item| source_type_identity_display_with_resolver(item, resolve_path))
            .collect::<Option<Vec<_>>>()?;
        return Some(format!("({})", items.join(", ")));
    }
    if let Some(start) = original.find('<')
        && original.ends_with('>')
    {
        let base = resolve_path(&original[..start])?;
        let inner = &original[start + 1..original.len() - 1];
        let args = split_top_level_rust_args(inner)
            .into_iter()
            .map(|arg| source_type_identity_display_with_resolver(arg, resolve_path))
            .collect::<Option<Vec<_>>>()?;
        return Some(format!("{base}<{}>", args.join(", ")));
    }
    resolve_path(original)
}

/// Project written Rust type syntax into canonical namespace identity in the declaring module.
fn source_module_type_identity_display(module: Module, text: &str, db: &RootDatabase) -> Option<String> {
    source_type_identity_display_with_resolver(text, &|path| source_module_type_path_identity_display(module, path, db))
}

/// Project a written Rust signature type into canonical namespace identity without losing scalar width or adding
/// defaulted generic arguments.
fn source_function_type_identity_display(f: Function, text: &str, db: &RootDatabase) -> Option<String> {
    let type_params = source_function_type_params(f, db);
    source_type_identity_display_with_resolver(text, &|path| {
        let path = path.trim();
        if type_params.iter().any(|type_param| type_param == path) {
            Some(path.to_string())
        } else {
            source_module_type_path_identity_display(f.module(db), path, db)
                .or_else(|| canonicalize_imported_single_segment_type_display(path, f, db))
        }
    })
}

/// Resolve a constant's written type through its declaring module without widening source scalar types.
fn source_const_type_identity_display(constant: HirConst, db: &RootDatabase) -> Option<String> {
    let source = constant.source(db)?;
    let text = source.value.ty()?.to_string();
    source_module_type_identity_display(constant.module(db), text.as_str(), db)
}

/// Resolve a static's written type through its declaring module without widening source scalar types.
fn source_static_type_identity_display(static_: HirStatic, db: &RootDatabase) -> Option<String> {
    let source = static_.source(db)?;
    let text = source.value.ty()?.to_string();
    source_module_type_identity_display(static_.module(db), text.as_str(), db)
}

/// Resolve a path as written in Rust source to the canonical spelling inspection records for it.
///
/// Whitespace is dropped, a leading `::` selects the absolute form, and every other spelling is resolved relative to
/// the declaring module through `resolve_relative_source_path`. The result is the ancestral path a consumer compares
/// against, so a re-export such as `::prost::alloc::boxed::Box` names the same item wherever it is written.
fn resolve_source_path(text: &str, crate_name: &str, module: Module, db: &RootDatabase) -> Option<String> {
    let text = text.trim().replace(' ', "");
    if text.is_empty() {
        return None;
    }

    // A leading `::` is the *absolute* path form — the opposite of the `self::`/`super::` markers below — so it
    // must never reach the owner-relative joiner. prost-generated code spells every standard type this way
    // (`::prost::alloc::boxed::Box<T>`), and joining that onto the owning module recorded
    // `substrait::proto::fetch_rel::prost::alloc::boxed::Box` as a field type: a path no consumer can ever
    // name, so the type never unified with the caller's `Box`. Resolve it semantically instead; HIR follows the
    // re-export to the crate that defines the item, which is the ancestral namespace every alias must record.
    let (absolute, text) = match text.strip_prefix("::") {
        Some(rest) => (true, rest.to_string()),
        None => (false, text),
    };
    if !absolute && (text.starts_with("crate::") || text.starts_with("self::") || text.starts_with("super::")) {
        return resolve_relative_source_path(text.as_str(), crate_name, module, db);
    }

    let segments: Vec<Name> = text
        .split("::")
        .filter(|segment| !segment.is_empty())
        .map(Name::new_root)
        .collect();
    if !segments.is_empty()
        && let Some(mut resolved) = module.resolve_mod_path(db, segments)
        && let Some(item) = resolved.next()
        && let Some(path) = canonical_module_def_path(item.into_module_def(), db)
    {
        return Some(path);
    }
    if absolute {
        // An absolute path that HIR cannot resolve is still absolute. Returning it as written keeps the identity
        // stable across the lookup aliases in `cache.rs`; the one thing it must not become is owner-relative.
        return Some(text);
    }

    if !text.contains("::") {
        for (name, def) in module.scope(db, None) {
            if name.as_str() != text {
                continue;
            }
            let ScopeDef::ModuleDef(module_def) = def else {
                continue;
            };
            if let Some(path) = canonical_module_def_path(module_def, db) {
                return Some(path);
            }
        }
    }

    if text.contains("::") {
        return Some(text);
    }

    None
}

/// Classify the source-level shape represented by a Rust display type.
fn source_type_shape(text: &str, crate_name: &str, module: Module, db: &RootDatabase) -> RustTypeShape {
    parse_rust_type_shape_text(
        text,
        |path| resolve_source_path(path, crate_name, module, db),
        RustTypeShapePathFallback::Unknown,
    )
}

/// Detect a source-level borrow annotation and return whether it is mutable plus the inner type text.
fn source_borrow_kind(text: &str) -> Option<(bool, &str)> {
    let after_amp = text.trim().strip_prefix('&')?.trim_start();
    let after_lifetime = if let Some(rest) = after_amp.strip_prefix('\'') {
        let end = rest
            .char_indices()
            .find_map(|(idx, ch)| (!(ch == '_' || ch.is_ascii_alphanumeric())).then_some(idx))
            .unwrap_or(rest.len());
        rest[end..].trim_start()
    } else {
        after_amp
    };
    if let Some(rest) = after_lifetime.strip_prefix("mut")
        && rest.chars().next().is_none_or(char::is_whitespace)
    {
        return Some((true, rest.trim_start()));
    }
    Some((false, after_lifetime))
}

/// Return the exact written type syntax for either a named or positional Rust field.
fn source_field_type_text(field: &ra_ap_hir::Field, db: &RootDatabase) -> Option<String> {
    let source = field.source(db)?;
    Some(match source.value {
        FieldSource::Named(field) => field.ty()?.to_string(),
        FieldSource::Pos(field) => field.ty()?.to_string(),
    })
}

/// Parse a field's written type into the metadata shape used at the Incan interop boundary.
fn source_field_type_shape(field: &ra_ap_hir::Field, db: &RootDatabase, crate_name: &str) -> Option<RustTypeShape> {
    let text = source_field_type_text(field, db)?;
    let module = field_module(field, db);
    Some(source_type_shape(text.as_str(), crate_name, module, db))
}

/// Resolve a field's written type to its canonical Rust namespace identity.
fn source_field_type_display(field: &ra_ap_hir::Field, db: &RootDatabase) -> Option<String> {
    let text = source_field_type_text(field, db)?;
    let source = field.source(db)?;
    let syntax = match &source.value {
        FieldSource::Named(field) => field.syntax(),
        FieldSource::Pos(field) => field.syntax(),
    };
    let module = field_module(field, db);
    source_type_identity_display_with_resolver(text.as_str(), &|path| {
        source_module_type_path_identity_display(module, path, db)
            .or_else(|| imported_type_path_in_syntax_scope(syntax, path))
    })
}

/// Return whether a metadata shape still contains an unsubstituted Rust type parameter.
fn rust_type_shape_contains_type_param(shape: &RustTypeShape) -> bool {
    match shape {
        RustTypeShape::Option(inner) | RustTypeShape::Ref(inner) => rust_type_shape_contains_type_param(inner),
        RustTypeShape::Result(ok, err) => {
            rust_type_shape_contains_type_param(ok) || rust_type_shape_contains_type_param(err)
        }
        RustTypeShape::Tuple(items) | RustTypeShape::RustPath { args: items, .. } => {
            items.iter().any(rust_type_shape_contains_type_param)
        }
        RustTypeShape::TypeParam(_) => true,
        RustTypeShape::Never
        | RustTypeShape::Bool
        | RustTypeShape::Float
        | RustTypeShape::Int
        | RustTypeShape::Str
        | RustTypeShape::Bytes
        | RustTypeShape::Unit
        | RustTypeShape::Unknown => false,
    }
}

/// Prefer written source identity unless doing so would undo a concrete substitution through a type alias.
fn source_field_metadata(
    field: &ra_ap_hir::Field,
    hir_shape: &RustTypeShape,
    db: &RootDatabase,
    crate_name: &str,
) -> Option<(String, RustTypeShape)> {
    let source_shape = source_field_type_shape(field, db, crate_name)?;
    if rust_type_shape_contains_type_param(&source_shape) && !rust_type_shape_contains_type_param(hir_shape) {
        return None;
    }
    Some((source_field_type_display(field, db)?, source_shape))
}

/// Return the source label used to reconstruct a Rust field constructor.
///
/// rust-analyzer may expose a raw named field such as `r#type` through a safe internal name. Incan needs the source
/// spelling instead: `type` should be accepted in Incan and later emitted as `r#type`, while an ordinary Rust field
/// named `type_` must remain `type_`. Tuple fields intentionally use an empty label; lowering already recognizes that
/// representation as a positional Rust constructor.
fn source_field_constructor_label(field: &ra_ap_hir::Field, db: &RootDatabase) -> Option<String> {
    let source = field.source(db)?;
    match source.value {
        FieldSource::Named(field) => {
            let raw = field.name()?.syntax().text().to_string();
            Some(raw.strip_prefix("r#").unwrap_or(raw.as_str()).to_string())
        }
        FieldSource::Pos(_) => Some(String::new()),
    }
}

/// Record a variant payload as its semantic type, remembering a stripped `Box<T>` carrier so lowering can restore it.
fn normalize_variant_payload_shape(shape: RustTypeShape) -> (RustTypeShape, RustPayloadCarrier) {
    match shape {
        RustTypeShape::RustPath { path, args } if is_box_carrier_path(path.as_str()) => (
            args.into_iter().next().unwrap_or(RustTypeShape::Unknown),
            RustPayloadCarrier::Boxed,
        ),
        other => (other, RustPayloadCarrier::Direct),
    }
}

/// Classify a rust-analyzer HIR type into the shared structural shape consumed by interop metadata.
fn rust_type_shape(ty: &Type<'_>, db: &RootDatabase, dt: DisplayTarget) -> RustTypeShape {
    if ty.is_never() {
        return RustTypeShape::Never;
    }
    if ty.is_bool() {
        return RustTypeShape::Bool;
    }
    if ty.is_float() {
        return RustTypeShape::Float;
    }
    if ty.is_int_or_uint() {
        return RustTypeShape::Int;
    }
    if ty.is_str() {
        return RustTypeShape::Str;
    }
    if ty.is_unit() {
        return RustTypeShape::Unit;
    }
    if let Some((inner, _)) = ty.as_reference() {
        if let Some(slice_inner) = inner.as_slice() {
            let slice_display = normalize_display_path(format_ty(&slice_inner, db, dt).as_str());
            if slice_display == "u8" {
                return RustTypeShape::Bytes;
            }
        }
        return RustTypeShape::Ref(Box::new(rust_type_shape(&inner, db, dt)));
    }
    if let Some(slice_inner) = ty.as_slice() {
        let slice_display = normalize_display_path(format_ty(&slice_inner, db, dt).as_str());
        if slice_display == "u8" {
            return RustTypeShape::Bytes;
        }
    }
    if ty.is_tuple() {
        return RustTypeShape::Tuple(ty.tuple_fields(db).iter().map(|t| rust_type_shape(t, db, dt)).collect());
    }

    let display = normalize_display_path(format_ty(ty, db, dt).as_str());
    if matches!(
        display.as_str(),
        "String" | "std::string::String" | "alloc::string::String"
    ) {
        return RustTypeShape::Str;
    }

    if let Some((adt, args)) = ty.as_adt_with_args() {
        let base = split_display_base(display.as_str()).to_string();
        let arg_shapes: Vec<RustTypeShape> = args
            .into_iter()
            .map(|arg| {
                arg.map(|ty| rust_type_shape(&ty, db, dt))
                    .unwrap_or(RustTypeShape::Unknown)
            })
            .collect();
        match base.as_str() {
            "Option" | "std::option::Option" | "core::option::Option" => {
                return RustTypeShape::Option(Box::new(
                    arg_shapes.into_iter().next().unwrap_or(RustTypeShape::Unknown),
                ));
            }
            "Result" | "std::result::Result" | "core::result::Result" => {
                let mut it = arg_shapes.into_iter();
                return RustTypeShape::Result(
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                );
            }
            "Vec" | "std::vec::Vec" | "alloc::vec::Vec" if display.ends_with("<u8>") => {
                return RustTypeShape::Bytes;
            }
            _ => {}
        }
        let path = canonical_adt_path(adt, db).unwrap_or(base);
        return RustTypeShape::RustPath { path, args: arg_shapes };
    }

    if display_looks_like_type_param(display.as_str()) {
        return RustTypeShape::TypeParam(display);
    }

    if !display.is_empty() && display.contains("::") {
        return RustTypeShape::RustPath {
            path: display,
            args: Vec::new(),
        };
    }

    RustTypeShape::Unknown
}

/// Render a Rust signature type in source-oriented form.
fn function_sig_type_display(ty: &Type<'_>, db: &RootDatabase, dt: DisplayTarget) -> String {
    let raw = normalize_display_path(format_ty(ty, db, dt).as_str());
    if let Some(display) = exact_numeric_boundary_display(raw.as_str()) {
        return display;
    }
    match rust_type_shape(ty, db, dt) {
        RustTypeShape::Unknown => raw,
        other => render_rust_type_shape(&other),
    }
}

/// Resolve a function's declared source return annotation into a canonical display string.
///
/// rust-analyzer can still surface opaque async return displays such as `impl ?Sized` for some free functions. When
/// that happens, the written source annotation is the more faithful contract: it still contains the concrete `Result<T,
/// E>` (or other) return that downstream typechecking expects, and we can canonicalize it against the function's
/// defining module.
fn source_function_return_type_display(f: Function, db: &RootDatabase) -> Option<String> {
    let source = f.source(db)?;
    let text = source.value.ret_type()?.ty()?.to_string();
    source_function_type_identity_display(f, text.as_str(), db)
}

/// Render source type text in the same canonical display form used by extracted function metadata.
fn source_function_type_display(f: Function, text: &str, db: &RootDatabase) -> Option<String> {
    source_function_type_identity_display(f, text, db)
}

/// Return the written RHS of a Rust `type` alias when available.
///
/// HIR type displays may erase callable trait-object arguments inside aliases to `_`. The source RHS is the
/// authoritative contract for contextual typing at Rust boundaries, so preserve it when rust-analyzer can recover the
/// defining syntax.
fn source_type_alias_target_display(alias: ra_ap_hir::TypeAlias, db: &RootDatabase) -> Option<String> {
    let source = alias.source(db)?;
    source.value.ty().map(|ty| ty.to_string().trim().to_string())
}

/// Resolve a type alias RHS through both its semantic module and syntax-local imports.
fn source_type_alias_identity_display(alias: ra_ap_hir::TypeAlias, text: &str, db: &RootDatabase) -> Option<String> {
    let source = alias.source(db)?;
    let module = alias.module(db);
    source_type_identity_display_with_resolver(text, &|path| {
        source_module_type_path_identity_display(module, path, db)
            .or_else(|| imported_type_path_in_syntax_scope(source.value.syntax(), path))
    })
}

fn join_use_path(prefix: Option<&str>, path: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}::{path}"),
        _ => path.to_string(),
    }
}

/// Recover the fully qualified imported path for one name from a possibly nested Rust use tree.
fn use_tree_import_path(tree: &ast::UseTree, target_name: &str, prefix: Option<&str>) -> Option<String> {
    let path = tree.path().map(|path| path.to_string().replace(' ', ""));
    let qualified = path.as_deref().map(|path| join_use_path(prefix, path));

    if let Some(use_tree_list) = tree.use_tree_list() {
        let next_prefix = qualified.as_deref();
        for child in use_tree_list.use_trees() {
            if let Some(path) = use_tree_import_path(&child, target_name, next_prefix) {
                return Some(path);
            }
        }
    }

    if let Some(rename) = tree.rename() {
        return rename
            .name()
            .is_some_and(|name| name.to_string() == target_name)
            .then_some(qualified)
            .flatten();
    }

    if let Some(qualified) = qualified {
        let imported_name = qualified.rsplit("::").next().unwrap_or(qualified.as_str());
        if imported_name == target_name {
            return Some(qualified);
        }
    }

    None
}

/// Resolve one imported type name from the nearest Rust item-list or source-file scope.
fn imported_type_path_in_syntax_scope(syntax: &SyntaxNode, target_name: &str) -> Option<String> {
    if let Some(item_list) = syntax.ancestors().find_map(ast::ItemList::cast) {
        for item in item_list.items() {
            let ast::Item::Use(use_item) = item else {
                continue;
            };
            if let Some(path) = use_item
                .use_tree()
                .and_then(|tree| use_tree_import_path(&tree, target_name, None))
            {
                return Some(path);
            }
        }
        return None;
    }

    let source_file = syntax.ancestors().find_map(ast::SourceFile::cast)?;
    for item in source_file.items() {
        let ast::Item::Use(use_item) = item else {
            continue;
        };
        if let Some(path) = use_item
            .use_tree()
            .and_then(|tree| use_tree_import_path(&tree, target_name, None))
        {
            return Some(path);
        }
    }
    None
}

/// Resolve one imported type name from the syntax scope that declares a function.
fn imported_type_path_in_function_scope(f: Function, target_name: &str, db: &RootDatabase) -> Option<String> {
    let source = f.source(db)?;
    imported_type_path_in_syntax_scope(source.value.syntax(), target_name)
}

fn canonicalize_imported_single_segment_type_display(text: &str, f: Function, db: &RootDatabase) -> Option<String> {
    let normalized = text.trim().replace(' ', "");
    if let Some(inner) = normalized.strip_prefix("&mut") {
        return imported_type_path_in_function_scope(f, inner, db).map(|path| format!("&mut {path}"));
    }
    if let Some(inner) = normalized.strip_prefix('&') {
        return imported_type_path_in_function_scope(f, inner, db).map(|path| format!("&{path}"));
    }
    if normalized.contains("::") || normalized.contains(['<', '>', '(', ')', '[', ']', ',']) {
        return None;
    }
    imported_type_path_in_function_scope(f, normalized.as_str(), db)
}

/// Resolve a function parameter's declared source annotation into a canonical display string.
///
/// rust-analyzer sometimes degrades borrowed parameter displays to `&?` even when the written source still carries a
/// concrete imported or local pointee type. When that happens, the source annotation is the more faithful contract and
/// should drive metadata so downstream typechecking/codegen can keep the concrete borrow boundary.
fn source_function_param_type_text(f: Function, param: &ra_ap_hir::Param<'_>, db: &RootDatabase) -> Option<String> {
    let source = f.source(db)?;
    let param_list = source.value.param_list()?;
    let source_params = param_list.params().collect::<Vec<_>>();
    if let Some(name) = param.name(db) {
        let source_param = source_params.iter().find(|source_param| {
            source_param.pat().is_some_and(|pattern| {
                let pattern = pattern.to_string();
                let pattern = pattern.trim();
                pattern.strip_prefix("mut ").unwrap_or(pattern) == name.as_str()
            })
        });
        if let Some(source_param) = source_param {
            return source_param.ty().map(|ty| ty.to_string());
        }
    }
    let self_offset = usize::from(param_list.self_param().is_some());
    if param.index() < self_offset {
        return None;
    }
    let source_param = source_params.get(param.index() - self_offset)?;
    source_param.ty().map(|ty| ty.to_string())
}

/// Resolve a function parameter's source annotation or callable generic bound into a canonical display string.
fn source_function_param_type_display(f: Function, param: &ra_ap_hir::Param<'_>, db: &RootDatabase) -> Option<String> {
    let text = source_function_param_type_text(f, param, db)?;
    let source = f.source(db)?;
    if rust_source_type_param_has_as_fd_bound(source.value.syntax().text().to_string().as_str(), text.as_str()) {
        return Some("&impl AsFd".to_string());
    }
    if let Some(display) =
        rust_source_borrowed_type_param_bound_display(source.value.syntax().text().to_string().as_str(), text.as_str())
    {
        return Some(display);
    }
    if let Some(display) = rust_source_callable_bound_for_type_param(
        source.value.syntax().text().to_string().as_str(),
        text.as_str(),
        |ty| source_function_type_display(f, ty, db),
    ) {
        return Some(display);
    }
    source_function_type_display(f, text.as_str(), db)
}

/// Return source-declared function type parameters in declaration order.
fn source_function_type_params(f: Function, db: &RootDatabase) -> Vec<String> {
    source_owner_generics(f.source(db).and_then(|source| source.value.generic_param_list())).type_params
}

/// Return source-declared generics for an ADT receiver.
fn source_adt_generics(adt: Adt, db: &RootDatabase) -> SourceOwnerGenerics {
    let params = match adt {
        Adt::Struct(item) => item.source(db).and_then(|source| source.value.generic_param_list()),
        Adt::Union(item) => item.source(db).and_then(|source| source.value.generic_param_list()),
        Adt::Enum(item) => item.source(db).and_then(|source| source.value.generic_param_list()),
    };
    source_owner_generics(params)
}

/// Canonicalize declared default type arguments against the owner module while retaining parameter alignment.
fn canonical_type_param_defaults(
    generics: &SourceOwnerGenerics,
    module: Module,
    db: &RootDatabase,
) -> Vec<Option<String>> {
    if !generics.type_param_defaults.iter().any(Option::is_some) {
        return Vec::new();
    }
    let Some(crate_name) = module
        .krate(db)
        .display_name(db)
        .map(|name| name.canonical_name().as_str().to_owned())
    else {
        return generics.type_param_defaults.clone();
    };
    generics
        .type_param_defaults
        .iter()
        .map(|default| {
            default.as_deref().map(|default| {
                source_module_type_identity_display(module, default, db).unwrap_or_else(|| {
                    match source_type_shape(default, crate_name.as_str(), module, db) {
                        RustTypeShape::Unknown => normalize_display_path(default),
                        shape => render_rust_type_shape(&shape),
                    }
                })
            })
        })
        .collect()
}

/// Gather exact unconditional trait implementations emitted by one ADT's successfully expanded derive macros.
///
/// rust-analyzer's public trait-solver index omits some proc-macro-generated derive implementations even when the
/// expansion server is active. Its expansion API still exposes the generated syntax. Resolve each generated `impl`
/// back through HIR and retain only unconditional implementations whose receiver is this exact ADT, including their
/// concrete associated-type assignments. Derive names alone carry no implementation authority, while conditional
/// generated implementations remain solver-only.
fn expanded_adt_derived_traits(adt: Adt, db: &RootDatabase) -> Vec<RustExpandedDeriveTrait> {
    expanded_adt_derived_traits_with_probe_predicates(adt, db, false)
}

/// Gather derive expansions while optionally retaining predicates on the compiler-owned empty probe.
fn expanded_adt_derived_traits_with_probe_predicates(
    adt: Adt,
    db: &RootDatabase,
    retain_probe_predicates: bool,
) -> Vec<RustExpandedDeriveTrait> {
    let semantics = Semantics::new(db);
    let Some(source) = semantics.source(adt) else {
        return Vec::new();
    };
    let attrs = source.value.attrs().collect::<Vec<_>>();
    let mut identities = Vec::new();
    for attr in attrs {
        let Some(expansions) = semantics.expand_derive_macro(&attr) else {
            continue;
        };
        for expansion in expansions.into_iter().flatten() {
            if expansion.err.is_some() {
                continue;
            }
            for impl_item in expansion.value.descendants().filter_map(ast::Impl::cast) {
                if !retain_probe_predicates
                    && (impl_item.generic_param_list().is_some() || impl_item.where_clause().is_some())
                {
                    continue;
                }
                let Some(implementation) = semantics.to_def(&impl_item) else {
                    continue;
                };
                if implementation.self_ty(db).as_adt() != Some(adt) {
                    continue;
                }
                let Some(trait_) = implementation.trait_(db) else {
                    continue;
                };
                if let Some(path) = canonical_module_def_path(ModuleDef::Trait(trait_), db) {
                    identities.push(RustExpandedDeriveTrait {
                        path,
                        associated_type_bindings: implementation_associated_type_bindings(implementation, db),
                    });
                }
            }
        }
    }
    identities.sort_by(|left, right| {
        (&left.path, &left.associated_type_bindings).cmp(&(&right.path, &right.associated_type_bindings))
    });
    identities.dedup();
    identities
}

/// Return the implementations observed from one macro on its compiler-generated probe.
///
/// The generated inspection root invokes each requested derive on a distinct top-level probe using the exact
/// canonical macro path. Matching that compiler-authored attribute supplies provenance without assuming that a macro
/// and any trait it implements share a name or module. Dependency crates cannot contribute evidence because their
/// crates have reverse dependencies and therefore are not graph heads.
fn macro_derive_probe_outputs(canonical_path: &str, db: &RootDatabase) -> Vec<RustExpandedDeriveTrait> {
    let expected_attribute = format!("#[derive({canonical_path})]");
    for krate in Crate::all(db) {
        if krate.is_builtin(db) || !krate.reverse_dependencies(db).is_empty() {
            continue;
        }
        for (_, definition) in krate.root_module(db).scope(db, None) {
            let ScopeDef::ModuleDef(ModuleDef::Adt(adt)) = definition else {
                continue;
            };
            if !adt.name(db).as_str().starts_with(INCAN_DERIVE_PROBE_PREFIX) {
                continue;
            }
            let semantics = Semantics::new(db);
            let Some(source) = semantics.source(adt) else {
                continue;
            };
            let invokes_macro = source.value.attrs().any(|attribute| {
                attribute
                    .syntax()
                    .text()
                    .to_string()
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .eq(expected_attribute.chars())
            });
            if invokes_macro {
                // A derive may branch on its input or emit `where Self: ...` predicates. This probe observation is
                // enough to select a candidate ABI, but generated Rust remains authoritative for whether the real
                // local declaration satisfies that contract.
                return expanded_adt_derived_traits_with_probe_predicates(adt, db, true);
            }
        }
    }
    Vec::new()
}

/// Return the canonical identity of one concrete ADT or builtin Rust type.
fn canonical_concrete_type_path(ty: &Type<'_>, db: &RootDatabase) -> Option<String> {
    if let Some(adt) = ty.as_adt() {
        return canonical_adt_path(adt, db);
    }
    ty.as_builtin().map(|builtin| builtin.name().as_str().to_owned())
}

/// Collect concrete associated-type assignments from one inspected implementation.
fn implementation_associated_type_bindings(implementation: Impl, db: &RootDatabase) -> Vec<RustAssociatedTypeBinding> {
    let mut bindings = implementation
        .items(db)
        .into_iter()
        .filter_map(|item| match item {
            AssocItem::TypeAlias(alias) => Some(RustAssociatedTypeBinding {
                name: alias.name(db).as_str().to_owned(),
                value_path: canonical_concrete_type_path(&alias.ty(db), db)?,
            }),
            AssocItem::Function(_) | AssocItem::Const(_) => None,
        })
        .collect::<Vec<_>>();
    bindings.sort();
    bindings.dedup();
    bindings
}

/// Return the canonical trait identity named by one source type bound.
///
/// This consumes the parsed Rust syntax tree rather than comparing the final segment of a source spelling. A path
/// that cannot resolve in its defining module is deliberately ignored instead of becoming nominal evidence.
fn source_type_bound_trait_path(
    bound: &ast::TypeBound,
    crate_name: &str,
    module: Module,
    db: &RootDatabase,
) -> Option<String> {
    let ast::Type::PathType(path_type) = bound.ty()? else {
        return None;
    };
    let path = path_type.path()?;
    let text = path.syntax().text().to_string();
    let base = text.split('<').next()?.trim();
    resolve_source_path(base, crate_name, module, db)
}

/// Return whether a source type bound has no generic arguments, associated-type equality, or higher-ranked form.
fn source_type_bound_is_unparameterized(bound: &ast::TypeBound) -> bool {
    bound.for_binder().is_none()
        && bound.question_mark_token().is_none()
        && matches!(
            bound.ty(),
            Some(ast::Type::PathType(path_type))
                if path_type
                    .path()
                    .and_then(|path| path.segment())
                    .is_some_and(|segment| segment.generic_arg_list().is_none())
        )
}

/// Convert the exactly representable part of one source trait bound into canonical associated-type equalities.
///
/// A missing generic argument list and equality-only associated-type arguments are complete at this metadata layer.
/// Type, const, lifetime, higher-ranked, optional, and associated-bound arguments remain solver-only.
fn source_bound_associated_type_requirements(
    bound: &ast::TypeBound,
    crate_name: &str,
    module: Module,
    db: &RootDatabase,
) -> Option<Vec<RustAssociatedTypeRequirement>> {
    if bound.for_binder().is_some() || bound.question_mark_token().is_some() {
        return None;
    }
    let ast::Type::PathType(path_type) = bound.ty()? else {
        return None;
    };
    let path = path_type.path()?;
    let trait_path = source_type_bound_trait_path(bound, crate_name, module, db)?;
    let Some(arguments) = path.segment()?.generic_arg_list() else {
        return Some(Vec::new());
    };
    arguments
        .generic_args()
        .map(|argument| {
            let ast::GenericArg::AssocTypeArg(argument) = argument else {
                return None;
            };
            if argument.eq_token().is_none()
                || argument.param_list().is_some()
                || argument.generic_arg_list().is_some()
                || argument.type_bound_list().is_some()
                || argument.const_arg().is_some()
            {
                return None;
            }
            let name = argument.name_ref()?.text().to_string();
            let ast::Type::PathType(value) = argument.ty()? else {
                return None;
            };
            let value_text = value.path()?.syntax().text().to_string();
            if value_text.contains('<') {
                return None;
            }
            Some(RustAssociatedTypeRequirement {
                trait_path: trait_path.clone(),
                name,
                value_path: resolve_source_path(value_text.as_str(), crate_name, module, db)?,
            })
        })
        .collect()
}

/// Return whether every predicate affecting the inner type parameter is represented by fallback metadata.
fn mutable_reference_fallback_source_is_complete(
    implementation: Impl,
    inner_param_name: &str,
    db: &RootDatabase,
) -> bool {
    let Some(source) = implementation.source(db) else {
        return false;
    };
    let item = source.value;
    let generics_are_representable = item.generic_param_list().is_some_and(|params| {
        params.generic_params().all(|param| match param {
            ast::GenericParam::TypeParam(param) => param.name().is_some_and(|name| name.text() == inner_param_name),
            ast::GenericParam::LifetimeParam(_) => true,
            ast::GenericParam::ConstParam(_) => false,
        })
    });
    let where_predicates_are_representable = item.where_clause().is_none_or(|clause| {
        clause.predicates().all(|predicate| {
            predicate
                .ty()
                .is_some_and(|ty| ty.syntax().text().to_string().trim() == inner_param_name)
        })
    });
    generics_are_representable && where_predicates_are_representable
}

/// Gather parsed source bounds whose subject is exactly one generic parameter.
fn source_bounds_for_type_param(implementation: Impl, param_name: &str, db: &RootDatabase) -> Vec<ast::TypeBound> {
    let Some(source) = implementation.source(db) else {
        return Vec::new();
    };
    let item = source.value;
    let inline = item
        .generic_param_list()
        .into_iter()
        .flat_map(|params| params.generic_params())
        .flat_map(|param| match param {
            ast::GenericParam::TypeParam(param) if param.name().is_some_and(|name| name.text() == param_name) => param
                .type_bound_list()
                .into_iter()
                .flat_map(|bounds| bounds.bounds())
                .collect::<Vec<_>>(),
            ast::GenericParam::ConstParam(_)
            | ast::GenericParam::LifetimeParam(_)
            | ast::GenericParam::TypeParam(_) => Vec::new(),
        });
    let where_clause = item
        .where_clause()
        .into_iter()
        .flat_map(|clause| clause.predicates())
        .filter(|predicate| {
            predicate
                .ty()
                .is_some_and(|ty| ty.syntax().text().to_string().trim() == param_name)
        })
        .flat_map(|predicate| {
            predicate
                .type_bound_list()
                .into_iter()
                .flat_map(|bounds| bounds.bounds())
        });
    inline.chain(where_clause).collect()
}

/// Gather parsed source bounds whose subject is exactly one ADT generic parameter.
fn source_bounds_for_adt_type_param(adt: Adt, param_name: &str, db: &RootDatabase) -> Vec<ast::TypeBound> {
    let Some(source) = adt.source(db) else {
        return Vec::new();
    };
    let item = source.value;
    let inline = item
        .generic_param_list()
        .into_iter()
        .flat_map(|params| params.generic_params())
        .filter_map(|param| match param {
            ast::GenericParam::TypeParam(param) if param.name().is_some_and(|name| name.text() == param_name) => Some(
                param
                    .type_bound_list()
                    .into_iter()
                    .flat_map(|bounds| bounds.bounds())
                    .collect::<Vec<_>>(),
            ),
            ast::GenericParam::ConstParam(_)
            | ast::GenericParam::LifetimeParam(_)
            | ast::GenericParam::TypeParam(_) => None,
        })
        .flatten();
    let where_clause = item
        .where_clause()
        .into_iter()
        .flat_map(|clause| clause.predicates())
        .filter(|predicate| {
            predicate
                .ty()
                .is_some_and(|ty| ty.syntax().text().to_string().trim() == param_name)
        })
        .flat_map(|predicate| {
            predicate
                .type_bound_list()
                .into_iter()
                .flat_map(|bounds| bounds.bounds())
        });
    inline.chain(where_clause).collect()
}

/// Return tuple arities whose implementation of `bound` is exactly element-wise over the same bound.
///
/// A foreign owner may accept a tuple through a dedicated implementation, through element-wise composition, or not
/// at all. Only the element-wise form permits the frontend to recurse into tuple leaves when selecting mutable
/// reference candidates.
fn trait_tuple_composition_arities(bound: Trait, db: &RootDatabase) -> Vec<usize> {
    let Some(bound_path) = canonical_module_def_path(ModuleDef::Trait(bound), db) else {
        return Vec::new();
    };
    let implementations = Impl::all_for_trait(db, bound);
    let mut arities = implementations
        .into_iter()
        .filter_map(|implementation| {
            if implementation.is_negative(db) {
                return None;
            }
            let fields = implementation.self_ty(db).tuple_fields(db);
            if fields.is_empty() {
                return None;
            }
            let field_params = fields
                .iter()
                .map(|field| field.as_type_param(db))
                .collect::<Option<Vec<_>>>()?;
            let generic_params = GenericDef::Impl(implementation).params(db);
            if generic_params
                .iter()
                .any(|param| matches!(param, GenericParam::ConstParam(_)))
            {
                return None;
            }
            let declared_params = generic_params
                .into_iter()
                .filter_map(|param| match param {
                    GenericParam::TypeParam(param) if !param.is_implicit(db) => Some(param),
                    GenericParam::TypeParam(_) | GenericParam::ConstParam(_) | GenericParam::LifetimeParam(_) => None,
                })
                .collect::<Vec<_>>();
            let field_names = field_params
                .iter()
                .map(|param| param.name(db).as_str().to_string())
                .collect::<HashSet<_>>();
            let declared_names = declared_params
                .iter()
                .map(|param| param.name(db).as_str().to_string())
                .collect::<HashSet<_>>();
            if field_names.len() != fields.len() || field_names != declared_names {
                return None;
            }
            let module = implementation.module(db);
            let crate_name = module
                .krate(db)
                .display_name(db)
                .map(|name| name.canonical_name().as_str().to_owned())?;
            field_params
                .iter()
                .all(|param| {
                    let source_bounds = source_bounds_for_type_param(implementation, param.name(db).as_str(), db);
                    source_bounds.len() == 1
                        && source_type_bound_trait_path(&source_bounds[0], crate_name.as_str(), module, db)
                            .is_some_and(|path| path == bound_path)
                })
                .then_some(fields.len())
        })
        .collect::<Vec<_>>();
    arities.sort_unstable();
    arities.dedup();
    arities
}

/// Return generic parameters whose declared trait bounds admit a mutable-reference alternative.
///
/// A `mut Foreign[T]` source parameter must not globally turn `T` into `&mut T`: ordinary containers such as
/// `Vec<T>` own their elements. The foreign type itself supplies the necessary structural evidence when one of its
/// parameter bounds has an implementation shaped like `impl<U> Bound for &mut U`. Each alternative retains both the
/// owner parameter's direct bounds and the inner `U` bounds, so consumers can reject a reference when the supplied
/// type already satisfies the direct bound or cannot satisfy the reference implementation. This never examines
/// provider/type names, so any equivalent foreign generic contract receives the same treatment.
fn adt_mutable_reference_type_params(adt: Adt, db: &RootDatabase) -> Vec<RustMutableReferenceTypeParam> {
    GenericDef::Adt(adt)
        .params(db)
        .into_iter()
        .filter_map(|param| match param {
            GenericParam::TypeParam(param) if !param.is_implicit(db) => Some(param),
            GenericParam::TypeParam(_) | GenericParam::ConstParam(_) | GenericParam::LifetimeParam(_) => None,
        })
        .filter_map(|param| {
            let source_bounds = source_bounds_for_adt_type_param(adt, param.name(db).as_str(), db);
            if source_bounds.is_empty() || !source_bounds.iter().all(source_type_bound_is_unparameterized) {
                return None;
            }
            let module = adt.module(db);
            let crate_name = module
                .krate(db)
                .display_name(db)
                .map(|name| name.canonical_name().as_str().to_owned())?;
            let source_bound_paths = source_bounds
                .iter()
                .filter_map(|bound| source_type_bound_trait_path(bound, crate_name.as_str(), module, db))
                .collect::<HashSet<_>>();
            let mut canonical_param_trait_bounds = param
                .trait_bounds(db)
                .into_iter()
                .filter_map(|bound| canonical_module_def_path(ModuleDef::Trait(bound), db).map(|path| (path, bound)))
                .filter(|(path, _)| source_bound_paths.contains(path))
                .collect::<Vec<_>>();
            canonical_param_trait_bounds.sort_by(|left, right| left.0.cmp(&right.0));
            canonical_param_trait_bounds.dedup_by(|left, right| left.0 == right.0);
            let [(owner_trait_path, owner_trait)] = canonical_param_trait_bounds.as_slice() else {
                return None;
            };

            let mut mutable_reference_candidates = generic_mutable_reference_candidates(*owner_trait, db);
            mutable_reference_candidates.sort_by(|left, right| {
                (
                    &left.required_traits,
                    &left.required_associated_type_bindings,
                    left.fallback_is_complete,
                )
                    .cmp(&(
                        &right.required_traits,
                        &right.required_associated_type_bindings,
                        right.fallback_is_complete,
                    ))
            });
            mutable_reference_candidates.dedup();

            (!mutable_reference_candidates.is_empty()).then(|| RustMutableReferenceTypeParam {
                type_param: param.name(db).as_str().to_string(),
                direct_trait_bounds: vec![owner_trait_path.clone()],
                mutable_reference_candidates,
                tuple_composition_arities: trait_tuple_composition_arities(*owner_trait, db),
            })
        })
        .collect()
}

/// Return generic `&mut T` implementations of `bound`, including complete parsed predicates on their inner `T`.
fn generic_mutable_reference_candidates(bound: Trait, db: &RootDatabase) -> Vec<RustMutableReferenceCandidate> {
    Impl::all_for_trait(db, bound)
        .into_iter()
        .filter_map(|implementation| {
            if implementation.is_negative(db) {
                return None;
            }
            let Some((inner, Mutability::Mut)) = implementation.self_ty(db).as_reference() else {
                return None;
            };
            let inner_display = inner
                .display(db, DisplayTarget::from_crate(db, bound.module(db).krate(db).base()))
                .to_string();
            GenericDef::Impl(implementation)
                .params(db)
                .into_iter()
                .filter_map(|param| match param {
                    GenericParam::TypeParam(param) if !param.is_implicit(db) => Some(param),
                    GenericParam::TypeParam(_) | GenericParam::ConstParam(_) | GenericParam::LifetimeParam(_) => None,
                })
                .find(|param| param.name(db).as_str() == inner_display)
                .map(|param| {
                    let module = implementation.module(db);
                    let crate_name = module
                        .krate(db)
                        .display_name(db)
                        .map(|name| name.canonical_name().as_str().to_owned())
                        .unwrap_or_default();
                    let has_source = implementation.source(db).is_some();
                    let source_bounds = source_bounds_for_type_param(implementation, param.name(db).as_str(), db);
                    let mut required_traits = source_bounds
                        .iter()
                        .filter_map(|required| source_type_bound_trait_path(required, crate_name.as_str(), module, db))
                        .collect::<Vec<_>>();
                    required_traits.sort();
                    required_traits.dedup();
                    let mut hir_required_traits = param
                        .trait_bounds(db)
                        .into_iter()
                        .filter_map(|required| canonical_module_def_path(ModuleDef::Trait(required), db))
                        .filter(|required| required != "core::marker::Sized" && required != "Sized")
                        .collect::<Vec<_>>();
                    hir_required_traits.sort();
                    hir_required_traits.dedup();
                    let source_trait_identities_are_complete = has_source && required_traits == hir_required_traits;
                    if !source_trait_identities_are_complete {
                        required_traits = hir_required_traits;
                    }
                    let fallback_requirements = source_bounds
                        .iter()
                        .map(|required| {
                            source_bound_associated_type_requirements(required, crate_name.as_str(), module, db)
                        })
                        .collect::<Option<Vec<_>>>();
                    let fallback_is_complete =
                        mutable_reference_fallback_source_is_complete(implementation, param.name(db).as_str(), db)
                            && has_source
                            && source_trait_identities_are_complete
                            && fallback_requirements.is_some();
                    let mut required_associated_type_bindings = fallback_requirements
                        .unwrap_or_default()
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>();
                    required_associated_type_bindings.sort();
                    required_associated_type_bindings.dedup();
                    RustMutableReferenceCandidate {
                        required_traits,
                        required_associated_type_bindings,
                        fallback_is_complete,
                    }
                })
        })
        .collect()
}

/// Return source-declared generics for a type alias receiver.
fn source_type_alias_generics(alias: ra_ap_hir::TypeAlias, db: &RootDatabase) -> SourceOwnerGenerics {
    source_owner_generics(alias.source(db).and_then(|source| source.value.generic_param_list()))
}

/// Extract a Rust function signature from inspection metadata.
fn extract_function_sig(f: Function, db: &RootDatabase, dt: DisplayTarget) -> RustFunctionSig {
    let params = f
        .assoc_fn_params(db)
        .into_iter()
        .map(|p| {
            let mut type_display = function_sig_type_display(p.ty(), db, dt);
            if let Some(source_type_display) = source_function_param_type_display(f, &p, db) {
                type_display = source_type_display;
            }
            RustParam {
                name: p.name(db).map(|n| n.as_str().to_owned()),
                type_display,
            }
        })
        .collect();
    let output_type = f.async_ret_type(db).unwrap_or_else(|| f.ret_type(db));
    let mut return_type = function_sig_type_display(&output_type, db, dt);
    if let Some(source_return_type) = source_function_return_type_display(f, db) {
        return_type = source_return_type;
    }
    RustFunctionSig {
        type_params: source_function_type_params(f, db),
        params,
        return_type,
        is_async: f.is_async(db),
        // `hir::Function` does not yet expose a cheap `is_unsafe` predicate without reaching into
        // private `FunctionId` bits; Phase 1 keeps this conservative default.
        is_unsafe: false,
    }
}

/// Collect exported inherent methods for a Rust type, keyed deterministically by method name.
fn collect_inherent_methods(ty: Type<'_>, db: &RootDatabase, dt: DisplayTarget) -> Vec<RustMethodSig> {
    let mut by_name: BTreeMap<String, RustMethodSig> = BTreeMap::new();
    let _: Option<()> = ty.iterate_assoc_items(db, |item| {
        if let AssocItem::Function(f) = item {
            let name = f.name(db).as_str().to_owned();
            let sig = extract_function_sig(f, db, dt);
            if is_exported_rust_api(f.visibility(db)) {
                by_name.insert(name.clone(), RustMethodSig { name, signature: sig });
            }
        }
        None
    });
    by_name.into_values().collect()
}

/// Collect the queried surface crate and every transitive crate dependency that can define its Rust API traits.
fn crate_dependency_closure(surface_crate: Crate, db: &RootDatabase) -> HashSet<Crate> {
    let mut closure = HashSet::new();
    let mut pending = vec![surface_crate];
    while let Some(krate) = pending.pop() {
        if !closure.insert(krate) {
            continue;
        }
        pending.extend(krate.dependencies(db).into_iter().map(|dependency| dependency.krate));
    }
    closure
}

/// Collect non-blanket trait impls whose traits belong to the queried Rust surface dependency closure.
///
/// `Impl::all_for_type` scans the entire loaded rust-analyzer graph. Rust's orphan rules allow a downstream crate to
/// define its own trait and implement it for an imported type, but that trait is not part of the queried crate's API.
/// Restricting the trait's defining crate preserves intrinsic dependency traits while excluding ambient downstream
/// impls whose presence varies with workspace, cache, and host-platform graph shape.
fn collect_implemented_traits(
    ty: Type<'_>,
    authorized_trait_crates: &HashSet<Crate>,
    mutable_reference: bool,
    db: &RootDatabase,
) -> Vec<RustImplementedTrait> {
    let mut traits = BTreeMap::new();
    for impl_def in Impl::all_for_type(db, ty.clone()) {
        let Some(trait_def) = impl_def.trait_(db) else {
            continue;
        };
        if !authorized_trait_crates.contains(&trait_def.module(db).krate(db)) {
            continue;
        }
        if !ty.clone().impls_trait(db, trait_def, &[]) {
            continue;
        }
        let path = canonical_module_def_path(ModuleDef::Trait(trait_def), db)
            .unwrap_or_else(|| trait_def.name(db).as_str().to_owned());
        traits.insert(
            path.clone(),
            RustImplementedTrait {
                path,
                mutable_reference,
            },
        );
    }
    traits.into_values().collect()
}

/// Collect persisted direct and mutable-reference trait implementations for one concrete Rust type.
fn collect_type_implemented_traits(
    ty: Type<'_>,
    authorized_trait_crates: &HashSet<Crate>,
    db: &RootDatabase,
) -> Vec<RustImplementedTrait> {
    let mut implementations = collect_implemented_traits(ty.clone(), authorized_trait_crates, false, db);
    implementations.extend(collect_implemented_traits(
        ty.add_reference(Mutability::Mut),
        authorized_trait_crates,
        true,
        db,
    ));
    implementations
        .sort_by(|left, right| (&left.path, left.mutable_reference).cmp(&(&right.path, right.mutable_reference)));
    implementations.dedup();
    implementations
}

/// Collect public Rust fields in declaration order with source-facing names and semantic type shapes.
///
/// Field names are taken from Rust source when possible so raw identifiers surface to Incan without `r#`, and codegen
/// can later decide whether that source-facing name needs raw Rust emission.
fn collect_public_fields(ty: Type<'_>, db: &RootDatabase, dt: DisplayTarget, crate_name: &str) -> Vec<RustFieldInfo> {
    if let Some(adt) = ty.as_adt() {
        let type_args: Vec<Type<'_>> = ty.type_arguments().collect();
        let fields = match adt {
            Adt::Struct(strukt) => strukt.fields(db),
            Adt::Union(union) => union.fields(db),
            Adt::Enum(_) => Vec::new(),
        };
        let mut collected = Vec::new();
        for field in fields {
            if !is_exported_rust_api(field.visibility(db)) {
                continue;
            }
            let field_ty = field.ty_with_args(db, type_args.iter().cloned());
            let hir_shape = rust_type_shape(&field_ty, db, dt);
            let (type_display, type_shape) = source_field_metadata(&field, &hir_shape, db, crate_name)
                .unwrap_or_else(|| (function_sig_type_display(&field_ty, db, dt), hir_shape));
            collected.push(RustFieldInfo {
                name: source_field_constructor_label(&field, db).unwrap_or_else(|| field.name(db).as_str().to_owned()),
                type_display,
                type_shape,
            });
        }
        return collected;
    }

    let mut fields = Vec::new();
    for (field, field_ty) in ty.fields(db) {
        if !is_exported_rust_api(field.visibility(db)) {
            continue;
        }
        let hir_shape = rust_type_shape(&field_ty, db, dt);
        let (type_display, type_shape) = source_field_metadata(&field, &hir_shape, db, crate_name)
            .unwrap_or_else(|| (function_sig_type_display(&field_ty, db, dt), hir_shape));
        fields.push(RustFieldInfo {
            name: source_field_constructor_label(&field, db).unwrap_or_else(|| field.name(db).as_str().to_owned()),
            type_display,
            type_shape,
        });
    }
    fields
}

/// Record every variant of an enum with the semantic shape of each payload field and the carrier it is stored in.
///
/// A payload written as `Box<T>` through any re-export of `alloc::boxed::Box` is recorded as `T` with a `Boxed`
/// carrier, so consumers see the semantic type while lowering can restore the storage the constructor needs.
fn collect_enum_variant_payloads(
    enum_: Enum,
    ty: Type<'_>,
    db: &RootDatabase,
    dt: DisplayTarget,
    crate_name: &str,
) -> Vec<RustVariantInfo> {
    let type_args: Vec<Type<'_>> = ty.type_arguments().collect();
    let mut variants = Vec::new();
    for variant in enum_.variants(db) {
        let (fields, field_carriers) = collect_variant_payloads(variant, &type_args, db, dt, crate_name)
            .into_iter()
            .unzip();
        variants.push(RustVariantInfo {
            name: variant.name(db).as_str().to_owned(),
            fields,
            field_carriers,
        });
    }
    variants.sort_by(|a, b| a.name.cmp(&b.name));
    variants
}

/// Collect one enum variant's exported payload fields using source-preserved type identities where available.
///
/// Each entry pairs the semantic payload shape with the storage carrier the Rust field actually uses.
fn collect_variant_payloads(
    variant: Variant,
    type_args: &[Type<'_>],
    db: &RootDatabase,
    dt: DisplayTarget,
    crate_name: &str,
) -> Vec<(RustTypeShape, RustPayloadCarrier)> {
    variant
        .fields(db)
        .iter()
        .filter(|field| is_exported_rust_api(field.visibility(db)))
        .map(|field| {
            let field_ty = field.ty_with_args(db, type_args.iter().cloned());
            let hir_shape = rust_type_shape(&field_ty, db, dt);
            let shape = source_field_metadata(field, &hir_shape, db, crate_name)
                .map_or(hir_shape, |(_, source_shape)| source_shape);
            normalize_variant_payload_shape(shape)
        })
        .collect()
}

fn module_children(module: Module, db: &RootDatabase) -> RustModuleInfo {
    let mut children = Vec::new();
    for (name, def) in module.scope(db, None) {
        let ScopeDef::ModuleDef(md) = def else {
            continue;
        };
        if !is_exported_rust_api(md.visibility(db)) {
            continue;
        }
        let kind_hint = match md {
            ModuleDef::Module(_) => RustModuleChildKind::Module,
            ModuleDef::Adt(_) | ModuleDef::BuiltinType(_) => RustModuleChildKind::Type,
            ModuleDef::Function(_) => RustModuleChildKind::Function,
            ModuleDef::Const(_) | ModuleDef::Static(_) => RustModuleChildKind::Constant,
            ModuleDef::Trait(_) => RustModuleChildKind::Trait,
            ModuleDef::TypeAlias(_) => RustModuleChildKind::Type,
            ModuleDef::Variant(_) => RustModuleChildKind::Type,
            ModuleDef::Macro(_) => RustModuleChildKind::Other,
        };
        children.push(RustModuleChild {
            name: name.as_str().to_owned(),
            kind_hint,
        });
    }
    children.sort_by(|a, b| a.name.cmp(&b.name));
    RustModuleInfo { children }
}

/// Extract a trait's public associated items and any same-path derive-macro expansion contract.
///
/// `derive_path` is the consumer-visible import path rather than the trait definition path because Rust keeps traits
/// and derive macros in separate namespaces and a facade may re-export them from different crates.
fn trait_info(tr: Trait, derive_path: &str, db: &RootDatabase, dt: DisplayTarget) -> RustTraitInfo {
    let mut items = Vec::new();
    for item in tr.items(db) {
        match item {
            AssocItem::Function(f) => {
                if !is_exported_rust_api(f.visibility(db)) {
                    continue;
                }
                items.push(RustTraitAssoc::Function {
                    name: f.name(db).as_str().to_owned(),
                    signature: extract_function_sig(f, db, dt),
                });
            }
            AssocItem::Const(c) => {
                if !is_exported_rust_api(c.visibility(db)) {
                    continue;
                }
                // Anonymous or nameless associated consts in extracted metadata surface as empty `name`.
                let n = c.name(db).map(|name| name.as_str().to_owned()).unwrap_or_default();
                items.push(RustTraitAssoc::Constant {
                    name: n,
                    type_display: format_ty(&c.ty(db), db, dt),
                });
            }
            AssocItem::TypeAlias(t) => {
                if !is_exported_rust_api(t.visibility(db)) {
                    continue;
                }
                items.push(RustTraitAssoc::TypeAlias {
                    name: t.name(db).as_str().to_owned(),
                });
            }
        }
    }
    // Traits and derive macros live in separate Rust namespaces. A public facade can therefore re-export a trait
    // from its defining crate and a same-spelling derive macro from another crate. Probe the path the consumer
    // actually imported, not the trait's definition path, because only the former preserves that macro namespace.
    let derive_macro = (!derive_path.is_empty())
        .then(|| macro_derive_probe_outputs(derive_path, db))
        .filter(|outputs| !outputs.is_empty())
        .map(|expanded_traits| RustMacroInfo { expanded_traits });
    RustTraitInfo { items, derive_macro }
}

/// Find a crate by any spelling that can legally name it across Cargo and Rust surfaces.
///
/// rust-inspect queries use canonical Rust paths, so the first segment may be the Rust crate name even when Cargo
/// registered the package with hyphens or via a differently-cased display name.
fn find_crate(workspace: &RustWorkspace, crate_name: &str) -> Option<Crate> {
    workspace.crate_by_name(crate_name)
}

/// Resolve a public Rust path while retaining the module-definition namespace when a same-spelling macro also exists.
///
/// rust-analyzer's direct path resolver is preferred. The scope walk is the compatibility fallback needed for facade
/// paths whose final spelling occupies more than one Rust namespace.
fn resolve_module_def(db: &RootDatabase, krate: Crate, segments: &[Name]) -> Result<ModuleDef, RustMetadataError> {
    let root = krate.root_module(db);
    if let Some(mut it) = root.resolve_mod_path(db, segments.iter().cloned())
        && let Some(first) = it.next()
    {
        return Ok(first.into_module_def());
    }

    let mut module = root;
    for (idx, segment) in segments.iter().enumerate() {
        let is_last = idx + 1 == segments.len();
        let mut matches = module
            .scope(db, None)
            .into_iter()
            .filter(|(name, _)| name.as_str() == segment.as_str());

        if is_last {
            let Some((_, scope_def)) = matches.next() else {
                return Err(RustMetadataError::PathNotResolved(segments_display(segments)));
            };
            return match scope_def {
                ScopeDef::ModuleDef(def) => Ok(def),
                _ => Err(RustMetadataError::PathNotResolved(segments_display(segments))),
            };
        }

        let next_module = matches.find_map(|(_, scope_def)| match scope_def {
            ScopeDef::ModuleDef(ModuleDef::Module(module)) => Some(module),
            _ => None,
        });
        let Some(found) = next_module else {
            return Err(RustMetadataError::PathNotResolved(segments_display(segments)));
        };
        module = found;
    }
    Err(RustMetadataError::PathNotResolved(segments_display(segments)))
}

fn segments_display(segments: &[Name]) -> String {
    segments.iter().map(|n| n.as_str()).collect::<Vec<_>>().join("::")
}

/// Parse `crate::a::b` style paths (as used in [`incan::frontend::symbols::RustItemInfo::path`]).
fn split_canonical_path(path: &str) -> Result<(&str, Vec<Name>), RustMetadataError> {
    let parts: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return Err(RustMetadataError::PathNotResolved(path.to_owned()));
    }
    let crate_name = parts[0];
    let segments: Vec<Name> = parts[1..].iter().map(|s| Name::new_root(s)).collect();
    Ok((crate_name, segments))
}

/// Extract metadata for `canonical_path` (e.g. `hashbrown::HashMap`, `regex::Regex`).
///
/// ## Contract
///
/// rust-analyzer's type layer uses thread-local database attachment; this entry point wraps the implementation in
/// [`attach_db`] so callers only need a `RootDatabase` reference.
pub fn extract_rust_item(
    workspace: &RustWorkspace,
    canonical_path: &str,
) -> Result<RustItemMetadata, RustMetadataError> {
    let db = workspace.db();
    attach_db(db, || extract_rust_item_inner(workspace, db, canonical_path))
}

/// Compare two fully concrete Rust types without treating their rust-analyzer solver environments as identity.
fn same_concrete_rust_type(left: &Type<'_>, right: &Type<'_>, db: &RootDatabase) -> bool {
    match (left.as_reference(), right.as_reference()) {
        (Some((left, left_mutability)), Some((right, right_mutability))) => {
            return left_mutability == right_mutability && same_concrete_rust_type(&left, &right, db);
        }
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    if let (Some((left_adt, left_args)), Some((right_adt, right_args))) =
        (left.as_adt_with_args(), right.as_adt_with_args())
    {
        return left_adt == right_adt
            && left_args.len() == right_args.len()
            && left_args
                .iter()
                .zip(right_args.iter())
                .all(|(left, right)| match (left, right) {
                    (Some(left), Some(right)) => same_concrete_rust_type(left, right, db),
                    (None, None) => true,
                    (Some(_), None) | (None, Some(_)) => false,
                });
    }
    if let (Some(left), Some(right)) = (left.as_builtin(), right.as_builtin()) {
        return left == right;
    }
    if left.is_tuple() && right.is_tuple() {
        let left = left.tuple_fields(db);
        let right = right.tuple_fields(db);
        return left.len() == right.len()
            && left
                .iter()
                .zip(right.iter())
                .all(|(left, right)| same_concrete_rust_type(left, right, db));
    }
    false
}

/// Return whether the complete loaded graph contains an unconditional, non-generic implementation for this exact
/// concrete type. This closes rust-analyzer's cross-crate environment blind spot without weakening a solver-negative
/// generic or associated-type obligation.
fn graph_has_exact_unconditional_trait_impl(db: &RootDatabase, trait_: Trait, queried_type: &Type<'_>) -> bool {
    Impl::all_for_trait(db, trait_).into_iter().any(|implementation| {
        if implementation.is_negative(db) || !same_concrete_rust_type(&implementation.self_ty(db), queried_type, db) {
            return false;
        }
        implementation
            .source(db)
            .is_some_and(|source| source.value.generic_param_list().is_none() && source.value.where_clause().is_none())
    })
}

/// Ask rust-analyzer's trait solver whether one concrete foreign type implements a concrete foreign trait.
///
/// Unlike the persisted metadata index, this preserves generic arguments, associated-type equalities, and other
/// obligations carried by Rust's trait solver. `mutable_reference` checks the corresponding `&mut Type` obligation.
pub fn rust_type_implements_trait(
    workspace: &RustWorkspace,
    type_path: &str,
    trait_path: &str,
    mutable_reference: bool,
) -> Result<bool, RustMetadataError> {
    let db = workspace.db();
    attach_db(db, || {
        let (type_crate_name, type_segments) = split_canonical_path(type_path)?;
        let type_crate = find_crate(workspace, type_crate_name)
            .ok_or_else(|| RustMetadataError::CrateNotFound(type_crate_name.to_owned()))?;
        let type_def = resolve_module_def(db, type_crate, &type_segments)?;
        let type_ = match type_def {
            ModuleDef::Adt(adt) => adt.ty(db),
            ModuleDef::BuiltinType(builtin) => builtin.ty(db),
            _ => return Err(RustMetadataError::PathNotResolved(type_path.to_owned())),
        };

        let (trait_crate_name, trait_segments) = split_canonical_path(trait_path)?;
        let trait_crate = find_crate(workspace, trait_crate_name)
            .ok_or_else(|| RustMetadataError::CrateNotFound(trait_crate_name.to_owned()))?;
        let trait_def = resolve_module_def(db, trait_crate, &trait_segments)?;
        let ModuleDef::Trait(trait_) = trait_def else {
            return Err(RustMetadataError::PathNotResolved(trait_path.to_owned()));
        };

        // `Adt::ty` carries the type owner's solver environment, but a legal implementation may live in the trait
        // owner's crate and become visible only from the consuming graph root. Query every non-sysroot graph head;
        // these are the smallest set of environments that retain complete downstream implementation authority.
        // The tuple round-trip is rust-analyzer's public API for retaining the exact internal type while changing its
        // environment; it does not alter the queried Rust type.
        let mut authority_types = vec![type_.clone()];
        for authority in Crate::all(db)
            .into_iter()
            .filter(|krate| !krate.is_builtin(db) && krate.reverse_dependencies(db).is_empty())
        {
            authority_types.extend(Type::new_tuple(authority.base(), std::slice::from_ref(&type_)).tuple_fields(db));
        }
        let solver_result = authority_types.into_iter().any(|authority_type| {
            let authority_type = if mutable_reference {
                authority_type.add_reference(Mutability::Mut)
            } else {
                authority_type
            };
            authority_type.impls_trait(db, trait_, &[])
        });
        if solver_result {
            return Ok(true);
        }
        let queried_type = if mutable_reference {
            type_.add_reference(Mutability::Mut)
        } else {
            type_
        };
        Ok(graph_has_exact_unconditional_trait_impl(db, trait_, &queried_type))
    })
}

/// Extract metadata after the rust-analyzer database has been attached for the current thread.
fn extract_rust_item_inner(
    workspace: &RustWorkspace,
    db: &RootDatabase,
    canonical_path: &str,
) -> Result<RustItemMetadata, RustMetadataError> {
    let (crate_name, segments) = split_canonical_path(canonical_path)?;
    let krate =
        find_crate(workspace, crate_name).ok_or_else(|| RustMetadataError::CrateNotFound(crate_name.to_owned()))?;
    let dt = DisplayTarget::from_crate(db, krate.base());
    let authorized_trait_crates = crate_dependency_closure(krate, db);
    let def = resolve_module_def(db, krate, &segments)?;
    let vis = map_visibility(def.visibility(db));
    let kind = match def {
        ModuleDef::Module(m) => RustItemKind::Module(module_children(m, db)),
        ModuleDef::Function(f) => RustItemKind::Function(extract_function_sig(f, db, dt)),
        ModuleDef::Adt(adt) => {
            let ty = adt.ty(db);
            let generics = source_adt_generics(adt, db);
            let type_param_defaults = canonical_type_param_defaults(&generics, adt.module(db), db);
            RustItemKind::Type(RustTypeInfo {
                type_params: generics.type_params,
                type_param_defaults,
                mutable_reference_type_params: adt_mutable_reference_type_params(adt, db),
                expanded_derive_traits: expanded_adt_derived_traits(adt, db),
                has_const_params: generics.has_const_params,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: collect_inherent_methods(ty.clone(), db, dt),
                implemented_traits: collect_type_implemented_traits(ty.clone(), &authorized_trait_crates, db),
                fields: collect_public_fields(ty.clone(), db, dt, crate_name),
                variants: match adt {
                    Adt::Enum(enum_) => collect_enum_variant_payloads(enum_, ty, db, dt, crate_name),
                    _ => Vec::new(),
                },
            })
        }
        ModuleDef::BuiltinType(b) => {
            let ty = b.ty(db);
            RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: collect_inherent_methods(ty.clone(), db, dt),
                implemented_traits: collect_type_implemented_traits(ty.clone(), &authorized_trait_crates, db),
                fields: collect_public_fields(ty, db, dt, crate_name),
                variants: Vec::new(),
            })
        }
        ModuleDef::Const(c) => RustItemKind::Constant {
            type_display: source_const_type_identity_display(c, db).unwrap_or_else(|| format_ty(&c.ty(db), db, dt)),
        },
        ModuleDef::Static(s) => RustItemKind::Constant {
            type_display: source_static_type_identity_display(s, db).unwrap_or_else(|| format_ty(&s.ty(db), db, dt)),
        },
        ModuleDef::Trait(t) => RustItemKind::Trait(trait_info(t, canonical_path, db, dt)),
        ModuleDef::TypeAlias(a) => {
            let ty = a.ty(db);
            let generics = source_type_alias_generics(a, db);
            let type_param_defaults = canonical_type_param_defaults(&generics, a.module(db), db);
            RustItemKind::Type(RustTypeInfo {
                type_params: generics.type_params,
                type_param_defaults,
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: generics.has_const_params,
                alias_target: source_type_alias_target_display(a, db)
                    .and_then(|target| source_type_alias_identity_display(a, target.as_str(), db).or(Some(target)))
                    .or_else(|| Some(format_ty(&ty, db, dt))),
                metadata_completeness: Default::default(),
                methods: collect_inherent_methods(ty.clone(), db, dt),
                implemented_traits: collect_type_implemented_traits(ty.clone(), &authorized_trait_crates, db),
                fields: collect_public_fields(ty, db, dt, crate_name),
                variants: Vec::new(),
            })
        }
        ModuleDef::Variant(_) => RustItemKind::Unsupported {
            description: "enum variant".to_owned(),
        },
        ModuleDef::Macro(_) => RustItemKind::Macro(RustMacroInfo {
            expanded_traits: macro_derive_probe_outputs(canonical_path, db),
        }),
    };
    Ok(RustItemMetadata {
        canonical_path: canonical_path.to_owned(),
        definition_path: canonical_module_def_path(def, db),
        visibility: vis,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use incan_core::interop::{RustItemKind, RustTypeShape};

    use super::{RustWorkspace, exact_numeric_boundary_display, extract_rust_item};
    use crate::cache::RustMetadataCache;

    #[test]
    fn exact_numeric_boundary_display_preserves_widths() {
        assert_eq!(exact_numeric_boundary_display("u32").as_deref(), Some("u32"));
        assert_eq!(exact_numeric_boundary_display("& i32").as_deref(), Some("&i32"));
        assert_eq!(exact_numeric_boundary_display("String"), None);
    }

    #[test]
    fn function_metadata_preserves_canonical_source_type_identity() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "canonical_identity_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"use std::vec::Vec as ByteVec;

pub struct Payload;
pub struct Codec;

impl Codec {
    pub fn bytes(&self) -> ByteVec<u8> { Vec::new() }
    pub fn signed(&self) -> Vec<i32> { Vec::new() }
    pub fn payload(&self) -> Option<Payload> { None }
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "canonical_identity_probe::Codec")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        let return_type = |name: &str| {
            info.methods
                .iter()
                .find(|method| method.name == name)
                .map(|method| method.signature.return_type.as_str())
        };
        assert_eq!(return_type("bytes"), Some("std::vec::Vec<u8>"));
        assert_eq!(return_type("signed"), Some("Vec<i32>"));
        assert_eq!(
            return_type("payload"),
            Some("Option<canonical_identity_probe::Payload>")
        );
        assert!(
            info.methods
                .iter()
                .all(|method| !method.signature.return_type.contains("alloc::alloc::Global")),
            "source identity must not gain rust-analyzer's expanded default allocator"
        );
        Ok(())
    }

    #[test]
    fn expanded_tuple_contract_flows_through_metadata_and_disk_cache() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let driver = tmp.path().join("tuple-driver");
        let provider = tmp.path().join("tuple-provider");
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::create_dir_all(driver.join("src"))?;
        fs::create_dir_all(provider.join("src"))?;
        fs::write(
            driver.join("Cargo.toml"),
            r#"[package]
name = "tuple-driver"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true
"#,
        )?;
        fs::write(
            driver.join("src/lib.rs"),
            r#"use proc_macro::{TokenStream, TokenTree};

#[proc_macro]
pub fn tuple_range(input: TokenStream) -> TokenStream {
    let callback = input
        .into_iter()
        .find_map(|token| match token {
            TokenTree::Ident(ident) => Some(ident.to_string()),
            _ => None,
        })
        .expect("callback identifier");
    format!("pub struct GeneratedByDriver; {callback}!(A, B); {callback}!(A, B, C);")
        .parse()
        .expect("valid generated tuple implementations")
}

#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let name = input
        .into_iter()
        .skip_while(|token| !matches!(token, TokenTree::Ident(ident) if ident.to_string() == "struct"))
        .nth(1)
        .and_then(|token| match token {
            TokenTree::Ident(ident) => Some(ident.to_string()),
            _ => None,
        })
        .expect("derived struct identifier");
    let implementation = if name.starts_with("__IncanDeriveProbe") {
        format!("impl Component for {name} where Self: Send + Sync + 'static {{ type Mutability = Mutable; }}")
    } else {
        format!("impl Component for {name} {{ type Mutability = Mutable; }}")
    };
    implementation.parse().expect("valid generated component implementation")
}

#[proc_macro_derive(Misleading)]
pub fn derive_misleading(input: TokenStream) -> TokenStream {
    let name = input
        .into_iter()
        .skip_while(|token| !matches!(token, TokenTree::Ident(ident) if ident.to_string() == "struct"))
        .nth(1)
        .and_then(|token| match token {
            TokenTree::Ident(ident) => Some(ident.to_string()),
            _ => None,
        })
        .expect("derived struct identifier");
    format!("impl Other for {name} {{}}")
        .parse()
        .expect("valid generated other implementation")
}
"#,
        )?;
        fs::write(
            provider.join("Cargo.toml"),
            r#"[package]
name = "tuple-provider-probe"
version = "0.1.0"
edition = "2021"

[dependencies]
tuple-driver = { path = "../tuple-driver" }
"#,
        )?;
        fs::write(
            provider.join("src/lib.rs"),
            r#"use tuple_driver::tuple_range;
pub use tuple_driver::{Component, Misleading};

pub trait QueryData {}
pub mod component {
    pub trait Component { type Mutability; }
}
pub use component::Component;
pub trait Misleading {}
pub trait Other {}
pub struct Mutable;
pub struct Immutable;
pub struct FooBar<T: QueryData>(pub T);
pub struct Defaulted<T = Mutable>(pub T);

pub struct __IncanDeriveProbeDependencyCollision;
impl Component for __IncanDeriveProbeDependencyCollision { type Mutability = Immutable; }

#[derive(tuple_driver::Component)]
pub struct Widget;

#[derive(tuple_driver::Misleading)]
pub struct MisleadingWidget;

impl<T: Component<Mutability = Mutable>> QueryData for &mut T {}

macro_rules! impl_tuple_query_data {
    ($($name:ident),*) => {
        impl<$($name: QueryData),*> QueryData for ($($name,)*) {}
    };
}

tuple_range!(impl_tuple_query_data, 2, 3, F);
#[cfg(any())]
tuple_range!(impl_tuple_query_data, 7, 8, F);
#[cfg_attr(all(), cfg(any()))]
tuple_range!(impl_tuple_query_data, 9, 10, F);
"#,
        )?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "tuple-consumer-probe"
version = "0.1.0"
edition = "2021"

[dependencies]
tuple-provider-probe = { path = "tuple-provider" }
tuple-driver = { path = "tuple-driver" }
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub use tuple_provider_probe::*;

#[derive(tuple_provider_probe::Component)]
struct __IncanDeriveProbe0;

#[derive(tuple_provider_probe::Misleading)]
struct __IncanDeriveProbe1;

#[derive(tuple_driver::Component)]
struct __IncanDeriveProbe2;

#[derive(tuple_driver::Misleading)]
struct __IncanDeriveProbe3;
"#,
        )?;

        let expanded_workspace = RustWorkspace::load_with_options(tmp.path(), &|_| {}, true)?;
        let generated = extract_rust_item(&expanded_workspace, "tuple_provider_probe::GeneratedByDriver")?;
        assert!(matches!(generated.kind, RustItemKind::Type(_)));
        let widget = extract_rust_item(&expanded_workspace, "tuple_provider_probe::Widget")?;
        let RustItemKind::Type(widget_info) = widget.kind else {
            return Err(std::io::Error::other("expected expanded Widget type metadata").into());
        };
        assert_eq!(widget_info.expanded_derive_traits.len(), 1);
        assert_eq!(
            widget_info.expanded_derive_traits[0].path,
            "tuple_provider_probe::component::Component"
        );
        assert_eq!(widget_info.expanded_derive_traits[0].associated_type_bindings.len(), 1);
        assert_eq!(
            widget_info.expanded_derive_traits[0].associated_type_bindings[0].name,
            "Mutability"
        );
        assert_eq!(
            widget_info.expanded_derive_traits[0].associated_type_bindings[0].value_path,
            "tuple_provider_probe::Mutable"
        );
        let misleading = extract_rust_item(&expanded_workspace, "tuple_provider_probe::MisleadingWidget")?;
        let RustItemKind::Type(misleading_info) = misleading.kind else {
            return Err(std::io::Error::other("expected expanded MisleadingWidget type metadata").into());
        };
        assert_eq!(misleading_info.expanded_derive_traits.len(), 1);
        assert_eq!(
            misleading_info.expanded_derive_traits[0].path, "tuple_provider_probe::Other",
            "derive metadata must reflect the generated impl rather than the derive macro's name"
        );
        let component = extract_rust_item(&expanded_workspace, "tuple_provider_probe::Component")?;
        let RustItemKind::Trait(component_info) = component.kind else {
            return Err(std::io::Error::other("expected Component trait metadata").into());
        };
        let component_derive = component_info
            .derive_macro
            .as_ref()
            .and_then(|macro_info| macro_info.expanded_traits.first())
            .ok_or_else(|| std::io::Error::other("expected Component derive-macro output"))?;
        assert_eq!(component_derive.path, "tuple_provider_probe::component::Component");
        assert_eq!(component_derive.associated_type_bindings.len(), 1);
        assert_eq!(component_derive.associated_type_bindings[0].name, "Mutability");
        assert_eq!(
            component_derive.associated_type_bindings[0].value_path,
            "tuple_provider_probe::Mutable"
        );
        let misleading_trait = extract_rust_item(&expanded_workspace, "tuple_provider_probe::Misleading")?;
        let RustItemKind::Trait(misleading_trait_info) = misleading_trait.kind else {
            return Err(std::io::Error::other("expected same-spelling Misleading trait metadata").into());
        };
        assert_eq!(
            misleading_trait_info
                .derive_macro
                .as_ref()
                .and_then(|macro_info| macro_info.expanded_traits.first())
                .map(|implementation| implementation.path.as_str()),
            Some("tuple_provider_probe::Other"),
            "the co-located macro namespace must retain its actual output when a same-spelling trait resolves first"
        );
        let other = extract_rust_item(&expanded_workspace, "tuple_provider_probe::Other")?;
        let RustItemKind::Trait(other_info) = other.kind else {
            return Err(std::io::Error::other("expected Other trait metadata").into());
        };
        assert!(
            other_info.derive_macro.is_none(),
            "an implemented trait must not acquire macro identity from another spelling"
        );
        let component_macro = extract_rust_item(&expanded_workspace, "tuple_driver::Component")?;
        let RustItemKind::Macro(component_macro_info) = component_macro.kind else {
            return Err(std::io::Error::other("expected Component derive macro metadata").into());
        };
        assert_eq!(component_macro_info.expanded_traits.len(), 1);
        assert_eq!(
            component_macro_info.expanded_traits[0].path, "tuple_provider_probe::component::Component",
            "a macro-only path must carry the actual implemented trait identity"
        );
        assert_eq!(
            component_macro_info.expanded_traits[0].associated_type_bindings[0].value_path,
            "tuple_provider_probe::Mutable"
        );
        let misleading_macro = extract_rust_item(&expanded_workspace, "tuple_driver::Misleading")?;
        let RustItemKind::Macro(misleading_macro_info) = misleading_macro.kind else {
            return Err(std::io::Error::other("expected Misleading derive macro metadata").into());
        };
        assert_eq!(
            misleading_macro_info
                .expanded_traits
                .iter()
                .map(|implementation| implementation.path.as_str())
                .collect::<Vec<_>>(),
            vec!["tuple_provider_probe::Other"],
            "macro metadata must preserve expansion output rather than infer a same-name trait"
        );
        let expanded = extract_rust_item(&expanded_workspace, "tuple_provider_probe::FooBar")?;
        let RustItemKind::Type(expanded_info) = &expanded.kind else {
            return Err(std::io::Error::other("expected expanded FooBar type metadata").into());
        };
        let candidate = &expanded_info.mutable_reference_type_params[0].mutable_reference_candidates[0];
        assert!(candidate.fallback_is_complete);
        assert_eq!(candidate.required_associated_type_bindings.len(), 1);
        assert_eq!(
            candidate.required_associated_type_bindings[0].trait_path,
            "tuple_provider_probe::component::Component"
        );
        assert_eq!(candidate.required_associated_type_bindings[0].name, "Mutability");
        assert_eq!(
            candidate.required_associated_type_bindings[0].value_path,
            "tuple_provider_probe::Mutable"
        );
        assert_eq!(
            expanded_info.mutable_reference_type_params[0].tuple_composition_arities,
            [2, 3],
            "Cargo-authorized proc-macro expansion must expose the generated tuple impls through HIR"
        );
        let defaulted = extract_rust_item(&expanded_workspace, "tuple_provider_probe::Defaulted")?;
        let RustItemKind::Type(defaulted_info) = &defaulted.kind else {
            return Err(std::io::Error::other("expected Defaulted type metadata").into());
        };
        assert_eq!(
            defaulted_info.type_param_defaults,
            [Some("tuple_provider_probe::Mutable".to_string())],
            "HIR metadata must retain canonical declared default type arguments"
        );

        let assert_contract = |metadata: &incan_core::interop::RustItemMetadata| -> Result<(), std::io::Error> {
            let RustItemKind::Type(info) = &metadata.kind else {
                return Err(std::io::Error::other("expected FooBar type metadata"));
            };
            assert_eq!(info.mutable_reference_type_params.len(), 1);
            assert_eq!(info.mutable_reference_type_params[0].tuple_composition_arities, [2, 3]);
            Ok(())
        };
        let assert_widget_contract = |metadata: &incan_core::interop::RustItemMetadata| -> Result<(), std::io::Error> {
            let RustItemKind::Type(info) = &metadata.kind else {
                return Err(std::io::Error::other("expected Widget type metadata"));
            };
            assert_eq!(info.expanded_derive_traits.len(), 1);
            assert_eq!(
                info.expanded_derive_traits[0].path,
                "tuple_provider_probe::component::Component"
            );
            assert_eq!(info.expanded_derive_traits[0].associated_type_bindings.len(), 1);
            assert_eq!(
                info.expanded_derive_traits[0].associated_type_bindings[0].value_path,
                "tuple_provider_probe::Mutable"
            );
            Ok(())
        };
        let assert_defaulted_contract =
            |metadata: &incan_core::interop::RustItemMetadata| -> Result<(), std::io::Error> {
                let RustItemKind::Type(info) = &metadata.kind else {
                    return Err(std::io::Error::other("expected Defaulted type metadata"));
                };
                assert_eq!(
                    info.type_param_defaults,
                    [Some("tuple_provider_probe::Mutable".to_string())]
                );
                Ok(())
            };

        let cache = RustMetadataCache::new();
        let metadata = cache.get_or_extract(tmp.path(), "tuple_provider_probe::FooBar", &|_| ())?;
        assert_contract(metadata.as_ref())?;
        let widget = cache.get_or_extract(tmp.path(), "tuple_provider_probe::Widget", &|_| ())?;
        assert_widget_contract(widget.as_ref())?;
        let defaulted = cache.get_or_extract(tmp.path(), "tuple_provider_probe::Defaulted", &|_| ())?;
        assert_defaulted_contract(defaulted.as_ref())?;
        let component = cache.get_or_extract(tmp.path(), "tuple_provider_probe::Component", &|_| ())?;
        let RustItemKind::Trait(component_info) = &component.kind else {
            return Err(std::io::Error::other("expected cached Component trait metadata").into());
        };
        assert_eq!(
            component_info
                .derive_macro
                .as_ref()
                .and_then(|macro_info| macro_info.expanded_traits.first())
                .map(|implementation| implementation.path.as_str()),
            Some("tuple_provider_probe::component::Component")
        );
        let misleading_trait = cache.get_or_extract(tmp.path(), "tuple_provider_probe::Misleading", &|_| ())?;
        let RustItemKind::Trait(misleading_trait_info) = &misleading_trait.kind else {
            return Err(std::io::Error::other("expected cached same-spelling Misleading trait metadata").into());
        };
        assert_eq!(
            misleading_trait_info
                .derive_macro
                .as_ref()
                .and_then(|macro_info| macro_info.expanded_traits.first())
                .map(|implementation| implementation.path.as_str()),
            Some("tuple_provider_probe::Other")
        );
        let component_macro = cache.get_or_extract(tmp.path(), "tuple_driver::Component", &|_| ())?;
        let RustItemKind::Macro(component_macro_info) = &component_macro.kind else {
            return Err(std::io::Error::other("expected cached Component macro metadata").into());
        };
        assert_eq!(
            component_macro_info.expanded_traits[0].path,
            "tuple_provider_probe::component::Component"
        );
        let cache_payload = fs::read_to_string(tmp.path().join(".incan_rust_inspect_cache.json"))?;
        let persisted: serde_json::Value = serde_json::from_str(cache_payload.as_str())?;
        assert_eq!(
            persisted["items"]["tuple_provider_probe::FooBar"]["kind"]["Type"]["mutable_reference_type_params"][0]["tuple_composition_arities"],
            serde_json::json!([2, 3]),
            "persisted cache must retain the expanded dependency contract"
        );
        assert_eq!(
            persisted["items"]["tuple_provider_probe::Widget"]["kind"]["Type"]["expanded_derive_traits"][0]["associated_type_bindings"]
                [0]["value_path"],
            "tuple_provider_probe::Mutable",
            "persisted cache must retain expanded derive associated-type evidence"
        );
        assert_eq!(
            persisted["items"]["tuple_provider_probe::Defaulted"]["kind"]["Type"]["type_param_defaults"][0],
            "tuple_provider_probe::Mutable",
            "persisted cache must retain canonical default type arguments"
        );
        assert_eq!(
            persisted["items"]["tuple_provider_probe::Component"]["kind"]["Trait"]["derive_macro"]["expanded_traits"]
                [0]["associated_type_bindings"][0]["value_path"],
            "tuple_provider_probe::Mutable",
            "persisted cache must retain the local derive probe contract"
        );
        assert_eq!(
            persisted["items"]["tuple_provider_probe::Misleading"]["kind"]["Trait"]["derive_macro"]["expanded_traits"]
                [0]["path"],
            "tuple_provider_probe::Other",
            "persisted cache must retain a same-spelling trait and derive macro as separate namespace facts"
        );
        assert_eq!(
            persisted["items"]["tuple_driver::Component"]["kind"]["Macro"]["expanded_traits"][0]["associated_type_bindings"]
                [0]["value_path"],
            "tuple_provider_probe::Mutable",
            "persisted cache must retain macro-only derive output"
        );

        drop(expanded_workspace);
        let fresh_cache = RustMetadataCache::new();
        let reloaded = fresh_cache.get_or_extract_complete(tmp.path(), "tuple_provider_probe::FooBar", &|_| ())?;
        assert_contract(reloaded.as_ref())?;
        let reloaded_widget =
            fresh_cache.get_or_extract_complete(tmp.path(), "tuple_provider_probe::Widget", &|_| ())?;
        assert_widget_contract(reloaded_widget.as_ref())?;
        let reloaded_defaulted =
            fresh_cache.get_or_extract_complete(tmp.path(), "tuple_provider_probe::Defaulted", &|_| ())?;
        assert_defaulted_contract(reloaded_defaulted.as_ref())?;
        let reloaded_component =
            fresh_cache.get_or_extract_complete(tmp.path(), "tuple_provider_probe::Component", &|_| ())?;
        let RustItemKind::Trait(reloaded_component_info) = &reloaded_component.kind else {
            return Err(std::io::Error::other("expected reloaded Component trait metadata").into());
        };
        assert_eq!(
            reloaded_component_info
                .derive_macro
                .as_ref()
                .and_then(|macro_info| macro_info.expanded_traits.first())
                .map(|implementation| implementation.path.as_str()),
            Some("tuple_provider_probe::component::Component")
        );
        let reloaded_misleading =
            fresh_cache.get_or_extract_complete(tmp.path(), "tuple_provider_probe::Misleading", &|_| ())?;
        let RustItemKind::Trait(reloaded_misleading_info) = &reloaded_misleading.kind else {
            return Err(std::io::Error::other("expected reloaded same-spelling Misleading trait metadata").into());
        };
        assert_eq!(
            reloaded_misleading_info
                .derive_macro
                .as_ref()
                .and_then(|macro_info| macro_info.expanded_traits.first())
                .map(|implementation| implementation.path.as_str()),
            Some("tuple_provider_probe::Other")
        );
        let reloaded_macro = fresh_cache.get_or_extract_complete(tmp.path(), "tuple_driver::Component", &|_| ())?;
        let RustItemKind::Macro(reloaded_macro_info) = &reloaded_macro.kind else {
            return Err(std::io::Error::other("expected reloaded Component macro metadata").into());
        };
        assert_eq!(
            reloaded_macro_info.expanded_traits[0].path,
            "tuple_provider_probe::component::Component"
        );
        Ok(())
    }

    #[test]
    fn type_metadata_records_direct_trait_impls() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_trait_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub trait Labelled {}

pub struct Thing;

impl Labelled for Thing {}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_trait_probe::Thing")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        assert!(
            info.implemented_traits
                .iter()
                .any(|implemented| implemented.path == "demo_trait_probe::Labelled"),
            "expected direct Labelled impl in metadata, got {:?}",
            info.implemented_traits
        );
        Ok(())
    }

    /// Proves ambient downstream impl crates cannot change ADT or alias metadata for an upstream Rust surface.
    #[test]
    fn type_metadata_ignores_traits_from_loaded_downstream_crates() -> Result<(), Box<dyn std::error::Error>> {
        // ---- Fixture: upstream API plus an unrelated loaded downstream impl ----
        let tmp = tempfile::tempdir()?;
        let trait_api = tmp.path().join("trait-api");
        let surface_api = tmp.path().join("surface-api");
        let downstream_api = tmp.path().join("downstream-api");
        let clean_probe = tmp.path().join("clean-probe");
        let polluted_probe = tmp.path().join("polluted-probe");
        for root in [&trait_api, &surface_api, &downstream_api, &clean_probe, &polluted_probe] {
            fs::create_dir_all(root.join("src"))?;
        }

        fs::write(
            trait_api.join("Cargo.toml"),
            "[package]\nname = \"trait_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(trait_api.join("src/lib.rs"), "pub trait Intrinsic {}\n")?;
        fs::write(
            surface_api.join("Cargo.toml"),
            "[package]\nname = \"surface_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntrait_api = { path = \"../trait-api\" }\n",
        )?;
        fs::write(
            surface_api.join("src/lib.rs"),
            "pub struct Thing;\npub type ThingAlias = Thing;\n\nimpl trait_api::Intrinsic for Thing {}\n",
        )?;
        fs::write(
            downstream_api.join("Cargo.toml"),
            "[package]\nname = \"downstream_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsurface_api = { path = \"../surface-api\" }\n",
        )?;
        fs::write(
            downstream_api.join("src/lib.rs"),
            "pub trait Ambient {}\n\nimpl Ambient for surface_api::Thing {}\n",
        )?;
        fs::write(
            clean_probe.join("Cargo.toml"),
            "[package]\nname = \"clean_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsurface_api = { path = \"../surface-api\" }\n",
        )?;
        fs::write(clean_probe.join("src/lib.rs"), "pub fn load_surface() {}\n")?;
        fs::write(
            polluted_probe.join("Cargo.toml"),
            "[package]\nname = \"polluted_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsurface_api = { path = \"../surface-api\" }\ndownstream_api = { path = \"../downstream-api\" }\n",
        )?;
        fs::write(polluted_probe.join("src/lib.rs"), "pub fn load_graph() {}\n")?;

        // ---- Extraction: compare the same surface under clean and polluted graphs ----
        let clean_workspace = RustWorkspace::load(&clean_probe, &|_| ())?;
        let clean = extract_rust_item(&clean_workspace, "surface_api::Thing")?;
        let polluted_workspace = RustWorkspace::load(&polluted_probe, &|_| ())?;
        let polluted = extract_rust_item(&polluted_workspace, "surface_api::Thing")?;
        let RustItemKind::Type(info) = &polluted.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };

        // ---- Contract: retain intrinsic traits and reject ambient downstream traits ----
        assert!(
            info.implemented_traits
                .iter()
                .any(|implemented| implemented.path == "trait_api::Intrinsic"),
            "expected intrinsic dependency trait in metadata, got {:?}",
            info.implemented_traits
        );
        assert!(
            info.implemented_traits
                .iter()
                .all(|implemented| implemented.path != "downstream_api::Ambient"),
            "downstream-only trait leaked into metadata: {:?}",
            info.implemented_traits
        );
        assert_eq!(
            clean, polluted,
            "loaded downstream crates must not change surface metadata"
        );

        let clean_alias = extract_rust_item(&clean_workspace, "surface_api::ThingAlias")?;
        let polluted_alias = extract_rust_item(&polluted_workspace, "surface_api::ThingAlias")?;
        assert_eq!(
            clean_alias, polluted_alias,
            "loaded downstream crates must not change type-alias metadata"
        );
        Ok(())
    }

    #[test]
    fn type_metadata_preserves_struct_field_declaration_order() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_field_order_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct Pair {
    pub zeta: i64,
    pub alpha: i64,
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_field_order_probe::Pair")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        let fields = info.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>();
        assert_eq!(fields, ["zeta", "alpha"]);
        Ok(())
    }

    #[test]
    fn type_metadata_unescapes_raw_keyword_fields_without_rewriting_plain_names()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_raw_field_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct JoinRel {
    pub r#type: i64,
    pub type_: i64,
    pub r#match: i64,
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_raw_field_probe::JoinRel")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        let fields = info.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>();
        assert_eq!(fields, ["type", "type_", "match"]);
        Ok(())
    }

    #[test]
    fn type_metadata_preserves_tuple_struct_constructor_positions() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_tuple_struct_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct ClearColor(pub Color);
pub struct Color;
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_tuple_struct_probe::ClearColor")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected tuple-struct type metadata").into());
        };
        assert_eq!(info.fields.len(), 1);
        assert_eq!(info.fields[0].name, "");
        assert_eq!(info.fields[0].type_display, "demo_tuple_struct_probe::Color");
        Ok(())
    }

    #[test]
    fn type_metadata_preserves_canonical_source_field_identity_without_default_arguments()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_field_identity_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct Payload;

pub struct Envelope {
    pub names: Vec<String>,
    pub bytes: Vec<u8>,
    pub payload: Option<Payload>,
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_field_identity_probe::Envelope")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected field identity metadata").into());
        };
        assert_eq!(info.fields[0].type_display, "Vec<String>");
        assert_eq!(info.fields[1].type_display, "Vec<u8>");
        assert_eq!(info.fields[1].type_shape, RustTypeShape::Bytes);
        assert_eq!(
            info.fields[2].type_display,
            "Option<demo_field_identity_probe::Payload>"
        );
        assert!(
            info.fields.iter().all(|field| !field.type_display.contains("Global")),
            "source-declared fields must not gain implementation-only allocator arguments: {:?}",
            info.fields
        );
        Ok(())
    }

    #[test]
    fn source_metadata_resolves_module_aliases_to_definition_namespaces() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_module_alias_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"mod hidden {
    pub struct Payload;
}

use hidden as backend;
pub use hidden::Payload;

pub struct Envelope {
    pub payload: backend::Payload,
}

pub fn consume(payload: backend::Payload) {
    let _ = payload;
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let envelope = extract_rust_item(&workspace, "demo_module_alias_probe::Envelope")?;
        let RustItemKind::Type(info) = envelope.kind else {
            return Err(std::io::Error::other("expected field metadata").into());
        };
        assert_eq!(info.fields[0].type_display, "demo_module_alias_probe::hidden::Payload");

        let consume = extract_rust_item(&workspace, "demo_module_alias_probe::consume")?;
        let RustItemKind::Function(signature) = consume.kind else {
            return Err(std::io::Error::other("expected function metadata").into());
        };
        assert_eq!(
            signature.params[0].type_display,
            "demo_module_alias_probe::hidden::Payload"
        );
        Ok(())
    }

    #[test]
    fn constant_metadata_uses_the_declared_types_canonical_namespace() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"demo_constant_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            "pub struct Stamp;\npub const EPOCH: Stamp = Stamp;\n",
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_constant_probe::EPOCH")?;
        let RustItemKind::Constant { type_display } = metadata.kind else {
            return Err(std::io::Error::other("expected constant metadata").into());
        };
        assert_eq!(type_display, "demo_constant_probe::Stamp");
        Ok(())
    }

    #[test]
    fn direct_dependency_namespace_wins_over_a_duplicate_transitive_crate_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        for path in ["src", "selected/src", "transitive/src", "bridge/src"] {
            fs::create_dir_all(tmp.path().join(path))?;
        }
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "duplicate_root"
version = "0.1.0"
edition = "2021"

[dependencies]
shared = { path = "selected", version = "2" }
bridge = { path = "bridge" }
"#,
        )?;
        fs::write(tmp.path().join("src/lib.rs"), "pub fn root() {}\n")?;
        fs::write(
            tmp.path().join("selected/Cargo.toml"),
            "[package]\nname = \"shared\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            tmp.path().join("selected/src/lib.rs"),
            "pub struct Marker { pub selected: u32 }\n",
        )?;
        fs::write(
            tmp.path().join("transitive/Cargo.toml"),
            "[package]\nname = \"shared\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            tmp.path().join("transitive/src/lib.rs"),
            "pub struct Marker { pub transitive: u32 }\n",
        )?;
        fs::write(
            tmp.path().join("bridge/Cargo.toml"),
            "[package]\nname = \"bridge\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nshared = { path = \"../transitive\", version = \"1\" }\n",
        )?;
        fs::write(
            tmp.path().join("bridge/src/lib.rs"),
            "pub fn marker() -> shared::Marker { shared::Marker { transitive: 1 } }\n",
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "shared::Marker")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected direct dependency type metadata").into());
        };
        assert_eq!(
            info.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
            ["selected"],
            "the root-facing crate namespace must select the direct dependency version"
        );
        Ok(())
    }

    #[test]
    fn type_alias_metadata_canonicalizes_simple_trait_object_namespaces() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_alias_identity_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"use std::sync::Arc;

pub trait Array {}
pub type ArrayRef = Arc<dyn Array>;
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_alias_identity_probe::ArrayRef")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected alias identity metadata").into());
        };
        assert_eq!(
            info.alias_target.as_deref(),
            Some("std::sync::Arc<dyn demo_alias_identity_probe::Array>")
        );
        Ok(())
    }

    #[test]
    fn type_alias_metadata_preserves_source_target_shape() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_alias_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"use std::sync::Arc;

pub struct ColumnarValue;
pub struct CallbackError;

pub type SliceCallback =
    Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue, CallbackError> + Send + Sync>;
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_alias_probe::SliceCallback")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        assert_eq!(
            info.alias_target.as_deref(),
            Some("Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue, CallbackError> + Send + Sync>")
        );
        Ok(())
    }

    #[test]
    fn type_metadata_preserves_borrowed_slice_params_and_borrowed_option_returns()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_borrow_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct Codec;

pub static CODEC: Codec = Codec;

impl Codec {
    pub fn for_label(label: &[u8]) -> Option<&'static Codec> {
        let _ = label;
        Some(&CODEC)
    }

    pub fn decode<'a>(&'static self, bytes: &'a [u8]) -> (&'a [u8], &'static Codec, bool) {
        (bytes, self, false)
    }
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_borrow_probe::Codec")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        let for_label = info
            .methods
            .iter()
            .find(|method| method.name == "for_label")
            .ok_or_else(|| std::io::Error::other("expected for_label metadata"))?;
        assert_eq!(for_label.signature.params[0].type_display, "&[u8]");
        assert_eq!(for_label.signature.return_type, "Option<&demo_borrow_probe::Codec>");
        let decode = info
            .methods
            .iter()
            .find(|method| method.name == "decode")
            .ok_or_else(|| std::io::Error::other("expected decode metadata"))?;
        assert_eq!(decode.signature.params[1].type_display, "&[u8]");
        Ok(())
    }

    #[test]
    fn type_metadata_preserves_owner_type_parameters() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_owner_generic_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct Factory<'a, T, const N: usize, U> {
    left: &'a T,
    right: U,
}

impl<'a, T, const N: usize, U> Factory<'a, T, N, U> {
    pub fn new(left: &'a T, right: U) -> Self {
        Self { left, right }
    }
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_owner_generic_probe::Factory")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected type metadata").into());
        };
        assert_eq!(info.type_params, ["T", "U"]);
        assert!(info.has_const_params);
        assert_eq!(info.methods[0].signature.type_params, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn type_metadata_marks_generic_bounds_with_structural_mutable_reference_impls()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_structural_mut_ref_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub trait MutableData {}
pub trait ParameterizedData<T> {}
pub struct Mutable;
pub trait MutableComponent {
    type Mutability;
}

pub struct FooBar<T: MutableData>(pub T);
pub struct ValueBag<T>(pub T);
pub struct ParameterizedOwner<T: ParameterizedData<u8>>(pub T);

macro_rules! impl_tuple_mutable_data {
    ($($name:ident),*) => {
        impl<$($name: MutableData),*> MutableData for ($($name,)*) {}
    };
}
impl_tuple_mutable_data!(A, B);
impl<T: MutableComponent<Mutability = Mutable>> MutableData for &mut T {}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_structural_mut_ref_probe::FooBar")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected FooBar type metadata").into());
        };
        assert_eq!(info.type_params, ["T"]);
        assert_eq!(info.mutable_reference_type_params.len(), 1);
        let rule = &info.mutable_reference_type_params[0];
        assert_eq!(rule.type_param, "T");
        assert_eq!(rule.direct_trait_bounds, ["demo_structural_mut_ref_probe::MutableData"]);
        assert_eq!(rule.tuple_composition_arities, [2]);
        assert_eq!(rule.mutable_reference_candidates.len(), 1);
        assert_eq!(
            rule.mutable_reference_candidates[0].required_traits,
            ["demo_structural_mut_ref_probe::MutableComponent"]
        );
        let metadata = extract_rust_item(&workspace, "demo_structural_mut_ref_probe::ValueBag")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected ValueBag type metadata").into());
        };
        assert!(
            info.mutable_reference_type_params.is_empty(),
            "an unconstrained generic container must not acquire borrowed element lowering"
        );

        let metadata = extract_rust_item(&workspace, "demo_structural_mut_ref_probe::ParameterizedOwner")?;
        let RustItemKind::Type(info) = metadata.kind else {
            return Err(std::io::Error::other("expected ParameterizedOwner type metadata").into());
        };
        assert!(
            info.mutable_reference_type_params.is_empty(),
            "parameterized owner bounds must fail closed instead of being reduced to a trait path"
        );

        Ok(())
    }

    #[test]
    fn trait_solver_preserves_associated_type_constraints_for_mutable_reference_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_trait_solver_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub trait QueryData {}
pub trait Component { type Mutability; }
pub struct Mutable;
pub struct Immutable;
pub struct Dynamic;
pub struct Static;
pub struct Entity;

impl Component for Dynamic { type Mutability = Mutable; }
impl Component for Static { type Mutability = Immutable; }
impl<T: Component<Mutability = Mutable>> QueryData for &mut T {}
impl QueryData for Entity {}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        assert!(super::rust_type_implements_trait(
            &workspace,
            "demo_trait_solver_probe::Dynamic",
            "demo_trait_solver_probe::QueryData",
            true,
        )?);
        assert!(
            !super::rust_type_implements_trait(
                &workspace,
                "demo_trait_solver_probe::Static",
                "demo_trait_solver_probe::QueryData",
                true,
            )?,
            "the solver must retain Component::Mutability = Mutable rather than matching Component by name"
        );
        assert!(super::rust_type_implements_trait(
            &workspace,
            "demo_trait_solver_probe::Entity",
            "demo_trait_solver_probe::QueryData",
            false,
        )?);
        let dynamic = super::extract_rust_item(&workspace, "demo_trait_solver_probe::Dynamic")?;
        let RustItemKind::Type(dynamic_info) = dynamic.kind else {
            return Err("expected Dynamic type metadata".into());
        };
        assert!(dynamic_info.implemented_traits.iter().any(|implementation| {
            implementation.path == "demo_trait_solver_probe::QueryData" && implementation.mutable_reference
        }));
        let static_type = super::extract_rust_item(&workspace, "demo_trait_solver_probe::Static")?;
        let RustItemKind::Type(static_info) = static_type.kind else {
            return Err("expected Static type metadata".into());
        };
        assert!(!static_info.implemented_traits.iter().any(|implementation| {
            implementation.path == "demo_trait_solver_probe::QueryData" && implementation.mutable_reference
        }));
        Ok(())
    }

    #[test]
    fn function_metadata_applies_inline_source_callable_bound_display() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_callback_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct Data;
pub struct OutputCallbackInfo;

pub fn run_inline<D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static>(callback: D) {
    let _ = callback;
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_callback_probe::run_inline")?;
        let RustItemKind::Function(sig) = metadata.kind else {
            return Err(std::io::Error::other("expected function metadata").into());
        };
        assert_eq!(sig.type_params, ["D"]);
        assert_eq!(
            sig.params[0].type_display,
            "impl FnMut(&mut demo_callback_probe::Data, &demo_callback_probe::OutputCallbackInfo)"
        );
        Ok(())
    }

    #[test]
    fn function_metadata_preserves_generic_slice_in_source_callable_bound() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "demo_slice_callback_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub struct OutputCallbackInfo;

pub fn run<T, D: FnMut(&mut [T], &OutputCallbackInfo)>(callback: D) {
    let _ = callback;
}
"#,
        )?;

        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        let metadata = extract_rust_item(&workspace, "demo_slice_callback_probe::run")?;
        let RustItemKind::Function(sig) = metadata.kind else {
            return Err(std::io::Error::other("expected function metadata").into());
        };
        assert_eq!(sig.type_params, ["T", "D"]);
        assert_eq!(
            sig.params[0].type_display,
            "impl FnMut(&mut [T], &demo_slice_callback_probe::OutputCallbackInfo)"
        );
        Ok(())
    }
}
