//! Native constant evaluation for typed immutable values planned in Incan (RFC 173, #2519).
//!
//! This adapter validates physical carriers and constructs Rust AST constants. Field selection, stored identities,
//! structural spelling, traversal policy and declaration closure belong to Incan lowering, never this module.

use crate::ast_build::{expression, global_path, ident};
use crate::error::PlanError;
use crate::plan::{CalleeKind, Constant, FrozenValue, Plan, PlanType, TerminatorKind};
use rustc_ast as ast;
use rustc_span::{Span, Symbol};
use std::collections::BTreeMap;
use thin_vec::{ThinVec, thin_vec};

/// Refuse malformed constants before native declaration injection.
fn invalid(reason: impl Into<String>) -> PlanError {
    PlanError::Invalid {
        function: "static value".into(),
        reason: reason.into(),
    }
}

/// Validate one generated accessor's namespace, output layout and entire immutable value tree.
pub fn validate(plan: &Plan, name: &str, ty: &PlanType, value: &FrozenValue) -> Result<(), PlanError> {
    if !name.starts_with("__incan_static_")
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || plan.functions.iter().any(|function| function.name == name)
    {
        return Err(invalid("static accessor has an invalid or colliding native name"));
    }
    validate_value(plan, ty, value)
}

/// Check the typed constant before rustc starts. This stage cannot intern symbols or construct AST paths:
/// validation runs outside rustc's session globals, while native item injection owns AST construction.
fn validate_value(plan: &Plan, ty: &PlanType, value: &FrozenValue) -> Result<(), PlanError> {
    match (ty, value) {
        (PlanType::Bool, FrozenValue::Scalar(Constant::Bool(_)))
        | (PlanType::Int, FrozenValue::Scalar(Constant::Int(_)))
        | (PlanType::StaticText | PlanType::FrozenStr, FrozenValue::Scalar(Constant::Text(_))) => Ok(()),
        (PlanType::StaticBytes | PlanType::FrozenBytes, FrozenValue::Bytes(values)) => {
            for value in values {
                u8::try_from(*value).map_err(|_| invalid("static byte is outside u8"))?;
            }
            Ok(())
        }
        (PlanType::FrozenCollection(name, types), FrozenValue::Collection(selected, values)) if ty == selected => {
            let width = match name.as_str() {
                "FrozenList" | "FrozenSet" if types.len() == 1 => 1,
                "FrozenDict" if types.len() == 2 => 2,
                _ => return Err(invalid("static collection has an invalid kind or type arity")),
            };
            if !values.len().is_multiple_of(width) {
                return Err(invalid("static collection has an incomplete entry"));
            }
            for entry in values.chunks(width) {
                for (value, ty) in entry.iter().zip(types) {
                    validate_value(plan, ty, value)?;
                }
            }
            Ok(())
        }
        (PlanType::Model(owner, name), FrozenValue::Model(selected, values)) if owner == selected => {
            let model = usize::try_from(*owner)
                .ok()
                .and_then(|index| plan.models.get(index))
                .ok_or_else(|| invalid("static model owner is missing"))?;
            if model.name != *name || model.fields.len() != values.len() || !model.unavailable_native_fields.is_empty()
            {
                return Err(invalid("static model does not match its checked storage"));
            }
            for (field, value) in model.fields.iter().zip(values) {
                validate_value(plan, &field.ty, value)?;
            }
            Ok(())
        }
        (PlanType::Enum(owner, name), FrozenValue::Enum(selected, variant, values)) if owner == selected => {
            let declaration = usize::try_from(*owner)
                .ok()
                .and_then(|index| plan.enums.get(index))
                .ok_or_else(|| invalid("static enum owner is missing"))?;
            let variant = usize::try_from(*variant)
                .ok()
                .and_then(|index| declaration.variants.get(index))
                .ok_or_else(|| invalid("static enum variant is missing"))?;
            if declaration.name != *name || variant.fields.len() != values.len() {
                return Err(invalid("static enum does not match its checked storage"));
            }
            for (ty, value) in variant.fields.iter().zip(values) {
                validate_value(plan, ty, value)?;
            }
            Ok(())
        }
        _ => Err(invalid("static value does not match its immutable declared carrier")),
    }
}

