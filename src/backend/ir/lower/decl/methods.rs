//! Method lowering: model methods, class methods, trait impl methods, and general method lowering.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::{
    FunctionParam, FunctionParamDefault, IrAssociatedType, IrDecl, IrDeclKind, IrFunction, IrImpl, IrMethodProjection,
    IrSourceMethodProjection, Visibility,
};
use super::super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind, VarAccess, VarRefKind};
use super::super::super::stmt::{IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::super::{FunctionSignature, IrSpan, Mutability, TypedExpr};
use super::super::AstLowering;
use super::super::TraitImplLoweringInput;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::symbols::ResolvedType;
use incan_core::lang::callables;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::magic_methods::{self, MagicMethodId};
use incan_core::lang::traits as core_traits;
use incan_core::lang::traits::TraitId;

/// Instantiated trait signature used while matching concrete and alias-backed implementation methods.
#[derive(Clone, Copy)]
struct TraitImplSignature<'a> {
    type_params: &'a [ast::TypeParam],
    type_args: &'a [IrType],
}

impl AstLowering {
    /// Retain the established source-spelled native Rust surface without duplicating authored method bodies.
    ///
    /// The canonical method remains the sole implementation. A unique source spelling receives a forwarding
    /// projection; overloaded type/instance spellings do not because Rust inherent impls cannot overload names by
    /// receiver shape. Magic methods already retain their Rust ABI slots and use [`IrMethodProjection`] in the other
    /// direction, so they are not part of this compatibility surface.
    fn source_method_projections(
        &mut self,
        owner: &str,
        methods: &[Spanned<ast::MethodDecl>],
        properties: &[Spanned<ast::PropertyDecl>],
    ) -> Result<Vec<IrSourceMethodProjection>, LoweringError> {
        let mut spelling_counts = HashMap::<String, usize>::new();
        for method in methods
            .iter()
            .filter(|method| magic_methods::from_str(&method.node.name).is_none())
        {
            *spelling_counts.entry(method.node.name.clone()).or_default() += 1;
        }
        for property in properties {
            *spelling_counts.entry(property.node.name.clone()).or_default() += 1;
        }

        let mut projections = Vec::new();
        for method in methods.iter().filter(|method| {
            magic_methods::from_str(&method.node.name).is_none() && spelling_counts.get(&method.node.name) == Some(&1)
        }) {
            if let Some(identity) = self.emitted_method_identity(owner, method)? {
                projections.push(IrSourceMethodProjection {
                    source_name: method.node.name.clone(),
                    identity,
                });
            }
        }
        for property in properties
            .iter()
            .filter(|property| spelling_counts.get(&property.node.name) == Some(&1))
        {
            let identity = self.required_member_identity(owner, &property.node.name, property.span)?;
            projections.push(IrSourceMethodProjection {
                source_name: property.node.name.clone(),
                identity,
            });
        }
        Ok(projections)
    }

    /// Return whether this method is the compiler-generated forwarding helper for a source method-partial binding.
    fn is_generated_method_partial_wrapper(&self, owner: &str, method: &Spanned<ast::MethodDecl>) -> bool {
        self.generated_method_partial_wrappers.contains(&(
            owner.to_string(),
            method.span.start,
            method.span.end,
            method.node.name.clone(),
        )) || self.local_generated_method_partial_wrappers.contains(&(
            method.span.start,
            method.span.end,
            method.node.name.clone(),
        ))
    }

    /// Return the exact source-owned method identity for an emitted declaration.
    ///
    /// Method-partial wrappers are explicitly classified compiler helpers and therefore return `None`. Absence for a
    /// source-written declaration is terminal: lowering never reconstructs member provenance from spellings.
    fn emitted_method_identity(
        &mut self,
        owner: &str,
        method: &Spanned<ast::MethodDecl>,
    ) -> Result<Option<incan_semantics_core::CanonicalSymbolId>, LoweringError> {
        if self.is_generated_method_partial_wrapper(owner, method) {
            return Ok(None);
        }
        let identity = self.required_member_identity(owner, &method.node.name, method.span)?;
        let projection = (owner.to_string(), method.node.name.clone(), identity.clone());
        if !self.emitted_member_projections.contains(&projection) {
            self.emitted_member_projections.push(projection);
        }
        Ok(Some(identity))
    }

    /// Require the compiler-owned identity for one linker-visible source member declaration.
    fn required_member_identity(
        &self,
        owner: &str,
        name: &str,
        span: ast::Span,
    ) -> Result<incan_semantics_core::CanonicalSymbolId, LoweringError> {
        self.type_info
            .as_ref()
            .and_then(|info| {
                info
                    .declarations
                    .member_declaration_identities
                    .get(&(span.start, span.end))
                    .filter(|identity| {
                        matches!(
                            identity.kind,
                            incan_semantics_core::SemanticSourceTargetKind::Method
                                | incan_semantics_core::SemanticSourceTargetKind::Property
                        ) && matches!(
                            identity.origin,
                            incan_semantics_core::SymbolOrigin::Module(_)
                                | incan_semantics_core::SymbolOrigin::Package { .. }
                        )
                    })
                    .cloned()
            })
            .ok_or_else(|| LoweringError {
                message: format!(
                    "linker-visible Incan member `{owner}.{name}` reached lowering without its compiler-owned canonical identity"
                ),
                span: span.into(),
            })
    }

