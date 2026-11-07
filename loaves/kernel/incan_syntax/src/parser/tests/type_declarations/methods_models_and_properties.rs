//! Trait and method declarations, models, classes, docstrings, field metadata and computed properties.

use super::*;

#[test]
fn test_parse_trait_method_named_from() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait From[T]:
  @classmethod
  def from(cls, value: T) -> Self: ...
"#;
    let program = parse_str(source)?;
    let trait_decl = require_trait_decl(&program.declarations[0])?;
    assert_eq!(trait_decl.name, "From");
    assert_eq!(trait_decl.methods.len(), 1);
    assert_eq!(trait_decl.methods[0].node.name, "from");
    Ok(())
}

#[test]
fn receiverless_methods_accept_mutable_first_parameters_issue1884() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Counter:
  value: int

  @staticmethod
  def make(mut n: int) -> Counter:
    n += 1
    return Counter(value=n)

trait Factory:
  @staticmethod
  def make(mut n: int) -> int
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    let trait_decl = require_trait_decl(&program.declarations[1])?;

    for method in [&class.methods[0].node, &trait_decl.methods[0].node] {
        assert!(method.receiver.is_none());
        assert_eq!(method.params.len(), 1);
        assert!(method.params[0].node.is_mut);
        assert_eq!(method.params[0].node.name, "n");
    }
    Ok(())
}

#[test]
fn method_receiver_bindings_retain_the_exact_source_token() -> Result<(), Vec<CompileError>> {
    let source = r#"
class ReceiverExamples:
  def immutable(self) -> None: ...
  def mutable(mut self) -> None: ...

  @classmethod
  def create(cls) -> Self: ...
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    assert_eq!(class.methods.len(), 3);

    let immutable = class.methods[0].node.receiver_binding.as_ref().ok_or_else(|| {
        vec![CompileError::new(
            "parser test internal error: immutable receiver has no source binding".to_string(),
            class.methods[0].span,
        )]
    })?;
    let mutable = class.methods[1].node.receiver_binding.as_ref().ok_or_else(|| {
        vec![CompileError::new(
            "parser test internal error: mutable receiver has no source binding".to_string(),
            class.methods[1].span,
        )]
    })?;
    let class_receiver = class.methods[2].node.receiver_binding.as_ref().ok_or_else(|| {
        vec![CompileError::new(
            "parser test internal error: class receiver has no source binding".to_string(),
            class.methods[2].span,
        )]
    })?;

    assert_eq!(immutable.node, "self");
    assert_eq!(immutable.span, require_source_span(source, "self", 0)?);
    assert_eq!(mutable.node, "self");
    assert_eq!(mutable.span, require_source_span(source, "self", 1)?);
    assert_eq!(class_receiver.node, "cls");
    assert_eq!(class_receiver.span, require_source_span(source, "cls", 0)?);
    Ok(())
}

#[test]
fn test_parse_model() -> Result<(), Vec<CompileError>> {
    let source = r#"
model User:
  name: str
  age: int = 0
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 1);
    let m = require_model_decl(&program.declarations[0])?;
    assert_eq!(m.name, "User");
    assert_eq!(m.fields.len(), 2);
    assert!(m.traits.is_empty());
    Ok(())
}

#[test]
fn test_parse_generic_method_type_params() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Box:
  def get[T with Clone](self, value: T) -> T:
    return value
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    assert_eq!(class.methods.len(), 1);
    let method = &class.methods[0].node;
    assert_eq!(method.name, "get");
    assert_eq!(method.type_params.len(), 1);
    assert_eq!(method.type_params[0].name, "T");
    assert_eq!(method.type_params[0].bounds.len(), 1);
    assert_eq!(method.type_params[0].bounds[0].name, "Clone");
    Ok(())
}

#[test]
fn test_parse_trait_bodyless_method_is_abstract() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Serializer:
  def serialize(self, value: str) -> str
  def deserialize(self, data: str) -> str: ...
"#;
    let program = parse_str(source)?;
    let trait_decl = require_trait_decl(&program.declarations[0])?;
    assert_eq!(trait_decl.methods.len(), 2);
    assert_eq!(trait_decl.methods[0].node.name, "serialize");
    assert!(trait_decl.methods[0].node.body.is_none());
    assert_eq!(trait_decl.methods[1].node.name, "deserialize");
    assert!(trait_decl.methods[1].node.body.is_none());
    Ok(())
}

#[test]
fn test_parse_bodyless_methods_outside_traits_are_rejected() {
    for source in [
        "model User:\n  def name(self) -> str\n",
        "class User:\n  def name(self) -> str\n",
        "type UserId = newtype str:\n  def display(self) -> str\n",
        "enum Token:\n  Word\n  def text(self) -> str\n",
    ] {
        let errs = parse_str_err(source, "bodyless methods outside traits should fail");
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Expected ':' after method return type")),
            "expected method body diagnostic, got: {errs:?}"
        );
    }
}

