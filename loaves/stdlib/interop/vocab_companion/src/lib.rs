//! Standard-library vocabulary companion for checked C binding declarations.

use incan_vocab::{
    DeclarationSurface, DesugarError, DesugarOutput, IncanClassDeclaration, IncanDeclaration, IncanExpr,
    IncanFromImportDeclaration, IncanImportItem, LibraryManifest, VocabBodyItem, VocabDesugarer, VocabRegistration,
    VocabSyntaxNode,
};

/// Namespace that activates the checked C binding surface.
pub const NAMESPACE: &str = "std.interop";
/// Import-activated declaration keyword.
pub const BINDING_KEYWORD: &str = "binding";
/// Declarative non-executable foreign-symbol member keyword.
pub const SYMBOL_KEYWORD: &str = "symbol";
/// Declarative opaque C resource member keyword.
pub const RESOURCE_KEYWORD: &str = "resource";
/// Declarative C enum member keyword.
pub const ENUM_KEYWORD: &str = "enum";
/// Declarative plain C structure member keyword.
pub const STRUCT_KEYWORD: &str = "struct";
/// Declarative raw-call outcome keyword nested under one C symbol.
pub const OUTCOME_KEYWORD: &str = "outcome";

/// Return the checked C binding vocabulary registration.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import(NAMESPACE).with_declaration(
                DeclarationSurface::named(BINDING_KEYWORD)
                    .with_header_args()
                    .with_statement_body()
                    .desugars_to_declarations()
                    .with_declaration(
                        DeclarationSurface::named(SYMBOL_KEYWORD)
                            .with_signature_head()
                            .with_statement_body()
                            .with_declaration(
                                DeclarationSurface::named(OUTCOME_KEYWORD)
                                    .with_header_args()
                                    .with_statement_body(),
                            ),
                    )
                    .with_declaration(
                        DeclarationSurface::named(RESOURCE_KEYWORD)
                            .with_header_args()
                            .with_statement_body(),
                    )
                    .with_declaration(
                        DeclarationSurface::named(ENUM_KEYWORD)
                            .with_header_args()
                            .with_statement_body(),
                    )
                    .with_declaration(
                        DeclarationSurface::named(STRUCT_KEYWORD)
                            .with_header_args()
                            .with_statement_body(),
                    ),
            ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(InteropBindingDesugarer)
}

/// Generic declaration desugarer for the checked C binding surface.
#[derive(Default)]
pub struct InteropBindingDesugarer;

impl VocabDesugarer for InteropBindingDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let VocabSyntaxNode::Declaration(declaration) = node else {
            return Err(DesugarError::new("binding vocabulary expected a declaration node"));
        };
        let Some(name) = declaration.head.name.as_ref() else {
            return Err(DesugarError::new("binding declarations require one binding name"));
        };
        if !declaration.head.header_args.is_empty() {
            return Err(DesugarError::new(
                "binding declarations do not accept additional header arguments",
            ));
        }
        let decorator_args = binding_decorator_args(&declaration.body)?;
        let declarative_members = declaration
            .body
            .iter()
            .filter_map(|item| match item {
                VocabBodyItem::Declaration(member) => Some(member.clone()),
                _ => None,
            })
            .collect();

        Ok(DesugarOutput::Declarations(vec![
            IncanDeclaration::FromImport(IncanFromImportDeclaration {
                visibility: incan_vocab::IncanVisibility::Private,
                module: vec!["std".to_string(), "interop".to_string()],
                items: vec![IncanImportItem {
                    name: "BindingDeclaration".to_string(),
                    alias: None,
                }],
            }),
            IncanDeclaration::Class(IncanClassDeclaration {
                visibility: incan_vocab::IncanVisibility::Private,
                name: name.clone(),
                decorators: vec![incan_vocab::Decorator {
                    path: vec!["c".to_string(), "binding".to_string()],
                    args: decorator_args,
                    span: declaration.span,
                }],
                extends: Some("BindingDeclaration".to_string()),
                declarative_members,
            }),
        ]))
    }
}

/// Extract the C binding descriptor fields from the generic binding declaration body.
fn binding_decorator_args(body: &[VocabBodyItem]) -> Result<Vec<incan_vocab::DecoratorArg>, DesugarError> {
    let mut header = None;
    let mut link = None;

    for item in body {
        if matches!(item, VocabBodyItem::Declaration(_)) {
            continue;
        }
        let (target, value) = match item {
            VocabBodyItem::Statement(incan_vocab::IncanStatement::Let { name, value, .. }) => (name, value),
            VocabBodyItem::Statement(incan_vocab::IncanStatement::Assign { target, value }) => (target, value),
            _ => {
                return Err(DesugarError::new(
                    "binding declarations accept only `header` and `link` assignments",
                ));
            }
        };
        let slot = match target.as_str() {
            "header" => &mut header,
            "link" => &mut link,
            _ => {
                return Err(DesugarError::new(
                    "binding declarations accept only `header` and `link` assignments",
                ));
            }
        };
        if slot.replace(value.clone()).is_some() {
            return Err(DesugarError::new(format!("binding declaration repeats `{target}`")));
        }
    }

    let Some(IncanExpr::Str(header)) = header else {
        return Err(DesugarError::new(
            "binding declarations require `header` to be a string literal",
        ));
    };
    let Some(link) = link else {
        return Err(DesugarError::new("binding declarations require `link`"));
    };

    Ok(vec![
        incan_vocab::DecoratorArg {
            name: Some("header".to_string()),
            value: incan_vocab::DecoratorArgValue::Str(header),
        },
        incan_vocab::DecoratorArg {
            name: Some("link".to_string()),
            value: incan_vocab::DecoratorArgValue::Expr(link),
        },
    ])
}

incan_vocab::export_wasm_desugarer!(InteropBindingDesugarer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_c_surface_contains_checked_resource_members() {
        let registration = library_vocab();
        let binding = &registration.surfaces()[0].declarations[0];
        let names = binding
            .declarations
            .iter()
            .map(|declaration| declaration.keyword.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, [SYMBOL_KEYWORD, RESOURCE_KEYWORD, ENUM_KEYWORD, STRUCT_KEYWORD]);
        assert_eq!(binding.declarations[0].declarations[0].keyword, OUTCOME_KEYWORD);
    }
}
