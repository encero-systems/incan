//! Native nominal representation identities retain physical callable inputs without source binding metadata.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use incan_lang::lang::types::collections::from_str as collection_type;
use incan_semantics_core::body_ir::NominalDeclaration;
use incan_semantics_core::{
    CanonicalSymbolId, IncanCallableParam, IncanType, SymbolOrigin, function_type, generic, native_array_type,
};

/// A representation wrapper retained through generic substitution; it is not a source type constructor.
pub const OWNED_CALLABLE_STORAGE: &str = "<owned-callable>";

/// Declaration positions for one nominal inventory, without copied declaration facts.
///
/// `owners` and `generic_owners` keep the first match by carrier key. `generic_arities` retains template validation
/// after concrete layout specialization removes the source declaration rows. `named` keeps every position by
/// declaration name in table order, since a carrier key can differ from the name. `source_files` and
/// `package_versions` retain the authenticated project inputs needed when a checked trait identity becomes a plan
/// fact. `validated_constructors` admits generated Validate calls without rescanning every declaration at every call
/// site. `count` is the inventory length indexed so far.
#[derive(Clone, Debug, Default)]
pub struct ConstructorIndex {
    pub owners: HashMap<String, usize>,
    pub generic_owners: HashMap<String, usize>,
    pub generic_arities: HashMap<String, usize>,
    pub named: BTreeMap<String, Vec<usize>>,
    pub source_files: BTreeMap<String, String>,
    pub package_versions: BTreeMap<String, String>,
    pub validated_constructors: BTreeSet<CanonicalSymbolId>,
    pub count: usize,
}

/// Index each retained constructor once, preserving independent all-row and generic-row first-match order.
pub fn constructor_index(declarations: &[NominalDeclaration]) -> ConstructorIndex {
    let mut constructors = ConstructorIndex::default();
    extend_constructor_index(&mut constructors, declarations);
    constructors
}

/// Index retained constructors and copy authenticated module files and package selections once for plan facts.
pub fn constructor_index_with_packages(
    declarations: &[NominalDeclaration],
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
) -> ConstructorIndex {
    let mut constructors = constructor_index(declarations);
    constructors.source_files = source_files.clone();
    constructors.package_versions = package_versions.clone();
    constructors
}

/// Index the positions appended since the last generation. The indexed rows must be an unchanged prefix of
/// `declarations`; earlier first matches keep their positions, so the result equals a rebuild.
pub fn extend_constructor_index(constructors: &mut ConstructorIndex, declarations: &[NominalDeclaration]) {
    for (position, declaration) in declarations.iter().enumerate().skip(constructors.count) {
        let base = declaration.template_carrier_name();
        constructors.owners.entry(base.clone()).or_insert(position);
        if declaration.type_parameter_count != 0 {
            constructors.generic_owners.entry(base.clone()).or_insert(position);
            constructors
                .generic_arities
                .entry(base)
                .or_insert(declaration.type_parameter_count as usize);
        }
        constructors.named.entry(declaration.name.clone()).or_default().push(position);
        if let Some(contract) = &declaration.validated_constructor {
            constructors.validated_constructors.insert(contract.constructor.clone());
        }
    }
    constructors.count = declarations.len();
}

/// Retain source-template arities beside specialized layout positions without copying invalid template indices.
pub fn extend_generic_constructor_arities(constructors: &mut ConstructorIndex, templates: &ConstructorIndex) {
    for (base, arity) in &templates.generic_arities {
        constructors.generic_arities.entry(base.clone()).or_insert(*arity);
    }
}

/// Return layout constructors carrying compact source-template arities without copying template positions.
pub fn with_generic_constructor_arities(
    mut constructors: ConstructorIndex,
    templates: &ConstructorIndex,
) -> ConstructorIndex {
    extend_generic_constructor_arities(&mut constructors, templates);
    constructors
}

/// Add authenticated module provenance and package selections discovered with resolved runtime declarations.
pub fn extend_constructor_authority(
    constructors: &mut ConstructorIndex,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
) {
    for (module, file) in source_files {
        constructors.source_files.insert(module.clone(), file.clone());
    }
    for (package, version) in package_versions {
        constructors.package_versions.insert(package.clone(), version.clone());
    }
}

/// Every indexed position whose declaration has this name, in table order.
pub fn named_positions(constructors: &ConstructorIndex, name: &str) -> Vec<usize> {
    constructors.named.get(name).cloned().unwrap_or_default()
}

/// Authenticate Rust-owned layouts independently of source Loaf artifacts that also have a native item.
pub fn native_owner(declaration: &NominalDeclaration) -> bool {
    matches!(declaration.canonical.origin, SymbolOrigin::RustCrate(_))
}

/// Close authenticated native arguments by erasing metadata absent from native callable storage.
///
/// Source instance symbols and checked callable types retain parameter names, defaults and partial presets. A Rust
/// function pointer or erased owned callable has none of those fields. Passing modes and nested source nominal
/// identities remain intact, so this representation boundary grants no semantic type compatibility or coercion.
pub fn representation_arguments(
    declarations: &[NominalDeclaration],
    constructors: &ConstructorIndex,
    arguments: &[IncanType],
) -> Vec<IncanType> {
    representation_types(declarations, constructors, arguments)
}

fn representation_types(
    declarations: &[NominalDeclaration],
    constructors: &ConstructorIndex,
    types: &[IncanType],
) -> Vec<IncanType> {
    types.iter().map(|ty| representation_type(declarations, constructors, ty)).collect()
}

/// Erase callable binding metadata recursively while retaining distinct authored nominal instantiations.
fn representation_type(declarations: &[NominalDeclaration], constructors: &ConstructorIndex, ty: &IncanType) -> IncanType {
    let close = |inner: &IncanType| representation_type(declarations, constructors, inner);
    match ty {
        IncanType::Function { params, return_type } => {
            let retained = params
                .iter()
                .map(|parameter| IncanCallableParam {
                    name: None,
                    ty: close(&parameter.ty),
                    kind: parameter.kind,
                    has_default: false,
                    is_partial_preset: false,
                    is_mut: parameter.is_mut,
                    native_owned: parameter.native_owned,
                    input_access: parameter.input_access,
                })
                .collect();
            function_type(retained, close(return_type))
        }
        IncanType::Generic { base, args } => {
            if retains_source_arguments(declarations, constructors, base) {
                return ty.clone();
            }
            generic(base.clone(), representation_types(declarations, constructors, args))
        }
        IncanType::Tuple(elements) => IncanType::Tuple(representation_types(declarations, constructors, elements)),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(close(inner))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(close(inner))),
        IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(close(inner))),
        IncanType::NativeArray { element, length } => native_array_type(close(element), *length),
        IncanType::NativePointer { mutable, pointee } => {
            IncanType::NativePointer { mutable: *mutable, pointee: Box::new(close(pointee)) }
        }
        _ => ty.clone(),
    }
}

/// Normalize only authenticated native or registry constructors; unknown source enum carriers stay exact.
fn retains_source_arguments(declarations: &[NominalDeclaration], constructors: &ConstructorIndex, base: &str) -> bool {
    if let Some(position) = constructors.owners.get(base) {
        return !native_owner(&declarations[*position]);
    }
    if base == OWNED_CALLABLE_STORAGE {
        return false;
    }
    collection_type(base).is_none()
}
