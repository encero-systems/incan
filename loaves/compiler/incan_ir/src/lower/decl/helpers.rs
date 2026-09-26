//! Shared helpers: type parameter lowering, trait-bound mapping, and derive extraction.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::{
    IrRustAttrArg, IrRustAttribute, IrRustLintAllow, IrTraitBound, IrTraitBoundOrigin, IrTypeParam, StructField,
};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::ast::{self, Spanned};
use incan_frontend::decorator_resolution;
use incan_lang::interop::{is_rust_callable_capability_bound, is_rust_capability_bound};
use incan_lang::lang::callables;
use incan_lang::lang::decorators::{self, DecoratorId};
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::stdlib;
use incan_lang::lang::trait_bounds;
use incan_lang::lang::traits as core_traits;

const SERDE_SERIALIZE_DERIVE: &str = "serde::Serialize";
const SERDE_DESERIALIZE_DERIVE: &str = "serde::Deserialize";

impl AstLowering {
    // ========================================================================
    // RFC 023: Type parameter lowering with trait bounds
    // ========================================================================

    /// Return whether this decorator should lower through RFC 036 user-defined decorator semantics.
    pub(in crate::lower) fn is_user_defined_decorator_candidate(&self, dec: &ast::Decorator) -> bool {
        let resolved = decorator_resolution::resolve_decorator_path(dec, &self.import_aliases);
        if decorators::from_segments(&resolved) == Some(DecoratorId::Describe) {
            // RFC 113 owns `@describe` as a compiler-recognized declaration form. Its runtime registration is emitted
            // through the defining registry static during module initialization, not through RFC 036 callable wrapping.
            return false;
        }
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
    pub(in crate::lower) fn lower_type_params(&self, ast_params: &[ast::TypeParam]) -> Vec<IrTypeParam> {
        let type_param_names = ast_params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<HashSet<_>>();
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
                let prev_is_capability_bounded = prev
                    .bounds
                    .iter()
                    .any(|bound| matches!(bound.origin, IrTraitBoundOrigin::RustCapability));
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

            lowered.push(self.lower_type_param(tp, &type_param_names));
        }
        lowered
    }

    /// Lower the type parameters of a callable declaration, giving each RFC 041 `Fn`-family marker its callable shape.
    ///
    /// `F with Fn[int]` asks for a Rust `Fn(i64) -> R`: a parameter list the marker names and a return type it does
    /// not. Rust spells that requirement only in its parenthesized form; the angle-bracket `Fn<i64>` a plain trait
    /// path produces is the unstable form rustc refuses (#1716). The canonical `std.traits.callable.CallableN[Args...,
    /// R]` trait carries exactly that shape, and its generated declaration provides the blanket implementation for
    /// native functions and closures, so each marker lowers to that nominal bound by arity. The return type becomes a
    /// hidden type parameter the caller's argument determines, appended after the declared parameters like the other
    /// hidden parameters of a callable; an arity the callable vocabulary does not cover keeps the marker as written.
    ///
    /// Nominal owners (models, classes, enums, traits, newtypes) keep [`Self::lower_type_params`]: a hidden parameter
    /// there would change the type's arity everywhere it is named.
    pub(in crate::lower) fn lower_callable_type_params(&self, ast_params: &[ast::TypeParam]) -> Vec<IrTypeParam> {
        let mut type_params = self.lower_type_params(ast_params);
        let mut hidden_return_types = Vec::new();
        for type_param in &mut type_params {
            for bound in &mut type_param.bounds {
                let Some(callable) = Self::callable_marker_trait(bound) else {
                    continue;
                };
                let hidden_name = format!("__IncanFnReturn{}", hidden_return_types.len());
                let mut type_args = std::mem::take(&mut bound.type_args);
                type_args.push(IrType::Generic(hidden_name.clone()));
                *bound = IrTraitBound::source_callable(Self::source_callable_trait_path(callable), type_args);
                hidden_return_types.push(IrTypeParam {
                    name: hidden_name,
                    bounds: Vec::new(),
                });
            }
        }
        type_params.extend(hidden_return_types);
        type_params
    }

