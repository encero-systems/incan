//! Markdown and signature rendering for checked API metadata.

use super::*;

/// Format a checked public function preview as hover markdown.
pub(super) fn api_function_markdown(function: &ApiFunction) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, function.docstring.as_deref());
    if !function.decorators.is_empty() {
        facts.push(format!("decorators: {}", function.decorators.len()));
    }
    checked_api_markdown(
        format!("pub {}", format_api_function_signature(function)),
        "public function",
        facts,
    )
}

/// Format a checked public partial callable preset preview as hover markdown.
pub(super) fn api_partial_markdown(partial: &ApiPartial) -> String {
    let mut facts = Vec::new();
    facts.push(format!("target: `{}`", partial.target_path.join("::")));
    facts.push(format!("target kind: `{:?}`", partial.target_kind));
    if !partial.presets.is_empty() {
        facts.push(format!(
            "presets: {}",
            partial
                .presets
                .iter()
                .map(|preset| inline_code(&preset.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    checked_api_markdown(
        format!(
            "pub {} = partial {}{}({})",
            partial.name,
            partial.target_path.join("::"),
            format_type_params(&partial.type_params),
            format_params(&partial.params)
        ),
        "public partial",
        facts,
    )
}

/// Format a checked public model preview as hover markdown.
pub(super) fn api_model_markdown(model: &ApiModel) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, model.docstring.as_deref());
    push_list_fact(&mut facts, "derives", &model.derives);
    push_list_fact(&mut facts, "traits", &model.traits);
    facts.push(format!("fields: {}", model.fields.len()));
    facts.push(format!("methods: {}", model.methods.len()));
    checked_api_markdown(
        format!("pub model {}{}", model.name, format_type_params(&model.type_params)),
        "public model",
        facts,
    )
}

/// Format a checked public class preview as hover markdown.
pub(super) fn api_class_markdown(class: &ApiClass) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, class.docstring.as_deref());
    if let Some(parent) = &class.extends {
        facts.push(format!("extends: `{parent}`"));
    }
    push_list_fact(&mut facts, "derives", &class.derives);
    push_list_fact(&mut facts, "traits", &class.traits);
    facts.push(format!("fields: {}", class.fields.len()));
    facts.push(format!("methods: {}", class.methods.len()));
    checked_api_markdown(
        format!("pub class {}{}", class.name, format_type_params(&class.type_params)),
        "public class",
        facts,
    )
}

/// Format a checked public trait preview as hover markdown.
pub(super) fn api_trait_markdown(trait_decl: &ApiTrait) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, trait_decl.docstring.as_deref());
    if !trait_decl.supertraits.is_empty() {
        facts.push(format!(
            "supertraits: {}",
            trait_decl
                .supertraits
                .iter()
                .map(format_type_bound)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    facts.push(format!("requirements: {}", trait_decl.requires.len()));
    facts.push(format!("methods: {}", trait_decl.methods.len()));
    checked_api_markdown(
        format!(
            "pub trait {}{}",
            trait_decl.name,
            format_type_params(&trait_decl.type_params)
        ),
        "public trait",
        facts,
    )
}

/// Format a checked public enum preview as hover markdown.
pub(super) fn api_enum_markdown(enum_decl: &ApiEnum) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, enum_decl.docstring.as_deref());
    push_list_fact(&mut facts, "derives", &enum_decl.derives);
    facts.push(format!("variants: {}", enum_decl.variants.len()));
    if let Some(value_type) = enum_decl.value_type {
        facts.push(format!("value type: `{}`", enum_value_type_display(value_type)));
    }
    checked_api_markdown(
        format!(
            "pub enum {}{}",
            enum_decl.name,
            format_type_params(&enum_decl.type_params)
        ),
        "public enum",
        facts,
    )
}

