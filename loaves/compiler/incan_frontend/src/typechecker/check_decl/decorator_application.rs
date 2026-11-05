//! User-defined function and method decorators and decorator callables.

use super::*;

impl TypeChecker {
    /// Apply RFC 036 user-defined decorators to a top-level function binding after its body has been checked.
    ///
    /// The undecorated function is already present in the module symbol table from collection. Each user-defined
    /// decorator is checked as `decorator(current_binding)`, bottom-up, and the module-visible binding is then replaced
    /// with the resulting type as an ordinary immutable value.
    pub(super) fn apply_user_defined_function_decorators(&mut self, func: &FunctionDecl, span: Span) {
        if !func
            .decorators
            .iter()
            .any(|decorator| self.is_user_defined_decorator_candidate(&decorator.node))
        {
            return;
        }

        let Some(original_function_info) = self.function_info_for_declaration_span(&func.name, span) else {
            return;
        };
        let original_ty = function_info_callable_type(&original_function_info);

        let mut binding_ty = original_ty.clone();
        for decorator in func.decorators.iter().rev() {
            if self.is_user_defined_decorator_candidate(&decorator.node) {
                binding_ty = self.apply_user_defined_decorator(decorator, binding_ty, &func.name);
            }
        }

        let binding = DecoratedFunctionBindingInfo {
            ty: binding_ty.clone(),
            original_ty,
            type_params: original_function_info.type_params.clone(),
            type_param_bounds: original_function_info.type_param_bounds.clone(),
            type_param_bound_details: original_function_info.type_param_bound_details.clone(),
            is_async: original_function_info.is_async,
        };
        self.type_info
            .declarations
            .decorated_function_bindings_by_span
            .insert(span, binding);

        if let Some(symbol_id) = self.symbols.lookup(&func.name)
            && let Some(symbol) = self.symbols.get_mut(symbol_id)
        {
            match &mut symbol.kind {
                SymbolKind::Function(_) => {
                    symbol.kind = SymbolKind::Variable(VariableInfo {
                        ty: binding_ty,
                        is_mutable: false,
                        is_used: false,
                    });
                    symbol.span = span;
                }
                SymbolKind::FunctionOverloads(overloads) => {
                    if let Some(overload) = overloads.iter_mut().find(|overload| overload.span == span)
                        && let ResolvedType::Function(params, ret) = binding_ty
                    {
                        overload.info.params = params;
                        overload.info.return_type = *ret;
                    }
                    self.type_info
                        .declarations
                        .function_overloads
                        .insert(func.name.clone(), overloads.clone());
                }
                _ => {}
            }
        }
    }

    /// Return the function candidate declared at `span`, preserving same-name overload identity.
    pub(super) fn function_info_for_declaration_span(&self, name: &str, span: Span) -> Option<FunctionInfo> {
        let symbol = self.lookup_symbol(name)?;
        match &symbol.kind {
            SymbolKind::Function(info) => Some(info.clone()),
            SymbolKind::FunctionOverloads(overloads) => overloads
                .iter()
                .find(|overload| overload.span == span)
                .map(|overload| overload.info.clone()),
            _ => None,
        }
    }

    /// Apply RFC 036 user-defined decorators to a method table entry when the current method representation can store
    /// the resulting callable signature.
    pub(super) fn apply_user_defined_method_decorators(&mut self, method: &MethodDecl, owner: &str) {
        if !method
            .decorators
            .iter()
            .any(|decorator| self.is_user_defined_decorator_candidate(&decorator.node))
        {
            return;
        }

        let Some(mut method_info) = self.lookup_method_info_for_update(owner, &method.name) else {
            return;
        };
        let receiver = DecoratedMethodReceiver::new(owner, method.receiver);
        let original_binding_ty = method_info_callable_type(&method_info, receiver.surface_param());
        let mut binding_ty = original_binding_ty.clone();
        for decorator in method.decorators.iter().rev() {
            if self.is_user_defined_decorator_candidate(&decorator.node) {
                binding_ty = self.apply_user_defined_method_decorator(decorator, binding_ty, &method.name, &receiver);
            }
        }

        let ResolvedType::Function(params, ret) = binding_ty.clone() else {
            return;
        };
        let Some((_receiver, surface_params)) = params.split_first() else {
            return;
        };
        self.type_info.declarations.decorated_method_bindings.insert(
            (owner.to_string(), method.name.clone()),
            DecoratedMethodBindingInfo {
                unbound_ty: receiver.with_passing_receiver(binding_ty),
                original_unbound_ty: receiver.with_passing_receiver(original_binding_ty),
            },
        );
        method_info.params = surface_params.to_vec();
        method_info.return_type = *ret;
        self.replace_method_info(owner, &method.name, method_info);
    }

