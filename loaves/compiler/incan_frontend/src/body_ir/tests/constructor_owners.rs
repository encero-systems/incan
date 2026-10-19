//! Checked classmethod construction retains its owner's declaration before project carrier qualification.

use super::{TypeChecker, bir, lexer, parser};
use incan_semantics_core::executable_representation::{decode_module, encode_module};

// The unchanged input from `builtin_method_markers_output_matches_legacy`, including both closed generic calls.
const BUILTIN_METHOD_MARKERS: &str = r#"class Factory:
    @classmethod
    def answer(cls) -> int:
        return 42

    @staticmethod
    def twice(value: int) -> int:
        return value * 2


model Page:
    n: int

    @staticmethod
    def make(n: int) -> Page:
        return Page(n=n)

    @classmethod
    def build(cls, n: int) -> Self:
        return cls(n=n)


@derive(Clone)
class FactoryBox[T with Clone]:
    pub value: T

    @classmethod
    def make(cls, value: T) -> Self:
        return cls(value=value)


def main() -> None:
    println(Factory.answer())
    println(Factory.twice(21))
    println(Page.make(3).n)
    println(Page.build(5).n)
    println(FactoryBox[int].make(1).value)
    println(FactoryBox[str].make("two").value)
"#;

/// Every original constructor, including nongeneric and generic `cls(...)`, keeps its exact owner and field layout
/// in both local and project profiles, before and after production executable encoding.
#[test]
fn builtin_method_marker_constructors_retain_checked_owners() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(BUILTIN_METHOD_MARKERS).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut missing = Vec::new();
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, checker.type_info())
        };
        assert_eq!(decode_module(&encode_module(&module)?)?, module);
        let mut constructions = 0;
        for body in &module.bodies {
            for statement in &body.block.stmts {
                let bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _),
                    ..
                } = &statement.kind
                else {
                    continue;
                };
                let owner = if target.name == "Page" || body.name == "build" {
                    "Page"
                } else {
                    "FactoryBox"
                };
                let declaration = module
                    .nominal_declarations
                    .iter()
                    .find(|declaration| declaration.canonical.declaration_name == owner)
                    .ok_or_else(|| format!("missing original {owner} declaration"))?;
                constructions += 1;
                if target.canonical.as_ref() != Some(&declaration.canonical)
                    || target.direct_declaration_id.as_ref() != Some(&declaration.direct_declaration_id)
                    || target.canonical_field_layout.as_ref() != Some(&declaration.fields)
                {
                    missing.push(format!(
                        "project={project}, body={}, constructor={target:?}, expected={declaration:?}\n{}",
                        body.name,
                        body.render_snapshot()
                    ));
                }
            }
        }
        assert_eq!(constructions, 3, "project={project}");
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    Ok(())
}
