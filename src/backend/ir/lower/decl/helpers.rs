//! Shared helpers: type parameter lowering, trait-bound mapping, and derive extraction.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::{IrRustAttrArg, IrRustAttribute, IrTraitBound, IrTypeParam};
use super::super::super::types::IrType;
use super::super::AstLowering;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::decorator_resolution;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::trait_bounds;

impl AstLowering {
    // ========================================================================
    // RFC 023: Type parameter lowering with trait bounds
    // ========================================================================

    /// Lower AST type parameters to IR type parameters, mapping explicit `with` bounds to Rust trait paths.
    ///
    /// RFC 023: Incan trait names (e.g., `Eq`) are mapped to their Rust equivalents (e.g., `PartialEq`).
    /// Inferred bounds from body scanning are added later during emission.
    pub(in crate::backend::ir::lower) fn lower_type_params(ast_params: &[ast::TypeParam]) -> Vec<IrTypeParam> {
        ast_params.iter().map(Self::lower_type_param).collect()
    }

    /// Lower a single AST type parameter to its IR representation.
    fn lower_type_param(tp: &ast::TypeParam) -> IrTypeParam {
        let bounds = tp.bounds.iter().map(Self::lower_trait_bound).collect();
        IrTypeParam {
            name: tp.name.clone(),
            bounds,
        }
    }

    /// Lower a type that appears inside a generic trait bound.
    ///
    /// These positions only need surface-level lowering because they occur in generic declarations, not in
    /// expression-bearing contexts.
    fn lower_bound_type(ty: &ast::Type) -> IrType {
        match ty {
            ast::Type::Simple(name) => match name.as_str() {
                "int" => IrType::Int,
                "float" => IrType::Float,
                "bool" => IrType::Bool,
                "str" => IrType::String,
                "None" | "Unit" => IrType::Unit,
                other => IrType::Generic(other.to_string()),
            },
            ast::Type::Generic(base, args) => {
                let lowered_args = args.iter().map(|arg| Self::lower_bound_type(&arg.node)).collect();
                IrType::NamedGeneric(base.clone(), lowered_args)
            }
            ast::Type::Function(params, ret) => IrType::Function {
                params: params.iter().map(|param| Self::lower_bound_type(&param.node)).collect(),
                ret: Box::new(Self::lower_bound_type(&ret.node)),
            },
            ast::Type::Unit => IrType::Unit,
            ast::Type::Tuple(items) => {
                IrType::Tuple(items.iter().map(|item| Self::lower_bound_type(&item.node)).collect())
            }
            ast::Type::SelfType => IrType::SelfType,
        }
    }

    /// Map an Incan trait bound to the corresponding Rust trait bound.
    ///
    /// Uses the `incan_core::lang::trait_bounds` registry to resolve known Incan names to their Rust trait paths (e.g.,
    /// Incan `Eq` → Rust `PartialEq`). Unknown names are passed through as-is, allowing user-defined trait bounds.
    fn lower_trait_bound(bound: &ast::TraitBound) -> IrTraitBound {
        let trait_path = trait_bounds::incan_to_rust(&bound.name)
            .map(str::to_string)
            .unwrap_or_else(|| bound.name.clone());
        let type_args = bound
            .type_args
            .iter()
            .map(|arg| Self::lower_bound_type(&arg.node))
            .collect();
        IrTraitBound::with_type_args(trait_path, type_args)
    }

    /// Whether `name` resolves to a locally-known trait during lowering.
    ///
    /// This is used to preserve RFC 042 trait-typed signature annotations as compiler-managed abstract types instead of
    /// lowering them as concrete Rust type names.
    pub(in crate::backend::ir::lower) fn is_known_trait_name(&self, name: &str) -> bool {
        self.trait_decls.contains_key(name)
            || self
                .type_info
                .as_ref()
                .is_some_and(|info| info.trait_direct_supertraits.contains_key(name))
    }

    /// Lower an annotation like `Collection[int]` to a Rust trait bound shape.
    pub(in crate::backend::ir::lower) fn lower_trait_annotation_bound(
        &self,
        ty: &ast::Type,
        type_param_names: Option<&HashSet<&str>>,
    ) -> Option<IrTraitBound> {
        match ty {
            ast::Type::Simple(name)
                if !type_param_names.is_some_and(|params| params.contains(name.as_str()))
                    && self.is_known_trait_name(name) =>
            {
                let trait_path = trait_bounds::incan_to_rust(name)
                    .map(str::to_string)
                    .unwrap_or_else(|| name.clone());
                Some(IrTraitBound::simple(trait_path))
            }
            ast::Type::Generic(base, args) if self.is_known_trait_name(base) => {
                let trait_path = trait_bounds::incan_to_rust(base)
                    .map(str::to_string)
                    .unwrap_or_else(|| base.clone());
                let type_args = args
                    .iter()
                    .map(|arg| self.lower_type_with_type_params(&arg.node, type_param_names))
                    .collect();
                Some(IrTraitBound::with_type_args(trait_path, type_args))
            }
            _ => None,
        }
    }

