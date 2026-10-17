//! Direct-only automatic conversion candidates preserve checked support and exclude source overrides.

use super::{build, build_body_ir_module_v0};
use crate::{
    ast, lexer, parser,
    typechecker::{TypeCheckInfo, TypeChecker},
};
use incan_semantics_core::{
    SymbolOrigin,
    executable_representation::{ExecutableDeclaration, SurfaceReader, build_surface},
};
use std::collections::BTreeSet;

const ORIGINAL: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/codegen_models_and_collections/environ_get_as_composed_newtype.incn"
);

/// The original raw chain retains candidates only on its actual checked canonical declarations.
#[test]
fn composed_newtypes_retain_checked_conversion_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(ORIGINAL, &["m", "composed_conversion"])?;
    for name in ["Port", "WrappedPort"] {
        let declaration = module
            .nominal_declarations
            .iter()
            .find(|value| value.name == name)
            .ok_or("original conversion owner absent")?;
        assert_eq!(
            serde_json::to_value(declaration)?["newtype_string_conversion"],
            "Automatic"
        );
        assert_eq!(declaration.canonical.declaration_name, name);
        assert_eq!(
            declaration.field_identities,
            std::slice::from_ref(&declaration.canonical)
        );
        assert!(module.is_well_formed_nominal_declaration(declaration));
    }
    Ok(())
}

/// A raw int tuple cannot grant a conversion after the existing checker support witness is removed.
#[test]
fn automatic_conversion_requires_checked_support() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Port = newtype int\n\ndef main() -> None:\n    pass\n";
    let (program, mut facts) = checked(source)?;
    let module = build_body_ir_module_v0(&program, &["composed_conversion".to_owned()], &facts);
    let original = module.nominal_declarations.first().ok_or("original raw owner absent")?;
    assert_eq!(
        serde_json::to_value(original)?["newtype_string_conversion"],
        "Automatic"
    );
    facts
        .declarations
        .newtype_construction
        .get_mut("Port")
        .ok_or("checked construction absent")?
        .supports_string_conversion = false;
    let damaged = build_body_ir_module_v0(&program, &["composed_conversion".to_owned()], &facts);
    assert_eq!(damaged.nominal_declarations.len(), 1);
    let changed = damaged.nominal_declarations.first().ok_or("damaged raw owner absent")?;
    assert_eq!(changed.field_types, original.field_types);
    assert_eq!(
        serde_json::to_value(changed)?["newtype_string_conversion"],
        "Unselected"
    );
    Ok(())
}

/// Closed str chains retain the same candidates with independently checked active and inactive imports.
#[test]
fn string_chain_candidates_do_not_depend_on_activation() -> Result<(), Box<dyn std::error::Error>> {
    for imports in ["", "from std.traits.convert import TryFrom\n\n"] {
        let source = format!(
            "{imports}type Label = newtype str\ntype Outer = newtype Label\ntype Envelope = newtype Outer\n\ndef main() -> None:\n    pass\n"
        );
        let module = build(&source, &["m", "string_conversion_candidates"])?;
        for name in ["Label", "Outer", "Envelope"] {
            let declaration = module
                .nominal_declarations
                .iter()
                .find(|value| value.name == name)
                .ok_or("raw string chain owner absent")?;
            assert_eq!(
                serde_json::to_value(declaration)?["newtype_string_conversion"],
                "Automatic"
            );
            assert!(module.is_well_formed_nominal_declaration(declaration));
        }
    }
    Ok(())
}

/// Existing layouts outside the raw closed profile cannot acquire automatic conversion from support alone.
#[test]
fn hooks_constraints_binders_and_other_primitives_remain_unselected() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Checked = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        return Ok(Checked(value))

type Positive = newtype int[gt=0]
type Box[T] = newtype T
type Ratio = newtype float
type Flag = newtype bool

def main() -> None:
    pass
"#,
        &["m", "conversion_profile_exclusions"],
    )?;
    for name in ["Checked", "Positive", "Box", "Ratio", "Flag"] {
        let declaration = module
            .nominal_declarations
            .iter()
            .find(|value| value.name == name)
            .ok_or("excluded ordinary layout absent")?;
        assert_eq!(
            serde_json::to_value(declaration)?["newtype_string_conversion"],
            "Unselected"
        );
    }
    Ok(())
}

/// Explicit source adoption can prove support without authorizing an automatic implementation over it.
#[test]
fn explicit_conversion_support_does_not_grant_an_automatic_inner() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.traits.convert import TryFrom

type Port = newtype int with TryFrom[str]:
    @classmethod
    def try_from(cls, value: str) -> Result[Self, str]:
        return Ok(Port(91))

type WrappedPort = newtype Port
type Envelope = newtype WrappedPort

def main() -> None:
    pass