/// Declare each reached constant accessor once; repeated names must describe exactly the same typed value.
pub fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut seen = BTreeMap::new();
    let mut items = Vec::new();
    for block in plan.functions.iter().flat_map(|function| &function.blocks) {
        let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
            continue;
        };
        let CalleeKind::StaticValue(name, ty, value) = &callee.kind else {
            continue;
        };
        validate(plan, name, ty, value)?;
        if let Some((earlier_type, earlier_value)) = seen.insert(name, (ty, value)) {
            if earlier_type != ty || earlier_value != value {
                return Err(invalid("static accessor name describes two different constants"));
            }
            continue;
        }
        let value = initializer(plan, ty, value, &mut items, name, span)?;
        items.push(crate::field_metadata::accessor_item(name, ty, value, span));
    }
    Ok(items)
}

/// Construct an exact literal node; the surrounding checked carrier supplies its native numeric type.
fn literal(kind: ast::token::LitKind, text: &str, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Lit(ast::token::Lit::new(kind, Symbol::intern(text), None)),
        span,
    )
}

/// Construct a positional call from an already selected native path, without parsing source strings.
fn call(path: ast::Path, values: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(expression(ast::ExprKind::Path(None, path), span), values),
        span,
    )
}

/// Take an immutable static slice; constant evaluation owns its lifetime and concrete enum layout.
fn slice(values: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::AddrOf(
            ast::BorrowKind::Ref,
            ast::Mutability::Not,
            expression(ast::ExprKind::Array(values), span),
        ),
        span,
    )
}

