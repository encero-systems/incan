//! Method lowering: model methods, class methods, trait impl methods, and general method lowering.

use std::collections::HashSet;

use super::super::super::decl::{FunctionParam, IrFunction, IrImpl, Visibility};
use super::super::super::types::IrType;
use super::super::super::{IrSpan, Mutability};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::ResolvedType;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::keywords::{self, KeywordId};

impl AstLowering {
    /// Return whether a method carries a resolved builtin decorator.
    fn method_has_decorator(method: &ast::MethodDecl, id: DecoratorId) -> bool {
        method
            .decorators
            .iter()
            .any(|decorator| decorators::from_segments(&decorator.node.path.segments) == Some(id))
    }

    /// Trait type-parameter names from either local AST declarations or typechecker metadata.
    fn trait_type_param_names(&self, trait_name: &str) -> Option<Vec<String>> {
        if let Some(decl) = self.trait_decls.get(trait_name) {
            return Some(decl.type_params.iter().map(|tp| tp.name.clone()).collect());
        }
        self.type_info
            .as_ref()
            .and_then(|info| info.trait_type_params.get(trait_name).cloned())
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
        let Some(direct_supertraits) = type_info.trait_direct_supertraits.get(trait_name) else {
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

        vec![(
            bound.name.clone(),
            bound.type_args.iter().map(|arg| self.lower_type(&arg.node)).collect(),
        )]
    }

    /// Lower model methods into an impl block.
    pub(in crate::backend::ir::lower) fn lower_model_methods(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = methods
            .iter()
            .map(|m| self.lower_method_with_type_params(&m.node, Some(&type_param_names)))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            trait_type_args: Vec::new(),
            methods: lowered_methods,
        })
    }

    /// Lower trait implementation for a class.
    ///
    /// Only methods matching trait signatures go in `impl Trait for Type`.
    pub(in crate::backend::ir::lower) fn lower_trait_impl(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        trait_name: &str,
        trait_type_args: Vec<IrType>,
        impl_methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        let prev = self.current_impl_type.replace(type_name.to_string());
        let lowered_result = (|| {
            // Avoid holding an immutable borrow of `self` across lowering calls.
            //
            // In multi-module lowering, imported trait declarations may live in a different module AST and therefore
            // not be present in `self.trait_decls` for this module. Typechecker already validates trait
            // conformance, so lowering should stay permissive and emit an impl block from the methods we do
            // have instead of hard-failing.
            let Some(trait_decl) = self.trait_decls.get(trait_name).cloned() else {
                let mut methods: Vec<IrFunction> = Vec::new();
                for method in impl_methods {
                    methods.push(self.lower_impl_method_for_trait(&method.node, Some(&type_param_names))?);
                }
                return Ok(IrImpl {
                    target_type: type_name.to_string(),
                    type_params: Self::lower_type_params(type_params),
                    trait_name: Some(trait_name.to_string()),
                    trait_type_args,
                    methods,
                });
            };
            let trait_methods = trait_decl.methods;

            let mut methods: Vec<IrFunction> = Vec::new();
            for trait_method in &trait_methods {
                let method_name = trait_method.node.name.as_str();

                // Prefer the implementing type's override, if present.
                let mut found_override: Option<&ast::MethodDecl> = None;
                for m in impl_methods {
                    if m.node.name == method_name {
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
                    methods.push(self.lower_impl_method_for_trait(&trait_method.node, Some(&type_param_names))?);
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
                methods,
            })
        })();
        self.current_impl_type = prev;
        lowered_result
    }

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
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: false,
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
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = methods
            .iter()
            .map(|m| self.lower_method_with_type_params(&m.node, Some(&type_param_names)))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            trait_type_args: Vec::new(),
            methods: lowered_methods,
        })
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
                // For mutable parameters, wrap in RefMut
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(base_ty.clone()))
                } else {
                    base_ty.clone()
                };
                self.define_local_binding(p.node.name.clone(), ty.clone(), false);
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: p.node.name == keywords::as_str(KeywordId::SelfKw),
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
        let body_result = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)
        } else {
            // Abstract method with no body
            Ok(vec![])
        };
        self.current_classmethod_constructor = previous_classmethod_constructor;
        let body = body_result?;
        self.pop_scope();

        // Incan methods are part of the type's public surface. Trait-impl methods are handled separately in
        // `lower_impl_method_for_trait`, so inherent methods can be emitted as public here.
        let visibility = Visibility::Public;
        let is_extern = Self::has_rust_extern_decorator(&m.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&m.decorators);
        let lint_allows = self.extract_rust_lint_allows(&m.decorators);
        let mut all_type_params = Self::lower_type_params(&m.type_params);
        all_type_params.extend(hidden_type_params);

        Ok(IrFunction {
            name: m.name.clone(),
            params,
            return_type,
            body,
            is_async: m.is_async(),
            visibility,
            type_params: std::mem::take(&mut all_type_params),
            is_extern,
            rust_attributes,
            lint_allows,
        })
    }
}
