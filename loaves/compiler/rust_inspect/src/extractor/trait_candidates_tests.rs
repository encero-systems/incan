//! Compatibility checks for query-scoped acquisition from the existing analyzer trait index.

use std::collections::HashSet;
use std::fs;

use incan_lang::interop::RustItemKind;
use ra_ap_hir::{Impl, ModuleDef, Mutability, Name, attach_db};

use super::{
    RustWorkspace, collect_implemented_traits, crate_dependency_closure, extract_rust_item, find_crate,
    resolve_rust_path, surface_trait_candidates,
};
use crate::loader::OVEN_DIRECT_INSPECTION_MARKER;

/// Preserve the original non-blanket candidate set for aliases, generics, primitives, references and derives.
///
/// The graph-wide API is used only as a compatibility oracle in this test. Production uses its existing per-crate
/// index and never maintains another candidate cache or solver implementation.
#[test]
fn scoped_trait_candidates_match_original_analyzer_semantics() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Fixture: distinct receiver constructors and multiple implementations of the same trait ----
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "scoped_traits"
version = "0.1.0"

[rust]
name = "scoped_traits"
edition = "2021"
type = "lib"
"#,
    )?;
    fs::write(tmp.path().join(OVEN_DIRECT_INSPECTION_MARKER), b"direct\n")?;
    fs::write(
        tmp.path().join("src/lib.rs"),
        r#"#[derive(Clone, Copy, Debug)]
pub struct Thing;
pub trait Direct {}
impl Direct for Thing {}
pub trait Blanket {}
impl<T> Blanket for T {}
pub trait Reference {}
impl<T> Reference for &mut T {}
pub trait ConstExpansion {}
const _: () = { impl ConstExpansion for Thing {} };
pub struct Generic<T>(pub T);
impl Direct for Generic<u8> {}
impl Direct for Generic<u16> {}
pub type ConcreteGeneric = Generic<u8>;
pub type ThingAlias = Thing;
pub type Primitive = i32;
impl Direct for i32 {}
pub type MutableThing = &'static mut Thing;
pub struct LifetimeBounded<'a: 'static>(pub &'a Thing);
impl<'a: 'static> Direct for LifetimeBounded<'a> {}
pub type LifetimeAlias<'a> = &'a mut Thing;
"#,
    )?;
    let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
    let db = workspace.db();

    // ---- Candidate parity: use the exact original shallow index, including mutable receiver forms ----
    attach_db(db, || -> Result<(), Box<dyn std::error::Error>> {
        let krate =
            find_crate(&workspace, "scoped_traits").ok_or_else(|| std::io::Error::other("fixture crate missing"))?;
        let authorized = crate_dependency_closure(krate, db);
        for name in [
            "Thing",
            "Generic",
            "ConcreteGeneric",
            "ThingAlias",
            "Primitive",
            "MutableThing",
            "LifetimeBounded",
            "LifetimeAlias",
        ] {
            let resolved = resolve_rust_path(db, krate, &[Name::new_root(name)])?;
            let ty = match resolved.definition {
                ModuleDef::Adt(adt) => adt.ty(db),
                ModuleDef::TypeAlias(alias) => alias.ty(db),
                _ => return Err(std::io::Error::other("fixture did not resolve to a type").into()),
            };
            if name.starts_with("Lifetime") {
                let mut expected = collect_implemented_traits(ty.clone(), &authorized, false, db);
                expected.extend(collect_implemented_traits(
                    ty.add_reference(Mutability::Mut),
                    &authorized,
                    true,
                    db,
                ));
                expected.sort_by(|left, right| {
                    (&left.path, left.mutable_reference).cmp(&(&right.path, right.mutable_reference))
                });
                expected.dedup();
                let metadata = extract_rust_item(&workspace, &format!("scoped_traits::{name}"))?;
                let RustItemKind::Type(info) = metadata.kind else {
                    return Err(std::io::Error::other("expected lifetime-parameterized type metadata").into());
                };
                assert_eq!(
                    info.implemented_traits, expected,
                    "lifetime declaration environment changed for {name}"
                );
            }
            for receiver in [ty.clone(), ty.add_reference(Mutability::Mut)] {
                let intrinsic = |implementation: &Impl| {
                    implementation
                        .trait_(db)
                        .is_some_and(|trait_| authorized.contains(&trait_.module(db).krate(db)))
                };
                let original: HashSet<_> = Impl::all_for_type(db, receiver.clone())
                    .into_iter()
                    .filter(intrinsic)
                    .collect();
                let scoped: HashSet<_> = surface_trait_candidates(&receiver, &authorized, db)
                    .into_iter()
                    .filter(intrinsic)
                    .collect();
                assert_eq!(scoped, original, "candidate set changed for {name}");
            }
        }
        Ok(())
    })?;

    // ---- Metadata: preserve direct, reference and generated impls while excluding blanket impls ----
    let metadata = extract_rust_item(&workspace, "scoped_traits::Thing")?;
    let RustItemKind::Type(info) = metadata.kind else {
        return Err(std::io::Error::other("expected Thing type metadata").into());
    };
    for expected in ["scoped_traits::Direct", "scoped_traits::ConstExpansion"] {
        assert!(
            info.implemented_traits
                .iter()
                .any(|trait_| trait_.path == expected && !trait_.mutable_reference)
        );
    }
    assert!(
        info.implemented_traits
            .iter()
            .any(|trait_| { trait_.path == "scoped_traits::Reference" && trait_.mutable_reference })
    );
    assert!(
        info.implemented_traits
            .iter()
            .all(|trait_| trait_.path != "scoped_traits::Blanket")
    );
    Ok(())
}

