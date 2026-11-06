//! Project the registered standard checked C syntax into the compiler's existing binding declaration contract.
//!
//! This is compiler-owned semantics, selected only for the standard namespace owner; dependency vocabulary still
//! requires its own expansion implementation. Registration itself remains ordinary Incan producer code.

use incan_lang::lang::c_abi::BINDING_DECLARATION_BASE;
use incan_lang::lang::stdlib::{STDLIB_INTEROP, STDLIB_ROOT};
use incan_vocab::{
    Decorator, DecoratorArg, DecoratorArgValue, IncanClassDeclaration, IncanDeclaration, IncanExpr,
    IncanFromImportDeclaration, IncanImportItem, IncanStatement, IncanVisibility, VocabBodyItem, VocabDeclaration,
};

/// Convert a checked C binding into the already checked `@c.binding` class representation.
/// Nested declarative members remain data for C ABI verification; they are never executed as host statements.
pub(super) fn binding(declaration: &VocabDeclaration) -> Result<Vec<IncanDeclaration>, String> {
    let Some(name) = &declaration.head.name else {
        return Err("binding declarations require one binding name".to_string());
    };
    if !declaration.head.header_args.is_empty() {
        return Err("binding declarations do not accept additional header arguments".to_string());
    }
    let decorators = descriptor_fields(&declaration.body)?;
    let members = declaration.body.iter().filter_map(|item| match item {
        VocabBodyItem::Declaration(member) => Some(member.clone()),
        _ => None,
    });
    Ok(vec![
        IncanDeclaration::FromImport(IncanFromImportDeclaration {
            visibility: IncanVisibility::Private,
            module: vec![STDLIB_ROOT.to_string(), STDLIB_INTEROP.to_string()],
            items: vec![IncanImportItem {
                name: BINDING_DECLARATION_BASE.to_string(),
                alias: None,
            }],
        }),
        IncanDeclaration::Class(IncanClassDeclaration {
            visibility: IncanVisibility::Private,
            name: name.clone(),
            decorators: vec![Decorator {
                path: vec!["c".to_string(), "binding".to_string()],
                args: decorators,
                span: declaration.span,
            }],
            extends: Some(BINDING_DECLARATION_BASE.to_string()),
            declarative_members: members.collect(),
        }),
    ])
}

/// Require one string header and one link expression, rejecting every executable member of a binding descriptor.
fn descriptor_fields(body: &[VocabBodyItem]) -> Result<Vec<DecoratorArg>, String> {
    let mut header = None;
    let mut link = None;
    for item in body {
        let (target, value) = match item {
            VocabBodyItem::Declaration(_) => continue,
            VocabBodyItem::Statement(IncanStatement::Let { name, value, .. }) => (name, value),
            VocabBodyItem::Statement(IncanStatement::Assign { target, value }) => (target, value),
            _ => return Err("binding declarations accept only `header` and `link` assignments".to_string()),
        };
        let slot = match target.as_str() {
            "header" => &mut header,
            "link" => &mut link,
            _ => return Err("binding declarations accept only `header` and `link` assignments".to_string()),
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("binding declaration repeats `{target}`"));
        }
    }
    let Some(IncanExpr::Str(header)) = header else {
        return Err("binding declarations require `header` to be a string literal".to_string());
    };
    let Some(link) = link else {
        return Err("binding declarations require `link`".to_string());
    };
    Ok(vec![
        DecoratorArg {
            name: Some("header".to_string()),
            value: DecoratorArgValue::Str(header),
        },
        DecoratorArg {
            name: Some("link".to_string()),
            value: DecoratorArgValue::Expr(link),
        },
    ])
}
