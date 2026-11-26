//! Select authored rusttype edges once through native metadata, then retain exact source boundary applications.

use super::{RustPathMethodCall, TypeChecker};
use crate::ast::{CallArg, Expr, InteropAdapterKind, InteropDirection, InteropEdgeDecl, NewtypeDecl, Spanned};
use crate::symbols::ResolvedType;
use crate::typechecker::type_info::RustDeclaredConversion;

impl TypeChecker {
    /// Validate a nongeneric native adapter against inspected Rust metadata after its authored edge passes shape
    /// checking. Instance adapters receive the converted value as their receiver; associated adapters receive it as
    /// their sole argument. Infallible edges retain the exact conversion call; fallible edges retain declaration
    /// admission only until automatic fallible boundary conversion has an explicit lowering contract. Executable
    /// source methods keep their own contract and cannot become native calls by name.
    pub(in crate::typechecker) fn retain_declared_rusttype_edge(
        &mut self,
        declaration: &NewtypeDecl,
        edge: &Spanned<InteropEdgeDecl>,
        boundary: &ResolvedType,
        owner: &ResolvedType,
        adapter_has_receiver: bool,
    ) {
        if !declaration.is_rusttype || !declaration.type_params.is_empty() {
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
        let input_expr = Spanned::new(Expr::Ident("__interop_input".into()), edge.node.ty.span);
        let associated_arguments = [CallArg::Positional(input_expr.clone())];
        let associated_types = [input.clone()];
        let (receiver_expr, arguments, argument_types): (&Spanned<Expr>, &[CallArg], &[ResolvedType]) =
            if adapter_has_receiver {
                self.record_expr_type(input_expr.span, input.clone());
                (&input_expr, &[][..], &[][..])
            } else {
                self.check_type_receiver_expr(base);
                (base.as_ref(), &associated_arguments[..], &associated_types[..])
            };
        let errors_before = self.errors.len();
        self.resolve_declared_rusttype_adapter_call(RustPathMethodCall {
            rust_path: &path,
            method,
            receiver_metadata: None,
            receiver_binding: None,
            receiver_expr,
            type_args: &[],
            args: arguments,
            arg_types: argument_types,
            receiver_span: receiver_expr.span,
            expected_return_ty: None,
            span: edge.span,
        });
        if self.errors.len() != errors_before {
            return;
        }
        let Some(call) = self.type_info.rust.checked_calls.remove(&edge.span) else {
            return;
        };
        self.type_info.rust.validated_rusttype_edges.insert(edge.span);
        if edge.node.adapter_kind == InteropAdapterKind::Try {
            return;
        }
        self.type_info.rust.declared_conversions.push(RustDeclaredConversion {
            source: input.clone(),
            destination: output.clone(),
            span: edge.span,
            call,
        });
    }

    /// Resolve a body-less rusttype adapter through an inherent method when one exists, otherwise through the one
    /// imported trait that declares the receiver method. The explicit trait declaration retains its checked signature
    /// while rustc remains responsible for proving conditional and blanket implementations such as `ToString`.
    fn resolve_declared_rusttype_adapter_call(&mut self, call: RustPathMethodCall<'_>) -> Option<ResolvedType> {
        let has_inherent_method = self
            .rust_item_metadata_for_path(call.rust_path)
            .and_then(|metadata| match &metadata.kind {
                incan_lang::interop::RustItemKind::Type(info) => Some(info.methods.iter().any(|candidate| {
                    candidate.name == call.method && Self::rust_signature_has_receiver(&candidate.signature)
                })),
                _ => None,
            })
            .unwrap_or(false);
        if call.args.is_empty()
            && !has_inherent_method
            && let Some(import) = self.unique_rust_trait_import_for_method_call(call.method)
            && let Some(metadata) = self.rust_item_metadata_for_path(&import.trait_path)
            && let incan_lang::interop::RustItemKind::Trait(info) = &metadata.kind
        {
            return self.resolve_rust_declared_trait_method(
                RustPathMethodCall {
                    rust_path: &import.trait_path,
                    ..call
                },
                info,
            );
        }
        self.resolve_rust_path_method_call(call)
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
            .insert(expression.span, vec![edge.call]);
        destination.clone()
    }
}
