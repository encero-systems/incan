//! Select authored rusttype edges once through native metadata, then retain exact source boundary applications.

use super::{RustPathMethodCall, TypeChecker};
use crate::ast::{CallArg, Expr, InteropAdapterKind, InteropDirection, InteropEdgeDecl, NewtypeDecl, Spanned};
use crate::symbols::ResolvedType;
use crate::typechecker::type_info::RustDeclaredConversion;

impl TypeChecker {
    /// Retain an infallible, nongeneric native adapter only after its authored edge has passed shape validation.
    /// Executable source methods keep their own contract and cannot become native calls by name.
    pub(in crate::typechecker) fn retain_declared_rusttype_edge(
        &mut self,
        declaration: &NewtypeDecl,
        edge: &Spanned<InteropEdgeDecl>,
        boundary: &ResolvedType,
        owner: &ResolvedType,
    ) {
        if !declaration.is_rusttype
            || !declaration.type_params.is_empty()
            || edge.node.adapter_kind != InteropAdapterKind::Via
        {
            return;
        }
        let Expr::Field(base, method) = &edge.node.adapter.node else {
            return;
        };
        if !matches!(&base.node, Expr::Ident(name) if name == &declaration.name)
            || declaration.methods.iter().any(|candidate| {
                candidate.node.name == *method
                    && candidate
                        .node
                        .body
                        .as_ref()
                        .is_some_and(|body| !crate::typechecker::is_trivial_body(body))
            })
        {
            return;
        }
        let Some(path) = self
            .type_info
            .rust
            .rusttype_canonical_paths
            .get(&declaration.name)
            .cloned()
        else {
            return;
        };
        let (input, output) = match edge.node.direction {
            InteropDirection::From => (boundary, owner),
            InteropDirection::Into => (owner, boundary),
        };
        // Associated syntax passes even a shared method's receiver in its explicit first argument slot.
        self.check_type_receiver_expr(base);
        let arguments = [CallArg::Positional(Spanned::new(
            Expr::Ident("__interop_input".into()),
            edge.node.ty.span,
        ))];
        let errors_before = self.errors.len();
        self.resolve_rust_path_method_call(RustPathMethodCall {
            rust_path: &path,
            method,
            receiver_metadata: None,
            receiver_binding: None,
            receiver_expr: base,
            type_args: &[],
            args: &arguments,
            arg_types: &[input.clone()],
            receiver_span: base.span,
            expected_return_ty: None,
            span: edge.span,
        });
        if self.errors.len() != errors_before {
            return;
        }
        let Some(call) = self
            .type_info
            .rust
            .checked_calls
            .remove(&(edge.span.start, edge.span.end))
        else {
            return;
        };
        self.type_info.rust.declared_conversions.push(RustDeclaredConversion {
            source: input.clone(),
            destination: output.clone(),
            span: edge.span,
            call,
        });
    }

    /// Apply only an exact checked edge at a contextual source boundary, preserving the expression's original type.
    /// The returned type belongs to the destination; Body IR evaluates the original value before the retained call.
    pub(in crate::typechecker) fn retain_declared_conversion(
        &mut self,
        expression: &Spanned<Expr>,
        source: &ResolvedType,
        destination: Option<&ResolvedType>,
    ) -> ResolvedType {
        let Some(destination) = destination else {
            return source.clone();
        };
        let Some(edge) = self
            .type_info
            .rust
            .declared_conversions
            .iter()
            .find(|edge| edge.source == *source && edge.destination == *destination)
            .cloned()
        else {
            return source.clone();
        };
        self.type_info
            .rust
            .argument_adjustments
            .insert((expression.span.start, expression.span.end), vec![edge.call]);
        destination.clone()
    }
}