"#;
    let (program, facts) = checked(source)?;
    for name in ["Port", "WrappedPort"] {
        assert!(
            facts
                .declarations
                .newtype_construction
                .get(name)
                .ok_or("checked supported owner absent")?
                .supports_string_conversion
        );
    }
    let module = build_body_ir_module_v0(&program, &["composed_conversion".to_owned()], &facts);
    for name in ["Port", "WrappedPort", "Envelope"] {
        let declaration = module
            .nominal_declarations
            .iter()
            .find(|value| value.name == name)
            .ok_or("supported owner layout absent")?;
        assert_eq!(
            serde_json::to_value(declaration)?["newtype_string_conversion"],
            "Unselected"
        );
    }
    // The ordinary outer layout retains its exact inner identity without authorizing an automatic override.
    let outer = module
        .nominal_declarations
        .iter()
        .find(|value| value.name == "WrappedPort")
        .ok_or("raw outer absent")?;
    let inner = module
        .nominal_declarations
        .iter()
        .find(|value| value.name == "Port")
        .ok_or("explicit inner absent")?;
    assert_eq!(outer.named_type_identities.get("Port"), Some(&inner.canonical));
    Ok(())
}

/// A checked foreign edge and its local transitive wrapper keep layouts without scheduling unsupported roots.
#[test]
fn foreign_underlying_chains_preserve_layout_without_automatic_conversion() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source = "pub type Port = newtype int\n";
    let provider = build(provider_source, &["owners"])?;
    let identity = provider
        .nominal_declarations
        .first()
        .ok_or("dependency newtype absent")?
        .canonical
        .clone();
    let module = super::identities_and_imports::build_with_published_imports(
        "from owners import Port\nfrom std.environ import get_as\n\ntype Wrapper = newtype Port\ntype Envelope = newtype Wrapper\n\ndef main() -> None:\n    pass\n",
        &["main"],
        &[("owners", &["owners"], provider_source)],
        &[provider],
    )?;
    for name in ["Wrapper", "Envelope"] {
        let owner = module
            .nominal_declarations
            .iter()
            .find(|owner| owner.name == name)
            .ok_or("ordinary wrapper layout absent")?;
        assert_eq!(serde_json::to_value(owner)?["newtype_string_conversion"], "Unselected");
        assert!(module.is_well_formed_nominal_declaration(owner));
        if name == "Wrapper" {
            assert_eq!(owner.named_type_identities.get("Port"), Some(&identity));
        }
    }
    Ok(())
}

/// Preserve one checked module identity while deliberately damaging only the test's cloned support fact.
fn checked(source: &str) -> Result<(ast::Program, TypeCheckInfo), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["composed_conversion".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok((program, checker.type_info().clone()))
}

/// A genuinely package-owned raw layout grants no invocation-local source conversion authority.
#[test]
fn package_newtypes_do_not_select_automatic_conversion_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_package_newtype()?;
    let owner = module.nominal_declarations.first().ok_or("package raw owner absent")?;
    assert!(matches!(&owner.canonical.origin, SymbolOrigin::Package { library, .. } if library == "conversion_owner"));
    assert_eq!(serde_json::to_value(owner)?["newtype_string_conversion"], "Unselected");
    Ok(())
}

/// Publication removes a stale source candidate rather than exporting an obligation for an activated consumer.
#[test]
fn package_publication_clears_stale_automatic_conversion_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = checked_package_newtype()?;
    let owner = module.nominal_declarations.first().ok_or("package raw owner absent")?;
    let identity = owner.canonical.clone();
    let mut encoded = serde_json::to_value(owner)?;
    encoded["newtype_string_conversion"] = serde_json::Value::from("Automatic");
    module.nominal_declarations[0] = serde_json::from_value(encoded)?;
    let bytes = build_surface(
        &[module],
        "conversion_owner",
        "1.0.0",
        &BTreeSet::from([identity.clone()]),
        &BTreeSet::new(),
    )?;
    let published = SurfaceReader::open(&bytes)?.fragment(&identity)?;
    let ExecutableDeclaration::Nominal(nominal) = published else {
        return Err("published raw owner is not a nominal".into());
    };
    assert_eq!(
        serde_json::to_value(nominal)?["newtype_string_conversion"],
        "Unselected"
    );
    Ok(())
}

/// Produce the canonical Package owner through the existing checker before testing either boundary.
fn checked_package_newtype() -> Result<incan_semantics_core::body_ir::BodyIrModule, Box<dyn std::error::Error>> {
    let source = "pub type ExternalId = newtype int\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("conversion_owner".to_owned()));
    checker.set_current_module_path(Some(vec!["lib".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(
        &program,
        &["lib".to_owned()],
        checker.type_info(),
    ))
}
