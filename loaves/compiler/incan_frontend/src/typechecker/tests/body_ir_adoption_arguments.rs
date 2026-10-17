//! Checked bare-adoption argument witnesses are separate from the legacy explicit-clause snapshot.

use crate::ast::{Declaration, Program};
use crate::symbols::ResolvedType;
use crate::typechecker::{TypeCheckInfo, TypeChecker};
use crate::{lexer, parser};

const BARE: &str =
    include_str!("../../../../incan_test_support/fixtures/native_trait_adoption/bare_argument_witness.incn");

/// Check accepted source with an owning module identity before reading its immutable lowering facts.
fn checked(source: &str) -> Result<(Program, TypeCheckInfo), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".into(), "adoption_arguments".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok((program, checker.type_info().clone()))
}

/// Locate a source clause's own span instead of deriving an argument fact from the trait's spelling.
fn clause_key(program: &Program, owner: &str) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            Declaration::Model(model) if model.name == owner => {
                model.traits.first().map(|clause| (clause.span.start, clause.span.end))
            }
            _ => None,
        })
        .ok_or_else(|| std::io::Error::other("missing model adoption clause").into())
}

/// Bare adoption maps the owner's parameter position even when the trait declares a different parameter name.
#[test]
fn bare_generic_adoption_witnesses_are_body_ir_only() -> Result<(), Box<dyn std::error::Error>> {
    let (program, info) = checked(BARE)?;
    let key = clause_key(&program, "Cell")?;
    assert_eq!(
        info.traits.body_ir_adoption_type_args.get(&key).map(Vec::as_slice),
        Some([ResolvedType::TypeVar("Payload".into())].as_slice()),
    );
    assert!(
        !info.traits.adoption_type_args.contains_key(&key),
        "legacy's bare-clause omission changed"
    );
    Ok(())
}

/// Written explicit arguments retain their existing legacy fact and do not need the new bare-adoption table.
#[test]
fn explicit_adoption_keeps_the_legacy_argument_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"trait Read[Element]:
    def read(self) -> Element: ...

model Direct[Value] with Read[Value]:
    pub value: Value
    def read(self) -> Value:
        return self.value

def main() -> None:
    println(Direct(value=5).read())
"#;
    let (program, info) = checked(source)?;
    let key = clause_key(&program, "Direct")?;
    assert_eq!(
        info.traits.adoption_type_args.get(&key).map(Vec::as_slice),
        Some([ResolvedType::TypeVar("Value".into())].as_slice()),
    );
    Ok(())
}