#[test]
fn test_parse_bodyless_trait_method_followed_by_docstring_is_rejected() {
    let source = r#"
trait Named:
  def name(self) -> str
    "Return the display name."
"#;
    parse_str_err(source, "bodyless trait method docstring should require a colon body");
}

#[test]
fn test_parse_pub_class_preserves_authored_field_visibility() -> Result<(), Vec<CompileError>> {
    let source = r#"
pub class LazyFrame:
  _cursor: int
  pub schema: str
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    assert!(matches!(class.visibility, Visibility::Public));
    assert!(matches!(class.fields[0].node.visibility, Visibility::Private));
    assert!(matches!(class.fields[1].node.visibility, Visibility::Public));
    Ok(())
}

#[test]
fn test_parse_pub_model_uses_ordinary_field_visibility_issue884() -> Result<(), Vec<CompileError>> {
    let source = r#"
pub model SealedEnvelope:
  body: str
  pub envelope_id: str
"#;
    let program = parse_str(source)?;
    let model = require_model_decl(&program.declarations[0])?;
    assert!(matches!(model.visibility, Visibility::Public));
    assert!(matches!(model.fields[0].node.visibility, Visibility::Private));
    assert!(matches!(model.fields[1].node.visibility, Visibility::Public));
    Ok(())
}

#[test]
fn test_parse_pub_model_keeps_private_available_as_an_ordinary_field_name_issue884() -> Result<(), Vec<CompileError>> {
    let source = r#"
pub model Vocabulary:
  pub private [alias="private_value"]: str
"#;
    let program = parse_str(source)?;
    let model = require_model_decl(&program.declarations[0])?;
    assert_eq!(model.fields[0].node.name, "private");
    assert_eq!(model.fields[0].node.metadata.alias.as_deref(), Some("private_value"));
    assert!(matches!(model.fields[0].node.visibility, Visibility::Public));
    Ok(())
}

#[test]
fn test_parse_method_multiline_receiver_allows_trailing_comma() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Box:
  def get(
    self,
  ) -> int:
    return 1
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    assert_eq!(class.methods.len(), 1);
    let method = &class.methods[0].node;
    assert_eq!(method.name, "get");
    assert!(matches!(method.receiver, Some(Receiver::Immutable)));
    assert!(method.params.is_empty());
    Ok(())
}

#[test]
fn test_parse_class_docstring() -> Result<(), Vec<CompileError>> {
    let source = r#"
class FieldInfo:
  """
  Compiler-provided field metadata returned by __fields__().
  Instances are immutable and read-only.
  """
  name: str
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 1);
    let c = require_class_decl(&program.declarations[0])?;
    assert_eq!(c.name, "FieldInfo");
    assert_eq!(
        c.docstring.as_deref(),
        Some(
            "\n  Compiler-provided field metadata returned by __fields__().\n  Instances are immutable and read-only.\n  "
        )
    );
    assert_eq!(c.fields.len(), 1);
    assert_eq!(c.fields[0].node.name, "name");
    Ok(())
}

#[test]
fn test_parse_model_leading_docstring_stored_on_ast() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Widget:
  """Model-level narrative for tooling."""
  id: int
"#;
    let program = parse_str(source)?;
    let m = require_model_decl(&program.declarations[0])?;
    assert_eq!(m.docstring.as_deref(), Some("Model-level narrative for tooling."));
    assert_eq!(m.fields.len(), 1);
    assert_eq!(m.fields[0].node.name, "id");
    Ok(())
}

#[test]
fn test_parse_enum_leading_docstring_stored_on_ast() -> Result<(), Vec<CompileError>> {
    let source = r#"
enum Color:
  """Semantic colors for UI."""
  Red
  Green
"#;
    let program = parse_str(source)?;
    let en = require_enum_decl(&program.declarations[0])?;
    assert_eq!(en.docstring.as_deref(), Some("Semantic colors for UI."));
    assert_eq!(en.variants.len(), 2);
    assert_eq!(en.variants[0].node.name, "Red");
    assert_eq!(en.variants[1].node.name, "Green");
    Ok(())
}

#[test]
fn test_parse_model_field_metadata() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Account:
  type_ [alias="type", description="Account tier", renamed_from="kind", canonical=false]: str
  balance [description="Balance in cents", renamed_from=["amount", "cents"], canonical=true] as "balanceCents": int
"#;
    let program = parse_str(source)?;
    let model = require_model_decl(&program.declarations[0])?;
    let type_field = &model.fields[0].node;
    assert_eq!(type_field.metadata.alias.as_deref(), Some("type"));
    assert_eq!(type_field.metadata.description.as_deref(), Some("Account tier"));
    assert_eq!(type_field.metadata.renamed_from.len(), 1);
    assert_eq!(type_field.metadata.renamed_from[0].node, "kind");
    assert!(!type_field.metadata.renamed_from_was_list);
    assert_eq!(
        type_field.metadata.canonical.as_ref().map(|value| value.node),
        Some(false)
    );
    let balance_field = &model.fields[1].node;
    assert_eq!(balance_field.metadata.alias.as_deref(), Some("balanceCents"));
    assert_eq!(balance_field.metadata.description.as_deref(), Some("Balance in cents"));
    assert_eq!(
        balance_field
            .metadata
            .renamed_from
            .iter()
            .map(|value| value.node.as_str())
            .collect::<Vec<_>>(),
        vec!["amount", "cents"]
    );
    assert!(balance_field.metadata.renamed_from_was_list);
    assert_eq!(
        balance_field.metadata.canonical.as_ref().map(|value| value.node),
        Some(true)
    );
    Ok(())
}