    /// Return the callable trait an `Fn`-family capability marker bound stands for, by its parameter count.
    ///
    /// Only a marker that came in as a Rust capability (`Fn`, `FnMut`, `FnOnce` from `std.rust`) qualifies; a
    /// same-spelled trait from any other origin is not a capability marker.
    fn callable_marker_trait(bound: &IrTraitBound) -> Option<callables::CallableTraitId> {
        if bound.origin != IrTraitBoundOrigin::RustCapability
            || !is_rust_callable_capability_bound(&bound.trait_path)
            || !bound.assoc_types.is_empty()
        {
            return None;
        }
        callables::for_arity(bound.type_args.len())
    }

    /// Return the generated Rust path of one canonical callable trait, qualified from the crate root.
    ///
    /// The `std.rust` markers emit no `use` of their own (`std.rust` is a checker-only namespace), so the bound must
    /// name the trait absolutely; the compiled stdlib is mounted at `crate::__incan_std` in every generated crate.
    fn source_callable_trait_path(callable: callables::CallableTraitId) -> String {
        format!(
            "{}::{}::{}::{}",
            keywords::as_str(KeywordId::Crate),
            stdlib::INCAN_STD_NAMESPACE,
            callables::generated_module(),
            callables::info_for(callable).name
        )
    }

    /// Return the declared type parameters that no lowered field type mentions, in declaration order.
    ///
    /// This is the phantom-parameter fact recorded on every struct-shaped nominal (model, class, newtype). It is
    /// computed once here, after the fields are lowered with the owner's parameters in scope, so the answer does not
    /// depend on which declaration kind produced the fields. See [`IrStruct::phantom_type_params`] and #1370.
    ///
    /// [`IrStruct::phantom_type_params`]: super::super::super::decl::IrStruct::phantom_type_params
    pub(in crate::lower) fn phantom_type_params(type_params: &[IrTypeParam], fields: &[StructField]) -> Vec<String> {
        super::super::super::decl::phantom_type_params(
            type_params.iter().map(|param| param.name.as_str()),
            fields.iter().map(|field| &field.ty),
        )
    }

    /// Lower a single AST type parameter to its IR representation.
    fn lower_type_param(&self, tp: &ast::TypeParam, type_param_names: &HashSet<&str>) -> IrTypeParam {
        let bounds = tp
            .bounds
            .iter()
            .map(|bound| self.lower_trait_bound(bound, type_param_names))
            .collect();
        IrTypeParam {
            name: tp.name.clone(),
            bounds,
        }
    }

    /// Map an Incan trait bound to the corresponding Rust trait bound.
    ///
    /// A bound on a builtin the `incan_lang::lang::trait_bounds` registry maps (Incan `Eq` to Rust `PartialEq`) is
    /// resolved by the trait's identity through [`Self::rust_mapped_builtin_trait_path`], so an alias or a
    /// module-qualified spelling of the same declaration lowers to the same Rust trait. Unknown names are passed
    /// through as-is, allowing user-defined trait bounds.
    fn lower_trait_bound(&self, bound: &ast::TraitBound, type_param_names: &HashSet<&str>) -> IrTraitBound {
        let (module_path, source_name) = self.canonical_trait_identity(&bound.name);
        let callable = module_path
            .as_deref()
            .filter(|path| callables::module_path_matches(path))
            .and(source_name.as_deref())
            .and_then(callables::from_str);
        if let Some(callable) = callable {
            let arity = callables::info_for(callable).arity;
            if bound.type_args.len() == arity + 1 {
                let types = bound
                    .type_args
                    .iter()
                    .map(|arg| self.lower_type_with_type_params(&arg.node, Some(type_param_names)))
                    .collect::<Vec<_>>();
                let trait_path = self
                    .active_trait_default_type_path(&bound.name)
                    .map_or_else(|| bound.name.clone(), |path| path.join("::"));
                return IrTraitBound::source_callable(trait_path, types);
            }
        }
        let trait_path = self
            .rust_mapped_builtin_trait_path(&bound.name)
            .map(str::to_string)
            .or_else(|| self.source_owned_builtin_trait_path(&bound.name))
            .unwrap_or_else(|| bound.name.clone());
        let type_args = bound
            .type_args
            .iter()
            .map(|arg| self.lower_type_with_type_params(&arg.node, Some(type_param_names)))
            .collect();
        IrTraitBound::with_type_args_classified(trait_path, type_args)
    }