    /// Return the checked identity for a trait default whose declaration may belong to an imported source file.
    fn emitted_trait_default_method_identity(
        &self,
        trait_name: &str,
        method: &Spanned<ast::MethodDecl>,
    ) -> Result<Option<incan_semantics_core::CanonicalSymbolId>, LoweringError> {
        if self.is_generated_method_partial_wrapper(trait_name, method) {
            return Ok(None);
        }
        let visible_key = (trait_name.to_string(), method.node.name.clone());
        let (canonical_module_path, canonical_source_name) = self.canonical_trait_identity(trait_name);
        let canonical_key = match (canonical_module_path, canonical_source_name) {
            (Some(module_path), Some(source_name)) => Some((
                format!("{}.{}", module_path.join("."), source_name),
                method.node.name.clone(),
            )),
            _ => None,
        };
        if let Some(identity) = self
            .type_info
            .as_ref()
            .and_then(|info| {
                info.traits.method_identities.get(&visible_key).or_else(|| {
                    canonical_key
                        .as_ref()
                        .and_then(|key| info.traits.method_identities.get(key))
                })
            })
            .filter(|identity| {
                identity.kind == incan_semantics_core::SemanticSourceTargetKind::Method
                    && matches!(
                        identity.origin,
                        incan_semantics_core::SymbolOrigin::Module(_)
                            | incan_semantics_core::SymbolOrigin::Package { .. }
                    )
            })
            .cloned()
        {
            return Ok(Some(identity));
        }
        self.required_member_identity(trait_name, &method.node.name, method.span)
            .map(Some)
    }