#[test]
fn test_parse_model_field_alias_sugar() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Account:
  type_ as "type": str
"#;
    let program = parse_str(source)?;
    let model = match &program.declarations[0].node {
        Declaration::Model(m) => m,
        _ => panic!("Expected model"),
    };
    let field = &model.fields[0].node;
    assert_eq!(field.metadata.alias.as_deref(), Some("type"));
    assert_eq!(field.metadata.description, None);
    Ok(())
}

#[test]
fn test_parse_model_field_alias_and_as_error() {
    let source = r#"
model Account:
  type_ [alias="type"] as "type": str
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected alias + as sugar to be rejected");
    };
    assert!(
        err[0]
            .message
            .contains("Cannot combine 'alias=\"...\"' with 'as \"...\"'"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_parse_class_computed_property() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Dataset:
  property schema_fields -> list[str]:
    return self._fields
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[0])?;
    assert_eq!(class.properties.len(), 1);
    let property = &class.properties[0];
    assert_eq!(property.node.name, "schema_fields");
    assert!(matches!(property.node.visibility, Visibility::Private));
    assert!(
        matches!(property.node.return_type.node, Type::Generic(ref name, _) if collections::from_str(name) == Some(CollectionTypeId::List))
    );
    assert!(property.node.body.is_some());
    assert!(
        property.span.start < property.node.return_type.span.start
            && property.node.return_type.span.end <= property.span.end,
        "property span should enclose return type span"
    );
    Ok(())
}

#[test]
fn test_parse_model_pub_computed_property() -> Result<(), Vec<CompileError>> {
    let source = r#"
model User:
  name: str

  pub property display_name -> str:
    return self.name
"#;
    let program = parse_str(source)?;
    let model = require_model_decl(&program.declarations[0])?;
    assert_eq!(model.fields.len(), 1);
    assert_eq!(model.properties.len(), 1);
    let property = &model.properties[0].node;
    assert_eq!(property.name, "display_name");
    assert!(matches!(property.visibility, Visibility::Public));
    assert!(property.body.is_some());
    Ok(())
}

#[test]
fn test_parse_trait_abstract_computed_property() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait HasArea:
  property area -> float: ...
"#;
    let program = parse_str(source)?;
    let trait_decl = require_trait_decl(&program.declarations[0])?;
    assert_eq!(trait_decl.properties.len(), 1);
    let property = &trait_decl.properties[0].node;
    assert_eq!(property.name, "area");
    assert!(matches!(property.return_type.node, Type::Simple(ref name) if name == "float"));
    assert!(property.body.is_none());
    Ok(())
}

#[test]
fn test_parse_trait_abstract_computed_property_without_ellipsis() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait HasArea:
  property area -> float
"#;
    let program = parse_str(source)?;
    let trait_decl = require_trait_decl(&program.declarations[0])?;
    assert_eq!(trait_decl.properties.len(), 1);
    assert!(trait_decl.properties[0].node.body.is_none());
    Ok(())
}

#[test]
fn test_parse_class_abstract_property_is_focused_error() {
    let errs = parse_str_err(
        "class Shape:\n  property area -> float\n",
        "bodyless properties outside traits should fail",
    );
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Expected ':' after property return type")),
        "expected property body diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_parse_property_parameter_list_is_focused_error() {
    let errs = parse_str_err(
        "class Shape:\n  property area(self) -> float:\n    return 0.0\n",
        "property declarations with parameter lists should fail",
    );
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Computed properties do not accept parameter lists")),
        "expected property parameter diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_parse_async_before_property_is_a_reserved_word_error() {
    let errs = parse_str_err(
        "class Worker:\n  pub async property status -> str:\n    return \"ready\"\n",
        "async outside its declaration-modifier position should fail as a reserved word",
    );
    assert!(
        errs.iter()
            .any(|err| err.message.contains("`async` is a reserved word")),
        "expected reserved-word diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_parse_trait_with_docstring() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Debug:
    """Debug representation."""
    def __repr__(self) -> str: ...
"#;
    let program = parse_str(source)?;
    let tr = require_trait_decl(&program.declarations[0])?;
    assert_eq!(tr.name, "Debug");
    assert_eq!(tr.docstring.as_deref(), Some("Debug representation."));
    assert_eq!(tr.methods.len(), 1);
    assert_eq!(tr.methods[0].node.name, "__repr__");
    Ok(())
}
