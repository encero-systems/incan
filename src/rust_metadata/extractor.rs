//! Map rust-analyzer `hir` definitions into [`incan_core::interop::RustItemMetadata`].

use std::collections::BTreeMap;

use incan_core::interop::{
    RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustModuleChild, RustModuleChildKind,
    RustModuleInfo, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo, RustVisibility,
};
use ra_ap_hir::{
    AssocItem, Crate, DisplayTarget, Function, HasVisibility, HirDisplay, ItemInNs, Module, ModuleDef, Name, ScopeDef,
    Trait, Type, Visibility, attach_db,
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

fn extract_function_sig(f: Function, db: &RootDatabase, dt: DisplayTarget) -> RustFunctionSig {
    let params = f
        .assoc_fn_params(db)
        .into_iter()
        .map(|p| RustParam {
            name: p.name(db).map(|n| n.as_str().to_owned()),
            type_display: format_ty(p.ty(), db, dt),
        })
        .collect();
    let return_type = format_ty(&f.ret_type(db), db, dt);
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

fn find_crate(db: &RootDatabase, crate_name: &str) -> Option<Crate> {
    Crate::all(db).into_iter().find(|k| {
        k.display_name(db).is_some_and(|dn| {
            dn.to_string() == crate_name
                || dn.crate_name().as_str() == crate_name
                || dn.canonical_name().as_str() == crate_name
        })
    })
}

fn resolve_module_def(db: &RootDatabase, krate: Crate, segments: &[Name]) -> Result<ModuleDef, RustMetadataError> {
    let root = krate.root_module(db);
    let Some(mut it) = root.resolve_mod_path(db, segments.iter().cloned()) else {
        return Err(RustMetadataError::PathNotResolved(segments_display(segments)));
    };
    let Some(first) = it.next() else {
        return Err(RustMetadataError::PathNotResolved(segments_display(segments)));
    };
    match first {
        ItemInNs::Macros(_) => Err(RustMetadataError::UnsupportedMacro(segments_display(segments))),
        other => Ok(other.into_module_def()),
    }
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
                methods: collect_inherent_methods(ty, db, dt),
            })
        }
        ModuleDef::BuiltinType(b) => {
            let ty = b.ty(db);
            RustItemKind::Type(RustTypeInfo {
                methods: collect_inherent_methods(ty, db, dt),
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
                methods: collect_inherent_methods(ty, db, dt),
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
        visibility: vis,
        kind,
    })
}