/// Format a checked public enum variant preview as hover markdown.
pub(super) fn api_enum_variant_markdown(
    owner: &str,
    value_type: Option<EnumValueTypeExport>,
    variant: &incan_frontend::api_metadata::ApiEnumVariant,
) -> String {
    let mut facts = Vec::new();
    if !variant.fields.is_empty() {
        facts.push(format!(
            "fields: {}",
            variant
                .fields
                .iter()
                .map(format_type_ref)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(value_type) = value_type {
        facts.push(format!("value type: `{}`", enum_value_type_display(value_type)));
    }
    if let Some(value) = &variant.value {
        facts.push(format!("raw value: `{}`", enum_value_display(value)));
    }
    checked_api_markdown(
        format!("variant {owner}.{}", variant.name),
        "public enum variant",
        facts,
    )
}

/// Format a checked public newtype or rusttype preview as hover markdown.
pub(super) fn api_newtype_markdown(newtype: &ApiNewtype) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, newtype.docstring.as_deref());
    facts.push(format!("underlying: `{}`", format_type_ref(&newtype.underlying)));
    facts.push(format!("methods: {}", newtype.methods.len()));
    let kind = if newtype.is_rusttype { "rusttype" } else { "newtype" };
    checked_api_markdown(
        format!(
            "pub {kind} {}{} = {}",
            newtype.name,
            format_type_params(&newtype.type_params),
            format_type_ref(&newtype.underlying)
        ),
        &format!("public {kind}"),
        facts,
    )
}

/// Format a checked public type-alias preview as hover markdown.
pub(super) fn api_type_alias_markdown(alias: &ApiTypeAlias) -> String {
    checked_api_markdown(
        format!(
            "pub type {}{} = {}",
            alias.name,
            format_type_params(&alias.type_alias.type_params),
            format_type_ref(&alias.type_alias.target)
        ),
        "public type alias",
        Vec::new(),
    )
}

/// Format a checked public const preview as hover markdown.
pub(super) fn api_const_markdown(konst: &ApiConst) -> String {
    let mut facts = Vec::new();
    if let Some(value) = &konst.value {
        facts.push(format!("safe value: `{value:?}`"));
    }
    checked_api_markdown(
        format!("pub const {}: {}", konst.name, format_type_ref(&konst.ty)),
        "public const",
        facts,
    )
}

/// Format a checked public static preview as hover markdown.
pub(super) fn api_static_markdown(static_decl: &ApiStatic) -> String {
    checked_api_markdown(
        format!("pub static {}: {}", static_decl.name, format_type_ref(&static_decl.ty)),
        "public static",
        Vec::new(),
    )
}

/// Format a checked field preview on a public model or class as hover markdown.
pub(super) fn api_field_markdown(owner: &str, field: &FieldExport) -> String {
    let mut facts = Vec::new();
    if field.has_default {
        facts.push("has default: `true`".to_string());
    }
    if let Some(alias) = &field.alias {
        facts.push(format!("alias: `{alias}`"));
    }
    if let Some(description) = &field.description {
        facts.push(format!("description: {}", inline_code(description)));
    }
    let kind = match field.visibility {
        MemberVisibilityExport::Private => "private field",
        MemberVisibilityExport::Public => "public field",
    };
    checked_api_markdown(
        format!("field {}.{}: {}", owner, field.name, format_type_ref(&field.ty)),
        kind,
        facts,
    )
}

/// Format a checked method preview as hover markdown, naming its recorded visibility (#2570).
pub(super) fn api_method_markdown(owner: &str, method: &ApiMethod) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, method.docstring.as_deref());
    if !method.decorators.is_empty() {
        facts.push(format!("decorators: {}", method.decorators.len()));
    }
    if !method.has_body {
        facts.push("body: abstract".to_string());
    }
    let kind = match method.visibility {
        MemberVisibilityExport::Private => "private method",
        MemberVisibilityExport::Public => "public method",
    };
    checked_api_markdown(format_api_method_signature(owner, method), kind, facts)
}

/// Wrap a checked metadata signature and fact list in LSP markdown.
pub(super) fn checked_api_markdown(signature: String, kind: &str, facts: Vec<String>) -> String {
    let mut markdown = format!("```incan\n{signature}\n```\n\n*checked API metadata: {kind}*");
    for fact in facts {
        markdown.push_str("\n\n");
        markdown.push_str(&fact);
    }
    markdown
}

/// Add a trimmed raw docstring fact when metadata carries one.
pub(super) fn push_docstring_fact(facts: &mut Vec<String>, docstring: Option<&str>) {
    if let Some(docstring) = docstring.map(str::trim).filter(|docstring| !docstring.is_empty()) {
        facts.push(format!("docstring: {}", inline_code(docstring)));
    }
}

