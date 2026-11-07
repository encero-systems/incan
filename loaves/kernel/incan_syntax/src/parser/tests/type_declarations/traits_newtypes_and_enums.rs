//! Trait supertraits and adoptions, newtypes and rusttypes, interop edges, metadata keys and enums.

use super::*;

#[test]
fn test_parse_trait_with_supertraits() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
    def len(self) -> int: ...

trait BoundedDataSet[T] with DataSet[T]:
    def sorted(self) -> Self: ...

trait Combo with BoundedDataSet[int], DataSet[str]:
    def go(self) -> None: ...
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 3);

    let ds = require_trait_decl(&program.declarations[0])?;
    assert_eq!(ds.name, "DataSet");
    assert!(ds.traits.is_empty());

    let bounded = require_trait_decl(&program.declarations[1])?;
    assert_eq!(bounded.name, "BoundedDataSet");
    assert_eq!(bounded.traits.len(), 1);
    assert_eq!(bounded.traits[0].node.name, "DataSet");
    assert_eq!(bounded.traits[0].node.type_args.len(), 1);
    assert_eq!(require_simple_type(&bounded.traits[0].node.type_args[0])?, "T");

    let combo = require_trait_decl(&program.declarations[2])?;
    assert_eq!(combo.name, "Combo");
    assert_eq!(combo.traits.len(), 2);
    assert_eq!(combo.traits[0].node.name, "BoundedDataSet");
    assert_eq!(combo.traits[0].node.type_args.len(), 1);
    assert_eq!(combo.traits[1].node.name, "DataSet");
    assert_eq!(combo.traits[1].node.type_args.len(), 1);
    Ok(())
}

#[test]
fn test_parse_parenthesized_declaration_trait_adoptions() -> Result<(), Vec<CompileError>> {
    let source = r#"
class BinaryBuffer with (
    BinaryReader,
    BinaryRead[u8],
    BinaryWrite[u8],
):
    handle: Cursor[bytes]
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    assert_eq!(class.traits.len(), 3);
    assert_eq!(class.traits[0].node.name, "BinaryReader");
    assert_eq!(class.traits[1].node.name, "BinaryRead");
    assert_eq!(class.traits[1].node.type_args.len(), 1);
    assert_eq!(class.traits[2].node.name, "BinaryWrite");
    assert_eq!(class.traits[2].node.type_args.len(), 1);
    Ok(())
}

#[test]
fn test_parse_newtype_with_docstring() -> Result<(), Vec<CompileError>> {
    let source = r#"
type UserId[T] = newtype int:
    """Opaque identifier wrapper."""

    def raw(self) -> int:
        return 1
"#;
    let program = parse_str(source)?;
    let nt = require_newtype_decl(&program.declarations[0])?;
    assert_eq!(nt.name, "UserId");
    assert_eq!(nt.type_params.len(), 1);
    assert_eq!(nt.docstring.as_deref(), Some("Opaque identifier wrapper."));
    assert_eq!(nt.methods.len(), 1);
    assert_eq!(nt.methods[0].node.name, "raw");
    assert!(!nt.is_rusttype);
    assert!(nt.rebindings.is_empty());
    assert!(nt.interop_edges.is_empty());
    Ok(())
}

#[test]
fn test_parse_rusttype_with_rebinding_and_interop() -> Result<(), Vec<CompileError>> {
    let source = r#"
type Sender[T] = rusttype RustSender[T]:
    send_now = try_send

    interop:
        from str try Sender.parse
        into bytes via Sender.encode
"#;
    let program = parse_str(source)?;
    let nt = require_newtype_decl(&program.declarations[0])?;
    assert!(nt.is_rusttype);
    assert_eq!(nt.rebindings.len(), 1);
    assert_eq!(nt.rebindings[0].node.name, "send_now");
    assert_eq!(nt.interop_edges.len(), 2);
    assert!(matches!(nt.interop_edges[0].node.direction, InteropDirection::From));
    assert!(matches!(nt.interop_edges[0].node.adapter_kind, InteropAdapterKind::Try));
    assert!(matches!(nt.interop_edges[1].node.direction, InteropDirection::Into));
    assert!(matches!(nt.interop_edges[1].node.adapter_kind, InteropAdapterKind::Via));
    Ok(())
}

