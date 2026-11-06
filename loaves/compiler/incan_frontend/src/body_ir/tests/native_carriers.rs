//! Native stdlib carriers retain checked ownership independently of source types with the same spelling.

use super::{bir, body_named, build, library_nominal_declarations};
use crate::{lexer, parser, typechecker::TypeChecker};
use incan_semantics_core::{CompilerNodeId, IncanPrimitiveType, IncanType, SymbolOrigin};

/// An untouched ValidationError builtin in an unused signature still retains its foreign ABI carrier.
#[test]
fn validation_error_retains_builtin_native_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def forward(error: ValidationError) -> ValidationError:\n    return error\n\ndef main() -> None:\n    println(\"validation carrier\")\n";
    let program = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_owned(), "native_validation".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let declarations = library_nominal_declarations(checker.type_info());
    let error = declarations
        .iter()
        .find(|owner| owner.name == "ValidationError")
        .ok_or("missing native error carrier")?;
    assert_eq!(error.canonical.origin, SymbolOrigin::Builtin);
    assert_eq!(
        checker
            .type_info()
            .declarations
            .body_ir_native_type_identities
            .get("ValidationError"),
        Some(&error.canonical)
    );
    assert_eq!(
        incan_semantics_core::RuntimeBuiltinType::from_identity(&error.canonical),
        Some(incan_semantics_core::RuntimeBuiltinType::ValidationError)
    );
    assert_eq!(error.native_item, "incan_std_core::validation::ValidationError");
    assert_eq!(error.derives, ["Debug", "Clone", "PartialEq", "Eq"]);
    assert!(
        error.fields.is_empty(),
        "the error ABI stays opaque until field operations are proved"
    );
    let mut module = build("def main() -> None:\n    pass\n", &["m", "validation_owner"])?;
    module.module_id = CompilerNodeId::module("builtin::native::ValidationError");
    assert!(module.is_well_formed_nominal_declaration(error));
    let mut forged = error.clone();
    forged.native_item = "incan_std_core::reflection::FieldInfo".to_owned();
    assert!(!module.is_well_formed_nominal_declaration(&forged));
    Ok(())
}

/// A real source model called ValidationError keeps its own fields and never selects the builtin carrier.
#[test]
fn source_validation_error_keeps_its_declared_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model ValidationError:\n    reason: str\n\ndef main() -> None:\n    error = ValidationError(reason=\"custom\")\n    println(error.reason)\n";
    let module = build(source, &["m", "source_validation"])?;
    let error = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "ValidationError")
        .ok_or("missing source error")?;
    assert!(error.native_item.is_empty());
    assert_eq!(error.fields, ["reason"]);
    assert!(matches!(error.canonical.origin, SymbolOrigin::Module(_)));
    Ok(())
}

/// The std.web imports retain their actual foreign tuple layouts, including Html's fixed String argument.
#[test]
fn web_carriers_retain_foreign_tuple_layouts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.web import Json, Query, Html\n\nmodel Search:\n    q: str\n\ndef carriers(query: Query[Search], reply: Json[Search], page: Html) -> None:\n    pass\n\ndef main() -> None:\n    pass\n";
    let program = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_owned(), "web_carriers".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let declarations = library_nominal_declarations(checker.type_info());
    for (name, item) in [
        ("Json", "incan_stdlib_web::web::response::Json"),
        ("Query", "incan_stdlib_web::web::request::Query"),
        ("Html", "incan_stdlib_web::web::response::Html"),
    ] {
        let owner = declarations.iter().find(|owner| owner.name == name).ok_or_else(|| {
            format!(
                "missing {name} carrier; target: {:?}; surface: {:?}",
                checker
                    .symbols
                    .lookup(name)
                    .and_then(|id| checker.symbols.identity_of(id)),
                checker.surface_type_import_bindings.get(name)
            )
        })?;
        assert_eq!(owner.native_item, item);
        assert_eq!(owner.fields, ["0"]);
        assert!(
            matches!(owner.canonical.origin, SymbolOrigin::RustCrate(_)),
            "{:?}",
            owner.canonical
        );
        if name == "Html" {
            assert!(owner.type_parameters.is_empty());
            assert_eq!(owner.field_types, [IncanType::Primitive(IncanPrimitiveType::Str)]);
            let encoded = serde_json::to_value(owner)?;
            assert_eq!(
                encoded["native_type_arguments"],
                serde_json::to_value(vec![IncanType::Primitive(IncanPrimitiveType::Str)])?
            );
        } else {
            assert_eq!(owner.type_parameters, ["T"]);
            assert_eq!(owner.field_types, [IncanType::TypeVar("T".to_owned())]);
        }
    }
    Ok(())
}

/// A checked extractor read records the tuple slot before the payload field, and construction records that slot.
#[test]
fn web_payloads_retain_construction_and_projection_facts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.web import Query\n\nmodel Search:\n    q: str\n\ndef read(query: Query[Search]) -> str:\n    return query.value.q\n\ndef main() -> None:\n    println(read(Query(Search(q=\"payload\"))))\n";
    let module = build(source, &["m", "web_tuple_facts"])?;
    let read = body_named(&module, "read")?;
    let projection = read
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(value)),
            } => Some(&value.place.projection),
            _ => None,
        })
        .ok_or_else(|| format!("missing payload return: {}", read.render_snapshot()))?;
    assert_eq!(projection.len(), 2, "{projection:?}");
    assert!(
        matches!(&projection[0], bir::PlaceElem::Field { name, canonical: Some(_), structural: false } if name == "0"),
        "{projection:?}"
    );
    assert!(
        matches!(&projection[1], bir::PlaceElem::Field { name, canonical: Some(_), structural: false } if name == "q"),
        "{projection:?}"
    );
    let main = body_named(&module, "main")?;
    assert!(
        main.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Assign { rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _), .. }
                if target.name == "Query" && target.canonical_field_layout.as_deref() == Some(&["0".to_owned()])
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// A source Json model's value field remains its declared field, rather than becoming a foreign tuple projection.
#[test]
fn source_json_keeps_its_declared_value_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Json:\n    value: str\n\ndef read(value: Json) -> str:\n    return value.value\n\ndef main() -> None:\n    println(read(Json(value=\"source\")))\n";
    let module = build(source, &["m", "source_json"])?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Json")
        .ok_or("missing source Json")?;
    assert!(owner.native_item.is_empty());
    assert_eq!(owner.fields, ["value"]);
    let read = body_named(&module, "read")?;
    assert!(read.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Return { value: Some(bir::Operand::Place(value)) }
            if matches!(value.place.projection.as_slice(), [bir::PlaceElem::Field { name, canonical: Some(_), .. }] if name == "value")
    )), "{}", read.render_snapshot());
    Ok(())
}

/// Html aliases retain the native constructor and String payload even when legacy omits the argument expression type.
#[test]
fn html_string_payload_retains_native_argument_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.web import Html as Page\n\ndef render(text: str) -> Page:\n    return Page(text)\n\ndef main() -> None:\n    render(\"html\")\n    Page(\"literal\")\n";
    let module = build(source, &["m", "html_payload"])?;
    let render = body_named(&module, "render")?;
    assert_eq!(render.return_type, IncanType::Named("Html".to_owned()));
    assert!(
        render.locals.iter().all(|local| local.ty != IncanType::Unknown),
        "{}",
        render.render_snapshot()
    );
    assert!(render.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Assign { rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _), .. }
            if target.canonical.is_some() && target.canonical_field_layout.as_deref() == Some(&["0".to_owned()])
    )), "{}", render.render_snapshot());
    Ok(())
}
