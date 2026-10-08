//! Preserve concrete SDK task carriers through checked collection and binding relationships without changing
//! legacy expression or symbol typing.

use super::TypeChecker;
use crate::ast::{Expr, ListEntry, Pattern, Span, Spanned, SurfaceExprPayload};
use crate::symbols::ResolvedType;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

impl TypeChecker {
    /// Read an SDK projection already proven at an expression or binding span.
    fn sdk_task_projection(&self, span: Span) -> Option<ResolvedType> {
        self.type_info
            .calls
            .sdk_task_carrier_types
            .get(&(span.start, span.end))
            .cloned()
    }

    /// Record structural relationships the checker just checked. Lists require all checked item types to agree;
    /// no output is reconstructed from an opaque native display or a lowering operand.
    pub(in crate::typechecker) fn record_sdk_task_expression_type(&mut self, expr: &Spanned<Expr>) {
        let projected = match &expr.node {
            Expr::Ident(name) => self
                .symbols
                .lookup(name)
                .and_then(|id| self.type_info.calls.sdk_task_binding_types.get(&id))
                .cloned(),
            Expr::Paren(inner) => self.sdk_task_projection(inner.span),
            Expr::List(items) => self.sdk_task_list_projection(items),
            Expr::ListComp(comp) => self
                .sdk_task_projection(comp.expr.span)
                .map(|ty| ResolvedType::Generic("List".to_owned(), vec![ty])),
            Expr::Surface(surface)
                if crate::semantics_registry::semantics_registry().typecheck_surface_expr_action(&surface.key)
                    == Some(incan_semantics_core::SurfaceExprTypeCheck::AwaitCheck) =>
            {
                match &surface.payload {
                    SurfaceExprPayload::PrefixUnary(inner) => self
                        .sdk_task_projection(inner.span)
                        .and_then(|ty| self.await_output_type_from_type(&ty)),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(ty) = projected {
            self.type_info
                .calls
                .sdk_task_carrier_types
                .insert((expr.span.start, expr.span.end), ty);
        }
    }

    /// Project a checked literal only when an SDK item participates and every direct item has the same type.
    /// Spread entries retain their existing checked collection type until a spread-specific projection is proven.
    fn sdk_task_list_projection(&self, items: &[ListEntry]) -> Option<ResolvedType> {
        let mut types = Vec::new();
        let mut projected = false;
        for item in items {
            let ListEntry::Element(item) = item else {
                return None;
            };
            let ty = if let Some(ty) = self.sdk_task_projection(item.span) {
                projected = true;
                ty
            } else {
                self.type_info.expr_type(item.span)?.clone()
            };
            types.push(ty);
        }
        let first = types.first()?;
        (projected && types.iter().all(|ty| ty == first))
            .then(|| ResolvedType::Generic("List".to_owned(), vec![first.clone()]))
    }

    /// Associate a projected RHS with its exact checked binding, including the statement's declaration type.
    pub(in crate::typechecker) fn bind_sdk_task_projection(&mut self, name: &str, value: Span, statement: Span) {
        let Some(id) = self.symbols.lookup(name) else {
            return;
        };
        if let Some(ty) = self.sdk_task_projection(value) {
            self.type_info.calls.sdk_task_binding_types.insert(id, ty.clone());
            self.type_info
                .calls
                .sdk_task_carrier_types
                .insert((statement.start, statement.end), ty);
        } else {
            self.type_info.calls.sdk_task_binding_types.remove(&id);
        }
    }

    /// Preserve a checked list's item projection at the loop pattern and its lexical binding.
    pub(in crate::typechecker) fn bind_sdk_task_iteration_pattern(&mut self, pattern: &Spanned<Pattern>, iter: Span) {
        let Some(ResolvedType::Generic(base, args)) = self.sdk_task_projection(iter) else {
            return;
        };
        if collections::from_str(&base) != Some(CollectionTypeId::List) {
            return;
        }
        let [ty] = args.as_slice() else {
            return;
        };
        self.type_info
            .calls
            .sdk_task_carrier_types
            .insert((pattern.span.start, pattern.span.end), ty.clone());
        if let Pattern::Binding(name) = &pattern.node
            && let Some(id) = self.symbols.lookup(name)
        {
            self.type_info.calls.sdk_task_binding_types.insert(id, ty.clone());
        }
    }
}
