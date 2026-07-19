//! Map rust-analyzer `hir` definitions into [`incan_core::interop::RustItemMetadata`].

use std::collections::BTreeMap;

use incan_core::interop::{
    RustFieldInfo, RustFunctionSig, RustImplementedTrait, RustItemKind, RustItemMetadata, RustMethodSig,
    RustModuleChild, RustModuleChildKind, RustModuleInfo, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo,
    RustTypeShape, RustTypeShapePathFallback, RustVariantInfo, RustVisibility, parse_rust_type_shape_text,
    render_rust_type_shape, rust_display_is_callable_bound, rust_source_borrowed_type_param_bound_display,
    rust_source_callable_bound_for_type_param, rust_source_type_param_has_as_fd_bound, strip_rust_borrow_lifetimes,
};
use ra_ap_hir::{
    Adt, AssocItem, Crate, DisplayTarget, Enum, FieldSource, Function, HasSource, HasVisibility, HirDisplay, Impl,
    ItemInNs, Module, ModuleDef, Name, ScopeDef, Trait, Type, Variant, VariantDef, Visibility, attach_db,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::{
    AstNode,
    ast::{self, HasGenericParams, HasModuleItem, HasName},
};

use super::error::RustMetadataError;
use super::loader::RustWorkspace;

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

fn resolve_relative_source_path(text: &str, crate_name: &str, module: Module, db: &RootDatabase) -> Option<String> {
    let mut text = text.trim().trim_start_matches("::");
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

/// Normalize source type text from Rust inspection display output.
fn normalize_source_type_text(text: &str) -> String {
    strip_rust_borrow_lifetimes(text).trim().replace(' ', "")
}

/// Return the source spelling for a borrowed builtin Rust type.
fn borrowed_builtin_source_display(text: &str) -> Option<String> {
    let normalized = normalize_source_type_text(text);
    let (prefix, inner) = if let Some(inner) = normalized.strip_prefix("&mut") {
        ("&mut", inner)
    } else {
        let inner = normalized.strip_prefix('&')?;
        ("&", inner)
    };
    match inner {
        "str"
        | "[u8]"
        | "String"
        | "std::string::String"
        | "alloc::string::String"
        | "Vec<u8>"
        | "std::vec::Vec<u8>"
        | "alloc::vec::Vec<u8>" => Some(format!("{prefix}{inner}")),
        _ if is_exact_numeric_display(inner) => Some(format!("{prefix}{inner}")),
        _ => None,
    }
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

fn resolve_source_path(text: &str, crate_name: &str, module: Module, db: &RootDatabase) -> Option<String> {
    let text = text.trim().replace(' ', "");
    if text.is_empty() {
        return None;
    }

    if text.starts_with("::")
        || text.starts_with("crate::")
        || text.starts_with("self::")
        || text.starts_with("super::")
    {
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

fn source_field_type_shape(field: &ra_ap_hir::Field, db: &RootDatabase, crate_name: &str) -> Option<RustTypeShape> {
    let source = field.source(db)?;
    let text = match source.value {
        FieldSource::Named(field) => field.ty()?.to_string(),
        FieldSource::Pos(field) => field.ty()?.to_string(),
    };
    let module = field_module(field, db);
    Some(source_type_shape(text.as_str(), crate_name, module, db))
}

/// Return the Rust source spelling for a named field, removing only Rust's raw-identifier prefix.
///
/// rust-analyzer may expose a raw field such as `r#type` through a safe internal name. Incan needs the source spelling
/// instead: `type` should be accepted in Incan and later emitted as `r#type`, while an ordinary Rust field named
/// `type_` must remain `type_`.
fn source_field_name(field: &ra_ap_hir::Field, db: &RootDatabase) -> Option<String> {
    let source = field.source(db)?;
    let FieldSource::Named(field) = source.value else {
        return None;
    };
    let raw = field.name()?.syntax().text().to_string();
    Some(raw.strip_prefix("r#").unwrap_or(raw.as_str()).to_string())
}

fn normalize_variant_payload_shape(shape: RustTypeShape) -> RustTypeShape {
    match shape {
        RustTypeShape::RustPath { path, args }
            if matches!(path.as_str(), "Box" | "std::boxed::Box" | "alloc::boxed::Box") =>
        {
            args.into_iter().next().unwrap_or(RustTypeShape::Unknown)
        }
        other => other,
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
    if let Some(display) = borrowed_builtin_source_display(text.as_str()) {
        return Some(display);
    }
    let module = f.module(db);
    let crate_name = module
        .krate(db)
        .display_name(db)
        .map(|name| name.canonical_name().as_str().to_owned())?;
    let shape = source_type_shape(text.as_str(), crate_name.as_str(), module, db);
    if let Some(display) = exact_numeric_boundary_display(text.as_str()) {
        return Some(display);
    }
    Some(match shape {
        RustTypeShape::Unknown => normalize_display_path(text.as_str()),
        other => render_rust_type_shape(&other),
    })
}

/// Render source type text in the same canonical display form used by extracted function metadata.
fn source_function_type_display(f: Function, text: &str, db: &RootDatabase) -> Option<String> {
    if let Some(display) = borrowed_builtin_source_display(text) {
        return Some(display);
    }
    if let Some((is_mut, inner)) = source_borrow_kind(text) {
        let inner = source_function_type_display(f, inner, db)?;
        return Some(if is_mut {
            format!("&mut {inner}")
        } else {
            format!("&{inner}")
        });
    }
    if let Some(imported_display) = canonicalize_imported_single_segment_type_display(text, f, db) {
        return Some(imported_display);
    }
    let module = f.module(db);
    let crate_name = module
        .krate(db)
        .display_name(db)
        .map(|name| name.canonical_name().as_str().to_owned())?;
    let shape = source_type_shape(text, crate_name.as_str(), module, db);
    if let Some(display) = exact_numeric_boundary_display(text) {
        return Some(display);
    }
    if matches!(shape, RustTypeShape::TypeParam(_))
        && let Some(imported_display) = canonicalize_imported_single_segment_type_display(text, f, db)
    {
        return Some(imported_display);
    }
    let rendered = match shape {
        RustTypeShape::Unknown => normalize_display_path(text),
        other => render_rust_type_shape(&other),
    };
    if rendered.contains('?')
        && let Some(imported_display) = canonicalize_imported_single_segment_type_display(text, f, db)
    {
        return Some(imported_display);
    }
    Some(rendered)
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

fn join_use_path(prefix: Option<&str>, path: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}::{path}"),
        _ => path.to_string(),
    }
}

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

    if let Some(rename) = tree.rename()
        && let Some(name) = rename.name()
        && name.to_string() == target_name
    {
        return qualified;
    }

    if let Some(qualified) = qualified {
        let imported_name = qualified.rsplit("::").next().unwrap_or(qualified.as_str());
        if imported_name == target_name {
            return Some(qualified);
        }
    }

    None
}

fn imported_type_path_in_function_scope(f: Function, target_name: &str, db: &RootDatabase) -> Option<String> {
    let source = f.source(db)?;
    let syntax = source.value.syntax().clone();

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

/// Return whether any part of a structured Rust type still lacks reliable metadata.
fn type_shape_contains_unknown(shape: &RustTypeShape) -> bool {
    match shape {
        RustTypeShape::Option(inner) | RustTypeShape::Ref(inner) => type_shape_contains_unknown(inner),
        RustTypeShape::Result(ok, err) => type_shape_contains_unknown(ok) || type_shape_contains_unknown(err),
        RustTypeShape::Tuple(items) => items.iter().any(type_shape_contains_unknown),
        RustTypeShape::RustPath { args, .. } => args.iter().any(type_shape_contains_unknown),
        RustTypeShape::Unknown => true,
        RustTypeShape::Never
        | RustTypeShape::Bool
        | RustTypeShape::Float
        | RustTypeShape::Int
        | RustTypeShape::Str
        | RustTypeShape::Bytes
        | RustTypeShape::Unit
        | RustTypeShape::TypeParam(_) => false,
    }
}

/// Resolve a function parameter's declared source annotation into a canonical display string.
///
/// rust-analyzer sometimes degrades borrowed parameter displays to `&?` even when the written source still carries a
/// concrete imported or local pointee type. When that happens, the source annotation is the more faithful contract and
/// should drive metadata so downstream typechecking/codegen can keep the concrete borrow boundary.
fn source_function_param_type_text(f: Function, param: &ra_ap_hir::Param<'_>, db: &RootDatabase) -> Option<String> {
    let source = f.source(db)?;
    let param_list = source.value.param_list()?;
    let self_offset = usize::from(param_list.self_param().is_some());
    if param.index() < self_offset {
        return None;
    }
    let source_param = param_list.params().nth(param.index() - self_offset)?;
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

/// Return source-declared type parameters in their Rust turbofish order.
fn source_function_type_params(f: Function, db: &RootDatabase) -> Vec<String> {
    f.source(db)
        .into_iter()
        .flat_map(|source| source.value.generic_param_list())
        .flat_map(|params| params.generic_params())
        .filter_map(|param| match param {
            ast::GenericParam::TypeParam(param) => param.name().map(|name| name.text().to_string()),
            ast::GenericParam::ConstParam(_) | ast::GenericParam::LifetimeParam(_) => None,
        })
        .collect()
}

/// Extract a Rust function signature from inspection metadata.
fn extract_function_sig(f: Function, db: &RootDatabase, dt: DisplayTarget) -> RustFunctionSig {
    let params = f
        .assoc_fn_params(db)
        .into_iter()
        .map(|p| {
            let shape = rust_type_shape(p.ty(), db, dt);
            let mut type_display = function_sig_type_display(p.ty(), db, dt);
            if let Some(source_type_display) = source_function_param_type_display(f, &p, db) {
                let source_is_callable_bound = rust_display_is_callable_bound(source_type_display.as_str());
                if source_is_callable_bound
                    || type_shape_contains_unknown(&shape)
                    || p.ty().contains_unknown()
                    || type_display.contains('?')
                    || (source_type_display.starts_with('&') && !type_display.starts_with('&'))
                {
                    type_display = source_type_display;
                }
            }
            RustParam {
                name: p.name(db).map(|n| n.as_str().to_owned()),
                type_display,
            }
        })
        .collect();
    let output_type = f.async_ret_type(db).unwrap_or_else(|| f.ret_type(db));
    let output_shape = rust_type_shape(&output_type, db, dt);
    let mut return_type = function_sig_type_display(&output_type, db, dt);
    if (return_type.starts_with("impl ")
        || type_shape_contains_unknown(&output_shape)
        || output_type.contains_unknown()
        || return_type.contains('?'))
        && let Some(source_return_type) = source_function_return_type_display(f, db)
    {
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

/// Collect non-blanket trait impls that rust-analyzer can associate directly with `ty`.
fn collect_implemented_traits(ty: Type<'_>, db: &RootDatabase) -> Vec<RustImplementedTrait> {
    let mut traits = BTreeMap::new();
    for impl_def in Impl::all_for_type(db, ty) {
        let Some(trait_def) = impl_def.trait_(db) else {
            continue;
        };
        let path = canonical_module_def_path(ModuleDef::Trait(trait_def), db)
            .unwrap_or_else(|| trait_def.name(db).as_str().to_owned());
        traits.insert(path.clone(), RustImplementedTrait { path });
    }
    traits.into_values().collect()
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
            let mut type_shape = rust_type_shape(&field_ty, db, dt);
            if matches!(type_shape, RustTypeShape::Unknown) || field_ty.contains_unknown() {
                type_shape = source_field_type_shape(&field, db, crate_name).unwrap_or(type_shape);
            }
            collected.push(RustFieldInfo {
                name: source_field_name(&field, db).unwrap_or_else(|| field.name(db).as_str().to_owned()),
                type_display: format_ty(&field_ty, db, dt),
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
        let mut type_shape = rust_type_shape(&field_ty, db, dt);
        if matches!(type_shape, RustTypeShape::Unknown) || field_ty.contains_unknown() {
            type_shape = source_field_type_shape(&field, db, crate_name).unwrap_or(type_shape);
        }
        fields.push(RustFieldInfo {
            name: source_field_name(&field, db).unwrap_or_else(|| field.name(db).as_str().to_owned()),
            type_display: format_ty(&field_ty, db, dt),
            type_shape,
        });
    }
    fields
}

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
        variants.push(RustVariantInfo {
            name: variant.name(db).as_str().to_owned(),
            fields: collect_variant_payload_shapes(variant, &type_args, db, dt, crate_name),
        });
    }
    variants.sort_by(|a, b| a.name.cmp(&b.name));
    variants
}

fn collect_variant_payload_shapes(
    variant: Variant,
    type_args: &[Type<'_>],
    db: &RootDatabase,
    dt: DisplayTarget,
    crate_name: &str,
) -> Vec<RustTypeShape> {
    variant
        .fields(db)
        .iter()
        .filter(|field| is_exported_rust_api(field.visibility(db)))
        .map(|field| {
            let field_ty = field.ty_with_args(db, type_args.iter().cloned());
            let mut shape = rust_type_shape(&field_ty, db, dt);
            if matches!(shape, RustTypeShape::Unknown) || field_ty.contains_unknown() {
                shape = source_field_type_shape(field, db, crate_name).unwrap_or(shape);
            }
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

fn trait_info(tr: Trait, db: &RootDatabase, dt: DisplayTarget) -> RustTraitInfo {
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
    RustTraitInfo { items }
}

/// Find a crate by any spelling that can legally name it across Cargo and Rust surfaces.
///
/// rust-inspect queries use canonical Rust paths, so the first segment may be the Rust crate name even when Cargo
/// registered the package with hyphens or via a differently-cased display name.
fn find_crate(workspace: &RustWorkspace, crate_name: &str) -> Option<Crate> {
    workspace.crate_by_name(crate_name)
}

fn resolve_module_def(db: &RootDatabase, krate: Crate, segments: &[Name]) -> Result<ModuleDef, RustMetadataError> {
    let root = krate.root_module(db);
    if let Some(mut it) = root.resolve_mod_path(db, segments.iter().cloned())
        && let Some(first) = it.next()
    {
        return match first {
            ItemInNs::Macros(_) => Err(RustMetadataError::UnsupportedMacro(segments_display(segments))),
            other => Ok(other.into_module_def()),
        };
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
                ScopeDef::ModuleDef(def) => match def {
                    ModuleDef::Macro(_) => Err(RustMetadataError::UnsupportedMacro(segments_display(segments))),
                    other => Ok(other),
                },
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
    let def = resolve_module_def(db, krate, &segments)?;
    let vis = map_visibility(def.visibility(db));
    let kind = match def {
        ModuleDef::Module(m) => RustItemKind::Module(module_children(m, db)),
        ModuleDef::Function(f) => RustItemKind::Function(extract_function_sig(f, db, dt)),
        ModuleDef::Adt(adt) => {
            let ty = adt.ty(db);
            RustItemKind::Type(RustTypeInfo {
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: collect_inherent_methods(ty.clone(), db, dt),
                implemented_traits: collect_implemented_traits(ty.clone(), db),
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
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: collect_inherent_methods(ty.clone(), db, dt),
                implemented_traits: collect_implemented_traits(ty.clone(), db),
                fields: collect_public_fields(ty, db, dt, crate_name),
                variants: Vec::new(),
            })
        }
        ModuleDef::Const(c) => RustItemKind::Constant {
            type_display: format_ty(&c.ty(db), db, dt),
        },
        ModuleDef::Static(s) => RustItemKind::Constant {
            type_display: format_ty(&s.ty(db), db, dt),
        },
        ModuleDef::Trait(t) => RustItemKind::Trait(trait_info(t, db, dt)),
        ModuleDef::TypeAlias(a) => {
            let ty = a.ty(db);
            RustItemKind::Type(RustTypeInfo {
                alias_target: source_type_alias_target_display(a, db).or_else(|| Some(format_ty(&ty, db, dt))),
                metadata_completeness: Default::default(),
                methods: collect_inherent_methods(ty.clone(), db, dt),
                implemented_traits: collect_implemented_traits(ty.clone(), db),
                fields: collect_public_fields(ty, db, dt, crate_name),
                variants: Vec::new(),
            })
        }
        ModuleDef::Variant(_) => RustItemKind::Unsupported {
            description: "enum variant".to_owned(),
        },
        ModuleDef::Macro(_) => RustItemKind::Unsupported {
            description: "macro".to_owned(),
        },
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

    use incan_core::interop::RustItemKind;

    use super::{RustWorkspace, exact_numeric_boundary_display, extract_rust_item};

    #[test]
    fn exact_numeric_boundary_display_preserves_widths() {
        assert_eq!(exact_numeric_boundary_display("u32").as_deref(), Some("u32"));
        assert_eq!(exact_numeric_boundary_display("& i32").as_deref(), Some("&i32"));
        assert_eq!(exact_numeric_boundary_display("String"), None);
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
}
