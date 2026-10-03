//! Hover previews and declaration details: registry previews from checked type info, C binding previews and type
//! spellings, checked API metadata previews (callable rebounds, partials, model fields and methods, private field
//! visibility, value enums, inline metadata escaping), enum completion details, document symbol details, computed
//! property signatures and dotted C type display.

use super::{
    api_metadata_preview_at_offset, api_metadata_previews, c_binding_preview_at_offset, c_binding_previews,
    c_binding_type_spelling, enum_completion_detail, enum_variant_completion_detail, enum_variant_completion_label,
    find_property_symbol_info, format_partial_decl_signature, format_property_signature, format_type,
    lsp_document_symbol_name_and_detail, lsp_symbol_kind_for_decl, registry_preview_at_offset, registry_previews,
};
use incan_frontend::api_metadata::{CheckedApiMetadata, collect_checked_api_metadata};
use incan_frontend::ast::{Declaration, ParamKind, Span};
use incan_frontend::symbols::{CallableParam, ResolvedType, Symbol, SymbolKind, VariableInfo};
use incan_frontend::typechecker::{CBindingType, COutputMode, CResourceAccess};
use incan_frontend::{lexer, parser, typechecker};

#[test]
fn registry_hover_preview_consumes_checked_type_info() -> Result<(), String> {
    let source = r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
def normalize(value: str) -> str:
  return value
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;

    let previews = registry_previews(&checker.type_info().registry);
    assert_eq!(previews.len(), 1);
    let offset = source
        .find("@describe")
        .ok_or_else(|| "expected registry decorator".to_string())?;
    let preview =
        registry_preview_at_offset(&previews, offset).ok_or_else(|| "expected checked registry preview".to_string())?;
    assert!(preview.markdown.contains("*checked registry membership*"));
    assert!(preview.markdown.contains("Registry: `functions`"));
    assert!(preview.markdown.contains("FunctionId(\"normalize\")"));
    Ok(())
}

#[test]
fn checked_c_lsp_preview_consumes_a_source_derived_binding_descriptor() -> Result<(), String> {
    let source = r#"
from std.interop import BindingDeclaration, c

@c.binding(header="fixture.h", link=c.framework("FixtureKit"))
class Fixture extends BindingDeclaration:
    marker: str
"#;
    // The interop component's source is read from the checkout, not embedded: the stdlib is a directory
    // convention this package does not own a path into.
    let interop_path = oven_model::toolchain_layout::development_root().join("loaves/stdlib/interop/src/interop.incn");
    let interop_source = std::fs::read_to_string(&interop_path)
        .map_err(|error| format!("failed to read {}: {error}", interop_path.display()))?;
    let declaration_start = source
        .find("@c.binding")
        .ok_or_else(|| "expected binding declaration".to_string())?;
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let interop_tokens = lexer::lex(&interop_source).map_err(|errors| format!("interop lexer failed: {errors:?}"))?;
    let interop = parser::parse(&interop_tokens).map_err(|errors| format!("interop parser failed: {errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker
        .check_with_imports(&ast, &[("std.interop", &interop)])
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;

    let previews = c_binding_previews(&checker.type_info().c_abi, &["checked_c_lsp".to_string()]);
    let declaration_preview = c_binding_preview_at_offset(&previews, declaration_start)
        .ok_or_else(|| "expected checked binding declaration preview".to_string())?;
    assert!(declaration_preview.markdown.contains("*checked C binding*"));
    assert!(declaration_preview.markdown.contains("Identity: `sha256:"));
    assert!(declaration_preview.markdown.contains("Header: `fixture.h`"));
    assert!(
        declaration_preview
            .markdown
            .contains("Link: `c.framework(\"FixtureKit\")`")
    );
    assert_eq!(declaration_preview.span.start, declaration_start);
    Ok(())
}

#[test]
fn checked_c_lsp_type_spelling_preserves_output_and_borrowing_contracts() {
    let output = CBindingType::Output {
        mode: COutputMode::InOut,
        value: Box::new(CBindingType::Resource {
            access: CResourceAccess::BorrowedMut,
            resource: "Buffer".to_string(),
        }),
    };

    assert_eq!(c_binding_type_spelling(&output), "c.InOut[c.BorrowedMut[Buffer]]");
}

fn checked_metadata_for(source: &str) -> Result<(incan_frontend::ast::Program, CheckedApiMetadata), String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
    let metadata = collect_checked_api_metadata(&ast, &checker, vec!["lib".to_string()]);
    Ok((ast, metadata))
}

#[test]
fn checked_api_previews_preserve_source_signature_for_callable_rebound() -> Result<(), String> {
    let source = r#"
pub def endpoint() -> str:
    return "raw"
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;

    checker.symbols.define(Symbol {
        name: "endpoint".to_string(),
        kind: SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(
                vec![CallableParam::named("id", ResolvedType::Int, ParamKind::Normal)],
                Box::new(ResolvedType::Bool),
            ),
            is_mutable: false,
            is_used: false,
        }),
        span: Span::default(),
        scope: 0,
    });

    let metadata = collect_checked_api_metadata(&ast, &checker, vec!["lib".to_string()]);
    let previews = api_metadata_previews(&ast, &metadata);
    let function_offset = source
        .find("endpoint")
        .ok_or_else(|| "expected function name in fixture".to_string())?;
    let preview = api_metadata_preview_at_offset(&previews, function_offset)
        .ok_or_else(|| "expected checked function preview".to_string())?;

    assert!(
        preview.markdown.contains("pub def endpoint() -> str"),
        "expected source declaration signature in LSP preview, got:\n{}",
        preview.markdown
    );

    Ok(())
}

