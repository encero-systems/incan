//! Native derive selections projected from checked newtype and import facts, without changing legacy selections.

use super::{TypeCheckInfo, ast};
use incan_lang::lang::stdlib;

/// Canonicalize the newtype snapshot's source derive names using active checked import bindings.
///
/// Models already retain native selections, but newtypes retain source names alongside canonical adopted serde
/// capabilities. Resolve those names once at the Body IR boundary; lowering consumes only the resulting selections.
pub(super) fn checked_newtype_derives(newtype: &ast::NewtypeDecl, facts: &TypeCheckInfo) -> Option<Vec<String>> {
    let construction = facts.declarations.newtype_construction.get(&newtype.name)?;
    let mut selected = Vec::new();
    for name in &construction.explicit_derives {
        let path = checked_derive_path(name, facts);
        let names = if stdlib::is_stdlib_json_trait_module_path(&path) {
            vec!["serde::Serialize", "serde::Deserialize"]
        } else {
            let protocol = path
                .split_last()
                .and_then(|(name, module)| stdlib::stdlib_json_trait_id_for_identity(module, name));
            match protocol {
                Some(stdlib::StdlibJsonTraitId::Serialize) => vec!["serde::Serialize"],
                Some(stdlib::StdlibJsonTraitId::Deserialize) => vec!["serde::Deserialize"],
                None => vec![name.as_str()],
            }
        };
        for name in names {
            if !selected.iter().any(|retained| retained == name) {
                selected.push(name.to_owned());
            }
        }
    }
    Some(selected)
}

/// Preserve a checked bare import or extend a checked module binding by its member path; absent bindings stay bare.
fn checked_derive_path(name: &str, facts: &TypeCheckInfo) -> Vec<String> {
    if let Some(path) = facts.import_binding_path(name) {
        return path.to_vec();
    }
    if let Some((binding, members)) = name.split_once('.')
        && let Some(path) = facts.import_binding_path(binding)
    {
        return path
            .iter()
            .cloned()
            .chain(members.split('.').map(str::to_owned))
            .collect();
    }
    vec![name.to_owned()]
}