/// Add a comma-separated list fact when the list is non-empty.
pub(super) fn push_list_fact(facts: &mut Vec<String>, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    facts.push(format!(
        "{label}: {}",
        values
            .iter()
            .map(|value| inline_code(value))
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

/// Format a checked API function signature.
pub(super) fn format_api_function_signature(function: &ApiFunction) -> String {
    let prefix = if function.is_async { "async def" } else { "def" };
    format!(
        "{prefix} {}{}({}) -> {}{}",
        function.name,
        format_type_params(&function.type_params),
        format_params(&function.params),
        format_type_ref(&function.return_type),
        format_result_source(function.ownership.as_ref(), &function.params)
    )
}

/// Format a checked API method signature with its owning type.
pub(super) fn format_api_method_signature(owner: &str, method: &ApiMethod) -> String {
    let prefix = if method.is_async { "async def" } else { "def" };
    let mut params = Vec::new();
    if let Some(receiver) = &method.receiver {
        params.push(match receiver {
            ReceiverExport::Immutable => "self".to_string(),
            ReceiverExport::Mutable => "mut self".to_string(),
        });
    }
    params.extend(method.params.iter().map(format_param));
    format!(
        "{prefix} {owner}.{}{}({}) -> {}{}",
        method.name,
        format_type_params(&method.type_params),
        params.join(", "),
        format_type_ref(&method.return_type),
        format_result_source(method.ownership.as_ref(), &method.params)
    )
}

/// Render a retained source from checked slots and field identities, never from stale parameter spelling.
pub(super) fn format_result_source(
    ownership: Option<&incan_frontend::library_manifest::OwnershipExport>,
    params: &[ParamExport],
) -> String {
    use incan_frontend::library_manifest::OwnershipExport;
    use incan_frontend::ownership_metadata::SourceSlot;
    let Some(OwnershipExport::Checked(contract)) = ownership else {
        return String::new();
    };
    let Some(source) = &contract.result_source else {
        return String::new();
    };
    let root = match source.slot {
        SourceSlot::Receiver => "self",
        SourceSlot::Parameter(slot) => match params.get(slot) {
            Some(parameter) => &parameter.name,
            None => return String::new(),
        },
    };
    let mut text = format!(" from {root}");
    for field in &source.fields {
        text.push('.');
        text.push_str(&field.declaration_name);
    }
    text
}

/// Format checked API type parameters in Incan source-like syntax.
pub(super) fn format_type_params(type_params: &[TypeParamExport]) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    format!(
        "[{}]",
        type_params
            .iter()
            .map(|param| {
                if param.bounds.is_empty() {
                    param.name.clone()
                } else {
                    format!(
                        "{} with {}",
                        param.name,
                        param
                            .bounds
                            .iter()
                            .map(format_type_bound)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Format a checked API type bound.
pub(super) fn format_type_bound(bound: &TypeBoundExport) -> String {
    if bound.type_args.is_empty() {
        return bound.name.clone();
    }
    format!(
        "{}[{}]",
        bound.name,
        bound
            .type_args
            .iter()
            .map(format_type_ref)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Format a checked API parameter list.
pub(super) fn format_params(params: &[ParamExport]) -> String {
    params.iter().map(format_param).collect::<Vec<_>>().join(", ")
}

/// Format one checked API parameter, spelling its `mut` marker when the function type carries one.
pub(super) fn format_param(param: &ParamExport) -> String {
    let prefix = match param.kind {
        ParamKindExport::Normal => "",
        ParamKindExport::RestPositional => "*",
        ParamKindExport::RestKeyword => "**",
    };
    let marker = if param.is_mut { "mut " } else { "" };
    let default = if param.has_default { " = ..." } else { "" };
    format!(
        "{prefix}{marker}{}: {}{default}",
        param.name,
        format_type_ref(&param.ty)
    )
}

/// Format a manifest-level type reference for concise hover display.
pub(super) fn format_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name, .. } => name.clone(),
        TypeRef::Applied { name, args, .. } => {
            format!(
                "{name}[{}]",
                args.iter().map(format_type_ref).collect::<Vec<_>>().join(", ")
            )
        }
        TypeRef::Function { params, return_type } => {
            format!(
                "({}) -> {}",
                params.iter().map(format_type_ref).collect::<Vec<_>>().join(", "),
                format_type_ref(return_type)
            )
        }
        TypeRef::TypeToken { inner } => format!("Type[{}]", format_type_ref(inner)),
        TypeRef::Tuple { elements } => {
            format!(
                "({})",
                elements.iter().map(format_type_ref).collect::<Vec<_>>().join(", ")
            )
        }
        TypeRef::TypeParam { name } => name.clone(),
        TypeRef::SelfType => "Self".to_string(),
        TypeRef::Ref { inner } => format!("ref {}", format_type_ref(inner)),
        TypeRef::MutParam { inner } => format!("mut {}", format_type_ref(inner)),
        TypeRef::Ownership { mode, inner } => format!("{}[{}]", mode.as_str(), format_type_ref(inner)),
        TypeRef::RustPath { path } => format!("rust::{path}"),
        TypeRef::NativeUnion(native) => native
            .members
            .iter()
            .map(format_type_ref)
            .collect::<Vec<_>>()
            .join(" | "),
        TypeRef::Unknown => "_".to_string(),
    }
}

/// Format checked value-enum backing metadata using Incan surface spellings.
pub(super) fn enum_value_type_display(value_type: EnumValueTypeExport) -> &'static str {
    match value_type {
        EnumValueTypeExport::Str => "str",
        EnumValueTypeExport::Int => "int",
    }
}

/// Format a checked value-enum raw value as it appears in source-like LSP metadata.
pub(super) fn enum_value_display(value: &EnumValueExport) -> String {
    match value {
        EnumValueExport::Str(value) => format!("{value:?}"),
        EnumValueExport::Int(value) => value.to_string(),
    }
}

/// Format parsed value-enum backing metadata using Incan surface spellings.
pub(super) fn ast_value_enum_type_display(value_type: incan_frontend::ast::ValueEnumType) -> &'static str {
    match value_type {
        incan_frontend::ast::ValueEnumType::Str => "str",
        incan_frontend::ast::ValueEnumType::Int => "int",
    }
}

/// Format a parsed value-enum literal as source-like LSP metadata.
pub(super) fn ast_value_enum_literal_display(value: &incan_frontend::ast::ValueEnumLiteral) -> String {
    match value {
        incan_frontend::ast::ValueEnumLiteral::Str(value) => format!("{value:?}"),
        incan_frontend::ast::ValueEnumLiteral::Int(value) => value.value.to_string(),
    }
}

/// Format local enum completion detail, including RFC 032 backing metadata when present.
pub(super) fn enum_completion_detail(en: &incan_frontend::ast::EnumDecl) -> String {
    match en.value_type.as_ref() {
        Some(value_type) => format!("enum {}({})", en.name, ast_value_enum_type_display(value_type.node)),
        None => format!("enum {}", en.name),
    }
}

/// Format local enum variant completion labels as qualified enum constructors.
pub(super) fn enum_variant_completion_label(
    en: &incan_frontend::ast::EnumDecl,
    variant: &incan_frontend::ast::VariantDecl,
) -> String {
    format!("{}.{}", en.name, variant.name)
}

/// Format local enum variant completion detail, including RFC 032 raw value metadata when present.
pub(super) fn enum_variant_completion_detail(
    en: &incan_frontend::ast::EnumDecl,
    variant: &incan_frontend::ast::VariantDecl,
) -> String {
    let mut detail = format!("variant {}.{}", en.name, variant.name);
    if let Some(value_type) = &en.value_type {
        detail.push_str(&format!(": {}", ast_value_enum_type_display(value_type.node)));
    }
    if let Some(value) = &variant.value {
        detail.push_str(&format!(" = {}", ast_value_enum_literal_display(&value.node)));
    }
    detail
}

/// Escape a short metadata value as inline markdown code.
pub(super) fn inline_code(value: &str) -> String {
    if !value.contains('`') {
        return format!("`{value}`");
    }
    let mut max_run = 0;
    let mut current_run = 0;
    for ch in value.chars() {
        if ch == '`' {
            current_run += 1;
            max_run = max_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    let fence = "`".repeat(max_run + 1);
    format!("{fence} {value} {fence}")
}