    /// Return the Rust trait path a builtin bound maps to, keyed on the trait's resolved identity.
    ///
    /// [`trait_bounds::incan_to_rust`] is a registry over declaration names (`Eq` to `PartialEq`), while a bound or
    /// a checked dispatch carries a spelling. An alias (`from std.derives.comparison import Eq as Equality`) or a
    /// module-qualified name (`comparison.Eq`) would miss a spelling-keyed lookup and lower to the generated
    /// `__incan_std` source trait, which a `@derive(Eq)` type does not implement (#1374). The lookup therefore runs
    /// on the declaration name [`Self::canonical_trait_identity`] resolves, and only when that identity is the
    /// builtin's own: the declaring module is a stdlib module, or the spelling is the implicit builtin binding with
    /// no import or local declaration behind it. A user trait that merely shares the declaration name
    /// (`yaml.Serialize`, a local `trait Eq`) keeps its own path.
    ///
    /// The `std.serde.json` protocol traits are the one identity still looked up by spelling: their bounds and
    /// dispatches are shaped by the protocol machinery (#1431, #1712), where an alias such as `JsonSerialize` lowers
    /// as written and resolves through its re-export, and the registry's `serde::Serialize` mapping is reached only
    /// by the bare `Serialize` and `Deserialize` spellings, exactly as before.
    pub(in crate::lower) fn rust_mapped_builtin_trait_path(&self, visible_name: &str) -> Option<&'static str> {
        let (module_path, source_name) = self.canonical_trait_identity(visible_name);
        let source_name = source_name?;
        let stdlib_identity = module_path.as_deref().is_some_and(stdlib::is_any_stdlib_path);
        if !stdlib_identity && !self.is_implicit_builtin_trait_spelling(visible_name, &source_name) {
            return None;
        }
        let json_protocol = module_path
            .as_deref()
            .is_some_and(|segments| stdlib::stdlib_json_trait_id_for_identity(segments, &source_name).is_some());
        if json_protocol {
            return trait_bounds::incan_to_rust(visible_name);
        }
        trait_bounds::incan_to_rust(&source_name)
    }

    /// Return whether `visible_name` reaches a builtin trait through the implicit prelude binding alone: no module
    /// qualifier, no import, no local declaration claiming the spelling, and the spelling is the declaration name.
    fn is_implicit_builtin_trait_spelling(&self, visible_name: &str, source_name: &str) -> bool {
        !visible_name.contains('.')
            && !self.import_aliases.contains_key(visible_name)
            && !self.trait_decls.contains_key(visible_name)
            && visible_name == source_name
    }

    /// Return the generated path for a builtin trait that is owned by ordinary Incan stdlib source.
    ///
    /// Native Rust capability mappings such as Incan `Eq` to Rust `PartialEq` are handled first by
    /// [`Self::rust_mapped_builtin_trait_path`]. This helper covers source-owned protocols such as `Iterator[T]` and
    /// only accepts either their exact imported owner or the implicit builtin binding. A local or third-party
    /// same-named trait stays on its own path.
    fn source_owned_builtin_trait_path(&self, visible_name: &str) -> Option<String> {
        let (actual_module, source_name) = self.canonical_trait_identity(visible_name);
        let source_name = source_name?;
        let trait_id = core_traits::from_str(&source_name)?;
        let expected_module = core_traits::source_module(trait_id)?;
        let expected_segments = expected_module.split('.').collect::<Vec<_>>();
        let exact_import = actual_module.as_deref().is_some_and(|segments| {
            segments
                .iter()
                .map(String::as_str)
                .eq(expected_segments.iter().copied())
        });
        if !exact_import && !self.is_implicit_builtin_trait_spelling(visible_name, &source_name) {
            return None;
        }

        let generated_module = core_traits::generated_module(trait_id)?;
        Some(format!(
            "crate::__incan_std::{generated_module}::{}",
            core_traits::as_str(trait_id)
        ))
    }

    /// Whether `name` resolves to a locally-known trait during lowering.
    ///
    /// This is used to preserve RFC 042 trait-typed signature annotations as compiler-managed abstract types instead of
    /// lowering them as concrete Rust type names.
    pub(in crate::lower) fn is_known_trait_name(&self, name: &str) -> bool {
        self.trait_decls.contains_key(name)
            || self
                .type_info
                .as_ref()
                .is_some_and(|info| info.traits.type_params.contains_key(name))
    }

    /// Lower an annotation like `Collection[int]` to a Rust trait bound shape.
    pub(in crate::lower) fn lower_trait_annotation_bound(
        &self,
        ty: &ast::Type,
        type_param_names: Option<&HashSet<&str>>,
    ) -> Option<IrTraitBound> {
        match ty {
            ast::Type::Simple(name)
                if !type_param_names.is_some_and(|params| params.contains(name.as_str()))
                    && self.is_known_trait_name(name) =>
            {
                let trait_path = self
                    .rust_mapped_builtin_trait_path(name)
                    .map(str::to_string)
                    .or_else(|| self.source_owned_builtin_trait_path(name))
                    .unwrap_or_else(|| name.clone());
                Some(IrTraitBound::with_type_args_classified(trait_path, Vec::new()))
            }
            ast::Type::Generic(base, args) if self.is_known_trait_name(base) => {
                let trait_path = self
                    .rust_mapped_builtin_trait_path(base)
                    .map(str::to_string)
                    .or_else(|| self.source_owned_builtin_trait_path(base))
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
    pub(in crate::lower) fn lower_callable_param_type(
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
    pub(in crate::lower) fn lower_callable_return_type(
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
    /// Parses `@derive(...)` decorators and returns the Rust derive names or paths they require. Also adds
    /// prerequisite derives (e.g., Eq requires PartialEq).
    pub(in crate::lower) fn extract_derives(
        &mut self,
        decorators: &[Spanned<ast::Decorator>],
    ) -> (Vec<String>, HashMap<String, String>) {
        let mut derives = Vec::new();
        let mut derive_rust_modules = HashMap::new();

        for decorator in decorators {
            let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, &self.import_aliases);
            match decorators::from_segments(&resolved) {
                Some(DecoratorId::Derive) => {
                    // Extract derive arguments: @derive(Serialize, Deserialize)
                    for arg in &decorator.node.args {
                        if let ast::DecoratorArg::Positional(expr) = arg {
                            // Handle simple identifier expressions
                            if let ast::Expr::Ident(name) = &expr.node {
                                if derives::from_str(name) == Some(DeriveId::Descriptor) {
                                    // `Descriptor` is a compiler-checked structural-snapshot opt-in, not a Rust
                                    // derive macro. The checked registry fact retains it; generated Rust must not
                                    // attempt `#[derive(Descriptor)]`.
                                    continue;
                                }
                                if derives::from_str(name).is_some() {
                                    Self::push_unique(&mut derives, name.clone());
                                    continue;
                                }

                                if let Some(rust_path) = self.rust_import_aliases.get(name) {
                                    Self::push_rust_derive_path(&mut derives, rust_path.join("::"));
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
                                    let rust_derive_paths =
                                        self.rust_derive_paths_for_trait(module_segments, trait_name);
                                    if rust_derive_paths.is_empty() {
                                        if let Some(meta) =
                                            self.stdlib_cache.lookup_trait_meta(module_segments, trait_name)
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
                Some(DecoratorId::RustDerive) => {
                    for arg in &decorator.node.args {
                        let ast::DecoratorArg::Positional(expr) = arg else {
                            continue;
                        };
                        if let Some(path) = self.rust_derive_path_from_expr(&expr.node) {
                            Self::push_rust_derive_path(&mut derives, path);
                        }
                    }
                }
                _ => {}
            }
        }

        // Add prerequisite derives automatically, rule by rule in the registry's order (`Eq` brings `PartialEq`; `Ord`
        // brings `PartialOrd`, `Eq` and `PartialEq`). The typechecker's derive relation reads the same table.
        for (derive, implied) in derives::DERIVE_IMPLICATIONS {
            if !derives.iter().any(|d| d == derives::as_str(*derive)) {
                continue;
            }
            for implied_derive in *implied {
                let implied_name = derives::as_str(*implied_derive);
                if !derives.iter().any(|d| d == implied_name) {
                    derives.push(implied_name.to_string());
                }
            }
        }

        (derives, derive_rust_modules)
    }

    /// Convert an `@rust.derive(...)` positional argument into the emitted Rust derive path.
    fn rust_derive_path_from_expr(&self, expr: &ast::Expr) -> Option<String> {
        match expr {
            ast::Expr::Ident(name) => {
                if let Some(rust_path) = self.rust_import_aliases.get(name) {
                    return Some(rust_path.join("::"));
                }
                let resolved = self.resolve_derive_path(name);
                if resolved
                    .first()
                    .is_some_and(|segment| segment == keywords::as_str(KeywordId::Rust))
                    && resolved.len() >= 2
                {
                    return Some(resolved[1..].join("::"));
                }
                Some(name.clone())
            }
            ast::Expr::Literal(ast::Literal::String(path)) => Some(path.clone()),
            _ => None,
        }
    }

    /// Return trait impl targets introduced by RFC 024 module-level derives such as `@derive(json)`.
    pub(in crate::lower) fn derive_trait_impl_targets(
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
            .and_then(|info| info.derivations.derivable_modules.get(&key))
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
            .and_then(|info| info.derivations.trait_rust_derive_paths.get(&key))
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
            .and_then(|info| info.derivations.derivable_modules.get(&module_key))
            .is_some_and(|traits| traits.iter().any(|candidate| candidate == trait_name))
        {
            return true;
        }
        if let Some(paths) = self.type_info.as_ref().and_then(|info| {
            info.derivations
                .trait_rust_derive_paths
                .get(&format!("{module_key}.{trait_name}"))
        }) {
            return !paths.is_empty();
        }
        self.stdlib_cache.lookup_trait_meta(module_path, trait_name).is_some()
    }

    /// Append a string only when it is not already present.
    fn push_unique(items: &mut Vec<String>, value: String) {
        if !items.iter().any(|item| item == &value) {
            items.push(value);
        }
    }

    /// Add a Rust derive path, preserving serde derives as explicit Rust paths.
    fn push_rust_derive_path(derives: &mut Vec<String>, path: String) {
        Self::push_unique(derives, path);
    }

    /// Forward explicit `with Serialize` / `with Deserialize` adoption of the `std.serde.json` traits into Rust derive
    /// emission.
    ///
    /// This keeps direct-interop serde trait defaults honest: a type that adopts the stdlib serde trait surface must
    /// also satisfy the matching Rust-side serde capability when codegen expands those methods. The adoption is keyed
    /// on the trait's canonical identity, so a source trait that only shares the spelling (a local `Serialize`, or a
    /// `Serialize` exported by some other module) forwards nothing (#1431).
    pub(in crate::lower) fn extend_derives_with_adopted_serde_traits(
        &self,
        derives: &mut Vec<String>,
        trait_bounds: &[Spanned<ast::TraitBound>],
    ) {
        for bound in trait_bounds {
            let Some(protocol) = self.stdlib_json_protocol_for_adopted_trait(&bound.node.name) else {
                continue;
            };
            let derive = match protocol {
                stdlib::StdlibJsonTraitId::Serialize => SERDE_SERIALIZE_DERIVE,
                stdlib::StdlibJsonTraitId::Deserialize => SERDE_DESERIALIZE_DERIVE,
            };
            Self::push_unique(derives, derive.to_string());
        }
    }

    /// Return which `std.serde.json` protocol an adopted trait spelling names, by canonical trait identity.
    ///
    /// The visible spelling is resolved exactly as trait impl lowering resolves it: through the import identity the
    /// frontend proved (which follows a facade re-export to the declaring module) and otherwise through the written
    /// import alias. An alias (`JsonSerialize`), a module-qualified form (`json.Serialize`), the bare import, and a
    /// facade re-export all identify the stdlib trait, while an unrelated trait with the same basename does not.
    pub(in crate::lower) fn stdlib_json_protocol_for_adopted_trait(
        &self,
        visible_name: &str,
    ) -> Option<stdlib::StdlibJsonTraitId> {
        let (module_path, source_name) = self.canonical_trait_identity(visible_name);
        stdlib::stdlib_json_trait_id_for_identity(module_path.as_deref()?, source_name.as_deref()?)
    }

    /// Extract passthrough Rust attributes from decorators.
    pub(in crate::lower) fn extract_passthrough_attributes(
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
    pub(in crate::lower) fn extract_rust_lint_allows(
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
    /// Facet decorators are runtime/runner markers (e.g. `std.testing.parametrize`) and must not be emitted as Rust
    /// attributes — they are interpreted by the Incan test runner, not by `rustc`. Passthrough is reserved for
    /// external Rust-backed proc-macro crates like `incan_web_macros`.
    fn is_passthrough_rust_module(module_path: &str) -> bool {
        !incan_lang::lang::stdlib::facets::path_names_a_facet(module_path)
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
                type_args: Vec::new(),
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
            ast::Type::Dotted(segments) => segments.join("."),
            ast::Type::Generic(name, args) => {
                let inner = args
                    .iter()
                    .map(|a| Self::serialize_type(&a.node))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}<{inner}>")
            }
            ast::Type::DottedGeneric(segments, args) => {
                let inner = args
                    .iter()
                    .map(|a| Self::serialize_type(&a.node))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{inner}>", segments.join("."))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decl::IrTraitBoundOrigin;

    fn callable_bound(name: &str) -> ast::TraitBound {
        ast::TraitBound {
            name: name.to_string(),
            type_args: vec![
                Spanned::new(ast::Type::Simple("T".to_string()), ast::Span::default()),
                Spanned::new(ast::Type::Simple("U".to_string()), ast::Span::default()),
            ],
        }
    }

    fn adopted_bound(name: &str) -> Spanned<ast::TraitBound> {
        Spanned::new(
            ast::TraitBound {
                name: name.to_string(),
                type_args: Vec::new(),
            },
            ast::Span::default(),
        )
    }

    /// #1431: serde derive forwarding keys on the adopted trait's canonical identity, not on its basename.
    #[test]
    fn adopted_serde_derives_require_canonical_std_serde_json_owner() {
        let mut lowering = AstLowering::new();
        lowering.current_source_module_name = Some("main".to_string());
        let bounds = [adopted_bound("Serialize"), adopted_bound("Deserialize")];

        // A local (or otherwise unrelated) trait spelled `Serialize` forwards nothing.
        let mut derives = Vec::new();
        lowering.extend_derives_with_adopted_serde_traits(&mut derives, &bounds);
        assert!(derives.is_empty(), "{derives:?}");
        assert_eq!(lowering.stdlib_json_protocol_for_adopted_trait("Serialize"), None);

        // A trait exported by another module under the same spelling forwards nothing either.
        lowering.import_aliases.insert(
            "Serialize".to_string(),
            vec!["vendor".to_string(), "codec".to_string(), "Serialize".to_string()],
        );
        lowering.extend_derives_with_adopted_serde_traits(&mut derives, &bounds);
        assert!(derives.is_empty(), "{derives:?}");

        // The canonical stdlib traits forward the serde derives through every accepted spelling.
        let json_module = vec!["std".to_string(), "serde".to_string(), "json".to_string()];
        lowering.import_aliases.clear();
        lowering.import_aliases.insert("json".to_string(), json_module.clone());
        lowering.import_aliases.insert(
            "JsonDeserialize".to_string(),
            [json_module.as_slice(), &["Deserialize".to_string()]].concat(),
        );
        let stdlib_bounds = [adopted_bound("json.Serialize"), adopted_bound("JsonDeserialize")];
        lowering.extend_derives_with_adopted_serde_traits(&mut derives, &stdlib_bounds);
        assert_eq!(
            derives,
            vec![SERDE_SERIALIZE_DERIVE.to_string(), SERDE_DESERIALIZE_DERIVE.to_string()]
        );
        assert_eq!(
            lowering.stdlib_json_protocol_for_adopted_trait("json.Serialize"),
            Some(stdlib::StdlibJsonTraitId::Serialize)
        );

        // Forwarding is idempotent.
        lowering.extend_derives_with_adopted_serde_traits(&mut derives, &stdlib_bounds);
        assert_eq!(derives.len(), 2, "{derives:?}");
    }

    #[test]
    fn source_callable_lowering_requires_canonical_std_trait_owner() {
        let mut lowering = AstLowering::new();
        let type_params = HashSet::from(["T", "U"]);

        let local = lowering.lower_trait_bound(&callable_bound("Callable1"), &type_params);
        assert_eq!(local.origin, IrTraitBoundOrigin::Standard);
        assert_eq!(local.trait_path, "Callable1");

        lowering.import_aliases.insert(
            "Callable1".to_string(),
            vec![
                "std".to_string(),
                "traits".to_string(),
                "callable".to_string(),
                "Callable1".to_string(),
            ],
        );
        let canonical = lowering.lower_trait_bound(&callable_bound("Callable1"), &type_params);
        assert_eq!(canonical.origin, IrTraitBoundOrigin::SourceCallable);
        assert_eq!(canonical.trait_path, "Callable1");
        assert_eq!(
            canonical.type_args,
            vec![IrType::Generic("T".to_string()), IrType::Generic("U".to_string())]
        );
        assert!(canonical.assoc_types.is_empty());

        lowering.import_aliases.clear();
        lowering.active_trait_default_type_paths.push(HashMap::from([(
            "Callable1".to_string(),
            vec![
                "crate".to_string(),
                "__incan_std".to_string(),
                "traits".to_string(),
                "callable".to_string(),
                "Callable1".to_string(),
            ],
        )]));
        let expanded_default = lowering.lower_trait_bound(&callable_bound("Callable1"), &type_params);
        assert_eq!(expanded_default.origin, IrTraitBoundOrigin::SourceCallable);
        assert_eq!(
            expanded_default.trait_path,
            "crate::__incan_std::traits::callable::Callable1"
        );
    }

    fn field(name: &str, ty: IrType) -> StructField {
        StructField {
            name: name.to_string(),
            ty,
            surface_type_name: None,
            visibility: crate::decl::Visibility::Public,
            is_type_private: false,
            default: None,
            alias: None,
            description: None,
        }
    }

    /// Issue #1370: a parameter no field type mentions is phantom; a mention anywhere inside a field type, including a
    /// bare nominal spelling left by a declaration lowered without its parameters in scope, is not.
    #[test]
    fn phantom_type_params_are_the_parameters_no_field_mentions() {
        let type_params = vec![IrTypeParam::bare("T"), IrTypeParam::bare("U"), IrTypeParam::bare("V")];
        let fields = vec![
            field("sql", IrType::String),
            field("items", IrType::List(Box::new(IrType::Generic("U".to_string())))),
            field("raw", IrType::Struct("V".to_string())),
        ];
        assert_eq!(
            AstLowering::phantom_type_params(&type_params, &fields),
            vec!["T".to_string()]
        );
        assert_eq!(
            AstLowering::phantom_type_params(&type_params, &[field("value", IrType::Int)]),
            vec!["T".to_string(), "U".to_string(), "V".to_string()]
        );
        assert!(AstLowering::phantom_type_params(&[], &fields).is_empty());
    }
}
