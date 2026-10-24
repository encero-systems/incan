//! Checked local partial surfaces and their declaration-owned forwarding bodies.

use incan_semantics_core::canonical_module_identity;

use crate::symbols::ResolvedType;
use crate::typechecker::FunctionBindingInfo;

use super::args::fixed_elements;
use super::{
    BodyBuilder, CompilerNodeId, FunctionDefaultSource, HirSourceSpan, IncanType, SemanticSourceTargetKind, ast, bir,
    hir_span, semantic_type_from_resolved,
};

/// One checked function or retained constructor, including the complete source parameter surface and its defaults.
struct PartialClosureTarget {
    name: String,
    binding: FunctionBindingInfo,
    constructor: Option<bir::NominalDeclaration>,
    direct_call_id: Option<CompilerNodeId>,
    defaults: Option<Vec<FunctionDefaultSource>>,
    /// Ordered function arguments already instantiated by the checker at this partial's whole span.
    type_args: Vec<IncanType>,
}

/// The partial's lexical frame, whose private bindings are restored after its forwarding body is built.
struct PartialClosureFrame {
    scope: bir::ScopeId,
    captured_operands: Vec<bir::Operand>,
    capture_locals: Vec<bir::LocalId>,
    parameters: Vec<bir::CallableParam>,
    argument_locals: Vec<bir::LocalId>,
    previous_bindings: Vec<(String, Option<bir::LocalId>)>,
}

impl BodyBuilder<'_, '_> {
    /// Retain a module member's checked signature and selected function identity without reading the namespace.
    ///
    /// The receiver must be the checker's module binding, and the member must independently select a function.
    /// Its source name supplies diagnostics only; no local declaration or default body is selected by that name.
    fn module_partial_target(
        &self,
        partial: &ast::PartialExpr,
        expression_span: ast::Span,
    ) -> Result<PartialClosureTarget, String> {
        let ast::Expr::Field(base, name) = &partial.target.node else {
            return Err("partial callable with a non-function-name target".to_owned());
        };
        if !self
            .type_info
            .resolved_identity(base.span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Module)
        {
            return Err("partial callable without a checked module receiver".to_owned());
        }
        let identity = self
            .type_info
            .resolved_identity(partial.target.span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Function)
            .ok_or("module function value without a checked canonical function")?;
        let Some(ResolvedType::Function(params, result)) = self.type_info.expr_type(partial.target.span) else {
            return Err("module function value without a checked callable signature".to_owned());
        };
        if params.iter().any(|parameter| {
            parameter.name.is_none() || (!partial.args.is_empty() && parameter.kind != ast::ParamKind::Normal)
        }) {
            return Err("partial callable target with an unnamed or rest parameter".to_owned());
        }
        Ok(PartialClosureTarget {
            name: name.clone(),
            binding: FunctionBindingInfo {
                is_async: false,
                params: params.clone(),
                return_type: (**result).clone(),
                identity: Some(identity.clone()),
            },
            constructor: None,
            direct_call_id: None,
            defaults: None,
            type_args: self.partial_instance_arguments(partial, expression_span)?,
        })
    }

    /// Consume the checker's exact constructor selection or existing function binding, never an inferred backend name.
    fn local_partial_target(
        &self,
        partial: &ast::PartialExpr,
        expression_span: ast::Span,
    ) -> Result<PartialClosureTarget, String> {
        let ast::Expr::Ident(name) = &partial.target.node else {
            return self.module_partial_target(partial, expression_span);
        };

        // ---- Checked target identity and retained constructor layout ----
        let selected = self.type_info.resolved_identity(partial.target.span);
        let constructor = if selected.is_some_and(|identity| {
            matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Newtype
            )
        }) {
            let declaration = self
                .local_nominal_declarations
                .values()
                .find(|declaration| Some(&declaration.canonical) == selected)
                .filter(|declaration| {
                    declaration.type_parameters.is_empty()
                        || matches!(
                            declaration.canonical.kind,
                            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                        )
                })
                .filter(|declaration| {
                    canonical_module_identity(&declaration.canonical).as_deref() == Some(self.module_identity)
                })
                .ok_or("partial callable without a retained same-module constructor")?;
            if declaration.canonical.kind == SemanticSourceTargetKind::Newtype
                && !super::partials::plain_newtype_partial(&declaration.name, self.type_info)
            {
                return Err("partial newtype constructor requiring validation".to_owned());
            }
            Some(declaration.clone())
        } else {
            None
        };