    /// Return the method metadata currently visible for an owner and method name.
    pub(super) fn lookup_method_info_for_update(&self, owner: &str, method_name: &str) -> Option<MethodInfo> {
        let symbol_id = self.symbols.lookup(owner)?;
        let symbol = self.symbols.get(symbol_id)?;
        match &symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Type(TypeInfo::Class(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Type(TypeInfo::Newtype(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Type(TypeInfo::Enum(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Trait(info) => info.methods.get(method_name).cloned(),
            _ => None,
        }
    }

    /// Replace an owner method entry after decorator application has produced a representable callable type.
    pub(super) fn replace_method_info(&mut self, owner: &str, method_name: &str, method_info: MethodInfo) {
        let Some(symbol_id) = self.symbols.lookup(owner) else {
            return;
        };
        let Some(symbol) = self.symbols.get_mut(symbol_id) else {
            return;
        };
        match &mut symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => {
                info.methods.insert(method_name.to_string(), method_info.clone());
                if let Some(overloads) = info.method_overloads.get_mut(method_name) {
                    *overloads = vec![method_info];
                }
            }
            SymbolKind::Type(TypeInfo::Class(info)) => {
                info.methods.insert(method_name.to_string(), method_info.clone());
                if let Some(overloads) = info.method_overloads.get_mut(method_name) {
                    *overloads = vec![method_info];
                }
            }
            SymbolKind::Type(TypeInfo::Newtype(info)) => {
                info.methods.insert(method_name.to_string(), method_info);
            }
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                info.methods.insert(method_name.to_string(), method_info.clone());
                if let Some(overloads) = info.method_overloads.get_mut(method_name) {
                    *overloads = vec![method_info];
                }
            }
            SymbolKind::Trait(info) => {
                info.methods.insert(method_name.to_string(), method_info);
            }
            _ => {}
        }
    }

    /// Type-check one decorator application against the current decorated binding type.
    pub(super) fn apply_user_defined_decorator(
        &mut self,
        decorator: &Spanned<Decorator>,
        binding_ty: ResolvedType,
        binding_name: &str,
    ) -> ResolvedType {
        let display = Self::decorator_display(&decorator.node);
        let callable_ty = self.decorator_callable_type(decorator, &display);

        self.apply_decorator_callable(&display, callable_ty, binding_ty, binding_name, decorator.span)
    }

    /// Type-check a decorator's path or factory call and return the callable it applies to the declaration.
    pub(in crate::typechecker) fn decorator_callable_type(
        &mut self,
        decorator: &Spanned<Decorator>,
        display: &str,
    ) -> ResolvedType {
        if decorator.node.is_call {
            self.check_decorator_factory_expr(decorator, display)
        } else {
            self.check_expr(&Self::decorator_path_expr(&decorator.node, decorator.span))
        }
    }

    /// Type-check a decorator factory expression such as `@logged(label="x")` or `@app.get("/")`.
    pub(super) fn check_decorator_factory_expr(
        &mut self,
        decorator: &Spanned<Decorator>,
        display: &str,
    ) -> ResolvedType {
        let key = decorator.span;
        self.type_info
            .declarations
            .body_ir_decorator_static_receivers
            .remove(&key);
        let static_receiver = self.body_ir_static_decorator_receiver(decorator);
        self.type_info
            .declarations
            .body_ir_decorator_applications
            .remove(&decorator.span);
        let Some(args) = self.decorator_call_args(decorator, display) else {
            return ResolvedType::Unknown;
        };
        let path = &decorator.node.path.segments;
        let factory_expr = if path.len() >= 2 {
            let base_path = ImportPath {
                parent_levels: decorator.node.path.parent_levels,
                is_absolute: decorator.node.path.is_absolute,
                segments: path[..path.len() - 1].to_vec(),
            };
            let base = Self::decorator_path_expr_from_import_path(&base_path, decorator.span);
            let method = path.last().cloned().unwrap_or_default();
            Spanned::new(
                Expr::MethodCall(Box::new(base), method, decorator.node.type_args.clone(), args),
                decorator.span,
            )
        } else {
            let callee = Self::decorator_path_expr(&decorator.node, decorator.span);
            Spanned::new(
                Expr::Call(Box::new(callee), decorator.node.type_args.clone(), args),
                decorator.span,
            )
        };
        let previous = self.body_ir_decorator_factory_span.replace(decorator.span);
        let result = self.check_expr(&factory_expr);
        self.body_ir_decorator_factory_span = previous;
        if matches!(result, ResolvedType::Function(_, _))
            && self
                .type_info
                .resolved_identity(decorator.span)
                .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Method)
            && let Some(receiver) = static_receiver
        {
            self.type_info
                .declarations
                .body_ir_decorator_static_receivers
                .insert(key, receiver);
        }
        self.retain_body_ir_decorator_factory(decorator, &result);
        result
    }

    /// Snapshot only a checked static symbol used as a two-segment factory receiver. Namespace paths and local callable
    /// values do not acquire a storage identity through source spelling.
    pub(super) fn body_ir_static_decorator_receiver(
        &self,
        decorator: &Spanned<Decorator>,
    ) -> Option<BodyIrDecoratorStaticReceiver> {
        let [name, _] = decorator.node.path.segments.as_slice() else {
            return None;
        };
        let symbol = self.symbols.lookup(name)?;
        let Symbol {
            kind: SymbolKind::Static(storage),
            ..
        } = self.symbols.get(symbol)?
        else {
            return None;
        };
        Some(BodyIrDecoratorStaticReceiver {
            binding_name: name.clone(),
            identity: self.symbols.identity_of(symbol)?.clone(),
            ty: storage.ty.clone(),
        })
    }

    /// Retain ordered arguments from the factory declaration actually resolved by checking, without changing its
    /// legacy call-site facts. Unbound declaration parameters remain placeholders until decorator validation.
    pub(super) fn retain_body_ir_decorator_factory(&mut self, decorator: &Spanned<Decorator>, result: &ResolvedType) {
        let key = decorator.span;
        let partial_arguments = self
            .type_info
            .declarations
            .body_ir_decorator_applications
            .remove(&key)
            .map(|application| application.factory_type_args);
        if self
            .type_info
            .declarations
            .body_ir_decorator_static_receivers
            .contains_key(&key)
        {
            self.type_info.declarations.body_ir_decorator_applications.insert(
                key,
                BodyIrDecoratorApplication {
                    callable_type: result.clone(),
                    type_bindings: HashMap::new(),
                    factory_type_args: partial_arguments.unwrap_or_default(),
                },
            );
            return;
        }
        let [name] = decorator.node.path.segments.as_slice() else {
            return;
        };
        let Some(symbol_id) = self.symbols.lookup(name) else {
            return;
        };
        let Some(identity) = self.symbols.identity_of(symbol_id) else {
            return;
        };
        if Some(identity) != self.type_info.resolved_identity(decorator.span) {
            return;
        }
        let Some(Symbol {
            kind: SymbolKind::Function(info),
            ..
        }) = self.symbols.get(symbol_id)
        else {
            return;
        };
        let arguments = partial_arguments
            .or_else(|| self.type_info.calls.call_site_monomorph_type_args.get(&key).cloned())
            .unwrap_or_else(|| info.type_params.iter().cloned().map(ResolvedType::TypeVar).collect());
        self.type_info.declarations.body_ir_decorator_applications.insert(
            key,
            BodyIrDecoratorApplication {
                callable_type: result.clone(),
                type_bindings: HashMap::new(),
                factory_type_args: arguments,
            },
        );
    }

    /// Apply a callable decorator value to the decorated binding type and return the post-decoration callable type.
    pub(in crate::typechecker) fn apply_decorator_callable(
        &mut self,
        display: &str,
        callable_ty: ResolvedType,
        binding_ty: ResolvedType,
        binding_name: &str,
        span: Span,
    ) -> ResolvedType {
        let key = span;
        let factory_type_args = self
            .type_info
            .declarations
            .body_ir_decorator_applications
            .remove(&key)
            .map(|application| application.factory_type_args)
            .unwrap_or_default();
        self.type_info
            .declarations
            .generic_identity_decorator_applications
            .remove(&span);
        let ResolvedType::Function(params, ret) = callable_ty else {
            if !matches!(callable_ty, ResolvedType::Unknown) {
                self.errors.push(if display.contains('(') {
                    errors::decorator_factory_not_callable(display, span)
                } else {
                    errors::decorator_not_callable(display, span)
                });
            }
            return ResolvedType::Unknown;
        };

        let arg_expr = Spanned::new(Expr::Ident(binding_name.to_string()), span);
        let args = vec![CallArg::Positional(arg_expr)];
        let arg_types = vec![binding_ty];
        let mut type_bindings = HashMap::new();
        let error_count = self.errors.len();
        self.validate_callable_arg_bindings(display, &params, &args, &arg_types, &mut type_bindings, span);
        if self.errors.len() != error_count {
            return ResolvedType::Unknown;
        }
        if let [param] = params.as_slice()
            && matches!(param.ty, ResolvedType::TypeVar(_))
            && param.ty == *ret
        {
            self.type_info
                .declarations
                .generic_identity_decorator_applications
                .insert(span);
        }
        let result_ty = substitute_resolved_type(&ret, &type_bindings);
        if !matches!(result_ty, ResolvedType::Function(_, _) | ResolvedType::Unknown) {
            self.errors.push(errors::decorator_result_not_callable(display, span));
            return ResolvedType::Unknown;
        }
        self.type_info.declarations.body_ir_decorator_applications.insert(
            key,
            BodyIrDecoratorApplication {
                callable_type: ResolvedType::Function(params, ret),
                type_bindings,
                factory_type_args,
            },
        );
        result_ty
    }

    /// Convert decorator arguments into ordinary call arguments for user-defined decorator factory checking.
    pub(super) fn decorator_call_args(
        &mut self,
        decorator: &Spanned<Decorator>,
        display: &str,
    ) -> Option<Vec<CallArg>> {
        let mut args = Vec::new();
        let mut valid = true;
        for arg in &decorator.node.args {
            match arg {
                DecoratorArg::Positional(expr) => args.push(CallArg::Positional(expr.clone())),
                DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) => {
                    args.push(CallArg::Named(
                        Spanned::new(name.clone(), Span::default()),
                        expr.clone(),
                    ));
                }
                DecoratorArg::Named(_, DecoratorArgValue::Type(ty)) => {
                    self.errors
                        .push(errors::decorator_type_argument_not_supported(display, ty.span));
                    valid = false;
                }
            }
        }
        valid.then_some(args)
    }

    /// Render a decorator path with a coarse call marker for diagnostics.
    pub(in crate::typechecker) fn decorator_display(decorator: &Decorator) -> String {
        let path = decorator.path.segments.join(".");
        if decorator.is_call {
            if decorator.type_args.is_empty() {
                format!("{path}(...)")
            } else {
                format!("{path}[...](...)")
            }
        } else {
            path
        }
    }

    /// Build an expression from a decorator's path.
    pub(super) fn decorator_path_expr(decorator: &Decorator, span: Span) -> Spanned<Expr> {
        Self::decorator_path_expr_from_import_path(&decorator.path, span)
    }

    /// Build an identifier/field expression from an import-style decorator path.
    pub(super) fn decorator_path_expr_from_import_path(path: &ImportPath, span: Span) -> Spanned<Expr> {
        let mut segments = path.segments.iter();
        let Some(first) = segments.next() else {
            return Spanned::new(Expr::Ident(String::new()), span);
        };
        let mut expr = Spanned::new(Expr::Ident(first.clone()), span);
        for segment in segments {
            expr = Spanned::new(Expr::Field(Box::new(expr), segment.clone()), span);
        }
        expr
    }
}