    /// Lower a callable parameter type, synthesizing a hidden Rust generic when the source annotation names a trait.
    pub(in crate::backend::ir::lower) fn lower_callable_param_type(
        &self,
        ty: &ast::Type,
        type_param_names: Option<&HashSet<&str>>,
        hidden_type_params: &mut Vec<IrTypeParam>,
        hidden_counter: &mut usize,
    ) -> IrType {
        if let Some(bound) = self.lower_trait_annotation_bound(ty, type_param_names) {
            let hidden_name = format!("__IncanTrait{}", *hidden_counter);
            *hidden_counter += 1;
            hidden_type_params.push(IrTypeParam {
                name: hidden_name.clone(),
                bounds: vec![bound],
            });
            return IrType::Generic(hidden_name);
        }
        self.lower_type_with_type_params(ty, type_param_names)
    }

    /// Lower a callable return type, preserving trait annotations as Rust `impl Trait` where needed.
    pub(in crate::backend::ir::lower) fn lower_callable_return_type(
        &self,
        ty: &ast::Type,
        type_param_names: Option<&HashSet<&str>>,
    ) -> IrType {
        if let Some(bound) = self.lower_trait_annotation_bound(ty, type_param_names) {
            return IrType::ImplTrait(bound);
        }
        self.lower_type_with_type_params(ty, type_param_names)
    }

    /// Extract derives from decorators.
    ///
    /// Parses `@derive(Serialize, Deserialize)` decorators and returns the list of derive names.
    /// Also adds prerequisite derives (e.g., Eq requires PartialEq).
    pub(in crate::backend::ir::lower) fn extract_derives(
        &mut self,
        decorators: &[Spanned<ast::Decorator>],
    ) -> (Vec<String>, HashMap<String, String>) {
        let mut derives = Vec::new();
        let mut derive_rust_modules = HashMap::new();

        for decorator in decorators {
            if decorators::from_str(decorator.node.name.as_str()) == Some(DecoratorId::Derive) {
                // Extract derive arguments: @derive(Serialize, Deserialize)
                for arg in &decorator.node.args {
                    if let ast::DecoratorArg::Positional(expr) = arg {
                        // Handle simple identifier expressions
                        if let ast::Expr::Ident(name) = &expr.node {
                            derives.push(name.clone());
                            if let Some(module_path) = self.resolve_derive_module_path(name) {
                                derive_rust_modules.insert(name.clone(), module_path);
                            }
                        }
                    }
                }
            }
        }

        fn has(derives: &[String], name: &str) -> bool {
            derives.iter().any(|d| d == name)
        }

        // Add prerequisite derives automatically
        // Eq requires PartialEq
        let eq = derives::as_str(DeriveId::Eq);
        let partial_eq = derives::as_str(DeriveId::PartialEq);
        if has(&derives, eq) && !has(&derives, partial_eq) {
            derives.push(partial_eq.to_string());
        }
        // Ord requires PartialOrd and Eq (and thus PartialEq)
        let ord = derives::as_str(DeriveId::Ord);
        let partial_ord = derives::as_str(DeriveId::PartialOrd);
        if has(&derives, ord) {
            if !has(&derives, partial_ord) {
                derives.push(partial_ord.to_string());
            }
            if !has(&derives, eq) {
                derives.push(eq.to_string());
            }
            if !has(&derives, partial_eq) {
                derives.push(partial_eq.to_string());
            }
        }

        (derives, derive_rust_modules)
    }