/// A trait its crate does not export is no fact about a type: source cannot name it, and recording it would make a
/// re-exported type's facts depend on which crate's surface was queried, as `std::io::SizeHint` does for
/// `std::time::Duration` against `core::time::Duration`.
#[test]
fn private_trait_implementations_are_not_type_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname = \"private_traits\"\nversion = \"0.1.0\"\n\n[rust]\nname = \"private_traits\"\nedition = \"2021\"\ntype = \"lib\"\n",
    )?;
    fs::write(tmp.path().join(OVEN_DIRECT_INSPECTION_MARKER), b"direct\n")?;
    fs::write(
        tmp.path().join("src/lib.rs"),
        "pub struct Thing;\npub trait Public {}\nimpl Public for Thing {}\ntrait Hidden {}\nimpl Hidden for Thing {}\n",
    )?;
    let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
    let metadata = extract_rust_item(&workspace, "private_traits::Thing")?;
    let RustItemKind::Type(info) = metadata.kind else {
        return Err(std::io::Error::other("expected Thing type metadata").into());
    };
    let paths: Vec<_> = info
        .implemented_traits
        .iter()
        .map(|trait_| trait_.path.as_str())
        .collect();
    assert!(
        paths.contains(&"private_traits::Public"),
        "public trait missing: {paths:?}"
    );
    assert!(
        !paths.contains(&"private_traits::Hidden"),
        "private trait recorded: {paths:?}"
    );
    Ok(())
}

/// A callable value wins over a same-path module without hiding module traversal or other namespace partners.
#[test]
fn public_function_metadata_survives_same_named_module() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname='namespace_probe'\nversion='0.1.0'\n[rust]\nname='namespace_probe'\nedition='2021'\ntype='lib'\n",
    )?;
    fs::write(tmp.path().join(OVEN_DIRECT_INSPECTION_MARKER), b"direct\n")?;
    fs::write(
        tmp.path().join("src/lib.rs"),
        r#"mod imports { pub mod hash { pub fn nested() -> u32 { 1 } } }
use imports::hash;
/// Public callable sharing the imported module spelling.
pub fn hash(_data: &[u8]) -> u32 { 7 }
pub mod shared { pub fn nested() -> u32 { 2 } }
/// Public callable sharing a public module spelling.
pub fn shared() -> u32 { 3 }
pub mod module_only { pub fn nested() -> u32 { 4 } }
pub mod private_value { pub fn nested() -> u32 { 5 } }
/// A private value cannot hide the public module.
fn private_value() -> u32 { 6 }
mod values {
    /// Public definition imported privately by the root.
    pub fn imported_private() -> u32 { 8 }
    /// Public definition reexported by the root.
    pub fn exported() -> u32 { 9 }
}
use values::imported_private;
pub mod imported_private {}
pub use values::exported;
pub mod exported {}
pub use core::clone::Clone;
"#,
    )?;
    let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
    for path in [
        "hash",
        "shared",
        "shared::nested",
        "module_only::nested",
        "private_value::nested",
    ] {
        let query = format!("namespace_probe::{path}");
        let metadata = extract_rust_item(&workspace, &query)?;
        assert!(
            matches!(metadata.kind, RustItemKind::Function(_)),
            "public callable {query} must retain its value namespace: {metadata:?}"
        );
        assert_eq!(metadata.definition_path.as_deref(), Some(query.as_str()));
    }
    for path in ["module_only", "private_value", "imported_private"] {
        let metadata = extract_rust_item(&workspace, &format!("namespace_probe::{path}"))?;
        assert!(
            matches!(metadata.kind, RustItemKind::Module(_)),
            "a module without a public function partner must stay a module: {metadata:?}"
        );
    }
    let exported = extract_rust_item(&workspace, "namespace_probe::exported")?;
    assert!(matches!(exported.kind, RustItemKind::Function(_)));
    assert_eq!(
        exported.definition_path.as_deref(),
        Some("namespace_probe::values::exported")
    );
    attach_db(workspace.db(), || -> Result<(), Box<dyn std::error::Error>> {
        let krate = find_crate(&workspace, "namespace_probe").ok_or("namespace fixture missing")?;
        let resolved = resolve_rust_path(workspace.db(), krate, &[Name::new_root("Clone")])?;
        assert!(matches!(resolved.definition, ModuleDef::Trait(_)));
        assert!(
            resolved.derive_macro.is_some(),
            "the same-path derive partner must remain attached"
        );
        Ok(())
    })?;
    Ok(())
}