    /// Pair Rust trait slots with exact method identities without reconstructing either from a spelling.
    #[allow(clippy::too_many_arguments)] // Keeps each checked trait and owner axis explicit.
    fn trait_method_projections(
        &mut self,
        methods: &[IrFunction],
        concrete_methods: &[Spanned<ast::MethodDecl>],
        default_methods: &[Spanned<ast::MethodDecl>],
        trait_name: &str,
        trait_type_params: &[ast::TypeParam],
        trait_type_args: &[IrType],
        owner_type_param_names: &std::collections::HashSet<&str>,
    ) -> Result<Vec<IrMethodProjection>, LoweringError> {
        let concrete_owner = self.current_impl_type.clone().unwrap_or_else(|| trait_name.to_string());
        let mut projections = Vec::new();
        for method in methods {
            let trait_source = default_methods.iter().find(|source| source.node.name == method.name);
            let mut concrete_sources = Vec::new();
            for source in concrete_methods.iter().filter(|source| source.node.name == method.name) {
                if !self.method_trait_target_matches_impl(
                    &source.node,
                    trait_name,
                    trait_type_args,
                    owner_type_param_names,
                ) {
                    continue;
                }
                if trait_source.is_some_and(|trait_source| {
                    !self.trait_impl_override_matches(
                        &trait_source.node,
                        &source.node,
                        trait_type_params,
                        trait_type_args,
                        owner_type_param_names,
                    )
                }) {
                    continue;
                }
                concrete_sources.push(source);
            }
            let identity = if concrete_sources.is_empty() {
                let Some(source) = trait_source else {
                    // Backend-generated ABI helpers have no source declaration and therefore no Incan projection.
                    continue;
                };
                self.emitted_trait_default_method_identity(trait_name, source)?
            } else {
                let mut candidate = None;
                for source in concrete_sources {
                    let Some(identity) = self.emitted_method_identity(&concrete_owner, source)? else {
                        // A source method partial is a binding; its forwarding method is a generated helper.
                        continue;
                    };
                    if !self.emitted_inherent_method_identities.contains(&identity) {
                        candidate = Some(identity);
                        break;
                    }
                }
                candidate
            };
            let Some(identity) = identity else {
                continue;
            };
            projections.push(IrMethodProjection {
                abi_method_name: method.name.clone(),
                identity,
            });
        }
        Ok(projections)
    }

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
        trait_args: &[IrType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, Vec<IrType>)>,
    ) {
        let key = format!("{trait_name}<{trait_args:?}>");
        if !seen.insert(key) {
            return;
        }
        out.push((trait_name.to_string(), trait_args.to_vec()));

        let Some(type_info) = &self.type_info else {
            return;
        };
        let Some(direct_supertraits) = type_info.traits.direct_supertraits.get(trait_name) else {
            return;
        };
        let Some(param_names) = self.trait_type_param_names(trait_name) else {
            return;
        };
        let subst = param_names
            .iter()
            .cloned()
            .zip(trait_args.iter().cloned())
            .collect::<HashMap<_, _>>();

        for (supertrait_name, supertrait_args) in direct_supertraits {
            let instantiated_args = supertrait_args
                .iter()
                .map(|arg| Self::substitute_ir_type_params(self.lower_resolved_type(arg), &subst))
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
        let direct_args = self
            .infer_trait_impl_resolved_args(trait_name, type_params)
            .iter()
            .map(|arg| self.lower_resolved_type(arg))
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        self.collect_trait_impl_targets_recursive(trait_name, &direct_args, &mut seen, &mut out);
        out
    }

    /// Lower an adopted trait bound into the direct Rust impl target(s) required for codegen.
    ///
    /// Explicit type arguments on adopter bounds (for example `with From[int]`) are preserved directly from the AST
    /// and substituted through the complete supertrait closure.
    pub(in crate::backend::ir::lower) fn trait_impl_targets_for_adopted_trait_bound(
        &self,
        bound: &ast::TraitBound,
        owner_type_name: &str,
        type_params: &[ast::TypeParam],
    ) -> Vec<(String, Vec<IrType>)> {
        if bound.type_args.is_empty() {
            return self.trait_impl_targets_for_adopted_trait(&bound.name, type_params);
        }
        let type_param_names: std::collections::HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
        let owner_type = Self::trait_impl_owner_type(owner_type_name, type_params);
        let direct_args = bound
            .type_args
            .iter()
            .map(|arg| {
                let lowered = self.lower_type_with_type_params(&arg.node, Some(&type_param_names));
                Self::substitute_trait_impl_self_type(lowered, &owner_type)
            })
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        self.collect_trait_impl_targets_recursive(&bound.name, &direct_args, &mut seen, &mut out);
        out
    }

    /// Build the fully-instantiated owner type used when an adopted trait bound names `Self`.
    ///
    /// Trait implementation headers are outside a Rust method or trait body, so a bare `Self` is not valid there.
    /// The IR must instead carry the adopter's nominal type, including its declared generic parameters.
    fn trait_impl_owner_type(type_name: &str, type_params: &[ast::TypeParam]) -> IrType {
        if type_params.is_empty() {
            IrType::Struct(type_name.to_string())
        } else {
            IrType::NamedGeneric(
                type_name.to_string(),
                type_params
                    .iter()
                    .map(|param| IrType::Generic(param.name.clone()))
                    .collect(),
            )
        }
    }

    /// Replace `Self` in an adopted trait target with its concrete implementation owner.
    ///
    /// This substitution is intentionally performed at the trait-target boundary rather than by emission: `Self`
    /// still has its ordinary meaning in trait method bodies, while an impl header requires a concrete type.
    fn substitute_trait_impl_self_type(ty: IrType, owner_type: &IrType) -> IrType {
        match ty {
            IrType::SelfType => owner_type.clone(),
            IrType::List(inner) => IrType::List(Box::new(Self::substitute_trait_impl_self_type(*inner, owner_type))),
            IrType::Dict(key, value) => IrType::Dict(
                Box::new(Self::substitute_trait_impl_self_type(*key, owner_type)),
                Box::new(Self::substitute_trait_impl_self_type(*value, owner_type)),
            ),
            IrType::Set(inner) => IrType::Set(Box::new(Self::substitute_trait_impl_self_type(*inner, owner_type))),
            IrType::Tuple(items) => IrType::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::substitute_trait_impl_self_type(item, owner_type))
                    .collect(),
            ),
            IrType::Option(inner) => {
                IrType::Option(Box::new(Self::substitute_trait_impl_self_type(*inner, owner_type)))
            }
            IrType::Result(ok, err) => IrType::Result(
                Box::new(Self::substitute_trait_impl_self_type(*ok, owner_type)),
                Box::new(Self::substitute_trait_impl_self_type(*err, owner_type)),
            ),
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                name,
                args.into_iter()
                    .map(|arg| Self::substitute_trait_impl_self_type(arg, owner_type))
                    .collect(),
            ),
            IrType::TypeToken(inner) => {
                IrType::TypeToken(Box::new(Self::substitute_trait_impl_self_type(*inner, owner_type)))
            }
            IrType::Function { params, ret } => IrType::Function {
                params: params
                    .into_iter()
                    .map(|param| Self::substitute_trait_impl_self_type(param, owner_type))
                    .collect(),
                ret: Box::new(Self::substitute_trait_impl_self_type(*ret, owner_type)),
            },
            IrType::Ref(inner) => IrType::Ref(Box::new(Self::substitute_trait_impl_self_type(*inner, owner_type))),
            IrType::RefMut(inner) => {
                IrType::RefMut(Box::new(Self::substitute_trait_impl_self_type(*inner, owner_type)))
            }
            other => other,
        }
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
            let source_method_projections = self.source_method_projections(type_name, &inherent_methods, properties)?;
            let method_projections: Vec<IrMethodProjection> = inherent_methods
                .iter()
                .filter(|method| magic_methods::from_str(&method.node.name).is_some())
                .map(|method| {
                    self.emitted_method_identity(type_name, method).map(|identity| {
                        identity.map(|identity| IrMethodProjection {
                            abi_method_name: method.node.name.clone(),
                            identity,
                        })
                    })
                })
                .collect::<Result<Vec<_>, LoweringError>>()?
                .into_iter()
                .flatten()
                .collect();
            self.emitted_inherent_method_identities.extend(
                method_projections
                    .iter()
                    .map(|projection: &IrMethodProjection| projection.identity.clone()),
            );
            let mut lowered_methods = Vec::new();
            for method in inherent_methods {
                lowered_methods.extend(self.lower_decorated_or_plain_methods(
                    type_name,
                    &method,
                    Some(&type_param_names),
                )?);
            }
            for property in properties {
                lowered_methods.push(self.lower_property_with_type_params(
                    property,
                    Some(&type_param_names),
                    PropertyLoweringMode::Inherent,
                )?);
            }
            Ok((lowered_methods, method_projections, source_method_projections))
        })();
        self.current_impl_type = prev;
        let (lowered_methods, method_projections, source_method_projections) = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: self.lower_type_params(type_params),
            trait_name: None,
            trait_module_path: None,
            trait_source_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: lowered_methods,
            method_projections,
            source_method_projections,
        })
    }

    /// Resolve a visible trait spelling to its canonical source identity.
    pub(in crate::backend::ir::lower) fn canonical_trait_identity(
        &self,
        visible_name: &str,
    ) -> (Option<Vec<String>>, Option<String>) {
        if let Some(path) = self.import_aliases.get(visible_name)
            && let Some((source_name, module_path)) = path.split_last()
        {
            return (Some(module_path.to_vec()), Some(source_name.clone()));
        }
        if let Some((module_name, source_name)) = visible_name.rsplit_once('.')
            && let Some(module_path) = self.import_aliases.get(module_name)
        {
            return (Some(module_path.clone()), Some(source_name.to_string()));
        }
        if let Some(path) = self.active_trait_default_type_path(visible_name)
            && path.len() >= 4
            && path[0] == keywords::as_str(KeywordId::Crate)
            && path[1] == incan_core::lang::stdlib::INCAN_STD_NAMESPACE
            && let Some((source_name, module_path)) = path.split_last()
        {
            let mut canonical_module = vec![incan_core::lang::stdlib::STDLIB_ROOT.to_string()];
            canonical_module.extend(module_path.iter().skip(2).cloned());
            return (Some(canonical_module), Some(source_name.clone()));
        }
        let module_path = self.current_source_module_name.as_ref().map(|name| {
            name.split('.')
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
        (
            module_path,
            Some(visible_name.rsplit('.').next().unwrap_or(visible_name).to_string()),
        )
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
            let original_ty = self.lower_resolved_type(&binding.original_unbound_ty);
            let original_params = match binding.original_unbound_ty {
                crate::frontend::symbols::ResolvedType::Function(params, _) => params,
                _ => Vec::new(),
            };
            let crate::frontend::symbols::ResolvedType::Function(params, ret) = binding.unbound_ty else {
                continue;
            };
            let static_name = Self::decorator_method_static_binding_name(type_name, &method.node.name);
            let decorated_signature =
                self.decorated_method_callable_signature(&params, &ret, &method.node, Some(&original_params))?;
            let decorated_ty = IrType::Function {
                params: decorated_signature.params.into_iter().map(|param| param.ty).collect(),
                ret: Box::new(decorated_signature.return_type),
            };
            let original_ref = TypedExpr::new(
                IrExprKind::AssociatedFunction {
                    type_name: type_name.to_string(),
                    function_name: Self::decorator_original_method_adapter_name(&method.node.name),
                },
                original_ty,
            );
            let value =
                self.lower_decorator_application_value(&method.node.decorators, original_ref, decorated_ty.clone())?;
            out.push(IrDecl::new(IrDeclKind::Static {
                visibility: Visibility::Private,
                name: static_name,
                provenance: super::super::super::decl::IrStaticProvenance::CompilerGenerated,
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
        method: &Spanned<ast::MethodDecl>,
        type_param_names: Option<&HashSet<&str>>,
    ) -> Result<Vec<IrFunction>, LoweringError> {
        if self.type_info.as_ref().is_some_and(|info| {
            info.declarations
                .decorated_method_bindings
                .contains_key(&(owner.to_string(), method.node.name.clone()))
        }) {
            let original = self.lower_method_named_with_type_params(
                &method.node,
                Self::decorator_original_method_name(&method.node.name),
                Visibility::Private,
                type_param_names,
            )?;
            let adapter = self.decorated_method_original_adapter(owner, &method.node)?;
            let mut wrapper = self.lower_decorated_method_wrapper(owner, &method.node, type_param_names)?;
            if magic_methods::from_str(&method.node.name).is_none()
                && let Some(identity) = self.emitted_method_identity(owner, method)?
            {
                wrapper.name = incan_semantics_core::encode_incan_symbol_identity(&identity);
                self.emitted_inherent_method_identities.insert(identity);
            }
            Ok(vec![original, adapter, wrapper])
        } else {
            let mut lowered = self.lower_method_with_type_params(&method.node, type_param_names)?;
            if magic_methods::from_str(&method.node.name).is_none()
                && let Some(identity) = self.emitted_method_identity(owner, method)?
            {
                lowered.name = incan_semantics_core::encode_incan_symbol_identity(&identity);
                self.emitted_inherent_method_identities.insert(identity);
            }
            Ok(vec![lowered])
        }
    }

    /// Lower the public method wrapper that dispatches through the decorated callable static.
    fn lower_decorated_method_wrapper(
        &mut self,
        owner: &str,
        method: &ast::MethodDecl,
        owner_type_param_names: Option<&HashSet<&str>>,
    ) -> Result<IrFunction, LoweringError> {
        let Some(binding) = self.type_info.as_ref().and_then(|info| {
            info.declarations
                .decorated_method_bindings
                .get(&(owner.to_string(), method.name.clone()))
                .cloned()
        }) else {
            return self.lower_method_with_type_params(method, owner_type_param_names);
        };
        let crate::frontend::symbols::ResolvedType::Function(params, ret) = binding.unbound_ty else {
            return self.lower_method_with_type_params(method, owner_type_param_names);
        };
        let Some((receiver_param, surface_params)) = params.split_first() else {
            return self.lower_method_with_type_params(method, owner_type_param_names);
        };
        let receiver_ty = self.lower_resolved_type(&receiver_param.ty);
        let original_callable_params = match binding.original_unbound_ty {
            crate::frontend::symbols::ResolvedType::Function(original_params, _) => original_params,
            _ => Vec::new(),
        };
        let original_surface_params = original_callable_params.get(1..).unwrap_or(&[]);
        let defaults =
            self.decorated_param_defaults_for_surface(surface_params, original_surface_params, &method.params)?;
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
        wrapper_params.extend(self.function_params_from_callable_surface(
            surface_params,
            &defaults,
            Some(&method.params),
            Some(original_surface_params),
        ));
        let return_type = self.lower_resolved_type(&ret);
        let static_name = Self::decorator_method_static_binding_name(owner, &method.name);
        let callable_signature =
            self.decorated_method_callable_signature(&params, &ret, method, Some(&original_callable_params))?;
        let static_func = TypedExpr::new(
            IrExprKind::StaticRead {
                name: static_name,
                reference_kind: super::super::super::expr::IrStaticReferenceKind::CompilerGenerated,
            },
            IrType::Function {
                params: callable_signature.params.iter().map(|param| param.ty.clone()).collect(),
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
        args.extend(Self::forwarding_args_from_params(&wrapper_params[1..]));
        let call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(static_func),
                type_args: Vec::new(),
                args,
                callable_signature: Some(callable_signature),
                canonical_path: None,
            },
            return_type.clone(),
        );
        Ok(IrFunction {
            name: method.name.clone(),
            docstring: None,
            params: wrapper_params,
            return_type,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(call)))],
            is_async: method.is_async(),
            is_generator: false,
            visibility: Visibility::Public,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })
    }

    /// Build the callable ABI shared by a decorated method's static binding and public wrapper.
    fn decorated_method_callable_signature(
        &mut self,
        callable_params: &[crate::frontend::symbols::CallableParam],
        callable_ret: &crate::frontend::symbols::ResolvedType,
        method: &ast::MethodDecl,
        original_callable_params: Option<&[crate::frontend::symbols::CallableParam]>,
    ) -> Result<super::super::FunctionSignature, LoweringError> {
        let Some((receiver_param, surface_params)) = callable_params.split_first() else {
            return Err(LoweringError {
                message: format!(
                    "decorated method '{}' has no receiver in callable metadata",
                    method.name
                ),
                span: ast::Span::default().into(),
            });
        };
        let receiver = method.receiver.unwrap_or(ast::Receiver::Immutable);
        let mut params = vec![FunctionParam {
            name: "self".to_string(),
            ty: self.lower_resolved_type(&receiver_param.ty),
            mutability: if matches!(receiver, ast::Receiver::Mutable) {
                Mutability::Mutable
            } else {
                Mutability::Immutable
            },
            is_self: true,
            kind: ast::ParamKind::Normal,
            default: None,
        }];
        let original_surface_params = original_callable_params.and_then(|params| params.get(1..));
        params.extend(self.function_params_from_callable_surface(
            surface_params,
            &[],
            Some(&method.params),
            original_surface_params,
        ));
        Ok(super::super::FunctionSignature {
            params,
            return_type: self.lower_resolved_type(callable_ret),
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
        adapter_params.extend(self.function_params_from_callable_surface(
            surface_params,
            &[],
            Some(&method.params),
            Some(surface_params),
        ));
        let return_type = self.lower_resolved_type(&ret);
        let receiver = TypedExpr::new(
            IrExprKind::Var {
                name: "__incan_self".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            receiver_ty,
        );
        let args = Self::forwarding_args_from_params(&adapter_params[1..]);
        let call = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(receiver),
                method: Self::decorator_original_method_name(&method.name),
                dispatch: None,
                type_args: Vec::new(),
                args,
                callable_signature: Some(FunctionSignature {
                    params: adapter_params.iter().skip(1).cloned().collect(),
                    return_type: return_type.clone(),
                }),
                arg_policy: super::super::super::expr::MethodCallArgPolicy::Default,
            },
            return_type.clone(),
        );
        Ok(IrFunction {
            name: Self::decorator_original_method_adapter_name(&method.name),
            docstring: None,
            params: adapter_params,
            return_type,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(call)))],
            is_async: method.is_async(),
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
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
        trait_sig.0 == candidate_sig.0
            && trait_sig.1 == candidate_sig.1
            && Self::trait_impl_type_matches(&trait_sig.2, &candidate_sig.2)
    }

    /// Compare trait-implementation types after the trait target has replaced its `Self` argument with the adopter
    /// type.
    ///
    /// Concrete implementation methods retain `Self` in their source signature because it remains valid inside the
    /// generated impl body. At this matching boundary it denotes the same owner as the concrete trait target.
    fn trait_impl_type_matches(expected: &IrType, actual: &IrType) -> bool {
        match (expected, actual) {
            // At trait-implementation matching boundaries, `Self` on either side names the adopting nominal type.
            // The typechecker has already checked the concrete signature; this only decides whether lowering keeps
            // the concrete body instead of expanding the trait declaration's default body.
            (IrType::SelfType, _) | (_, IrType::SelfType) => true,
            (IrType::List(expected), IrType::List(actual))
            | (IrType::Set(expected), IrType::Set(actual))
            | (IrType::Option(expected), IrType::Option(actual))
            | (IrType::TypeToken(expected), IrType::TypeToken(actual))
            | (IrType::Ref(expected), IrType::Ref(actual))
            | (IrType::RefMut(expected), IrType::RefMut(actual)) => Self::trait_impl_type_matches(expected, actual),
            (IrType::Dict(expected_key, expected_value), IrType::Dict(actual_key, actual_value))
            | (IrType::Result(expected_key, expected_value), IrType::Result(actual_key, actual_value)) => {
                Self::trait_impl_type_matches(expected_key, actual_key)
                    && Self::trait_impl_type_matches(expected_value, actual_value)
            }
            (IrType::Tuple(expected), IrType::Tuple(actual))
            | (IrType::NamedGeneric(_, expected), IrType::NamedGeneric(_, actual)) => {
                expected.len() == actual.len()
                    && expected
                        .iter()
                        .zip(actual)
                        .all(|(expected, actual)| Self::trait_impl_type_matches(expected, actual))
            }
            (
                IrType::Function {
                    params: expected_params,
                    ret: expected_ret,
                },
                IrType::Function {
                    params: actual_params,
                    ret: actual_ret,
                },
            ) => {
                expected_params.len() == actual_params.len()
                    && expected_params
                        .iter()
                        .zip(actual_params)
                        .all(|(expected, actual)| Self::trait_impl_type_matches(expected, actual))
                    && Self::trait_impl_type_matches(expected_ret, actual_ret)
            }
            _ => expected == actual,
        }
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
        lowered_args.len() == trait_type_args.len()
            && trait_type_args
                .iter()
                .zip(&lowered_args)
                .all(|(expected, actual)| Self::trait_impl_type_matches(expected, actual))
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
        if callables::from_str(short_name).is_some() {
            callables::METHOD_NAMES
        } else {
            match incan_core::lang::stdlib::stdlib_json_trait_id(trait_name)
                .or_else(|| incan_core::lang::stdlib::stdlib_json_trait_id(short_name))
            {
                Some(id) => incan_core::lang::stdlib::stdlib_json_trait_method_names(id),
                None => &[],
            }
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
        incan_core::lang::stdlib::stdlib_json_trait_id(trait_name)
            .or_else(|| incan_core::lang::stdlib::stdlib_json_trait_id(short_name))
            .is_some_and(|id| incan_core::lang::stdlib::stdlib_json_trait_method_names(id).contains(&method_name))
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

    /// Return an alias-backed implementation for a required trait method.
    ///
    /// Same-type aliases are call-site rebindings, so they do not appear in a declaration's authored method list.
    /// A trait impl must still emit the required method name; clone the checked target body under that name only after
    /// confirming that the target is eligible for this trait impl and has the exact instantiated signature.
    fn aliased_trait_impl_override(
        &mut self,
        type_name: &str,
        trait_method: &ast::MethodDecl,
        impl_methods: &[Spanned<ast::MethodDecl>],
        trait_name: &str,
        trait_signature: TraitImplSignature<'_>,
        owner_type_param_names: &std::collections::HashSet<&str>,
    ) -> Option<ast::MethodDecl> {
        let target_name = self
            .type_method_rebindings
            .get(type_name)
            .and_then(|aliases| aliases.get(&trait_method.name))?
            .clone();
        let target = impl_methods.iter().find(|method| {
            method.node.name == target_name
                && self.method_trait_target_matches_impl(
                    &method.node,
                    trait_name,
                    trait_signature.type_args,
                    owner_type_param_names,
                )
                && self.trait_impl_override_matches(
                    trait_method,
                    &method.node,
                    trait_signature.type_params,
                    trait_signature.type_args,
                    owner_type_param_names,
                )
        })?;
        let mut alias = target.node.clone();
        alias.name = trait_method.name.clone();
        Some(alias)
    }

    /// Return whether a concrete method should be lowered only inside an adopted trait impl.
    fn method_matches_adopted_trait_impl(
        &mut self,
        method: &ast::MethodDecl,
        type_params: &[ast::TypeParam],
        owner_type_param_names: &std::collections::HashSet<&str>,
        adopted_traits: &[Spanned<ast::TraitBound>],
    ) -> bool {
        let owner_type_name = self.current_impl_type.clone().unwrap_or_default();
        for trait_ref in adopted_traits {
            for (trait_name, trait_type_args) in
                self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, &owner_type_name, type_params)
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
        let (trait_module_path, trait_source_name) = self.canonical_trait_identity(trait_name);
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
                        property,
                        Some(&type_param_names),
                        PropertyLoweringMode::TraitImpl,
                    )?);
                }
                let method_projections = self.trait_method_projections(
                    &methods,
                    impl_methods,
                    &[],
                    trait_name,
                    &[],
                    &trait_type_args,
                    &type_param_names,
                )?;
                return Ok(IrImpl {
                    target_type: type_name.to_string(),
                    type_params: self.lower_type_params(type_params),
                    trait_name: Some(trait_name.to_string()),
                    trait_module_path: trait_module_path.clone(),
                    trait_source_name: trait_source_name.clone(),
                    trait_type_args,
                    associated_types,
                    methods,
                    method_projections,
                    source_method_projections: Vec::new(),
                });
            };
            let trait_type_params = trait_decl.type_params;
            let trait_signature = TraitImplSignature {
                type_params: &trait_type_params,
                type_args: &trait_type_args,
            };
            let trait_properties = trait_decl.properties;
            let mut trait_methods = trait_decl.methods;
            if trait_name == core_traits::as_str(TraitId::Iterator) {
                trait_methods.retain(|method| method.node.name == magic_methods::as_str(MagicMethodId::Next));
            }

            let mut methods: Vec<IrFunction> = Vec::new();
            for trait_property in &trait_properties {
                let property_name = trait_property.node.name.as_str();

                let mut found_override: Option<&Spanned<ast::PropertyDecl>> = None;
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
                        found_override = Some(property);
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

                if let Some(alias_override) = self.aliased_trait_impl_override(
                    type_name,
                    &trait_method.node,
                    impl_methods,
                    trait_name,
                    trait_signature,
                    &type_param_names,
                ) {
                    methods.push(self.lower_impl_method_for_trait(&alias_override, Some(&type_param_names))?);
                    continue;
                }

                // Otherwise, expand a default method body into the impl (RFC 000: defaults may assume adopter fields).
                if trait_method.node.body.is_some() {
                    let helper_paths = self.trait_default_function_paths.get(trait_name).cloned();
                    let type_paths = self.trait_default_type_paths.get(trait_name).cloned();
                    let has_helper_paths = helper_paths.is_some();
                    let has_type_paths = type_paths.is_some();
                    if let Some(helper_paths) = helper_paths {
                        self.active_trait_default_function_paths.push(helper_paths);
                    }
                    if let Some(type_paths) = type_paths {
                        self.active_trait_default_type_paths.push(type_paths);
                    }
                    let substitutions = trait_type_params
                        .iter()
                        .map(|param| param.name.clone())
                        .zip(trait_type_args.iter().cloned())
                        .collect();
                    self.active_trait_type_substitutions.push(substitutions);
                    let lowered = self.lower_impl_method_for_trait(&trait_method.node, Some(&type_param_names));
                    self.active_trait_type_substitutions.pop();
                    if has_type_paths {
                        self.active_trait_default_type_paths.pop();
                    }
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

            let method_projections = self.trait_method_projections(
                &methods,
                impl_methods,
                &trait_methods,
                trait_name,
                &trait_type_params,
                &trait_type_args,
                &type_param_names,
            )?;
            Ok(IrImpl {
                target_type: type_name.to_string(),
                type_params: self.lower_type_params(type_params),
                trait_name: Some(trait_name.to_string()),
                trait_module_path,
                trait_source_name,
                trait_type_args,
                associated_types,
                methods,
                method_projections,
                source_method_projections: Vec::new(),
            })
        })();
        self.current_impl_type = prev;
        lowered_result
    }

    /// Lower one concrete impl method while preserving owner and method type parameters.
    fn source_callable_type_param_signatures(
        &self,
        type_params: &[ast::TypeParam],
        visible_type_params: &HashSet<&str>,
    ) -> HashMap<String, FunctionSignature> {
        type_params
            .iter()
            .filter_map(|type_param| {
                type_param.bounds.iter().find_map(|bound| {
                    let (module_path, source_name) = self.canonical_trait_identity(&bound.name);
                    let callable = module_path
                        .as_deref()
                        .filter(|path| callables::module_path_matches(path))
                        .and(source_name.as_deref())
                        .and_then(callables::from_str)?;
                    let arity = callables::info_for(callable).arity;
                    if bound.type_args.len() != arity + 1 {
                        return None;
                    }
                    let lowered = bound
                        .type_args
                        .iter()
                        .map(|arg| self.lower_type_with_type_params(&arg.node, Some(visible_type_params)))
                        .collect::<Vec<_>>();
                    let (return_type, params) = lowered.split_last()?;
                    Some((
                        type_param.name.clone(),
                        FunctionSignature {
                            params: params
                                .iter()
                                .enumerate()
                                .map(|(index, ty)| FunctionParam {
                                    name: format!("__incan_arg_{index}"),
                                    ty: ty.clone(),
                                    mutability: Mutability::Immutable,
                                    is_self: false,
                                    kind: ast::ParamKind::Normal,
                                    default: None,
                                })
                                .collect(),
                            return_type: return_type.clone(),
                        },
                    ))
                })
            })
            .collect()
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
        let nominal_callable_types =
            self.source_callable_type_param_signatures(&m.type_params, &combined_type_param_names);

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
            let concrete_self = self
                .current_impl_type
                .as_ref()
                .map_or(IrType::SelfType, |name| IrType::Struct(name.clone()));
            self.define_local_binding("self".to_string(), concrete_self, false);
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
                let base_ty = self.apply_mutable_rust_type_argument_projections(p.node.is_mut, &p.node.ty, base_ty);
                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                let mutability = self.lower_parameter_mutability(p.node.is_mut, &p.node.ty.node);
                Ok(FunctionParam {
                    name: p.node.name.clone(),
                    ty: param_ty,
                    mutability,
                    is_self: false,
                    kind: p.node.kind,
                    default: self
                        .lower_param_default_expr(p.node.default.as_ref())?
                        .map(FunctionParamDefault::source),
                })
            })
            .collect::<Result<_, LoweringError>>()?;
        params.extend(other_params);

        for (source_param, lowered_param) in m.params.iter().zip(params.iter().filter(|param| !param.is_self)) {
            let binding_type = if lowered_param.mutability == Mutability::Mutable {
                IrType::RefMut(Box::new(lowered_param.ty.clone()))
            } else {
                lowered_param.ty.clone()
            };
            self.define_local_binding(source_param.node.name.clone(), binding_type, false);
            if let ast::Type::Simple(type_name) = &source_param.node.ty.node
                && let Some(signature) = nominal_callable_types.get(type_name)
            {
                self.define_nominal_callable(source_param.node.name.clone(), signature.clone());
            }
        }

        let return_type = self.lower_callable_return_type(&m.return_type.node, Some(&combined_type_param_names));
        self.push_callable_param_scope(&params);
        self.push_callable_return_type(&return_type);
        self.active_callable_type_params.push(
            combined_type_param_names
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        );
        let body_result = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)
        } else {
            Ok(vec![])
        };
        self.active_callable_type_params.pop();
        self.pop_callable_param_scope();
        self.pop_callable_return_type();
        let body = match body_result {
            Ok(body) => body,
            Err(error) => {
                self.pop_scope();
                return Err(error);
            }
        };

        // RFC 023: detect @rust.extern decorator to mark this method as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&m.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&m.decorators);
        let lint_allows = self.extract_rust_lint_allows(&m.decorators);
        let mut all_type_params = self.lower_type_params(&m.type_params);
        all_type_params.extend(hidden_type_params);

        self.pop_scope();

        Ok(IrFunction {
            name: m.name.clone(),
            docstring: m.body.as_ref().and_then(|body| super::callable_docstring(body)),
            params,
            return_type,
            body,
            is_async: m.is_async(),
            is_generator: false,
            visibility: Visibility::Private,
            type_params: std::mem::take(&mut all_type_params),
            is_extern,
            rust_extern_name: is_extern.then(|| m.name.clone()),
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
            let source_method_projections = self.source_method_projections(type_name, &inherent_methods, properties)?;
            let method_projections: Vec<IrMethodProjection> = inherent_methods
                .iter()
                .filter(|method| magic_methods::from_str(&method.node.name).is_some())
                .map(|method| {
                    self.emitted_method_identity(type_name, method).map(|identity| {
                        identity.map(|identity| IrMethodProjection {
                            abi_method_name: method.node.name.clone(),
                            identity,
                        })
                    })
                })
                .collect::<Result<Vec<_>, LoweringError>>()?
                .into_iter()
                .flatten()
                .collect();
            self.emitted_inherent_method_identities.extend(
                method_projections
                    .iter()
                    .map(|projection: &IrMethodProjection| projection.identity.clone()),
            );
            let mut lowered_methods = Vec::new();
            for method in inherent_methods {
                lowered_methods.extend(self.lower_decorated_or_plain_methods(
                    type_name,
                    &method,
                    Some(&type_param_names),
                )?);
            }
            for property in properties {
                lowered_methods.push(self.lower_property_with_type_params(
                    property,
                    Some(&type_param_names),
                    PropertyLoweringMode::Inherent,
                )?);
            }
            Ok((lowered_methods, method_projections, source_method_projections))
        })();
        self.current_impl_type = prev;
        let (lowered_methods, method_projections, source_method_projections) = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: self.lower_type_params(type_params),
            trait_name: None,
            trait_module_path: None,
            trait_source_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: lowered_methods,
            method_projections,
            source_method_projections,
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
        let source_method_projections = self.source_method_projections(type_name, &inherent_methods, &[])?;
        let method_projections: Vec<IrMethodProjection> = inherent_methods
            .iter()
            .filter(|method| magic_methods::from_str(&method.node.name).is_some())
            .map(|method| {
                self.emitted_method_identity(type_name, method).map(|identity| {
                    identity.map(|identity| IrMethodProjection {
                        abi_method_name: method.node.name.clone(),
                        identity,
                    })
                })
            })
            .collect::<Result<Vec<_>, LoweringError>>()?
            .into_iter()
            .flatten()
            .collect();
        self.emitted_inherent_method_identities.extend(
            method_projections
                .iter()
                .map(|projection: &IrMethodProjection| projection.identity.clone()),
        );
        let lowered = inherent_methods
            .iter()
            .map(|m| self.lower_decorated_or_plain_methods(type_name, m, Some(&type_param_names)))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?.into_iter().flatten().collect();

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: self.lower_type_params(type_params),
            trait_name: None,
            trait_module_path: None,
            trait_source_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: lowered_methods,
            method_projections,
            source_method_projections,
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
    pub(in crate::backend::ir::lower) fn lower_method_with_type_params(
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
                let base_ty = self.apply_mutable_rust_type_argument_projections(p.node.is_mut, &p.node.ty, base_ty);
                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                let mutability = self.lower_parameter_mutability(p.node.is_mut, &p.node.ty.node);
                // Ordinary mutable Incan parameters are references. Direct Rust handles keep owned ABI identity.
                let ty = if mutability == Mutability::Mutable {
                    IrType::RefMut(Box::new(param_ty.clone()))
                } else {
                    param_ty.clone()
                };
                self.define_local_binding(p.node.name.clone(), ty.clone(), false);
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                Ok(FunctionParam {
                    name: p.node.name.clone(),
                    ty: param_ty,
                    mutability,
                    is_self: p.node.name == keywords::as_str(KeywordId::SelfKw),
                    kind: p.node.kind,
                    default: self
                        .lower_param_default_expr(p.node.default.as_ref())?
                        .map(FunctionParamDefault::source),
                })
            })
            .collect::<Result<_, LoweringError>>()?;
        params.extend(other_params);

        let return_type = self.lower_callable_return_type(&m.return_type.node, Some(&combined_type_param_names));
        let previous_classmethod_constructor = self.current_classmethod_constructor.take();
        if Self::method_has_decorator(m, DecoratorId::ClassMethod)
            && let Some(type_name) = self.current_impl_type.clone()
        {
            self.current_classmethod_constructor = Some(type_name);
        }
        self.push_callable_param_scope(&params);
        self.push_callable_return_type(&return_type);
        self.active_callable_type_params.push(
            combined_type_param_names
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        );
        let body_result = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)
        } else {
            // Abstract method with no body
            Ok(vec![])
        };
        self.active_callable_type_params.pop();
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
        self.pop_callable_return_type();
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
        let mut all_type_params = self.lower_type_params(&m.type_params);
        all_type_params.extend(hidden_type_params);

        Ok(IrFunction {
            name,
            docstring: m.body.as_ref().and_then(|body| super::callable_docstring(body)),
            params,
            return_type,
            body,
            is_async: m.is_async(),
            is_generator: false,
            visibility,
            type_params: std::mem::take(&mut all_type_params),
            is_extern,
            rust_extern_name: is_extern.then(|| m.name.clone()),
            rust_attributes,
            lint_allows,
        })
    }

    /// Lower a computed property declaration into the zero-argument function form used by IR emission.
    pub(in crate::backend::ir::lower) fn lower_property_with_type_params(
        &mut self,
        property: &Spanned<ast::PropertyDecl>,
        type_param_names: Option<&std::collections::HashSet<&str>>,
        mode: PropertyLoweringMode,
    ) -> Result<IrFunction, LoweringError> {
        let declaration = &property.node;
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

        let return_type = self.lower_callable_return_type(&declaration.return_type.node, type_param_names);
        self.push_callable_return_type(&return_type);
        self.active_callable_type_params.push(
            type_param_names
                .into_iter()
                .flat_map(|params| params.iter().map(|name| (*name).to_string()))
                .collect(),
        );
        let body_result = match mode {
            PropertyLoweringMode::TraitDecl => Ok(Vec::new()),
            PropertyLoweringMode::Inherent | PropertyLoweringMode::TraitImpl => {
                if let Some(body_stmts) = &declaration.body {
                    self.lower_statements(body_stmts)
                } else {
                    Ok(Vec::new())
                }
            }
        };
        self.active_callable_type_params.pop();
        self.pop_callable_return_type();
        self.pop_scope();
        let body = body_result?;

        let visibility = match mode {
            PropertyLoweringMode::Inherent => Self::map_visibility(declaration.visibility),
            PropertyLoweringMode::TraitDecl | PropertyLoweringMode::TraitImpl => Visibility::Private,
        };

        let mut name = declaration.name.clone();
        if matches!(mode, PropertyLoweringMode::Inherent) {
            let owner = self.current_impl_type.as_deref().unwrap_or("<unknown-owner>");
            let identity = self.required_member_identity(owner, &property.node.name, property.span)?;
            let projection = (owner.to_string(), property.node.name.clone(), identity.clone());
            if !self.emitted_member_projections.contains(&projection) {
                self.emitted_member_projections.push(projection);
            }
            name = incan_semantics_core::encode_incan_symbol_identity(&identity);
            self.emitted_inherent_method_identities.insert(identity);
        }

        Ok(IrFunction {
            name,
            docstring: None,
            params: std::mem::take(&mut params),
            return_type,
            body,
            is_async: false,
            is_generator: false,
            visibility,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{lexer, parser, typechecker::TypeChecker};

    #[test]
    fn decorated_method_static_signature_keeps_first_projected_surface_parameter() -> Result<(), String> {
        let source = r#"
def preserve[F]() -> ((F) -> F):
  return (function) => function

from rust::std::vec import Vec as ProviderHandle

class Container:
  @preserve()
  def replace(self, mut items: ProviderHandle[tuple[int, int]]) -> None:
    pass
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let class = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                ast::Declaration::Class(class) if class.name == "Container" => Some(class),
                _ => None,
            })
            .ok_or("missing Container class")?;
        let method = &class.methods.first().ok_or("Container has no methods")?.node;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
        let binding = checker
            .type_info()
            .declarations
            .decorated_method_bindings
            .get(&("Container".to_string(), "replace".to_string()))
            .cloned()
            .ok_or("missing decorated method binding")?;
        let crate::frontend::symbols::ResolvedType::Function(params, ret) = binding.unbound_ty else {
            return Err("decorated method has no callable surface".to_string());
        };
        let original_params = match binding.original_unbound_ty {
            crate::frontend::symbols::ResolvedType::Function(params, _) => params,
            _ => return Err("decorated method has no original callable surface".to_string()),
        };

        let mut type_info = checker.type_info().clone();
        let annotation = "ProviderHandle[tuple[int, int]]";
        let start = source
            .find(annotation)
            .ok_or("projection annotation missing from source")?;
        type_info.rust.mutable_reference_type_argument_projections.insert(
            (start, start + annotation.len()),
            vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![0], vec![1]],
            }],
        );
        let mut lowering = AstLowering::new_with_type_info(type_info);
        lowering
            .lower_program(&program)
            .map_err(|errors| format!("lowering failed: {errors:?}"))?;
        let signature = lowering
            .decorated_method_callable_signature(&params, &ret, method, Some(&original_params))
            .map_err(|error| format!("signature lowering failed: {error:?}"))?;

        assert_eq!(signature.params.len(), 2);
        assert_eq!(signature.params[1].mutability, Mutability::OwnedMutable);
        assert_eq!(
            signature.params[1].ty.rust_name(),
            "ProviderHandle<(&mut i64, &mut i64)>"
        );
        Ok(())
    }
}