/// Give every frozen collection explicit static storage, including nested model fields. Const-function calls
/// alone do not promote borrowed aggregate temporaries; each native slice must own a static initializer.
fn slice_storage(name: &str, element: Box<ast::Ty>, values: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Item> {
    let ty = Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: Box::new(ast::Ty {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::TyKind::Slice(element),
                    span,
                    tokens: None,
                }),
                mutbl: ast::Mutability::Not,
            },
        ),
        span,
        tokens: None,
    });
    crate::declarations::item(
        ast::ItemKind::Static(Box::new(ast::StaticItem {
            ident: ident(name, span),
            ty,
            safety: ast::Safety::Default,
            mutability: ast::Mutability::Not,
            expr: Some(slice(values, span)),
            define_opaque: None,
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Name a planned local alias; its existing declaration carries any linked provider path and generic arguments.
fn nominal_path(name: &str, span: Span) -> ast::Path {
    let mut path = ast::Path::from_ident(ident("crate", span));
    path.segments.push(ast::PathSegment::from_ident(ident(name, span)));
    path
}

/// Validate and materialize one immutable value according to the declared type, never its display name.
fn initializer(
    plan: &Plan,
    ty: &PlanType,
    value: &FrozenValue,
    storage: &mut Vec<Box<ast::Item>>,
    prefix: &str,
    span: Span,
) -> Result<Box<ast::Expr>, PlanError> {
    match (ty, value) {
        (PlanType::Bool, FrozenValue::Scalar(Constant::Bool(value))) => Ok(literal(
            ast::token::LitKind::Bool,
            if *value { "true" } else { "false" },
            span,
        )),
        (PlanType::Int, FrozenValue::Scalar(Constant::Int(value))) => {
            let magnitude = literal(ast::token::LitKind::Integer, &value.unsigned_abs().to_string(), span);
            Ok(if *value < 0 {
                expression(ast::ExprKind::Unary(ast::UnOp::Neg, magnitude), span)
            } else {
                magnitude
            })
        }
        (PlanType::StaticText | PlanType::FrozenStr, FrozenValue::Scalar(Constant::Text(value))) => {
            let text = literal(ast::token::LitKind::Str, &value.escape_default().to_string(), span);
            Ok(if *ty == PlanType::FrozenStr {
                call(
                    global_path(&["incan_std_primitives", "frozen", "FrozenStr", "new"], span),
                    thin_vec![text],
                    span,
                )
            } else {
                text
            })
        }
        (PlanType::StaticBytes | PlanType::FrozenBytes, FrozenValue::Bytes(values)) => {
            let mut escaped = String::new();
            for byte in values {
                let byte = u8::try_from(*byte).map_err(|_| invalid("static byte is outside u8"))?;
                use std::fmt::Write;
                write!(escaped, "\\x{byte:02x}").map_err(|_| invalid("static byte formatting failed"))?;
            }
            let bytes = literal(ast::token::LitKind::ByteStr, &escaped, span);
            Ok(if *ty == PlanType::FrozenBytes {
                call(
                    global_path(&["incan_std_primitives", "frozen", "FrozenBytes", "new"], span),
                    thin_vec![bytes],
                    span,
                )
            } else {
                bytes
            })
        }
        (PlanType::FrozenCollection(name, types), FrozenValue::Collection(selected, values)) if ty == selected => {
            let width = match name.as_str() {
                "FrozenList" | "FrozenSet" if types.len() == 1 => 1,
                "FrozenDict" if types.len() == 2 => 2,
                _ => return Err(invalid("static collection has an invalid kind or type arity")),
            };
            if !values.len().is_multiple_of(width) {
                return Err(invalid("static collection has an incomplete entry"));
            }
            let mut entries = ThinVec::new();
            for entry in values.chunks(width) {
                let elements = entry
                    .iter()
                    .zip(types)
                    .map(|(value, ty)| initializer(plan, ty, value, storage, prefix, span))
                    .collect::<Result<ThinVec<_>, _>>()?;
                if width == 1 {
                    entries.extend(elements);
                } else {
                    entries.push(expression(ast::ExprKind::Tup(elements), span));
                }
            }
            let element = if width == 1 {
                crate::declarations::ty(&types[0], span)
            } else {
                Box::new(ast::Ty {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::TyKind::Tup(types.iter().map(|ty| crate::declarations::ty(ty, span)).collect()),
                    span,
                    tokens: None,
                })
            };
            let storage_name = format!("{prefix}_data_{}", storage.len());
            storage.push(slice_storage(&storage_name, element, entries, span));
            Ok(call(
                global_path(&["incan_std_primitives", "frozen", name, "new"], span),
                thin_vec![crate::ast_build::local(&storage_name, span)],
                span,
            ))
        }
        (PlanType::Model(owner, name), FrozenValue::Model(selected, values)) if owner == selected => {
            let model = usize::try_from(*owner)
                .ok()
                .and_then(|index| plan.models.get(index))
                .ok_or_else(|| invalid("static model owner is missing"))?;
            if model.name != *name || model.fields.len() != values.len() || !model.unavailable_native_fields.is_empty()
            {
                return Err(invalid("static model does not match its checked storage"));
            }
            let path = nominal_path(name, span);
            if model.fields.len() == 1 && model.fields[0].name == "0" {
                return Ok(call(
                    path,
                    thin_vec![initializer(
                        plan,
                        &model.fields[0].ty,
                        &values[0],
                        storage,
                        prefix,
                        span
                    )?],
                    span,
                ));
            }
            let fields = model
                .fields
                .iter()
                .zip(values)
                .map(|(field, value)| {
                    Ok(ast::ExprField {
                        attrs: ThinVec::new(),
                        id: ast::DUMMY_NODE_ID,
                        span,
                        ident: ident(&field.name, span),
                        expr: initializer(plan, &field.ty, value, storage, prefix, span)?,
                        is_shorthand: false,
                        is_placeholder: false,
                    })
                })
                .collect::<Result<ThinVec<_>, PlanError>>()?;
            Ok(expression(
                ast::ExprKind::Struct(Box::new(ast::StructExpr {
                    qself: None,
                    path,
                    fields,
                    rest: ast::StructRest::None,
                })),
                span,
            ))
        }
        (PlanType::Enum(owner, name), FrozenValue::Enum(selected, variant, values)) if owner == selected => {
            let declaration = usize::try_from(*owner)
                .ok()
                .and_then(|index| plan.enums.get(index))
                .ok_or_else(|| invalid("static enum owner is missing"))?;
            let variant = usize::try_from(*variant)
                .ok()
                .and_then(|index| declaration.variants.get(index))
                .ok_or_else(|| invalid("static enum variant is missing"))?;
            if declaration.name != *name || variant.fields.len() != values.len() {
                return Err(invalid("static enum does not match its checked storage"));
            }
            let mut path = nominal_path(name, span);
            path.segments
                .push(ast::PathSegment::from_ident(ident(&variant.name, span)));
            if values.is_empty() {
                Ok(expression(ast::ExprKind::Path(None, path), span))
            } else {
                let values = values
                    .iter()
                    .zip(&variant.fields)
                    .map(|(value, ty)| initializer(plan, ty, value, storage, prefix, span))
                    .collect::<Result<ThinVec<_>, _>>()?;
                Ok(call(path, values, span))
            }
        }
        _ => Err(invalid("static value does not match its immutable declared carrier")),
    }
}