#[test]
fn checked_api_previews_include_public_partials() -> Result<(), String> {
    let source = r#"
pub def route(method: str, path: str = "/") -> str:
    return path

pub get = partial route(method="GET")
"#;
    let (ast, metadata) = checked_metadata_for(source)?;
    let previews = api_metadata_previews(&ast, &metadata);
    let partial_offset = source
        .find("get = partial")
        .ok_or_else(|| "expected partial declaration in fixture".to_string())?;
    let preview = api_metadata_preview_at_offset(&previews, partial_offset)
        .ok_or_else(|| "expected checked partial preview".to_string())?;

    assert!(
        preview.markdown.contains("*checked API metadata: public partial*"),
        "expected public partial metadata preview, got:\n{}",
        preview.markdown
    );
    assert!(
        preview
            .markdown
            .contains("pub get = partial route(method: str = ..., path: str = ...)"),
        "expected projected partial signature with defaulted param display, got:\n{}",
        preview.markdown
    );
    assert!(
        preview.markdown.contains("target: `route`") && preview.markdown.contains("presets: `method`"),
        "expected target and preset provenance, got:\n{}",
        preview.markdown
    );

    Ok(())
}

#[test]
fn local_partial_lsp_surfaces_completion_and_document_symbol_detail() -> Result<(), String> {
    let source = r#"
def route(method: str, path: str) -> str:
    return path

get = partial route(method="GET")
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let partial = ast
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Partial(partial) => Some(partial),
            _ => None,
        })
        .ok_or_else(|| "expected partial declaration".to_string())?;
    let partial_decl = ast
        .declarations
        .iter()
        .find(|decl| matches!(decl.node, Declaration::Partial(_)))
        .ok_or_else(|| "expected partial declaration span".to_string())?;

    assert_eq!(
        format_partial_decl_signature(partial, source),
        r#"get = partial route(method="GET")"#
    );
    assert_eq!(
        lsp_symbol_kind_for_decl(&partial_decl.node),
        Some(tower_lsp::lsp_types::SymbolKind::FUNCTION)
    );
    assert_eq!(
        lsp_document_symbol_name_and_detail(&partial_decl.node, source),
        Some(("get".to_string(), r#"get = partial route(method="GET")"#.to_string()))
    );

    Ok(())
}

#[test]
fn checked_api_previews_include_public_model_fields_and_methods() -> Result<(), String> {
    let source = r#"
pub const DEFAULT_LABEL = "none"

@derive(Clone)
pub model Order:
    """
    Order contract.
    """
    pub id [description="Stable id"] as "orderId": int
    pub label: str = DEFAULT_LABEL

    def label(self) -> str:
        """
        Return the display label.
        """
        return DEFAULT_LABEL
"#;
    let (ast, metadata) = checked_metadata_for(source)?;
    let previews = api_metadata_previews(&ast, &metadata);

    let field_offset = source
        .find("orderId")
        .ok_or_else(|| "expected field alias in fixture".to_string())?;
    let field_preview = api_metadata_preview_at_offset(&previews, field_offset)
        .ok_or_else(|| "expected checked field preview".to_string())?;
    assert!(
        field_preview.markdown.contains("*checked API metadata: public field*"),
        "expected public field metadata preview, got:\n{}",
        field_preview.markdown
    );
    assert!(
        field_preview.markdown.contains("alias: `orderId`"),
        "expected field alias in preview, got:\n{}",
        field_preview.markdown
    );
    assert!(
        field_preview.markdown.contains("description: `Stable id`"),
        "expected field description in preview, got:\n{}",
        field_preview.markdown
    );

    let method_offset = source
        .find("def label")
        .ok_or_else(|| "expected method in fixture".to_string())?;
    let method_preview = api_metadata_preview_at_offset(&previews, method_offset)
        .ok_or_else(|| "expected checked method preview".to_string())?;
    assert!(
        method_preview.markdown.contains("def Order.label(self) -> str"),
        "expected checked method signature, got:\n{}",
        method_preview.markdown
    );
    assert!(
        method_preview
            .markdown
            .contains("docstring: `Return the display label.`"),
        "expected method docstring in preview, got:\n{}",
        method_preview.markdown
    );

    Ok(())
}

#[test]
fn checked_api_previews_preserve_private_class_field_visibility_issue883() -> Result<(), String> {
    let source = r#"
pub class Vault:
    secret: str
    pub label: str
"#;
    let (ast, metadata) = checked_metadata_for(source)?;
    let previews = api_metadata_previews(&ast, &metadata);
    let secret_offset = source
        .find("secret")
        .ok_or_else(|| "expected private field in fixture".to_string())?;
    let secret_preview = api_metadata_preview_at_offset(&previews, secret_offset)
        .ok_or_else(|| "expected checked private field preview".to_string())?;
    assert!(
        secret_preview
            .markdown
            .contains("*checked API metadata: private field*"),
        "expected private field metadata preview, got:\n{}",
        secret_preview.markdown
    );
    assert!(
        secret_preview.markdown.contains("field Vault.secret: str"),
        "expected private field signature in preview, got:\n{}",
        secret_preview.markdown
    );
    Ok(())
}

#[test]
fn checked_api_previews_skip_private_declarations() -> Result<(), String> {
    let source = r#"
model Secret:
    value: int

pub model Public:
    pub value: int
"#;
    let (ast, metadata) = checked_metadata_for(source)?;
    let previews = api_metadata_previews(&ast, &metadata);

    let private_offset = source
        .find("Secret")
        .ok_or_else(|| "expected private model in fixture".to_string())?;
    assert!(
        api_metadata_preview_at_offset(&previews, private_offset).is_none(),
        "private declarations must not expose checked API metadata previews"
    );

    let public_offset = source
        .find("Public")
        .ok_or_else(|| "expected public model in fixture".to_string())?;
    assert!(
        api_metadata_preview_at_offset(&previews, public_offset).is_some(),
        "public declarations should expose checked API metadata previews"
    );

    Ok(())
}

#[test]
fn checked_api_previews_include_value_enum_backing_and_variant_values() -> Result<(), String> {
    let source = r#"
pub enum Environment(str):
    Dev = "development"
    Prod = "production"
"#;
    let (ast, metadata) = checked_metadata_for(source)?;
    let previews = api_metadata_previews(&ast, &metadata);

    let enum_offset = source
        .find("Environment")
        .ok_or_else(|| "expected enum name in fixture".to_string())?;
    let enum_preview = api_metadata_preview_at_offset(&previews, enum_offset)
        .ok_or_else(|| "expected checked enum preview".to_string())?;
    assert!(
        enum_preview.markdown.contains("value type: `str`"),
        "expected enum backing type in preview, got:\n{}",
        enum_preview.markdown
    );
    assert!(
        !enum_preview.markdown.contains("value type: `Str`"),
        "enum backing type should use Incan spelling, got:\n{}",
        enum_preview.markdown
    );

    let variant_offset = source
        .find("Prod =")
        .ok_or_else(|| "expected value enum variant in fixture".to_string())?;
    let variant_preview = api_metadata_preview_at_offset(&previews, variant_offset)
        .ok_or_else(|| "expected checked enum variant preview".to_string())?;
    assert!(
        variant_preview
            .markdown
            .contains("*checked API metadata: public enum variant*"),
        "expected enum variant metadata preview, got:\n{}",
        variant_preview.markdown
    );
    assert!(
        variant_preview.markdown.contains("value type: `str`"),
        "expected variant backing type in preview, got:\n{}",
        variant_preview.markdown
    );
    assert!(
        variant_preview.markdown.contains("raw value: `\"production\"`"),
        "expected variant raw value in preview, got:\n{}",
        variant_preview.markdown
    );

    Ok(())
}

#[test]
fn local_value_enum_completion_details_include_raw_values() -> Result<(), String> {
    let source = r#"
enum HttpStatus(int):
    Ok = 200
"#;
    let (ast, _metadata) = checked_metadata_for(source)?;
    let enum_decl = ast
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Enum(en) => Some(en),
            _ => None,
        })
        .ok_or_else(|| "expected enum declaration in fixture".to_string())?;
    let variant = enum_decl
        .variants
        .first()
        .ok_or_else(|| "expected enum variant in fixture".to_string())?;

    assert_eq!(enum_completion_detail(enum_decl), "enum HttpStatus(int)");
    assert_eq!(
        enum_variant_completion_detail(enum_decl, &variant.node),
        "variant HttpStatus.Ok: int = 200"
    );
    assert_eq!(enum_variant_completion_label(enum_decl, &variant.node), "HttpStatus.Ok");

    Ok(())
}

