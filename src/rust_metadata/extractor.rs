//! Map rust-analyzer `hir` definitions into [`incan_core::interop::RustItemMetadata`].

use std::collections::BTreeMap;

use incan_core::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustModuleChild,
    RustModuleChildKind, RustModuleInfo, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo, RustTypeShape,
    RustVariantInfo, RustVisibility,
};
use ra_ap_hir::{
    Adt, AssocItem, Crate, DisplayTarget, Enum, FieldSource, Function, HasSource, HasVisibility, HirDisplay, ItemInNs,
    Module, ModuleDef, Name, ScopeDef, Trait, Type, Variant, VariantDef, Visibility, attach_db,
};
use ra_ap_ide_db::RootDatabase;

use super::error::RustMetadataError;

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

fn render_shape_display(shape: &RustTypeShape) -> String {
    match shape {
        RustTypeShape::Bool => "bool".to_string(),
        RustTypeShape::Float => "f64".to_string(),
        RustTypeShape::Int => "i64".to_string(),
        RustTypeShape::Str => "String".to_string(),
        RustTypeShape::Bytes => "Vec<u8>".to_string(),
        RustTypeShape::Unit => "()".to_string(),
        RustTypeShape::Option(inner) => format!("Option<{}>", render_shape_display(inner)),
        RustTypeShape::Result(ok, err) => {
            format!("Result<{}, {}>", render_shape_display(ok), render_shape_display(err))
        }
        RustTypeShape::Tuple(items) => {
            let rendered: Vec<String> = items.iter().map(render_shape_display).collect();
            format!("({})", rendered.join(", "))
        }
        RustTypeShape::Ref(inner) => format!("&{}", render_shape_display(inner)),
        RustTypeShape::RustPath { path, args } => {
            if args.is_empty() {
                path.clone()
            } else {
                let rendered_args: Vec<String> = args.iter().map(render_shape_display).collect();
                format!("{path}<{}>", rendered_args.join(", "))
            }
        }
        RustTypeShape::TypeParam(name) => name.clone(),
        RustTypeShape::Unknown => "?".to_string(),
    }
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

    if text.contains("::") {
        return Some(text);
    }

    None
}

