//! Check empty source marker adoptions; serde derives supply their native supertrait obligations.

use incan_semantics_core::body_ir::NominalDeclaration;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// The closed source marker contracts whose serde obligations are checked here.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerTrait {
    TomlSerialize,
    TomlDeserialize,
}

/// Require canonical source identity, empty slots and arguments, and the retained serde derive.
pub fn validate_marker_traits(declaration: &NominalDeclaration) -> Result<(), String> {
    let mut result: Vec<MarkerTrait> = Vec::new();
    for adoption in &declaration.trait_adoptions {
        let Some(marker) = standard_marker(&adoption.trait_identity) else {
            continue;
        };
        if !adoption.arguments.is_empty() {
            return Err("unsupported Body IR type arguments on a native TOML marker trait".into());
        }
        match &adoption.methods {
            None => return Err("unsupported Body IR incomplete native TOML marker trait".into()),
            Some(methods) => {
                if !methods.is_empty() {
                    return Err("unsupported Body IR methods on a native TOML marker trait".into());
                }
            }
        }
        if !declaration
            .derives
            .iter()
            .any(|derive| derive == required_derive(marker))
        {
            return Err("unsupported Body IR native TOML marker without its serde derive".into());
        }
        if result.contains(&marker) {
            return Err("unsupported Body IR duplicate native TOML marker adoption".into());
        }
        result.push(marker);
    }
    Ok(())
}

fn standard_marker(identity: &CanonicalSymbolId) -> Option<MarkerTrait> {
    if identity.kind != SemanticSourceTargetKind::Trait || identity.namespace != SymbolNamespace::OrdinaryLexical {
        return None;
    }
    let SymbolOrigin::Package { library, module_path } = &identity.origin else {
        return None;
    };
    if library != "incan_stdlib_data" || module_path.len() != 1 || module_path[0] != "toml" {
        return None;
    }
    match identity.declaration_name.as_str() {
        "TomlSerialize" => Some(MarkerTrait::TomlSerialize),
        "TomlDeserialize" => Some(MarkerTrait::TomlDeserialize),
        _ => None,
    }
}

fn required_derive(marker: MarkerTrait) -> &'static str {
    match marker {
        MarkerTrait::TomlSerialize => "serde::Serialize",
        MarkerTrait::TomlDeserialize => "serde::Deserialize",
    }
}