#[test]
fn test_parse_rusttype_with_trait_adoption_method_target_and_associated_type() -> Result<(), Vec<CompileError>> {
    let source = r#"
type UserId = rusttype i64 with Display, Debug:
    type Output for Add[int] = UserId

    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        pass
"#;
    let program = parse_str(source)?;
    let nt = require_newtype_decl(&program.declarations[0])?;
    assert!(nt.is_rusttype);
    assert_eq!(nt.traits.len(), 2);
    assert_eq!(nt.traits[0].node.name, "Display");
    assert_eq!(nt.traits[1].node.name, "Debug");

    assert_eq!(nt.associated_types.len(), 1);
    let associated_type = &nt.associated_types[0].node;
    assert_eq!(associated_type.name, "Output");
    assert_eq!(associated_type.trait_target.node.name, "Add");
    assert_eq!(associated_type.trait_target.node.type_args.len(), 1);
    assert_eq!(
        require_simple_type(&associated_type.trait_target.node.type_args[0])?,
        "int"
    );
    assert!(matches!(associated_type.ty.node, Type::Simple(ref name) if name == "UserId"));

    assert_eq!(nt.methods.len(), 1);
    let method = &nt.methods[0].node;
    assert_eq!(method.name, "fmt");
    let Some(target) = &method.trait_target else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected method trait target".to_string(),
            nt.methods[0].span,
        )]);
    };
    assert_eq!(target.node.name, "Display");
    assert!(target.node.type_args.is_empty());
    assert!(matches!(
        method.return_type.node,
        Type::Generic(ref name, _) if name == collections::as_str(CollectionTypeId::Result)
    ));
    Ok(())
}

#[test]
fn test_parse_method_trait_target_after_return_type_is_rejected() {
    let source = r#"
type UserId = rusttype i64 with Display:
    def fmt(self, f: Formatter) -> Result[None, FmtError] for Display:
        pass
"#;
    let Err(errors) = parse_str(source) else {
        panic!("method trait target after return type should be rejected");
    };
    assert!(
        errors.iter().any(|err| err
            .message
            .contains("Method trait target must appear before the return type")),
        "expected focused method trait target placement diagnostic, got {errors:?}"
    );
}

#[test]
fn test_parse_rusttype_minimal() -> Result<(), Vec<CompileError>> {
    let source = r#"
type Email = rusttype RustEmailAddress
"#;
    let program = parse_str(source)?;
    let nt = require_newtype_decl(&program.declarations[0])?;
    assert!(nt.is_rusttype);
    assert!(nt.traits.is_empty());
    assert!(nt.associated_types.is_empty());
    assert!(nt.methods.is_empty());
    assert!(nt.rebindings.is_empty());
    assert!(nt.interop_edges.is_empty());
    Ok(())
}

#[test]
fn test_parse_rusttype_qualified_underlying() -> Result<(), Vec<CompileError>> {
    let source = "type MyBin = rusttype proto_type::Binary\n";
    let program = parse_str(source)?;
    let nt = require_newtype_decl(&program.declarations[0])?;
    assert!(nt.is_rusttype);
    assert!(matches!(
        &nt.underlying.node,
        Type::Qualified(segs) if segs == &vec!["proto_type".to_string(), "Binary".to_string()]
    ));
    Ok(())
}

#[test]
fn test_parse_into_try_interop_edge() -> Result<(), Vec<CompileError>> {
    let source = r#"
type Email = rusttype RustEmailAddress:
    def try_into_str(self) -> Result[str, str]:
        ...

    interop:
        into str try Email.try_into_str
"#;
    let program = parse_str(source)?;
    let nt = require_newtype_decl(&program.declarations[0])?;
    assert_eq!(nt.interop_edges.len(), 1);
    assert!(matches!(nt.interop_edges[0].node.direction, InteropDirection::Into));
    assert!(matches!(nt.interop_edges[0].node.adapter_kind, InteropAdapterKind::Try));
    Ok(())
}

#[test]
fn test_parse_interop_edge_missing_via_or_try_is_error() {
    let source = r#"
type Email = rusttype RustEmailAddress:
    interop:
        from str Email.parse
"#;
    let result = parse_str(source);
    assert!(
        result.is_err(),
        "expected parser error for missing `via`/`try` in interop edge"
    );
    let errs = match result {
        Err(errs) => errs,
        Ok(_) => Vec::new(),
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Expected `via` or `try` in interop edge")),
        "expected missing-adapter-kind parser error, got: {errs:?}"
    );
}