fn split_top_level_args(text: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut start = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            ',' if angle == 0 && paren == 0 && bracket == 0 => {
                args.push(text[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        args.push(tail);
    }
    args
}

fn source_type_shape(text: &str, crate_name: &str, module: Module, db: &RootDatabase) -> RustTypeShape {
    let text = text.trim().replace(' ', "");
    if text.is_empty() {
        return RustTypeShape::Unknown;
    }
    match text.as_str() {
        "bool" => return RustTypeShape::Bool,
        "f32" | "f64" => return RustTypeShape::Float,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => {
            return RustTypeShape::Int;
        }
        "str" | "String" | "std::string::String" | "alloc::string::String" => return RustTypeShape::Str,
        "()" => return RustTypeShape::Unit,
        _ => {}
    }

    if let Some(inner) = text.strip_prefix('&') {
        let inner = inner.strip_prefix("mut").unwrap_or(inner).trim();
        return RustTypeShape::Ref(Box::new(source_type_shape(inner, crate_name, module, db)));
    }

    if text == "[u8]" || text == "&[u8]" {
        return RustTypeShape::Bytes;
    }

    if text.starts_with('(') && text.ends_with(')') {
        let inner = &text[1..text.len() - 1];
        if inner.is_empty() {
            return RustTypeShape::Unit;
        }
        return RustTypeShape::Tuple(
            split_top_level_args(inner)
                .into_iter()
                .map(|arg| source_type_shape(arg, crate_name, module, db))
                .collect(),
        );
    }

    if let Some(start) = text.find('<')
        && text.ends_with('>')
    {
        let base =
            resolve_source_path(&text[..start], crate_name, module, db).unwrap_or_else(|| text[..start].to_string());
        let inner = &text[start + 1..text.len() - 1];
        let args: Vec<RustTypeShape> = split_top_level_args(inner)
            .into_iter()
            .map(|arg| source_type_shape(arg, crate_name, module, db))
            .collect();
        match base.as_str() {
            "Option" | "std::option::Option" | "core::option::Option" => {
                return RustTypeShape::Option(Box::new(args.into_iter().next().unwrap_or(RustTypeShape::Unknown)));
            }
            "Result" | "std::result::Result" | "core::result::Result" => {
                let mut it = args.into_iter();
                return RustTypeShape::Result(
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                );
            }
            "Vec" | "std::vec::Vec" | "alloc::vec::Vec"
                if matches!(args.first(), Some(RustTypeShape::Int)) && text.ends_with("<u8>") =>
            {
                return RustTypeShape::Bytes;
            }
            _ => {}
        }
        return RustTypeShape::RustPath { path: base, args };
    }

    if let Some(path) = resolve_source_path(text.as_str(), crate_name, module, db) {
        return RustTypeShape::RustPath { path, args: Vec::new() };
    }

    if display_looks_like_type_param(text.as_str()) {
        return RustTypeShape::TypeParam(text);
    }

    RustTypeShape::Unknown
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

fn rust_type_shape(ty: &Type<'_>, db: &RootDatabase, dt: DisplayTarget) -> RustTypeShape {
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

fn function_sig_type_display(ty: &Type<'_>, db: &RootDatabase, dt: DisplayTarget) -> String {
    match rust_type_shape(ty, db, dt) {
        RustTypeShape::Unknown => normalize_display_path(format_ty(ty, db, dt).as_str()),
        other => render_shape_display(&other),
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
    let module = f.module(db);
    let crate_name = module
        .krate(db)
        .display_name(db)
        .map(|name| name.canonical_name().as_str().to_owned())?;
    let shape = source_type_shape(text.as_str(), crate_name.as_str(), module, db);
    Some(match shape {
        RustTypeShape::Unknown => normalize_display_path(text.as_str()),
        other => render_shape_display(&other),
    })
}

fn extract_function_sig(f: Function, db: &RootDatabase, dt: DisplayTarget) -> RustFunctionSig {
    let params = f
        .assoc_fn_params(db)
        .into_iter()
        .map(|p| RustParam {
            name: p.name(db).map(|n| n.as_str().to_owned()),
            type_display: function_sig_type_display(p.ty(), db, dt),
        })
        .collect();
    let output_type = f.async_ret_type(db).unwrap_or_else(|| f.ret_type(db));
    let mut return_type = function_sig_type_display(&output_type, db, dt);
    if return_type.starts_with("impl ")
        && let Some(source_return_type) = source_function_return_type_display(f, db)
    {
        return_type = source_return_type;
    }
    RustFunctionSig {
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
                name: field.name(db).as_str().to_owned(),
                type_display: format_ty(&field_ty, db, dt),
                type_shape,
            });
        }
        collected.sort_by(|a, b| a.name.cmp(&b.name));
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
            name: field.name(db).as_str().to_owned(),
            type_display: format_ty(&field_ty, db, dt),
            type_shape,
        });
    }
    fields.sort_by(|a, b| a.name.cmp(&b.name));
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
/// rust-metadata queries use canonical Rust paths, so the first segment may be the Rust crate name even when Cargo
/// registered the package with hyphens or via a differently-cased display name.
fn find_crate(db: &RootDatabase, crate_name: &str) -> Option<Crate> {
    let normalized = crate_name.replace('-', "_");
    Crate::all(db).into_iter().find(|k| {
        k.display_name(db).is_some_and(|dn| {
            dn.to_string().replace('-', "_") == normalized
                || dn.crate_name().as_str().replace('-', "_") == normalized
                || dn.canonical_name().as_str().replace('-', "_") == normalized
        }) || k
            .root_module(db)
            .name(db)
            .is_some_and(|name| name.as_str().replace('-', "_") == normalized)
    })
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
pub fn extract_rust_item(db: &RootDatabase, canonical_path: &str) -> Result<RustItemMetadata, RustMetadataError> {
    attach_db(db, || extract_rust_item_inner(db, canonical_path))
}

fn extract_rust_item_inner(db: &RootDatabase, canonical_path: &str) -> Result<RustItemMetadata, RustMetadataError> {
    let (crate_name, segments) = split_canonical_path(canonical_path)?;
    let krate = find_crate(db, crate_name).ok_or_else(|| RustMetadataError::CrateNotFound(crate_name.to_owned()))?;
    let dt = DisplayTarget::from_crate(db, krate.base());
    let def = resolve_module_def(db, krate, &segments)?;
    let vis = map_visibility(def.visibility(db));
    let kind = match def {
        ModuleDef::Module(m) => RustItemKind::Module(module_children(m, db)),
        ModuleDef::Function(f) => RustItemKind::Function(extract_function_sig(f, db, dt)),
        ModuleDef::Adt(adt) => {
            let ty = adt.ty(db);
            RustItemKind::Type(RustTypeInfo {
                methods: collect_inherent_methods(ty.clone(), db, dt),
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
                methods: collect_inherent_methods(ty.clone(), db, dt),
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
                methods: collect_inherent_methods(ty.clone(), db, dt),
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
