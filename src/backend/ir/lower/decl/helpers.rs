//! Shared helpers: type parameter lowering, trait-bound mapping, and derive extraction.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::{IrRustAttrArg, IrRustAttribute, IrRustLintAllow, IrTraitBound, IrTypeParam};
use super::super::super::types::IrType;
use super::super::AstLowering;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::decorator_resolution;
use incan_core::interop::is_rust_capability_bound;
use incan_core::lang::conventions;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::trait_bounds;
use incan_core::lang::types::numerics::{self, NumericTypeId};

impl AstLowering {
    // ========================================================================
    // RFC 023: Type parameter lowering with trait bounds
    // ========================================================================

    /// Return whether this decorator should lower through RFC 036 user-defined decorator semantics.
    pub(in crate::backend::ir::lower) fn is_user_defined_decorator_candidate(&self, dec: &ast::Decorator) -> bool {
        let resolved = decorator_resolution::resolve_decorator_path(dec, &self.import_aliases);
        if decorators::from_segments(&resolved).is_some() {
            return false;
        }
        !resolved
            .first()
            .is_some_and(|first| decorators::is_known_decorator_namespace(first))
    }

    /// Lower AST type parameters to IR type parameters, mapping explicit `with` bounds to Rust trait paths.
    ///
    /// RFC 023: Incan trait names (e.g., `Eq`) are mapped to their Rust equivalents (e.g., `PartialEq`).
    /// Inferred bounds from body scanning are added later during emission.
    pub(in crate::backend::ir::lower) fn lower_type_params(ast_params: &[ast::TypeParam]) -> Vec<IrTypeParam> {
        let mut lowered: Vec<IrTypeParam> = Vec::new();
        for tp in ast_params {
            // RFC 041 capability shorthand support:
            // `T with Send, Sync` is parsed as two type params (`T with Send`, `Sync`).
            // Fold trailing bare capability markers back into the prior capability-bounded type param so codegen emits
            // `T: Send + Sync`.
            if tp.bounds.is_empty()
                && is_rust_capability_bound(tp.name.as_str())
                && let Some(prev) = lowered.last_mut()
            {
                let prev_is_capability_bounded = prev.bounds.iter().any(|bound| {
                    matches!(
                        bound.origin,
                        super::super::super::decl::IrTraitBoundOrigin::RustCapability
                    )
                });
                if prev_is_capability_bounded
                    && !prev.bounds.iter().any(|bound| {
                        bound.trait_path == tp.name && bound.type_args.is_empty() && bound.assoc_types.is_empty()
                    })
                {
                    prev.bounds
                        .push(IrTraitBound::with_type_args_classified(tp.name.clone(), Vec::new()));
                    continue;
                }
            }

            lowered.push(Self::lower_type_param(tp));
        }
        lowered
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
    /// Uses the same `incan_core` registries as [`AstLowering::lower_type_with_type_params`] for primitive name
    /// resolution so the two stay in sync when new primitive types are added.
    fn lower_bound_type(ty: &ast::Type) -> IrType {
        match ty {
            ast::Type::Qualified(segments) => IrType::Struct(segments.join("::")),
            ast::Type::Simple(name) => {
                let n = name.as_str();
                if n == conventions::NONE_TYPE_NAME || n == conventions::UNIT_TYPE_NAME {
                    return IrType::Unit;
                }
                if let Some(id) = numerics::from_str(n) {
                    return match n {
                        "int" => IrType::Int,
                        "float" => IrType::Float,
                        "bool" => IrType::Bool,
                        _ => match id {
                            NumericTypeId::Bool => IrType::Bool,
                            _ => IrType::Numeric(id),
                        },
                    };
                }
                if n == "str" {
                    return IrType::String;
                }
                IrType::Generic(name.clone())
            }
            ast::Type::ConstrainedPrimitive(name, _) => {
                let n = name.as_str();
                if let Some(id) = numerics::from_str(n) {
                    return match n {
                        "int" => IrType::Int,
                        "float" => IrType::Float,
                        "bool" => IrType::Bool,
                        _ => match id {
                            NumericTypeId::Bool => IrType::Bool,
                            _ => IrType::Numeric(id),
                        },
                    };
                }
                IrType::Generic(name.clone())
            }
            ast::Type::Generic(base, args) => {
                let lowered_args = args.iter().map(|arg| Self::lower_bound_type(&arg.node)).collect();
                IrType::NamedGeneric(base.clone(), lowered_args)
            }
            ast::Type::Function(params, ret) => IrType::Function {
                params: params.iter().map(|param| Self::lower_bound_type(&param.node)).collect(),
                ret: Box::new(Self::lower_bound_type(&ret.node)),
            },
            ast::Type::Ref(inner) => IrType::Ref(Box::new(Self::lower_bound_type(&inner.node))),
            ast::Type::RefMut(inner) => IrType::RefMut(Box::new(Self::lower_bound_type(&inner.node))),
            ast::Type::Unit => IrType::Unit,
            ast::Type::Tuple(items) => {
                IrType::Tuple(items.iter().map(|item| Self::lower_bound_type(&item.node)).collect())
            }
            ast::Type::SelfType => IrType::SelfType,
            ast::Type::IntLiteral(_) => IrType::Unknown,
            ast::Type::Infer => IrType::Unknown,
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
        IrTraitBound::with_type_args_classified(trait_path, type_args)
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
                .is_some_and(|info| info.trait_type_params.contains_key(name))
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
                Some(IrTraitBound::with_type_args_classified(trait_path, Vec::new()))
            }
            ast::Type::Generic(base, args) if self.is_known_trait_name(base) => {
                let trait_path = trait_bounds::incan_to_rust(base)
                    .map(str::to_string)
                    .unwrap_or_else(|| base.clone());
                let type_args = args
                    .iter()
                    .map(|arg| self.lower_type_with_type_params(&arg.node, type_param_names))
                    .collect();
                Some(IrTraitBound::with_type_args_classified(trait_path, type_args))
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
                            if derives::from_str(name).is_some() {
                                Self::push_unique(&mut derives, name.clone());
                                continue;
                            }

                            if let Some(module_path) = self.module_path_for_derive_name(name)
                                && let Some(traits) = self.derivable_traits_for_module(&module_path)
                            {
                                for trait_name in traits {
                                    for path in self.rust_derive_paths_for_trait(&module_path, &trait_name) {
                                        Self::push_rust_derive_path(&mut derives, path);
                                    }
                                }
                                continue;
                            }

                            let resolved = self.resolve_derive_path(name);
                            if resolved.len() >= 2 {
                                let module_segments = &resolved[..resolved.len() - 1];
                                let trait_name = &resolved[resolved.len() - 1];
                                let rust_derive_paths = self.rust_derive_paths_for_trait(module_segments, trait_name);
                                if rust_derive_paths.is_empty() {
                                    if let Some(meta) = self.stdlib_cache.lookup_trait_meta(module_segments, trait_name)
                                    {
                                        if let Some(module_path) = meta.rust_module_path {
                                            Self::push_unique(&mut derives, name.clone());
                                            derive_rust_modules.insert(name.clone(), module_path);
                                        }
                                        continue;
                                    }
                                    if self.derivable_trait_exists(module_segments, trait_name) {
                                        continue;
                                    }
                                } else {
                                    for path in rust_derive_paths {
                                        Self::push_rust_derive_path(&mut derives, path);
                                    }
                                    continue;
                                }
                            }

                            Self::push_unique(&mut derives, name.clone());
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

    /// Return trait impl targets introduced by RFC 024 module-level derives such as `@derive(json)`.
    pub(in crate::backend::ir::lower) fn derive_trait_impl_targets(
        &mut self,
        decorators: &[Spanned<ast::Decorator>],
    ) -> Vec<(String, Vec<IrType>)> {
        let mut targets = Vec::new();
        for decorator in decorators {
            if decorators::from_str(decorator.node.name.as_str()) != Some(DecoratorId::Derive) {
                continue;
            }
            for arg in &decorator.node.args {
                let ast::DecoratorArg::Positional(expr) = arg else {
                    continue;
                };
                let ast::Expr::Ident(name) = &expr.node else {
                    continue;
                };
                if derives::from_str(name).is_some() {
                    continue;
                }
                if let Some(module_path) = self.module_path_for_derive_name(name)
                    && let Some(traits) = self.derivable_traits_for_module(&module_path)
                {
                    for trait_name in traits {
                        let target = format!("{name}.{trait_name}");
                        if !targets.iter().any(|(existing, _)| existing == &target) {
                            targets.push((target, Vec::new()));
                        }
                    }
                    continue;
                }
                let resolved = self.resolve_derive_path(name);
                if resolved.len() >= 2 {
                    let module_segments = &resolved[..resolved.len() - 1];
                    let trait_name = &resolved[resolved.len() - 1];
                    if self.derivable_trait_exists(module_segments, trait_name)
                        && !targets.iter().any(|(existing, _)| existing == name)
                    {
                        targets.push((name.clone(), Vec::new()));
                    }
                }
            }
        }
        targets
    }

    /// Look up RFC 024 derivable traits for a module, preferring imported dependency metadata over stdlib metadata.
    fn derivable_traits_for_module(&mut self, module_path: &[String]) -> Option<Vec<String>> {
        let key = module_path.join(".");
        if let Some(traits) = self
            .type_info
            .as_ref()
            .and_then(|info| info.derivable_modules.get(&key))
        {
            return Some(traits.clone());
        }
        self.stdlib_cache.lookup_derivable_traits(module_path)
    }

    /// Return Rust `#[derive(...)]` paths attached to one derivable trait.
    fn rust_derive_paths_for_trait(&mut self, module_path: &[String], trait_name: &str) -> Vec<String> {
        let key = format!("{}.{}", module_path.join("."), trait_name);
        if let Some(paths) = self
            .type_info
            .as_ref()
            .and_then(|info| info.trait_rust_derive_paths.get(&key))
        {
            return paths.clone();
        }
        self.stdlib_cache
            .lookup_trait_meta(module_path, trait_name)
            .map(|meta| meta.rust_derive_paths)
            .unwrap_or_default()
    }

    /// Return whether a module-qualified trait is known to participate in the RFC 024 derive protocol.
    fn derivable_trait_exists(&mut self, module_path: &[String], trait_name: &str) -> bool {
        let module_key = module_path.join(".");
        if self
            .type_info
            .as_ref()
            .and_then(|info| info.derivable_modules.get(&module_key))
            .is_some_and(|traits| traits.iter().any(|candidate| candidate == trait_name))
        {
            return true;
        }
        if self.type_info.as_ref().is_some_and(|info| {
            info.trait_rust_derive_paths
                .contains_key(&format!("{module_key}.{trait_name}"))
        }) {
            return true;
        }
        self.stdlib_cache.lookup_trait_meta(module_path, trait_name).is_some()
    }

    /// Append a string only when it is not already present.
    fn push_unique(items: &mut Vec<String>, value: String) {
        if !items.iter().any(|item| item == &value) {
            items.push(value);
        }
    }

    /// Add a Rust derive path, normalizing serde derive paths to the existing derive registry names.
    fn push_rust_derive_path(derives: &mut Vec<String>, path: String) {
        let value = match path.as_str() {
            "serde::Serialize" => derives::as_str(DeriveId::Serialize).to_string(),
            "serde::Deserialize" => derives::as_str(DeriveId::Deserialize).to_string(),
            _ => path,
        };
        Self::push_unique(derives, value);
    }

    /// Forward explicit `with Serialize` / `with Deserialize` adoption into Rust derive emission.
    ///
    /// This keeps direct-interop serde trait defaults honest: a type that adopts the stdlib serde trait surface must
    /// also satisfy the matching Rust-side serde capability when codegen expands those methods.
    pub(in crate::backend::ir::lower) fn extend_derives_with_adopted_serde_traits(
        &self,
        derives: &mut Vec<String>,
        trait_bounds: &[Spanned<ast::TraitBound>],
    ) {
        fn has(derives: &[String], name: &str) -> bool {
            derives.iter().any(|d| d == name)
        }

        let serialize = derives::as_str(DeriveId::Serialize);
        let deserialize = derives::as_str(DeriveId::Deserialize);

        for bound in trait_bounds {
            match bound.node.name.as_str() {
                name if name == serialize && !has(derives, serialize) => derives.push(serialize.to_string()),
                name if name == deserialize && !has(derives, deserialize) => derives.push(deserialize.to_string()),
                name if name.ends_with(".Serialize") && !has(derives, serialize) => derives.push(serialize.to_string()),
                name if name.ends_with(".Deserialize") && !has(derives, deserialize) => {
                    derives.push(deserialize.to_string());
                }
                _ => {}
            }
        }
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

    /// Extract targeted Rust lint suppressions from RFC 057 `@rust.allow(...)` decorators.
    ///
    /// Typechecking validates the decorator shape and lint names; lowering only preserves the already-validated
    /// string literal payloads as explicit IR metadata for item-boundary emission.
    pub(in crate::backend::ir::lower) fn extract_rust_lint_allows(
        &self,
        decorators: &[Spanned<ast::Decorator>],
    ) -> Vec<IrRustLintAllow> {
        decorators
            .iter()
            .filter(|decorator| {
                let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, &self.import_aliases);
                decorators::from_segments(&resolved) == Some(DecoratorId::RustAllow)
            })
            .flat_map(|decorator| {
                decorator.node.args.iter().filter_map(|arg| match arg {
                    ast::DecoratorArg::Positional(expr) => match &expr.node {
                        ast::Expr::Literal(ast::Literal::String(lint)) => Some(IrRustLintAllow { lint: lint.clone() }),
                        _ => None,
                    },
                    ast::DecoratorArg::Named(_, _) => None,
                })
            })
            .collect()
    }

    /// Check whether a `rust.module()` path qualifies for decorator passthrough.
    ///
    /// `incan_stdlib::*` decorators are runtime/runner markers (e.g. `std.testing.parametrize`) and must not be emitted
    /// as Rust attributes — they are interpreted by the Incan test runner, not by `rustc`. Passthrough is reserved for
    /// external Rust-backed proc-macro crates like `incan_web_macros`.
    fn is_passthrough_rust_module(module_path: &str) -> bool {
        !module_path.starts_with("incan_stdlib::")
    }

    /// Resolve a derive argument through the import alias map as if it were a decorator path.
    fn resolve_derive_path(&self, derive_name: &str) -> Vec<String> {
        decorator_resolution::resolve_decorator_path(
            &ast::Decorator {
                path: ast::ImportPath {
                    segments: vec![derive_name.to_string()],
                    is_absolute: false,
                    parent_levels: 0,
                },
                name: derive_name.to_string(),
                is_call: false,
                args: Vec::new(),
            },
            &self.import_aliases,
        )
    }

    /// Return the imported module path for a whole-module derive argument.
    fn module_path_for_derive_name(&self, derive_name: &str) -> Option<Vec<String>> {
        self.import_aliases.get(derive_name).cloned()
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
                ast::Literal::Int(i) => Some(i.value.to_string()),
                ast::Literal::Float(f) => Some(f.value.to_string()),
                ast::Literal::Decimal(_) => None,
                ast::Literal::Bool(b) => Some(b.to_string()),
                ast::Literal::Bytes(bytes) => Some(format!("{bytes:?}")),
                ast::Literal::None => Some("()".to_string()),
            },
            ast::Expr::Ident(name) => Some(format!("{name:?}")),
            ast::Expr::List(items) => {
                let mut out = Vec::new();
                for item in items {
                    let ast::ListEntry::Element(value) = item else {
                        return None;
                    };
                    out.push(Self::serialize_expr(&value.node)?);
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
            ast::Type::Qualified(segments) => segments.join("::"),
            ast::Type::Generic(name, args) => {
                let inner = args
                    .iter()
                    .map(|a| Self::serialize_type(&a.node))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}<{inner}>")
            }
            ast::Type::ConstrainedPrimitive(_, _) => ty.to_string(),
            ast::Type::Function(_, _) => "fn".to_string(),
            ast::Type::Ref(inner) => format!("&{}", Self::serialize_type(&inner.node)),
            ast::Type::RefMut(inner) => format!("&mut {}", Self::serialize_type(&inner.node)),
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
            ast::Type::IntLiteral(value) => value.repr.clone(),
            ast::Type::Infer => "_".to_string(),
        }
    }
}
