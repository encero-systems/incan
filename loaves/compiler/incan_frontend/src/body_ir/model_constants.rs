//! Retain checked, effect-free model constant trees as per-read Body IR values.

use std::collections::HashMap;

use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind};

use super::{BodyBuilder, LocalNominalDeclarations, collect, parameter_types};
use crate::{ast, typechecker::TypeCheckInfo};

/// A checked constant tree with explicit storage types; it never represents a lazy static initializer.
#[derive(Clone)]
pub(super) struct ModelConstant {
    ty: IncanType,
    value: ModelConstantValue,
}

/// Only literal leaves and source-local model construction are admitted; unsupported const expressions stay absent.
#[derive(Clone)]
enum ModelConstantValue {
    Literal(bir::Constant),
    Model(Box<bir::ConstructorTarget>, Vec<ModelConstant>),
}

/// Collect validated model constants by declaration identity, leaving imported and unsupported initializers refused.
pub(super) fn collect(
    program: &ast::Program,
    facts: &TypeCheckInfo,
    models: &LocalNominalDeclarations,
) -> HashMap<CanonicalSymbolId, ModelConstant> {
    let sources = program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Const(item) = &declaration.node else {
                return None;
            };
            let canonical = facts
                .declarations
                .declaration_identities
                .get(&(declaration.span.start, declaration.span.end))?;
            Some((item.name.as_str(), (canonical, item)))
        })
        .collect::<HashMap<_, _>>();
    sources
        .iter()
        .filter_map(|(name, (canonical, _))| {
            let value = decode_named(name, &sources, facts, models, &mut Vec::new())?;
            matches!(value.value, ModelConstantValue::Model(..)).then(|| ((*canonical).clone(), value))
        })
        .collect()
}

/// Reuse the checker's successful const validation and cycle rejection, with an independent defensive cycle guard.
fn decode_named(
    name: &str,
    sources: &HashMap<&str, (&CanonicalSymbolId, &ast::ConstDecl)>,
    facts: &TypeCheckInfo,
    models: &LocalNominalDeclarations,
    active: &mut Vec<String>,
) -> Option<ModelConstant> {
    if active.iter().any(|entry| entry == name) || !facts.consts.const_kinds.contains_key(name) {
        return None;
    }
    let (_, source) = sources.get(name)?;
    let ty = parameter_types::checked_semantic_type(facts.expr_type(source.value.span)?, facts);
    active.push(name.to_string());
    let value = decode(&source.value.node, &ty, sources, facts, models, active);
    active.pop();
    value
}

/// Decode only checked literal trees and complete named model fields, whose order comes from the retained layout.
fn decode(
    expression: &ast::Expr,
    ty: &IncanType,
    sources: &HashMap<&str, (&CanonicalSymbolId, &ast::ConstDecl)>,
    facts: &TypeCheckInfo,
    models: &LocalNominalDeclarations,
    active: &mut Vec<String>,
) -> Option<ModelConstant> {
    // An owned string construction is not a Rust const value; do not turn a legacy build failure into execution.
    if *ty == IncanType::Primitive(IncanPrimitiveType::Str) {
        return None;
    }
    if let Some(value) = collect::frozen_literal_constant(expression, ty) {
        return Some(ModelConstant {
            ty: ty.clone(),
            value: ModelConstantValue::Literal(value),
        });
    }
    if let ast::Expr::Ident(name) = expression {
        let value = decode_named(name, sources, facts, models, active)?;
        return (value.ty == *ty).then_some(value);
    }
    let (name, arguments) = match expression {
        ast::Expr::Call(callee, type_args, arguments) if type_args.is_empty() => {
            let ast::Expr::Ident(name) = &callee.node else {
                return None;
            };
            (name, arguments)
        }
        ast::Expr::Constructor(name, arguments) => (name, arguments),
        _ => return None,
    };
    let model = models.get(name)?;
    if model.canonical.kind != SemanticSourceTargetKind::Model
        || !model.type_parameters.is_empty()
        || arguments.len() != model.fields.len()
    {
        return None;
    }
    let expected = parameter_types::checked_semantic_type(&crate::symbols::ResolvedType::Named(name.clone()), facts);
    if *ty != expected {
        return None;
    }
    let mut fields = Vec::with_capacity(model.fields.len());
    for (field, field_ty) in model.fields.iter().zip(&model.field_types) {
        let mut matches = arguments.iter().filter_map(|argument| match argument {
            ast::CallArg::Named(name, value) if name.node == *field => Some(value),
            _ => None,
        });
        let value = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        fields.push(decode(&value.node, field_ty, sources, facts, models, active)?);
    }
    let target = bir::ConstructorTarget {
        name: model.name.clone(),
        canonical: Some(model.canonical.clone()),
        direct_declaration_id: Some(model.direct_declaration_id.clone()),
        canonical_field_layout: Some(model.fields.clone()),
        binding: bir::ArgumentBinding::resolved_positional(fields.len()),
    };
    Some(ModelConstant {
        ty: ty.clone(),
        value: ModelConstantValue::Model(Box::new(target), fields),
    })
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Materialize a retained model constant at each read; canonical identity prevents local-shadow substitution.
    pub(super) fn model_constant_operand(
        &mut self,
        place: &bir::Place,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let values = self.model_constants;
        let value = values.get(&place.global()?.identity)?;
        Some(self.materialize_model_constant(value, scope, span, out))
    }

    /// Emit existing typed aggregates and literal operands, with no user calls or global storage reads.
    fn materialize_model_constant(
        &mut self,
        value: &ModelConstant,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        match &value.value {
            ModelConstantValue::Literal(literal) => {
                self.checked_constant_operand(literal.clone(), value.ty.clone(), scope, span, out)
            }
            ModelConstantValue::Model(target, fields) => {
                let fields = fields
                    .iter()
                    .map(|field| bir::ArgumentElement::One(self.materialize_model_constant(field, scope, span, out)))
                    .collect();
                self.push_assign_temp(
                    bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target.clone()), fields),
                    value.ty.clone(),
                    scope,
                    span,
                    out,
                )
            }
        }
    }
}