        // ---- Complete source parameter signature ----
        let binding = if let Some(declaration) = &constructor {
            let Some(ResolvedType::Function(params, result)) = self.type_info.expr_type(partial.target.span) else {
                return Err("partial constructor without a checked callable signature".to_owned());
            };
            FunctionBindingInfo {
                is_async: false,
                params: params.clone(),
                return_type: (**result).clone(),
                identity: Some(declaration.canonical.clone()),
            }
        } else {
            self.type_info
                .declarations
                .function_bindings
                .get(name)
                .cloned()
                .ok_or("partial callable target with no resolvable top-level function signature")?
        };
        if binding.params.iter().any(|parameter| {
            parameter.name.is_none() || (!partial.args.is_empty() && parameter.kind != ast::ParamKind::Normal)
        }) {
            return Err("partial callable target with an unnamed or rest parameter".to_owned());
        }

        // ---- Declaration-owned defaults and executable identity ----
        let source_name = constructor
            .as_ref()
            .map_or(name.as_str(), |declaration| declaration.name.as_str());
        let direct_call_id = self
            .local_function_declarations
            .get(source_name)
            .and_then(|candidates| match candidates.as_slice() {
                [span] => Some(CompilerNodeId::declaration_span(
                    self.module_identity,
                    span.start,
                    span.end,
                )),
                _ => None,
            });
        let defaults = self.function_default_sources.get(source_name).cloned();
        let name = source_name.to_owned();
        Ok(PartialClosureTarget {
            name,
            binding,
            constructor,
            direct_call_id,
            defaults,
            type_args: if selected.is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function) {
                self.partial_instance_arguments(partial, expression_span)?
            } else {
                Vec::new()
            },
        })
    }

    /// Snapshot written presets once before later expressions can change their source places, then install the checked
    /// parameter surface in its private closure scope. Source evaluation precedes synthetic capture bindings so an
    /// internal name cannot shadow a subsequent preset expression.
    fn local_partial_frame(
        &mut self,
        partial: &ast::PartialExpr,
        target: &PartialClosureTarget,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> PartialClosureFrame {
        let mut values = Vec::with_capacity(partial.args.len());
        for argument in &partial.args {
            let (ty, value) = self.snapshot_partial_value(&argument.value, scope, out);
            values.push((&argument.name, ty, value));
        }
        let mut frame = PartialClosureFrame {
            scope: self.new_scope(Some(scope), span),
            captured_operands: Vec::new(),
            capture_locals: Vec::new(),
            parameters: Vec::new(),
            argument_locals: Vec::new(),
            previous_bindings: Vec::new(),
        };
        let mut presets = std::collections::HashMap::new();

        // ---- Bind captures after all creation-site expressions have been evaluated ----
        for (preset, ty, value) in values {
            frame.captured_operands.push(value);
            let name = format!("__partial_preset_{preset}");
            let previous = self.bindings.get(&name).copied();
            let local = self.declare_new_local_with_reads(name.clone(), ty, frame.scope, span, 1);
            self.locals[local.index()].origin = bir::LocalOrigin::Captured;
            frame.capture_locals.push(local);
            presets.insert(preset.clone(), local);
            frame.previous_bindings.push((name, previous));
        }

        // ---- Full parameter surface with overridable defaults ----
        for (index, parameter) in target.binding.params.iter().enumerate() {
            let Some(name) = &parameter.name else {
                continue;
            };
            let ty = super::variadics::parameter_storage_type(self.checked_type(&parameter.ty), parameter.kind);
            let previous = self.bindings.get(name).copied();
            let local = self.declare_new_local_with_reads(name.clone(), ty.clone(), frame.scope, span, 1);
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            let source = target.defaults.as_ref().and_then(|defaults| defaults.get(index));
            let default = match presets.get(name).copied() {
                Some(capture) => bir::CallableParamDefault::PartialPreset { capture },
                None => match source {
                    Some(source) => self.lower_callable_default(source.default.as_ref(), frame.scope),
                    None if parameter.has_default => bir::CallableParamDefault::Unsupported {
                        span,
                        description: format!(
                            "partial target {} declares a default Body IR could not source",
                            target.name
                        ),
                    },
                    None => bir::CallableParamDefault::Required,
                },
            };
            frame.parameters.push(bir::CallableParam {
                local,
                kind: super::parameter_types::source_parameter_kind(parameter.kind),
                name: name.clone(),
                ty,
                span: source.map_or(span, |source| hir_span(source.param_span)),
                default,
                mutable: parameter.is_mut,
            });
            if parameter.is_mut {
                self.borrowed_parameters.insert(local);
            }
            frame.argument_locals.push(local);
            frame.previous_bindings.push((name.clone(), previous));
        }
        frame
    }

    /// Forward declaration-order arguments with their checked passing modes; construction retains physical field slots.
    fn local_partial_forward(
        &mut self,
        target: PartialClosureTarget,
        frame: &PartialClosureFrame,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        // ---- Declaration-order operands and passing modes ----
        let arguments = frame
            .argument_locals
            .iter()
            .zip(&target.binding.params)
            .map(|(&local, parameter)| {
                let ty = super::variadics::parameter_storage_type(self.checked_type(&parameter.ty), parameter.kind);
                let place = bir::Place::from_local(local);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                let operand = bir::Operand::place(place, fact, last_use);
                if parameter.is_mut {
                    self.borrow_for_mut_parameter(operand)
                } else {
                    operand
                }
            })
            .collect::<Vec<_>>();
        let ty = semantic_type_from_resolved(&target.binding.return_type);
        let binding = bir::ArgumentBinding::resolved_positional(arguments.len());

        // ---- Retained physical construction layout ----
        if let Some(declaration) = target.constructor {
            if !super::partials::constructor_parameter_layout_matches(&frame.parameters, &declaration) {
                return self.unsupported_operand(
                    "partial constructor parameters differ from retained field layout".to_owned(),
                    frame.scope,
                    span,
                    out,
                );
            }
            let constructor = bir::ConstructorTarget {
                name: declaration.name,
                canonical: Some(declaration.canonical),
                direct_declaration_id: Some(declaration.direct_declaration_id),
                canonical_field_layout: Some(declaration.fields),
                binding,
            };
            return self.push_assign_temp(
                bir::Rvalue::Aggregate(
                    bir::AggregateKind::Constructor(Box::new(constructor)),
                    fixed_elements(arguments),
                ),
                ty,
                frame.scope,
                span,
                out,
            );
        }

        // ---- Checked function forwarding ----
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: target.name,
                direct_call_id: target.direct_call_id,
                builtin: None,
                canonical: target.binding.identity,
                type_args: target.type_args,
                binding,
            })),
            fixed_elements(arguments),
            ty,
            frame.scope,
            span,
            false,
            out,
        )
    }

    /// Build a reusable preset environment and restore every private binding before returning its closure operand.
    pub(super) fn lower_local_partial(
        &mut self,
        partial: &ast::PartialExpr,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let span = hir_span(expr_span);
        let target = match self.local_partial_target(partial, expr_span) {
            Ok(target) => target,
            Err(reason) => return self.unsupported_operand(reason, scope, span, out),
        };
        let frame = self.local_partial_frame(partial, &target, scope, span, out);
        let mut statements = Vec::new();
        let result = self.local_partial_forward(target, &frame, span, &mut statements);
        for (name, previous) in frame.previous_bindings.into_iter().rev() {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }
        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Closure {
                params: frame.parameters,
                captured_operands: frame.captured_operands,
                body: Box::new(bir::ClosureBody {
                    native_storage: None,
                    function_item: false,
                    consumes_environment: false,
                    original_callable: None,
                    capture_locals: frame.capture_locals,
                    stmts: statements,
                    result,
                }),
            },
            ty,
            scope,
            span,
            out,
        )
    }
}