#[test]
fn checked_api_previews_escape_backticks_in_inline_metadata() -> Result<(), String> {
    let escaped = super::inline_code("Use `code` here.");
    assert_eq!(escaped, "`` Use `code` here. ``");
    Ok(())
}

#[test]
fn computed_property_hover_surfaces_owner_and_type() -> Result<(), String> {
    let source = r#"
model Account:
    cents: int

    property dollars -> int:
        return self.cents
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let model = ast
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Model(model) => Some(model),
            _ => None,
        })
        .ok_or_else(|| "expected model declaration".to_string())?;
    let property = model
        .properties
        .first()
        .ok_or_else(|| "expected computed property".to_string())?;

    assert_eq!(
        format_property_signature(&model.name, &property.node),
        "property Account.dollars -> int"
    );
    let offset = source
        .find("dollars")
        .ok_or_else(|| "expected property name in source".to_string())?;
    let info = find_property_symbol_info(&model.name, &model.properties, offset)
        .ok_or_else(|| "expected property hover symbol".to_string())?;
    assert_eq!(info.kind, "property");
    assert_eq!(info.detail, "property Account.dollars -> int");
    Ok(())
}

#[test]
fn type_display_preserves_dotted_c_type_spelling() -> Result<(), String> {
    let source = "type Handle = c.ConstPtr[c.i32]\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let alias = ast
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::TypeAlias(alias) => Some(alias),
            _ => None,
        })
        .ok_or_else(|| "expected type alias declaration".to_string())?;

    assert_eq!(format_type(&alias.target.node), "c.ConstPtr[c.i32]");
    Ok(())
}