#[test]
fn test_parse_interop_edge_missing_from_or_into_is_error() {
    let source = r#"
type Email = rusttype RustEmailAddress:
    interop:
        str via Email.parse
"#;
    let result = parse_str(source);
    assert!(
        result.is_err(),
        "expected parser error for missing `from`/`into` in interop edge"
    );
    let errs = match result {
        Err(errs) => errs,
        Ok(_) => Vec::new(),
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Expected `from` or `into` in interop edge")),
        "expected missing-direction parser error, got: {errs:?}"
    );
}

#[test]
fn test_parse_non_identifier_alias() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Weird:
  one_ [alias="1"]: int
"#;
    let program = parse_str(source)?;
    let model = match &program.declarations[0].node {
        Declaration::Model(m) => m,
        _ => panic!("Expected model"),
    };
    let field = &model.fields[0].node;
    assert_eq!(field.metadata.alias.as_deref(), Some("1"));
    Ok(())
}

#[test]
fn test_parse_duplicate_metadata_key_error() {
    // RFC 021: Duplicate metadata keys are compile-time errors
    let source = r#"
model Account:
  type_ [alias="a", alias="b"]: str
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected duplicate alias key error");
    };
    assert!(
        err[0].message.contains("Duplicate 'alias'"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_parse_duplicate_description_key_error() {
    // RFC 021: Duplicate metadata keys are compile-time errors
    let source = r#"
model Account:
  type_ [description="a", description="b"]: str
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected duplicate description key error");
    };
    assert!(
        err[0].message.contains("Duplicate 'description'"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn duplicate_rename_and_canonical_metadata_keys_are_refused() {
    for (metadata, key) in [
        ("renamed_from=\"email\", renamed_from=[\"address\"]", "renamed_from"),
        ("canonical=true, canonical=false", "canonical"),
    ] {
        let source = format!("model Account:\n  value [{metadata}]: str\n");
        let errors = parse_str_err(&source, "expected a duplicate field metadata key");
        assert!(
            errors[0].message.contains(&format!("Duplicate '{key}'")),
            "Unexpected error: {}",
            errors[0].message
        );
    }
}

#[test]
fn test_parse_unknown_metadata_key_error() {
    // RFC 021: Any other keys are compile-time errors
    let source = r#"
model Account:
  type_ [unknown="value"]: str
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected unknown metadata key error");
    };
    assert!(
        err[0].message.contains("Unknown field metadata key"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_parse_non_string_metadata_value_error() {
    let cases = [
        ("alias=123", "alias", "string literal"),
        ("description=true", "description", "string literal"),
        ("renamed_from=true", "renamed_from", "list of string literals"),
        ("renamed_from=[1]", "renamed_from", "list of string literals"),
        ("canonical=\"false\"", "canonical", "boolean literal"),
        ("canonical=0", "canonical", "boolean literal"),
    ];
    for (metadata, key, expected) in cases {
        let source = format!("model Account:\n  type_ [{metadata}]: str\n");
        let errors = parse_str_err(&source, "expected an invalid field metadata value");
        assert!(
            errors[0].message.contains(key),
            "Unexpected error: {}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains(expected),
            "Unexpected error: {}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains("type_"),
            "Unexpected error: {}",
            errors[0].message
        );
    }
}

#[test]
fn test_parse_model_with_traits() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Describable:
  def describe(self) -> str: ...

model User with Describable:
  name: str
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 2);
    match &program.declarations[1].node {
        Declaration::Model(m) => {
            assert_eq!(m.name, "User");
            assert_eq!(m.traits.len(), 1);
            assert_eq!(m.traits[0].node.name, "Describable");
        }
        _ => panic!("Expected model"),
    }
    Ok(())
}

#[test]
fn test_parse_model_with_multiple_traits() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A:
  def a(self) -> int: ...

trait B:
  def b(self) -> int: ...

model User with A, B:
  x: int
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 3);
    match &program.declarations[2].node {
        Declaration::Model(m) => {
            assert_eq!(m.name, "User");
            assert_eq!(m.traits.len(), 2);
            assert_eq!(m.traits[0].node.name, "A");
            assert_eq!(m.traits[1].node.name, "B");
        }
        _ => panic!("Expected model"),
    }
    Ok(())
}

#[test]
fn test_parse_model_with_generic_trait_adoption_named_from() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait From[T]:
  @classmethod
  def from(cls, value: T) -> Self: ...

model UserId with From[int]:
  value: int
"#;
    let program = parse_str(source)?;
    match &program.declarations[1].node {
        Declaration::Model(m) => {
            assert_eq!(m.name, "UserId");
            assert_eq!(m.traits.len(), 1);
            assert_eq!(m.traits[0].node.name, "From");
            assert_eq!(m.traits[0].node.type_args.len(), 1);
        }
        _ => panic!("Expected model"),
    }
    Ok(())
}

#[test]
fn test_enum_fat_arrow_mapping_rejected_with_hint() {
    let source = "enum Categories:\n    GROCERIES => Category(\"Groceries\")\n";
    let Err(err) = parse_str(source) else {
        panic!("Fat arrow in enum body should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("mapped values"),
        "Expected hint about mapped values, got: {msg}"
    );
}

#[test]
fn test_enum_dotted_variant_rejected_with_hint() {
    let source = "enum FlowType:\n    Cash.Inflow\n";
    let Err(err) = parse_str(source) else {
        panic!("Dotted variant in enum body should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("cannot contain dots"),
        "Expected hint about dots, got: {msg}"
    );
}

#[test]
fn test_enum_assigned_value_rejected_with_hint() {
    let source = "enum Color:\n    Red = 1\n";
    let Err(err) = parse_str(source) else {
        panic!("Assigned value in enum body should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("assigned values"),
        "Expected hint about assigned values, got: {msg}"
    );
}

#[test]
fn test_parse_value_enum_with_string_values() -> Result<(), Vec<CompileError>> {
    let source = "enum Color(str):\n    Red = \"red\"\n    Blue = \"blue\"\n    Cyan = alias Blue\n";
    let program = parse_str(source)?;
    let en = require_enum_decl(&program.declarations[0])?;

    assert!(matches!(
        en.value_type.as_ref().map(|ty| ty.node),
        Some(ValueEnumType::Str)
    ));
    assert_eq!(en.variants.len(), 2);
    assert_eq!(en.variant_aliases.len(), 1);
    assert_eq!(en.variants[0].node.name, "Red");
    assert!(en.variants[0].node.fields.is_empty());
    assert!(matches!(
        en.variants[0].node.value.as_ref().map(|value| &value.node),
        Some(ValueEnumLiteral::Str(value)) if value == "red"
    ));
    assert!(matches!(
        en.variants[1].node.value.as_ref().map(|value| &value.node),
        Some(ValueEnumLiteral::Str(value)) if value == "blue"
    ));
    assert_eq!(en.variant_aliases[0].node.name, "Cyan");
    assert_eq!(en.variant_aliases[0].node.target, "Blue");
    Ok(())
}

#[test]
fn test_parse_value_enum_with_integer_values() -> Result<(), Vec<CompileError>> {
    let source = "enum Status(int):\n    Pending = 1\n    Done = 2\n";
    let program = parse_str(source)?;
    let en = require_enum_decl(&program.declarations[0])?;

    assert!(matches!(
        en.value_type.as_ref().map(|ty| ty.node),
        Some(ValueEnumType::Int)
    ));
    assert_eq!(en.variants.len(), 2);
    assert!(matches!(
        en.variants[0].node.value.as_ref().map(|value| &value.node),
        Some(ValueEnumLiteral::Int(value)) if value.value == 1
    ));
    assert!(matches!(
        en.variants[1].node.value.as_ref().map(|value| &value.node),
        Some(ValueEnumLiteral::Int(value)) if value.value == 2
    ));
    Ok(())
}

#[test]
fn test_value_enum_variant_requires_explicit_value() {
    let source = "enum Color(str):\n    Red\n";
    let Err(err) = parse_str(source) else {
        panic!("Value enum variant without assigned value should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("explicit literal values"),
        "Expected hint about explicit literal values, got: {msg}"
    );
}

#[test]
fn test_value_enum_variant_payload_rejected() {
    let source = "enum Color(str):\n    Red(str) = \"red\"\n";
    let Err(err) = parse_str(source) else {
        panic!("Value enum variant payload should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("cannot carry tuple or struct payloads"),
        "Expected hint about value enum payloads, got: {msg}"
    );
}

#[test]
fn test_value_enum_variant_literal_type_must_match_header() {
    let source = "enum Color(str):\n    Red = 1\n";
    let Err(err) = parse_str(source) else {
        panic!("Value enum variant with wrong literal type should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("Expected string literal value"),
        "Expected hint about string literal values, got: {msg}"
    );
}

#[test]
fn test_value_enum_header_type_must_be_str_or_int() {
    let source = "enum Color(float):\n    Red = 1\n";
    let Err(err) = parse_str(source) else {
        panic!("Value enum with unsupported backing type should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("must be 'str' or 'int'"),
        "Expected hint about value enum backing types, got: {msg}"
    );
}

#[test]
fn test_enum_colon_annotation_rejected_with_hint() {
    let source = "enum Fields:\n    Name: str\n";
    let Err(err) = parse_str(source) else {
        panic!("Type annotation in enum body should be rejected");
    };
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("type annotations"),
        "Expected hint about type annotations, got: {msg}"
    );
}

#[test]
fn test_parse_enum_with_trait_adoption_and_method() -> Result<(), Vec<CompileError>> {
    let source = r#"
enum Lookup with Index[str, int]:
    Mapping
    Empty

    def __getitem__(self, key: str) -> int:
        return 0
"#;
    let program = parse_str(source)?;
    let en = require_enum_decl(&program.declarations[0])?;

    assert_eq!(en.name, "Lookup");
    assert_eq!(en.traits.len(), 1);
    assert_eq!(en.traits[0].node.name, "Index");
    assert_eq!(en.traits[0].node.type_args.len(), 2);
    assert_eq!(require_simple_type(&en.traits[0].node.type_args[0])?, "str");
    assert_eq!(require_simple_type(&en.traits[0].node.type_args[1])?, "int");
    assert_eq!(en.variants.len(), 2);
    assert_eq!(en.methods.len(), 1);
    assert_eq!(en.methods[0].node.name, "__getitem__");
    assert_eq!(en.methods[0].node.receiver, Some(Receiver::Immutable));
    assert_eq!(en.methods[0].node.params.len(), 1);
    assert!(en.methods[0].node.body.is_some());
    Ok(())
}

#[test]
fn test_parse_value_enum_with_trait_adoption_after_value_type() -> Result<(), Vec<CompileError>> {
    let source = r#"
enum Env(str) with From[str]:
    Dev = "development"
    Prod = "production"

    @classmethod
    def from(cls, value: str) -> Self:
        return Env.Dev
"#;
    let program = parse_str(source)?;
    let en = require_enum_decl(&program.declarations[0])?;

    assert!(matches!(
        en.value_type.as_ref().map(|ty| ty.node),
        Some(ValueEnumType::Str)
    ));
    assert_eq!(en.traits.len(), 1);
    assert_eq!(en.traits[0].node.name, "From");
    assert_eq!(en.traits[0].node.type_args.len(), 1);
    assert_eq!(require_simple_type(&en.traits[0].node.type_args[0])?, "str");
    assert_eq!(en.variants.len(), 2);
    assert_eq!(en.methods.len(), 1);
    assert_eq!(en.methods[0].node.name, "from");
    assert_eq!(en.methods[0].node.receiver, None);
    assert_eq!(en.methods[0].node.params.len(), 1);
    assert_eq!(en.methods[0].node.decorators.len(), 1);
    Ok(())
}

#[test]
fn test_valid_enum_still_parses() {
    let source = "enum Status:\n    Pending\n    Active\n    Done(str)\n";
    let Ok(program) = parse_str(source) else {
        panic!("Valid enum should parse");
    };
    assert_eq!(program.declarations.len(), 1);
    match &program.declarations[0].node {
        Declaration::Enum(e) => {
            assert!(e.value_type.is_none());
            assert_eq!(e.variants.len(), 3);
            assert_eq!(e.variants[0].node.name, "Pending");
            assert_eq!(e.variants[1].node.name, "Active");
            assert_eq!(e.variants[2].node.name, "Done");
            assert!(e.variants.iter().all(|variant| variant.node.value.is_none()));
            assert_eq!(e.variants[2].node.fields.len(), 1);
        }
        _ => panic!("Expected enum"),
    }
}

#[test]
fn test_newtype_still_parses_with_newtype_keyword() {
    // `type Foo = newtype Bar` must still produce a Newtype.
    let source = "type Foo = newtype Bar\n";
    let prog = match parse_str(source) {
        Ok(program) => program,
        Err(errs) => panic!("newtype should parse: {errs:?}"),
    };
    assert_eq!(prog.declarations.len(), 1);
    assert!(
        matches!(prog.declarations[0].node, Declaration::Newtype(_)),
        "Expected Newtype, got: {:?}",
        prog.declarations[0].node
    );
}
