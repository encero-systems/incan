//! Method lowering: model methods, class methods, trait impl methods, and general method lowering.

use std::collections::HashSet;

use super::super::super::decl::{FunctionParam, IrAssociatedType, IrDecl, IrDeclKind, IrFunction, IrImpl, Visibility};
use super::super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind, VarAccess, VarRefKind};
use super::super::super::stmt::{IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::super::{IrSpan, Mutability, TypedExpr};
use super::super::AstLowering;
use super::super::TraitImplLoweringInput;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::ResolvedType;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::traits as core_traits;
use incan_core::lang::traits::TraitId;

impl AstLowering {
    /// Return whether a method carries a resolved builtin decorator.
    fn method_has_decorator(method: &ast::MethodDecl, id: DecoratorId) -> bool {
        method
            .decorators
            .iter()
            .any(|decorator| decorators::from_segments(&decorator.node.path.segments) == Some(id))
    }

    /// Return the private emitted method name that stores an undecorated original method body.
    fn decorator_original_method_name(name: &str) -> String {
        format!("__incan_original_{name}")
    }

    /// Return the private emitted associated function name that adapts the original method to an unbound callable.
    fn decorator_original_method_adapter_name(name: &str) -> String {
        format!("__incan_original_{name}_adapter")
    }

    /// Return the private emitted static name that stores a decorated method callable.
    fn decorator_method_static_binding_name(owner: &str, method: &str) -> String {
        format!("__incan_decorated_{}_{}", owner.to_lowercase(), method)
    }

    /// Build the bottom-up decorator application expression for an instance method.
    fn decorator_method_application_expr(
        &self,
        owner: &str,
        method: &ast::MethodDecl,
    ) -> Result<Spanned<ast::Expr>, LoweringError> {
        let original = Spanned::new(
            ast::Expr::Ident(Self::decorator_original_method_name(&method.name)),
            ast::Span::default(),
        );
        let associated_original = Spanned::new(
            ast::Expr::Field(
                Box::new(Spanned::new(ast::Expr::Ident(owner.to_string()), ast::Span::default())),
                Self::decorator_original_method_adapter_name(&method.name),
            ),
            ast::Span::default(),
        );
        let mut current = original;
        for decorator in method.decorators.iter().rev() {
            if !self.is_user_defined_decorator_candidate(&decorator.node) {
                continue;
            }
            let callable = if decorator.node.is_call {
                let args = Self::decorator_call_args(decorator)?;
                let path = &decorator.node.path.segments;
                if path.len() >= 2 {
                    let base_path = ast::ImportPath {
                        parent_levels: decorator.node.path.parent_levels,
                        is_absolute: decorator.node.path.is_absolute,
                        segments: path[..path.len() - 1].to_vec(),
                    };
                    let base = Self::decorator_path_expr_from_import_path(&base_path, decorator.span);
                    let method_name = path.last().cloned().unwrap_or_default();
                    Spanned::new(
                        ast::Expr::MethodCall(Box::new(base), method_name, Vec::new(), args),
                        decorator.span,
                    )
                } else {
                    let callee = Self::decorator_path_expr(&decorator.node, decorator.span);
                    Spanned::new(ast::Expr::Call(Box::new(callee), Vec::new(), args), decorator.span)
                }
            } else {
                Self::decorator_path_expr(&decorator.node, decorator.span)
            };
            let arg = if matches!(current.node, ast::Expr::Ident(_)) {
                associated_original.clone()
            } else {
                current
            };
            current = Spanned::new(
                ast::Expr::Call(Box::new(callable), Vec::new(), vec![ast::CallArg::Positional(arg)]),
                decorator.span,
            );
        }
        Ok(current)
    }

    /// Trait type-parameter names from either local AST declarations or typechecker metadata.
    fn trait_type_param_names(&self, trait_name: &str) -> Option<Vec<String>> {
        if let Some(decl) = self.trait_decls.get(trait_name) {
            return Some(decl.type_params.iter().map(|tp| tp.name.clone()).collect());
        }
        self.type_info
            .as_ref()
            .and_then(|info| info.traits.type_params.get(trait_name).cloned())
    }

    /// Infer the concrete trait arguments for `impl Trait<...> for Type<...>` from the adopter's leading type params.
    ///
    /// RFC 042 uses the same positional convention as the typechecker for concrete adopters of generic traits:
    /// the adopted trait's type parameters map to the adopter's leading type parameters.
    fn infer_trait_impl_resolved_args(&self, trait_name: &str, type_params: &[ast::TypeParam]) -> Vec<ResolvedType> {
        let Some(param_names) = self.trait_type_param_names(trait_name) else {
            return Vec::new();
        };
        let arity = param_names.len();
        type_params
            .iter()
            .take(arity)
            .map(|tp| ResolvedType::TypeVar(tp.name.clone()))
            .collect()
    }

    /// Collect the full set of Rust trait impl targets required by a trait hierarchy.
    fn collect_trait_impl_targets_recursive(
        &self,
        trait_name: &str,
        trait_args: &[ResolvedType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, Vec<IrType>)>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",")
        );
        if !seen.insert(key) {
            return;
        }
        out.push((
            trait_name.to_string(),
            trait_args.iter().map(|arg| self.lower_resolved_type(arg)).collect(),
        ));

        let Some(type_info) = &self.type_info else {
            return;
        };
        let Some(direct_supertraits) = type_info.traits.direct_supertraits.get(trait_name) else {
            return;
        };
        let Some(param_names) = self.trait_type_param_names(trait_name) else {
            return;
        };
        let subst = type_param_subst_map(&param_names, trait_args);

        for (supertrait_name, supertrait_args) in direct_supertraits {
            let instantiated_args = supertrait_args
                .iter()
                .map(|arg| substitute_resolved_type(arg, &subst))
                .collect::<Vec<_>>();
            self.collect_trait_impl_targets_recursive(supertrait_name, &instantiated_args, seen, out);
        }
    }

    /// Expand a direct adopted trait into the full set of Rust impl targets required by its supertrait chain.
    pub(in crate::backend::ir::lower) fn trait_impl_targets_for_adopted_trait(
        &self,
        trait_name: &str,
        type_params: &[ast::TypeParam],
    ) -> Vec<(String, Vec<IrType>)> {
        let direct_args = self.infer_trait_impl_resolved_args(trait_name, type_params);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        self.collect_trait_impl_targets_recursive(trait_name, &direct_args, &mut seen, &mut out);
        out
    }

    /// Lower an adopted trait bound into the direct Rust impl target(s) required for codegen.
    ///
    /// Explicit type arguments on adopter bounds (for example `with From[int]`) are preserved directly from the AST.
    /// Recursive instantiated supertrait expansion is only available on the older positional-adoption path, which is
    /// sufficient for the current stdlib conversion hooks.
    pub(in crate::backend::ir::lower) fn trait_impl_targets_for_adopted_trait_bound(
        &self,
        bound: &ast::TraitBound,
        type_params: &[ast::TypeParam],
    ) -> Vec<(String, Vec<IrType>)> {
        if bound.type_args.is_empty() {
            return self.trait_impl_targets_for_adopted_trait(&bound.name, type_params);
        }
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();

        vec![(
            bound.name.clone(),
            bound
                .type_args
                .iter()
                .map(|arg| self.lower_type_with_type_params(&arg.node, Some(&type_param_names)))
                .collect(),
        )]
    }

    /// Return whether the typechecker proved a body-less rusttype Rust-trait adoption is satisfied by the backing type.
    pub(in crate::backend::ir::lower) fn rusttype_forwarding_satisfied_by_alias(
        &self,
        type_name: &str,
        trait_name: &str,
    ) -> bool {
        self.type_info.as_ref().is_some_and(|info| {
            info.rust
                .rusttype_forwarded_trait_adoptions
                .contains(&(type_name.to_string(), trait_name.to_string()))
        })
    }

    /// Lower model methods into an impl block.
    pub(in crate::backend::ir::lower) fn lower_model_methods(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
        properties: &[Spanned<ast::PropertyDecl>],
        adopted_traits: &[Spanned<ast::TraitBound>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = (|| {
            let inherent_methods = self.inherent_methods_for_rust_impl(type_params, methods, adopted_traits);
            let mut lowered_methods = Vec::new();
            for method in inherent_methods {
                lowered_methods.extend(self.lower_decorated_or_plain_methods(
                    type_name,
                    &method.node,
                    Some(&type_param_names),
                )?);
            }
            for property in properties {
                lowered_methods.push(self.lower_property_with_type_params(
                    &property.node,
                    Some(&type_param_names),
                    PropertyLoweringMode::Inherent,
                )?);
            }
            Ok(lowered_methods)
        })();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: lowered_methods,
        })
    }

    /// Lower a property return type into the comparable IR shape used for trait override matching.
    fn property_signature_for_match(
        &mut self,
        property: &ast::PropertyDecl,
        type_param_names: &std::collections::HashSet<&str>,
        subst: &std::collections::HashMap<String, IrType>,
    ) -> IrType {
        let return_type = self.lower_callable_return_type(&property.return_type.node, Some(type_param_names));
        Self::substitute_ir_type_params(return_type, subst)
    }

    /// Return whether a concrete property can satisfy an instantiated trait property requirement.
    fn trait_impl_property_override_matches(
        &mut self,
        trait_property: &ast::PropertyDecl,
        candidate: &ast::PropertyDecl,
        trait_type_params: &[ast::TypeParam],
        trait_type_args: &[IrType],
        owner_type_param_names: &std::collections::HashSet<&str>,
    ) -> bool {
        let trait_param_names: std::collections::HashSet<&str> =
            trait_type_params.iter().map(|tp| tp.name.as_str()).collect();
        let subst: std::collections::HashMap<String, IrType> = trait_type_params
            .iter()
            .map(|tp| tp.name.clone())
            .zip(trait_type_args.iter().cloned())
            .collect();
        let trait_return = self.property_signature_for_match(trait_property, &trait_param_names, &subst);
        let empty_subst = std::collections::HashMap::new();
        let candidate_return = self.property_signature_for_match(candidate, owner_type_param_names, &empty_subst);
        trait_return == candidate_return
    }

    /// Lower private statics that hold decorated method callable bindings for one owner type.
    pub(in crate::backend::ir::lower) fn lower_decorated_method_statics(
        &mut self,
        type_name: &str,
        methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<Vec<IrDecl>, LoweringError> {
        let mut out = Vec::new();
        for method in methods {
            let Some(binding) = self.type_info.as_ref().and_then(|info| {
                info.declarations
                    .decorated_method_bindings
                    .get(&(type_name.to_string(), method.node.name.clone()))
                    .cloned()
            }) else {
                continue;
            };
            let crate::frontend::symbols::ResolvedType::Function(params, ret) = binding.unbound_ty else {
                continue;
            };
            let static_name = Self::decorator_method_static_binding_name(type_name, &method.node.name);
            let decorated_ty = IrType::Function {
                params: params.iter().map(|param| self.lower_resolved_type(&param.ty)).collect(),
                ret: Box::new(self.lower_resolved_type(&ret)),
            };
            let application = self.decorator_method_application_expr(type_name, &method.node)?;
            let mut value = self.lower_expr_spanned(&application)?;
            value.ty = decorated_ty.clone();
            out.push(IrDecl::new(IrDeclKind::Static {
                visibility: Visibility::Private,
                name: static_name,
                ty: decorated_ty,
                value,
            }));
        }
        Ok(out)
    }

    /// Lower a method either as-is or as original adapter plus public decorated wrapper.
    fn lower_decorated_or_plain_methods(
        &mut self,
        owner: &str,
        method: &ast::MethodDecl,
        type_param_names: Option<&HashSet<&str>>,
    ) -> Result<Vec<IrFunction>, LoweringError> {
        if self.type_info.as_ref().is_some_and(|info| {
            info.declarations
                .decorated_method_bindings
                .contains_key(&(owner.to_string(), method.name.clone()))
        }) {
            let original = self.lower_method_named_with_type_params(
                method,
                Self::decorator_original_method_name(&method.name),
                Visibility::Private,
                type_param_names,
            )?;
            let adapter = self.decorated_method_original_adapter(owner, method)?;
            let wrapper = self.lower_decorated_method_wrapper(owner, method)?;
            Ok(vec![original, adapter, wrapper])
        } else {
            Ok(vec![self.lower_method_with_type_params(method, type_param_names)?])
        }
    }

    /// Lower the public method wrapper that dispatches through the decorated callable static.
    fn lower_decorated_method_wrapper(
        &mut self,
        owner: &str,
        method: &ast::MethodDecl,
    ) -> Result<IrFunction, LoweringError> {
        let Some(binding) = self.type_info.as_ref().and_then(|info| {
            info.declarations
                .decorated_method_bindings
                .get(&(owner.to_string(), method.name.clone()))
                .cloned()
        }) else {
            return self.lower_method_with_type_params(method, None);
        };
        let crate::frontend::symbols::ResolvedType::Function(params, ret) = binding.unbound_ty else {
            return self.lower_method_with_type_params(method, None);
        };
        let Some((receiver_param, surface_params)) = params.split_first() else {
            return self.lower_method_with_type_params(method, None);
        };
        let receiver_ty = self.lower_resolved_type(&receiver_param.ty);
        let mut wrapper_params = Vec::with_capacity(surface_params.len() + 1);
        let receiver = method.receiver.unwrap_or(ast::Receiver::Immutable);
        wrapper_params.push(FunctionParam {
            name: "self".to_string(),
            ty: IrType::Unknown,
            mutability: if matches!(receiver, ast::Receiver::Mutable) {
                Mutability::Mutable
            } else {
                Mutability::Immutable
            },
            is_self: true,
            kind: ast::ParamKind::Normal,
            default: None,
        });
        wrapper_params.extend(surface_params.iter().enumerate().map(|(idx, param)| {
            let base_ty = self.lower_resolved_type(&param.ty);
            FunctionParam {
                name: param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                ty: Self::lower_param_container_type(param.kind, base_ty),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: param.kind,
                default: None,
            }
        }));
        let return_type = self.lower_resolved_type(&ret);
        let static_name = Self::decorator_method_static_binding_name(owner, &method.name);
        let static_func = TypedExpr::new(
            IrExprKind::StaticRead { name: static_name },
            IrType::Function {
                params: params.iter().map(|param| self.lower_resolved_type(&param.ty)).collect(),
                ret: Box::new(return_type.clone()),
            },
        );
        let mut args = Vec::with_capacity(wrapper_params.len());
        args.push(IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr: TypedExpr::new(
                IrExprKind::Var {
                    name: "self".to_string(),
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                receiver_ty,
            ),
        });
        args.extend(wrapper_params.iter().skip(1).map(|param| IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr: TypedExpr::new(
                IrExprKind::Var {
                    name: param.name.clone(),
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                param.ty.clone(),
            ),
        }));
        let call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(static_func),
                type_args: Vec::new(),
                args,
                callable_signature: None,
                canonical_path: None,
            },
            return_type.clone(),
        );
        Ok(IrFunction {
            name: method.name.clone(),
            params: wrapper_params,
            return_type,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(call)))],
            is_async: method.is_async(),
            is_generator: false,
            visibility: Visibility::Public,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })
    }

    /// Lower the associated adapter that exposes the original method as an unbound callable value.
    fn decorated_method_original_adapter(
        &mut self,
        owner: &str,
        method: &ast::MethodDecl,
    ) -> Result<IrFunction, LoweringError> {
        let Some(binding) = self.type_info.as_ref().and_then(|info| {
            info.declarations
                .decorated_method_bindings
                .get(&(owner.to_string(), method.name.clone()))
                .cloned()
        }) else {
            return self.lower_method_with_type_params(method, None);
        };
        let crate::frontend::symbols::ResolvedType::Function(params, ret) = binding.original_unbound_ty else {
            return self.lower_method_with_type_params(method, None);
        };
        let Some((receiver_param, surface_params)) = params.split_first() else {
            return self.lower_method_with_type_params(method, None);
        };
        let receiver_ty = self.lower_resolved_type(&receiver_param.ty);
        let mut adapter_params = Vec::with_capacity(params.len());
        adapter_params.push(FunctionParam {
            name: "__incan_self".to_string(),
            ty: receiver_ty.clone(),
            mutability: Mutability::Immutable,
            is_self: false,
            kind: ast::ParamKind::Normal,
            default: None,
        });
        adapter_params.extend(surface_params.iter().enumerate().map(|(idx, param)| {
            let base_ty = self.lower_resolved_type(&param.ty);
            FunctionParam {
                name: param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                ty: Self::lower_param_container_type(param.kind, base_ty),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: param.kind,
                default: None,
            }
        }));
        let return_type = self.lower_resolved_type(&ret);
        let receiver = TypedExpr::new(
            IrExprKind::Var {
                name: "__incan_self".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            receiver_ty,
        );
        let args = adapter_params
            .iter()
            .skip(1)
            .map(|param| IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: TypedExpr::new(
                    IrExprKind::Var {
                        name: param.name.clone(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    param.ty.clone(),
                ),
            })
            .collect();
        let call = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(receiver),
                method: Self::decorator_original_method_name(&method.name),
                dispatch: None,
                type_args: Vec::new(),
                args,
                callable_signature: None,
                arg_policy: super::super::super::expr::MethodCallArgPolicy::Default,
            },
            return_type.clone(),
        );
        Ok(IrFunction {
            name: Self::decorator_original_method_adapter_name(&method.name),
            params: adapter_params,
            return_type,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(call)))],
            is_async: method.is_async(),
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })
    }

    /// Substitute generic IR type placeholders with instantiated trait arguments.
    fn substitute_ir_type_params(ty: IrType, subst: &std::collections::HashMap<String, IrType>) -> IrType {
        match ty {
            IrType::Generic(name) => subst.get(&name).cloned().unwrap_or(IrType::Generic(name)),
            IrType::List(inner) => IrType::List(Box::new(Self::substitute_ir_type_params(*inner, subst))),
            IrType::Dict(key, value) => IrType::Dict(
                Box::new(Self::substitute_ir_type_params(*key, subst)),
                Box::new(Self::substitute_ir_type_params(*value, subst)),
            ),
            IrType::Set(inner) => IrType::Set(Box::new(Self::substitute_ir_type_params(*inner, subst))),
            IrType::Tuple(items) => IrType::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::substitute_ir_type_params(item, subst))
                    .collect(),
            ),
            IrType::Option(inner) => IrType::Option(Box::new(Self::substitute_ir_type_params(*inner, subst))),
            IrType::Result(ok, err) => IrType::Result(
                Box::new(Self::substitute_ir_type_params(*ok, subst)),
                Box::new(Self::substitute_ir_type_params(*err, subst)),
            ),
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                name,
                args.into_iter()
                    .map(|arg| Self::substitute_ir_type_params(arg, subst))
                    .collect(),
            ),
            IrType::Function { params, ret } => IrType::Function {
                params: params
                    .into_iter()
                    .map(|param| Self::substitute_ir_type_params(param, subst))
                    .collect(),
                ret: Box::new(Self::substitute_ir_type_params(*ret, subst)),
            },
            IrType::Ref(inner) => IrType::Ref(Box::new(Self::substitute_ir_type_params(*inner, subst))),
            IrType::RefMut(inner) => IrType::RefMut(Box::new(Self::substitute_ir_type_params(*inner, subst))),
            other => other,
        }
    }

    /// Lower a method signature into the comparable shape used to pair trait obligations with overrides.
    fn lowered_method_signature_for_match(
        &mut self,
        method: &ast::MethodDecl,
        type_param_names: &std::collections::HashSet<&str>,
        subst: &std::collections::HashMap<String, IrType>,
    ) -> (Option<ast::Receiver>, Vec<(ast::ParamKind, IrType)>, IrType) {
        let mut hidden_type_params = Vec::new();
        let mut hidden_counter = 0usize;
        let params = method
            .params
            .iter()
            .map(|param| {
                let base_ty = self.lower_callable_param_type(
                    &param.node.ty.node,
                    Some(type_param_names),
                    &mut hidden_type_params,
                    &mut hidden_counter,
                );
                let param_ty = Self::lower_param_container_type(param.node.kind, base_ty);
                (param.node.kind, Self::substitute_ir_type_params(param_ty, subst))
            })
            .collect();
        let return_type = self.lower_callable_return_type(&method.return_type.node, Some(type_param_names));
        (
            method.receiver,
            params,
            Self::substitute_ir_type_params(return_type, subst),
        )
    }

    /// Return whether a concrete method has the instantiated signature required by one trait impl.
    fn trait_impl_override_matches(
        &mut self,
        trait_method: &ast::MethodDecl,
        candidate: &ast::MethodDecl,
        trait_type_params: &[ast::TypeParam],
        trait_type_args: &[IrType],
        owner_type_param_names: &std::collections::HashSet<&str>,
    ) -> bool {
        let trait_param_names: std::collections::HashSet<&str> =
            trait_type_params.iter().map(|tp| tp.name.as_str()).collect();
        let subst: std::collections::HashMap<String, IrType> = trait_type_params
            .iter()
            .map(|tp| tp.name.clone())
            .zip(trait_type_args.iter().cloned())
            .collect();
        let trait_sig = self.lowered_method_signature_for_match(trait_method, &trait_param_names, &subst);
        let empty_subst = std::collections::HashMap::new();
        let candidate_sig = self.lowered_method_signature_for_match(candidate, owner_type_param_names, &empty_subst);
        trait_sig == candidate_sig
    }

    /// Return whether a source-level trait target names the current Rust impl target.
    fn trait_bound_matches_impl_target(
        &mut self,
        target: &ast::TraitBound,
        trait_name: &str,
        trait_type_args: &[IrType],
        owner_type_param_names: &std::collections::HashSet<&str>,
    ) -> bool {
        if target.name != trait_name {
            return false;
        }
        let lowered_args = target
            .type_args
            .iter()
            .map(|arg| self.lower_type_with_type_params(&arg.node, Some(owner_type_param_names)))
            .collect::<Vec<_>>();
        lowered_args == trait_type_args
    }

    /// Return source-level method names for compiler-known imported traits whose declaration may be unavailable in
    /// this lowering unit.
    fn known_imported_trait_method_names(trait_name: &str) -> &'static [&'static str] {
        if let Some(trait_id) = core_traits::from_str(trait_name) {
            return core_traits::method_names(trait_id);
        }
        let short_name = trait_name
            .rsplit(['.', ':'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(trait_name);
        if let Some(trait_id) = core_traits::from_str(short_name) {
            return core_traits::method_names(trait_id);
        }
        match short_name {
            "Callable0" | "Callable1" | "Callable2" => &["__call__"],
            "Serialize" | "JsonSerialize" => &["to_json"],
            "Deserialize" | "JsonDeserialize" => &["from_json"],
            _ => &[],
        }
    }

    /// Return whether a required trait method is supplied by a backend derive instead of by a source method body.
    ///
    /// Serde JSON derives implement the Rust-side conversion hooks during codegen. Imported stdlib trait declarations
    /// still make those hooks visible to lowering, so this keeps the trait impl obligation aligned with the backend
    /// expansion without making all missing stdlib trait methods optional.
    fn backend_default_trait_method(trait_name: &str, method_name: &str) -> bool {
        let short_name = trait_name
            .rsplit(['.', ':'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(trait_name);
        matches!(
            (short_name, method_name),
            ("Serialize", "to_json")
                | ("JsonSerialize", "to_json")
                | ("Deserialize", "from_json")
                | ("JsonDeserialize", "from_json")
        )
    }

    /// Return whether a method is safe to emit into an imported trait impl when the trait declaration is missing.
    fn method_matches_imported_trait_without_decl(&self, method: &ast::MethodDecl, trait_name: &str) -> bool {
        if method.trait_target.is_some() {
            return true;
        }
        let known_methods = Self::known_imported_trait_method_names(trait_name);
        known_methods.iter().any(|name| *name == method.name)
    }

    /// Return whether a concrete method is eligible for the current trait impl.
    fn method_trait_target_matches_impl(
        &mut self,
        method: &ast::MethodDecl,
        trait_name: &str,
        trait_type_args: &[IrType],
        owner_type_param_names: &std::collections::HashSet<&str>,
    ) -> bool {
        method.trait_target.as_ref().is_none_or(|target| {
            self.trait_bound_matches_impl_target(&target.node, trait_name, trait_type_args, owner_type_param_names)
        })
    }

    /// Return whether a concrete method should be lowered only inside an adopted trait impl.
    fn method_matches_adopted_trait_impl(
        &mut self,
        method: &ast::MethodDecl,
        type_params: &[ast::TypeParam],
        owner_type_param_names: &std::collections::HashSet<&str>,
        adopted_traits: &[Spanned<ast::TraitBound>],
    ) -> bool {
        for trait_ref in adopted_traits {
            for (trait_name, trait_type_args) in
                self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, type_params)
            {
                let Some(trait_decl) = self.trait_decls.get(&trait_name).cloned() else {
                    continue;
                };
                for trait_method in &trait_decl.methods {
                    if trait_method.node.name == method.name
                        && self.method_trait_target_matches_impl(
                            method,
                            &trait_name,
                            &trait_type_args,
                            owner_type_param_names,
                        )
                        && self.trait_impl_override_matches(
                            &trait_method.node,
                            method,
                            &trait_decl.type_params,
                            &trait_type_args,
                            owner_type_param_names,
                        )
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Keep only methods that Rust can safely emit as inherent methods.
    ///
    /// Rust does not support inherent overloads by name. Same-name methods that match adopted trait obligations are
    /// emitted in trait impl blocks instead; a single remaining distinct-shape method can still be emitted inherently.
    fn inherent_methods_for_rust_impl(
        &mut self,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
        adopted_traits: &[Spanned<ast::TraitBound>],
    ) -> Vec<Spanned<ast::MethodDecl>> {
        let owner_type_param_names: std::collections::HashSet<&str> =
            type_params.iter().map(|tp| tp.name.as_str()).collect();
        let mut by_name: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
        for (idx, method) in methods.iter().enumerate() {
            by_name.entry(method.node.name.as_str()).or_default().push(idx);
        }

        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        for method in methods {
            if !visited.insert(method.node.name.as_str()) {
                continue;
            }
            let Some(indexes) = by_name.get(method.node.name.as_str()) else {
                continue;
            };
            if indexes.len() == 1 {
                out.push(methods[indexes[0]].clone());
                continue;
            }

            let mut inherent_indexes = Vec::new();
            for idx in indexes {
                if !self.method_matches_adopted_trait_impl(
                    &methods[*idx].node,
                    type_params,
                    &owner_type_param_names,
                    adopted_traits,
                ) {
                    inherent_indexes.push(*idx);
                }
            }
            if inherent_indexes.len() == 1 {
                out.push(methods[inherent_indexes[0]].clone());
            }
        }
        out
    }

    /// Lower trait implementation for a class.
    ///
    /// Only methods matching trait signatures go in `impl Trait for Type`.
    pub(in crate::backend::ir::lower) fn lower_trait_impl(
        &mut self,
        input: TraitImplLoweringInput<'_>,
    ) -> Result<IrImpl, LoweringError> {
        let TraitImplLoweringInput {
            type_name,
            type_params,
            trait_name,
            trait_type_args,
            impl_methods,
            impl_properties,
            impl_associated_types,
        } = input;
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        let prev = self.current_impl_type.replace(type_name.to_string());
        let lowered_result = (|| {
            let has_local_trait_decl = self.trait_decls.contains_key(trait_name);
            let associated_types = if has_local_trait_decl {
                // Source Incan traits do not currently lower associated type declarations into the trait header.
                Vec::new()
            } else {
                self.lower_associated_types_for_trait_impl(
                    impl_associated_types,
                    trait_name,
                    &trait_type_args,
                    &type_param_names,
                )
            };
            // Avoid holding an immutable borrow of `self` across lowering calls.
            //
            // In multi-module lowering, imported trait declarations may live in a different module AST and therefore
            // not be present in `self.trait_decls` for this module. Typechecker already validates trait
            // conformance, so lowering should stay permissive and emit an impl block from the methods we do
            // have instead of hard-failing.
            let Some(trait_decl) = self.trait_decls.get(trait_name).cloned() else {
                let mut methods: Vec<IrFunction> = Vec::new();
                for method in impl_methods {
                    if !self.method_matches_imported_trait_without_decl(&method.node, trait_name) {
                        continue;
                    }
                    if self.method_trait_target_matches_impl(
                        &method.node,
                        trait_name,
                        &trait_type_args,
                        &type_param_names,
                    ) {
                        methods.push(self.lower_impl_method_for_trait(&method.node, Some(&type_param_names))?);
                    }
                }
                for property in impl_properties {
                    methods.push(self.lower_property_with_type_params(
                        &property.node,
                        Some(&type_param_names),
                        PropertyLoweringMode::TraitImpl,
                    )?);
                }
                return Ok(IrImpl {
                    target_type: type_name.to_string(),
                    type_params: Self::lower_type_params(type_params),
                    trait_name: Some(trait_name.to_string()),
                    trait_type_args,
                    associated_types,
                    methods,
                });
            };
            let trait_type_params = trait_decl.type_params;
            let trait_properties = trait_decl.properties;
            let mut trait_methods = trait_decl.methods;
            if trait_name == core_traits::as_str(TraitId::Iterator) {
                trait_methods.retain(|method| method.node.name == "__next__");
            }

            let mut methods: Vec<IrFunction> = Vec::new();
            for trait_property in &trait_properties {
                let property_name = trait_property.node.name.as_str();

                let mut found_override: Option<&ast::PropertyDecl> = None;
                for property in impl_properties {
                    if property.node.name == property_name
                        && self.trait_impl_property_override_matches(
                            &trait_property.node,
                            &property.node,
                            &trait_type_params,
                            &trait_type_args,
                            &type_param_names,
                        )
                    {
                        found_override = Some(&property.node);
                        break;
                    }
                }
                if let Some(property) = found_override {
                    methods.push(self.lower_property_with_type_params(
                        property,
                        Some(&type_param_names),
                        PropertyLoweringMode::TraitImpl,
                    )?);
                    continue;
                }

                return Err(LoweringError {
                    message: format!(
                        "Type '{type_name}' does not implement required property '{property_name}' for trait '{trait_name}'"
                    ),
                    span: IrSpan::default(),
                });
            }

            for trait_method in &trait_methods {
                let method_name = trait_method.node.name.as_str();

                // Prefer the implementing type's override, if present.
                let mut found_override: Option<&ast::MethodDecl> = None;
                for m in impl_methods {
                    if m.node.name == method_name
                        && self.method_trait_target_matches_impl(
                            &m.node,
                            trait_name,
                            &trait_type_args,
                            &type_param_names,
                        )
                        && self.trait_impl_override_matches(
                            &trait_method.node,
                            &m.node,
                            &trait_type_params,
                            &trait_type_args,
                            &type_param_names,
                        )
                    {
                        found_override = Some(&m.node);
                        break;
                    }
                }
                if let Some(m) = found_override {
                    methods.push(self.lower_impl_method_for_trait(m, Some(&type_param_names))?);
                    continue;
                }

                // Otherwise, expand a default method body into the impl (RFC 000: defaults may assume adopter fields).
                if trait_method.node.body.is_some() {
                    let helper_paths = self.trait_default_function_paths.get(trait_name).cloned();
                    let has_helper_paths = helper_paths.is_some();
                    if let Some(helper_paths) = helper_paths {
                        self.active_trait_default_function_paths.push(helper_paths);
                    }
                    let lowered = self.lower_impl_method_for_trait(&trait_method.node, Some(&type_param_names));
                    if has_helper_paths {
                        self.active_trait_default_function_paths.pop();
                    }
                    methods.push(lowered?);
                    continue;
                }

                // Some stdlib traits expose source-level obligations that are intentionally satisfied by backend
                // derive expansion. Keep collecting ordinary missing-method errors for all other traits.
                if Self::backend_default_trait_method(trait_name, method_name) {
                    continue;
                }

                // Required trait method with no default implementation.
                return Err(LoweringError {
                    message: format!(
                        "Type '{type_name}' does not implement required method '{method_name}' for trait '{trait_name}'"
                    ),
                    span: IrSpan::default(),
                });
            }

            Ok(IrImpl {
                target_type: type_name.to_string(),
                type_params: Self::lower_type_params(type_params),
                trait_name: Some(trait_name.to_string()),
                trait_type_args,
                associated_types,
                methods,
            })
        })();
        self.current_impl_type = prev;
        lowered_result
    }

    /// Lower one concrete impl method while preserving owner and method type parameters.
    fn lower_impl_method_for_trait(
        &mut self,
        m: &ast::MethodDecl,
        type_param_names: Option<&std::collections::HashSet<&str>>,
    ) -> Result<IrFunction, LoweringError> {
        self.push_scope();
        let method_type_param_names: std::collections::HashSet<&str> =
            m.type_params.iter().map(|tp| tp.name.as_str()).collect();
        let combined_type_param_names: std::collections::HashSet<&str> = match type_param_names {
            Some(owner_type_param_names) => owner_type_param_names
                .iter()
                .copied()
                .chain(method_type_param_names.iter().copied())
                .collect(),
            None => method_type_param_names,
        };
        let mut hidden_type_params = Vec::new();
        let mut hidden_counter = 0usize;

        // Handle receiver (self) parameter
        let mut params = Vec::new();
        if let Some(receiver) = &m.receiver {
            params.push(FunctionParam {
                name: "self".to_string(),
                ty: IrType::SelfType,
                mutability: match receiver {
                    ast::Receiver::Immutable => Mutability::Immutable,
                    ast::Receiver::Mutable => Mutability::Mutable,
                },
                is_self: true,
                kind: ast::ParamKind::Normal,
                default: None,
            });
        }

        // Add regular parameters
        let other_params: Vec<FunctionParam> = m
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_callable_param_type(
                    &p.node.ty.node,
                    Some(&combined_type_param_names),
                    &mut hidden_type_params,
                    &mut hidden_counter,
                );
                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: param_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: false,
                    kind: p.node.kind,
                    default: match &p.node.default {
                        Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                        None => None,
                    },
                }
            })
            .collect();
        params.extend(other_params);

        let return_type = self.lower_callable_return_type(&m.return_type.node, Some(&combined_type_param_names));
        let body = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)?
        } else {
            vec![]
        };

        // RFC 023: detect @rust.extern decorator to mark this method as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&m.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&m.decorators);
        let lint_allows = self.extract_rust_lint_allows(&m.decorators);
        let mut all_type_params = Self::lower_type_params(&m.type_params);
        all_type_params.extend(hidden_type_params);

        self.pop_scope();

        Ok(IrFunction {
            name: m.name.clone(),
            params,
            return_type,
            body,
            is_async: m.is_async(),
            is_generator: false,
            visibility: Visibility::Private,
            type_params: std::mem::take(&mut all_type_params),
            is_extern,
            rust_attributes,
            lint_allows,
        })
    }

    /// Lower class methods into an impl block.
    pub(in crate::backend::ir::lower) fn lower_class_methods(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
        properties: &[Spanned<ast::PropertyDecl>],
        adopted_traits: &[Spanned<ast::TraitBound>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = (|| {
            let inherent_methods = self.inherent_methods_for_rust_impl(type_params, methods, adopted_traits);
            let mut lowered_methods = Vec::new();
            for method in inherent_methods {
                lowered_methods.extend(self.lower_decorated_or_plain_methods(
                    type_name,
                    &method.node,
                    Some(&type_param_names),
                )?);
            }
            for property in properties {
                lowered_methods.push(self.lower_property_with_type_params(
                    &property.node,
                    Some(&type_param_names),
                    PropertyLoweringMode::Inherent,
                )?);
            }
            Ok(lowered_methods)
        })();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: lowered_methods,
        })
    }

    /// Lower enum methods into an inherent impl block while preserving owner and method generic parameters.
    ///
    /// Enum method bodies share the same lowering rules as model/class methods, but this dedicated entry point keeps
    /// RFC 050 declaration assembly explicit at the enum boundary.
    pub(in crate::backend::ir::lower) fn lower_enum_methods(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
        adopted_traits: &[Spanned<ast::TraitBound>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        let inherent_methods = self.inherent_methods_for_rust_impl(type_params, methods, adopted_traits);
        let lowered = inherent_methods
            .iter()
            .map(|m| self.lower_decorated_or_plain_methods(type_name, &m.node, Some(&type_param_names)))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?.into_iter().flatten().collect();

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: lowered_methods,
        })
    }

    /// Lower associated type items whose `for Trait[...]` target matches this impl.
    fn lower_associated_types_for_trait_impl(
        &mut self,
        associated_types: &[Spanned<ast::AssociatedTypeDecl>],
        trait_name: &str,
        trait_type_args: &[IrType],
        type_param_names: &std::collections::HashSet<&str>,
    ) -> Vec<IrAssociatedType> {
        associated_types
            .iter()
            .filter_map(|associated_type| {
                if !self.trait_bound_matches_impl_target(
                    &associated_type.node.trait_target.node,
                    trait_name,
                    trait_type_args,
                    type_param_names,
                ) {
                    return None;
                }
                Some(IrAssociatedType {
                    name: associated_type.node.name.clone(),
                    ty: self.lower_type_with_type_params(&associated_type.node.ty.node, Some(type_param_names)),
                })
            })
            .collect()
    }

    /// Lower an inherent method while preserving owner and method generic parameters in signatures and bodies.
    ///
    /// During `@classmethod` bodies this also exposes the current impl target as the lowering target for source
    /// `cls(...)` constructor calls. The marker is scoped to the body lowering so ordinary methods and local `cls`
    /// bindings keep their normal value-call behavior.
    fn lower_method_with_type_params(
        &mut self,
        m: &ast::MethodDecl,
        type_param_names: Option<&std::collections::HashSet<&str>>,
    ) -> Result<IrFunction, LoweringError> {
        self.lower_method_named_with_type_params(m, m.name.clone(), Visibility::Public, type_param_names)
    }

    /// Lower an inherent method using an explicit emitted name and visibility.
    fn lower_method_named_with_type_params(
        &mut self,
        m: &ast::MethodDecl,
        name: String,
        visibility: Visibility,
        type_param_names: Option<&std::collections::HashSet<&str>>,
    ) -> Result<IrFunction, LoweringError> {
        self.push_scope();
        let method_type_param_names: std::collections::HashSet<&str> =
            m.type_params.iter().map(|tp| tp.name.as_str()).collect();
        let combined_type_param_names: std::collections::HashSet<&str> = match type_param_names {
            Some(owner_type_param_names) => owner_type_param_names
                .iter()
                .copied()
                .chain(method_type_param_names.iter().copied())
                .collect(),
            None => method_type_param_names,
        };
        let mut hidden_type_params = Vec::new();
        let mut hidden_counter = 0usize;

        let mut params: Vec<FunctionParam> = Vec::new();

        // Add self parameter if receiver is present
        if let Some(receiver) = m.receiver {
            let is_mut = matches!(receiver, ast::Receiver::Mutable);
            params.push(FunctionParam {
                name: "self".to_string(),
                ty: IrType::Unknown, // Will be determined by impl context
                mutability: if is_mut {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                },
                is_self: true,
                kind: ast::ParamKind::Normal,
                default: None,
            });
            // Add self to scope
            self.define_local_binding("self".to_string(), IrType::Unknown, false);
        }

        // Add regular parameters
        let other_params: Vec<FunctionParam> = m
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_callable_param_type(
                    &p.node.ty.node,
                    Some(&combined_type_param_names),
                    &mut hidden_type_params,
                    &mut hidden_counter,
                );
                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                // For mutable parameters, wrap in RefMut
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(param_ty.clone()))
                } else {
                    param_ty.clone()
                };
                self.define_local_binding(p.node.name.clone(), ty.clone(), false);
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: param_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: p.node.name == keywords::as_str(KeywordId::SelfKw),
                    kind: p.node.kind,
                    default: match &p.node.default {
                        Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                        None => None,
                    },
                }
            })
            .collect();
        params.extend(other_params);

        let return_type = self.lower_callable_return_type(&m.return_type.node, Some(&combined_type_param_names));
        let previous_classmethod_constructor = self.current_classmethod_constructor.take();
        if Self::method_has_decorator(m, DecoratorId::ClassMethod)
            && let Some(type_name) = self.current_impl_type.clone()
        {
            self.current_classmethod_constructor = Some(type_name);
        }
        self.push_callable_param_scope(&params);
        let body_result = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)
        } else {
            // Abstract method with no body
            Ok(vec![])
        };
        self.current_classmethod_constructor = previous_classmethod_constructor;
        if body_result.is_ok() {
            for param in &mut params {
                if matches!(param.ty, IrType::Function { .. }) {
                    let refined_ty = self.lookup_var(&param.name);
                    if matches!(refined_ty, IrType::Function { .. }) {
                        param.ty = refined_ty;
                    }
                }
            }
        }
        self.pop_callable_param_scope();
        let body = match body_result {
            Ok(body) => body,
            Err(err) => {
                self.pop_scope();
                return Err(err);
            }
        };
        self.pop_scope();

        let is_extern = Self::has_rust_extern_decorator(&m.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&m.decorators);
        let lint_allows = self.extract_rust_lint_allows(&m.decorators);
        let mut all_type_params = Self::lower_type_params(&m.type_params);
        all_type_params.extend(hidden_type_params);

        Ok(IrFunction {
            name,
            params,
            return_type,
            body,
            is_async: m.is_async(),
            is_generator: false,
            visibility,
            type_params: std::mem::take(&mut all_type_params),
            is_extern,
            rust_attributes,
            lint_allows,
        })
    }

    /// Lower a computed property declaration into the zero-argument function form used by IR emission.
    pub(in crate::backend::ir::lower) fn lower_property_with_type_params(
        &mut self,
        property: &ast::PropertyDecl,
        type_param_names: Option<&std::collections::HashSet<&str>>,
        mode: PropertyLoweringMode,
    ) -> Result<IrFunction, LoweringError> {
        self.push_scope();
        let mut params = vec![FunctionParam {
            name: "self".to_string(),
            ty: match mode {
                PropertyLoweringMode::TraitDecl | PropertyLoweringMode::TraitImpl => IrType::SelfType,
                PropertyLoweringMode::Inherent => IrType::Unknown,
            },
            mutability: Mutability::Immutable,
            is_self: true,
            kind: ast::ParamKind::Normal,
            default: None,
        }];
        self.define_local_binding("self".to_string(), IrType::Unknown, false);

        let return_type = self.lower_callable_return_type(&property.return_type.node, type_param_names);
        let body_result = match mode {
            PropertyLoweringMode::TraitDecl => Ok(Vec::new()),
            PropertyLoweringMode::Inherent | PropertyLoweringMode::TraitImpl => {
                if let Some(body_stmts) = &property.body {
                    self.lower_statements(body_stmts)
                } else {
                    Ok(Vec::new())
                }
            }
        };
        self.pop_scope();
        let body = body_result?;

        let visibility = match mode {
            PropertyLoweringMode::Inherent => Self::map_visibility(property.visibility),
            PropertyLoweringMode::TraitDecl | PropertyLoweringMode::TraitImpl => Visibility::Private,
        };

        Ok(IrFunction {
            name: property.name.clone(),
            params: std::mem::take(&mut params),
            return_type,
            body,
            is_async: false,
            is_generator: false,
            visibility,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })
    }
}

#[derive(Clone, Copy)]
pub(in crate::backend::ir::lower) enum PropertyLoweringMode {
    Inherent,
    TraitDecl,
    TraitImpl,
}