    /// Extract passthrough Rust attributes from decorators.
    pub(in crate::backend::ir::lower) fn extract_passthrough_attributes(
        &mut self,
        decorators: &[Spanned<ast::Decorator>],
    ) -> Vec<IrRustAttribute> {
        let mut attrs = Vec::new();
        for decorator in decorators {
            let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, &self.import_aliases);
            if resolved.len() < 2 {
                continue;
            }
            let module_segments = &resolved[..resolved.len() - 1];
            let name = resolved[resolved.len() - 1].clone();
            let Some(fn_info) = self.stdlib_cache.lookup_function_meta(module_segments, &name) else {
                continue;
            };
            if !fn_info.is_rust_extern {
                continue;
            }
            let Some(module_path) = fn_info.rust_module_path else {
                continue;
            };
            if !Self::is_passthrough_rust_module(&module_path) {
                continue;
            }
            attrs.push(IrRustAttribute {
                module_path,
                name,
                args: self.serialize_decorator_args(&decorator.node.args),
            });
        }
        attrs
    }

    /// Check whether a `rust.module()` path qualifies for decorator passthrough.
    ///
    /// `incan_stdlib::*` decorators are runtime/runner markers (e.g. `std.testing.parametrize`) and must not be emitted
    /// as Rust attributes — they are interpreted by the Incan test runner, not by `rustc`. Passthrough is reserved for
    /// external Rust-backed proc-macro crates like `incan_web_macros`.
    fn is_passthrough_rust_module(module_path: &str) -> bool {
        !module_path.starts_with("incan_stdlib::")
    }

    /// Resolve the `rust.module()` backing path for a `@derive(Trait)` reference.
    ///
    /// Uses the import alias table and the `StdlibAstCache` to map a trait name (e.g. `IntoResponse`) back to its
    /// owning Rust module path (e.g. `incan_web_macros`). Returns `None` if the trait is not from a `rust.module()`
    /// stdlib module.
    fn resolve_derive_module_path(&mut self, derive_name: &str) -> Option<String> {
        let resolved = decorator_resolution::resolve_decorator_path(
            &ast::Decorator {
                path: ast::ImportPath {
                    segments: vec![derive_name.to_string()],
                    is_absolute: false,
                    parent_levels: 0,
                },
                name: derive_name.to_string(),
                args: Vec::new(),
            },
            &self.import_aliases,
        );
        if resolved.len() < 2 {
            return None;
        }
        let module_segments = &resolved[..resolved.len() - 1];
        let trait_name = &resolved[resolved.len() - 1];
        self.stdlib_cache
            .lookup_trait_meta(module_segments, trait_name)
            .and_then(|meta| meta.rust_module_path)
    }

    /// Convert AST decorator arguments into their IR representation for Rust attribute emission.
    fn serialize_decorator_args(&self, args: &[ast::DecoratorArg]) -> Vec<IrRustAttrArg> {
        args.iter()
            .filter_map(|arg| match arg {
                ast::DecoratorArg::Positional(expr) => Self::serialize_expr(&expr.node).map(IrRustAttrArg::Positional),
                ast::DecoratorArg::Named(name, value) => match value {
                    ast::DecoratorArgValue::Expr(expr) => {
                        Self::serialize_expr(&expr.node).map(|v| IrRustAttrArg::Named {
                            name: name.clone(),
                            value: v,
                        })
                    }
                    ast::DecoratorArgValue::Type(ty) => Some(IrRustAttrArg::Named {
                        name: name.clone(),
                        value: Self::serialize_type(&ty.node),
                    }),
                },
            })
            .collect()
    }

    /// Serialize an AST expression to a string suitable for embedding in a Rust attribute argument.
    ///
    /// Supports literals, identifiers, and list expressions. Returns `None` for unsupported expression kinds.
    fn serialize_expr(expr: &ast::Expr) -> Option<String> {
        match expr {
            ast::Expr::Literal(lit) => match lit {
                ast::Literal::String(s) => Some(format!("{s:?}")),
                ast::Literal::Int(i) => Some(i.to_string()),
                ast::Literal::Float(f) => Some(f.to_string()),
                ast::Literal::Bool(b) => Some(b.to_string()),
                ast::Literal::Bytes(bytes) => Some(format!("{bytes:?}")),
                ast::Literal::None => Some("()".to_string()),
            },
            ast::Expr::Ident(name) => Some(format!("{name:?}")),
            ast::Expr::List(items) => {
                let mut out = Vec::new();
                for item in items {
                    out.push(Self::serialize_expr(&item.node)?);
                }
                Some(format!("[{}]", out.join(", ")))
            }
            _ => None,
        }
    }

    /// Serialize an AST type to a string suitable for embedding in a Rust attribute argument.
    fn serialize_type(ty: &ast::Type) -> String {
        match ty {
            ast::Type::Simple(name) => name.clone(),
            ast::Type::Generic(name, args) => {
                let inner = args
                    .iter()
                    .map(|a| Self::serialize_type(&a.node))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}<{inner}>")
            }
            ast::Type::Function(_, _) => "fn".to_string(),
            ast::Type::Unit => "()".to_string(),
            ast::Type::Tuple(items) => {
                let inner = items
                    .iter()
                    .map(|a| Self::serialize_type(&a.node))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({inner})")
            }
            ast::Type::SelfType => "Self".to_string(),
        }
    }
}
