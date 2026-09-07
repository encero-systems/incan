//! Decorator resolution and validation helpers for the first pass.
//!
//! This keeps decorator path resolution and validation logic out of the main collection flow while preserving RFC 022
//! semantics.

use std::collections::{HashMap, HashSet};

use crate::frontend::api_metadata::ApiDeclaration;
use crate::frontend::ast::*;
use crate::frontend::decorator_resolution;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::symbols::{ResolvedType, SymbolKind, SymbolTable, TypeInfo};
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::typechecker::type_info::{
    CBindingBuffer, CBindingDescriptor, CBindingEnum, CBindingEnumVariant, CBindingOutcome, CBindingParameter,
    CBindingResource, CBindingStruct, CBindingStructField, CBindingSymbol, CBindingType, COutputMode, CResourceAccess,
};
use incan_core::lang::c_abi::{
    self, BindingArgumentId, BindingMemberId, LinkCapabilityId, ResourceArgumentId, ResourceTypeConstructorId,
    SymbolArgumentId, SymbolOutcomeArgumentId,
};
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives;
use incan_core::lang::stdlib;
use incan_semantics_core::{
    CanonicalSymbolId, DecoratorFeature, HirSourceSpan, SemanticSourceTargetKind, SurfaceFeatureKey, SymbolNamespace,
    SymbolOrigin,
};

#[derive(Clone, Copy)]
enum DecoratorValidationTarget {
    AllowsUserDefined,
    RejectsUserDefined(&'static str),
}

/// Parsed outcome data before it is checked against one C symbol's result and output contracts.
struct RawCBindingOutcome {
    result: String,
    initializes: Vec<String>,
    updates: Vec<String>,
    invalidates: Vec<String>,
}

/// Raw data parsed from one C symbol before it becomes a checked binding descriptor.
type RawCBindingSymbolData = (String, Vec<(String, String)>, Vec<RawCBindingOutcome>);

/// Parsed C symbol data that must wait for the binding's complete enum namespace before outcome validation.
struct RawCBindingSymbol {
    span: Span,
    name: String,
    native: String,
    parameters: Vec<CBindingParameter>,
    return_type: CBindingType,
    bounds: Vec<(String, String)>,
    outcomes: Vec<RawCBindingOutcome>,
}

/// Fully checked members retained in one C binding descriptor.
struct CBindingMembers {
    resources: Vec<CBindingResource>,
    symbols: Vec<CBindingSymbol>,
    enums: Vec<CBindingEnum>,
    structs: Vec<CBindingStruct>,
}

/// Resolve a decorator path to a module path.
pub(in crate::frontend::typechecker) fn resolve_decorator_path(dec: &Decorator, symbols: &SymbolTable) -> Vec<String> {
    decorator_resolution::resolve_decorator_path(dec, symbols)
}

/// Resolve a decorator path to a decorator id.
pub(in crate::frontend::typechecker) fn resolve_decorator_id(
    dec: &Decorator,
    symbols: &SymbolTable,
) -> Option<DecoratorId> {
    let resolved = resolve_decorator_path(dec, symbols);
    decorators::from_segments(&resolved)
}

/// Find decorators by name.
pub(super) fn decorators_named<'a>(
    decorators: &'a [Spanned<Decorator>],
    symbols: &SymbolTable,
    id: DecoratorId,
) -> impl Iterator<Item = &'a Spanned<Decorator>> {
    decorators
        .iter()
        .filter(move |d| resolve_decorator_id(&d.node, symbols) == Some(id))
}

/// Extract positional identifier names from decorator arguments.
pub(super) fn positional_idents(args: &[DecoratorArg]) -> impl Iterator<Item = (&str, Span)> + '_ {
    args.iter().filter_map(|arg| match arg {
        DecoratorArg::Positional(expr) => {
            if let Expr::Ident(name) = &expr.node {
                Some((name.as_str(), expr.span))
            } else {
                None
            }
        }
        _ => None,
    })
}

impl TypeChecker {
    /// Return whether a checked provider manifest declares this resolved path as an extern-backed decorator function.
    fn provider_declares_decorator_function(&self, resolved: &[String]) -> bool {
        let Some((function_name, module_path)) = resolved.split_last() else {
            return false;
        };
        let Some(provider) = self.provider_plan.active_sdk_provider_for_module(module_path) else {
            return false;
        };
        let Some(api) = provider
            .manifest
            .as_deref()
            .and_then(|manifest| manifest.contract_metadata.api.as_ref())
        else {
            return false;
        };
        let provider_module_path = if module_path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
            &module_path[1..]
        } else {
            module_path
        };
        api.modules
            .iter()
            .filter(|module| module.module_path == provider_module_path)
            .flat_map(|module| module.declarations.iter())
            .any(|declaration| {
                let ApiDeclaration::Function(function) = declaration else {
                    return false;
                };
                function.name == *function_name
                    && function
                        .decorators
                        .iter()
                        .any(|decorator| decorator.path.as_slice() == ["rust", "extern"])
            })
    }

    /// Validate decorator paths for declarations that allow user-defined decorator candidates.
    pub(crate) fn validate_decorators_allowing_user_defined(&mut self, decorators: &[Spanned<Decorator>]) {
        self.validate_decorators_for_target(decorators, DecoratorValidationTarget::AllowsUserDefined);
    }

    /// Validate decorator paths for declarations that do not allow user-defined decorators.
    pub(crate) fn validate_decorators_rejecting_user_defined(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
    ) {
        self.validate_decorators_for_target(decorators, DecoratorValidationTarget::RejectsUserDefined(kind));
    }

    /// Validate decorator paths, preserving compiler-owned decorator diagnostics while deciding whether unknown
    /// non-compiler decorators are accepted as user-defined candidates or rejected for this target.
    ///
    /// When a decorator doesn't resolve to a known `DecoratorId`, the error message is contextual:
    /// - If the leading segment is a known namespace (e.g. `rust`, `std`), the error mentions the namespace and lists
    ///   available decorators within it.
    /// - Otherwise, supported function-like targets keep it for RFC 036 typechecking, while unsupported targets emit a
    ///   user-defined decorator target diagnostic.
    fn validate_decorators_for_target(&mut self, decorators: &[Spanned<Decorator>], target: DecoratorValidationTarget) {
        for dec in decorators {
            let resolved = resolve_decorator_path(&dec.node, &self.symbols);
            let feature = self.surface_context.decorator_feature_for_path(&resolved);

            let Some(id) = decorators::from_segments(&resolved) else {
                let is_stdlib_decorator_function = feature
                    == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
                    && resolved.len() >= 3
                    && (self.provider_declares_decorator_function(&resolved)
                        || self
                            .stdlib_cache
                            .lookup_function_meta(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
                            .is_some_and(|f| f.is_rust_extern && f.rust_module_path.is_some()));
                if is_stdlib_decorator_function {
                    continue;
                }

                let path = if resolved.is_empty() {
                    dec.node.name.clone()
                } else {
                    resolved.join(".")
                };

                // ---- Namespace-aware error (e.g. "@rust.blah" → "unknown in `rust` namespace") ----
                if let Some(first) = resolved.first()
                    && decorators::is_known_decorator_namespace(first)
                {
                    let known = decorators::decorators_in_namespace(first);
                    let known_display: Vec<_> = known.iter().map(|d| format!("@{d}")).collect();
                    let hint = if known_display.is_empty() {
                        format!("No decorators are currently defined in the `{first}` namespace")
                    } else {
                        format!("Known `{first}` decorators: {}", known_display.join(", "))
                    };
                    self.errors
                        .push(errors::unknown_decorator(&path, dec.span).with_hint(&hint));
                } else if let DecoratorValidationTarget::RejectsUserDefined(kind) = target {
                    self.errors
                        .push(errors::user_defined_decorator_unsupported_target(&path, kind, dec.span));
                } else {
                    continue;
                }
                continue;
            };

            self.type_info.record_resolved_identity(
                dec.span,
                CanonicalSymbolId {
                    namespace: SymbolNamespace::OrdinaryLexical,
                    origin: SymbolOrigin::Builtin,
                    declaration_name: decorators::as_str(id).to_string(),
                    kind: SemanticSourceTargetKind::Builtin,
                    scope_discriminant: None,
                    declaration_span: HirSourceSpan::new(0, 0),
                },
            );

            if id == DecoratorId::RustAllow {
                self.validate_rust_allow_args(dec);
            }
        }
    }

    /// Validate RFC 057 `@rust.allow(...)` arguments.
    ///
    /// The decorator is intentionally item-scoped and accepts only explicit lint paths so generated code can emit
    /// targeted `#[allow(...)]` attributes without introducing broad crate- or module-level suppression.
    pub(crate) fn validate_rust_allow_args(&mut self, dec: &Spanned<Decorator>) {
        let mut seen = HashSet::new();
        let mut positional_count = 0usize;

        for arg in &dec.node.args {
            match arg {
                DecoratorArg::Positional(expr) => {
                    positional_count += 1;
                    let Expr::Literal(Literal::String(name)) = &expr.node else {
                        self.errors
                            .push(errors::rust_allow_requires_positional_string(expr.span));
                        continue;
                    };
                    self.validate_single_rust_allow_lint(name, expr.span, &mut seen);
                }
                DecoratorArg::Named(name, _) => {
                    self.errors.push(errors::rust_allow_rejects_named_args(name, dec.span));
                }
            }
        }

        if positional_count == 0 {
            self.errors
                .push(errors::rust_allow_requires_positional_string(dec.span));
        }
    }

    /// Validate direct RFC 043 `@rust.derive(...)` passthrough on concrete type declarations.
    pub(crate) fn validate_rust_derives(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
        is_rusttype: bool,
        traits: &[Spanned<TraitBound>],
    ) {
        let rust_derives: Vec<_> = decorators_named(decorators, &self.symbols, DecoratorId::RustDerive).collect();
        if rust_derives.is_empty() {
            return;
        }

        if kind != "model" && kind != "class" && kind != "enum" && kind != "newtype" {
            for dec in rust_derives {
                self.errors
                    .push(errors::rust_derive_unsupported_attachment(kind, dec.span));
            }
            return;
        }

        if is_rusttype {
            for dec in rust_derives {
                self.errors.push(errors::rust_derive_unsupported_rusttype(dec.span));
            }
            return;
        }

        for dec in rust_derives {
            let mut positional_count = 0usize;
            for arg in &dec.node.args {
                match arg {
                    DecoratorArg::Positional(expr) => {
                        positional_count += 1;
                        self.validate_single_rust_derive_arg(&expr.node, expr.span, traits);
                    }
                    DecoratorArg::Named(name, _) => {
                        self.errors.push(errors::rust_derive_rejects_named_args(name, dec.span));
                    }
                }
            }
            if positional_count == 0 {
                self.errors.push(errors::rust_derive_requires_positional_arg(dec.span));
            }
        }
    }

    /// Validate one positional `@rust.derive(...)` argument.
    fn validate_single_rust_derive_arg(&mut self, expr: &Expr, span: Span, traits: &[Spanned<TraitBound>]) {
        match expr {
            Expr::Ident(name) => {
                let leaf = self
                    .rust_derive_leaf_for_ident(name)
                    .unwrap_or(name.as_str())
                    .to_string();
                self.validate_rust_derive_trait_conflict(&leaf, traits, span);
                if Self::is_builtin_rust_derive(&leaf) || self.rust_import_path_for_local_name(name).is_some() {
                    return;
                }
                self.errors.push(errors::rust_derive_unresolved(name, span));
            }
            Expr::Literal(Literal::String(path)) => {
                let Some(leaf) = Self::rust_path_leaf(path) else {
                    self.errors.push(errors::rust_derive_invalid_arg(span));
                    return;
                };
                self.validate_rust_derive_trait_conflict(leaf, traits, span);
                if Self::is_builtin_rust_derive(leaf) && !path.contains("::") {
                    return;
                }
                if self.rust_derive_path_has_declared_crate(path) {
                    return;
                }
                self.errors.push(errors::rust_derive_unresolved(path, span));
            }
            _ => self.errors.push(errors::rust_derive_invalid_arg(span)),
        }
    }

    /// Reject derive names that would duplicate an explicit trait adoption.
    fn validate_rust_derive_trait_conflict(&mut self, derive_leaf: &str, traits: &[Spanned<TraitBound>], span: Span) {
        for trait_ref in traits {
            let trait_leaf = self
                .rust_derive_leaf_for_ident(&trait_ref.node.name)
                .unwrap_or_else(|| Self::trait_name_leaf(&trait_ref.node.name));
            if trait_leaf == derive_leaf {
                self.errors.push(errors::rust_derive_conflicts_with_trait_adoption(
                    derive_leaf,
                    &trait_ref.node.name,
                    span,
                ));
            }
        }
    }

    /// Resolve an imported derive binding to its final path segment.
    fn rust_derive_leaf_for_ident(&self, name: &str) -> Option<&str> {
        self.import_binding_path(name)
            .and_then(|segments| segments.last().map(String::as_str))
            .or_else(|| {
                self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
                    SymbolKind::RustItem(info) => info.path.rsplit("::").next(),
                    _ => None,
                })
            })
    }

    /// Return the Rust path imported for a local derive binding name.
    fn rust_import_path_for_local_name(&self, name: &str) -> Option<String> {
        self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
            SymbolKind::RustItem(info) => Some(info.path.clone()),
            _ => None,
        })
    }

    /// Return whether a string Rust derive path names a crate available to generated Rust.
    fn rust_derive_path_has_declared_crate(&self, path: &str) -> bool {
        let segments: Vec<_> = path.split("::").collect();
        if segments.is_empty() || !segments.iter().all(|segment| Self::is_valid_rust_path_segment(segment)) {
            return false;
        }
        let Some(crate_name) = segments.first() else {
            return false;
        };
        if matches!(*crate_name, "std" | "core" | "alloc") {
            return true;
        }
        self.declared_crate_names
            .as_ref()
            .is_some_and(|declared| declared.contains(*crate_name))
    }

    /// Return the leaf segment for a syntactically valid Rust path string.
    fn rust_path_leaf(path: &str) -> Option<&str> {
        if path.split("::").all(Self::is_valid_rust_path_segment) {
            return path.rsplit("::").next();
        }
        None
    }

    /// Return whether one Rust path segment is acceptable in a derive path string.
    fn is_valid_rust_path_segment(segment: &str) -> bool {
        let segment = segment.strip_prefix("r#").unwrap_or(segment);
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    /// Return the final source trait segment for conflict comparisons.
    fn trait_name_leaf(name: &str) -> &str {
        name.rsplit('.').next().unwrap_or(name)
    }

    /// Return whether a derive name is built into Rust and needs no dependency metadata.
    fn is_builtin_rust_derive(name: &str) -> bool {
        matches!(
            derives::from_str(name),
            Some(
                derives::DeriveId::Clone
                    | derives::DeriveId::Copy
                    | derives::DeriveId::Debug
                    | derives::DeriveId::Default
                    | derives::DeriveId::Eq
                    | derives::DeriveId::Hash
                    | derives::DeriveId::Ord
                    | derives::DeriveId::PartialEq
                    | derives::DeriveId::PartialOrd
            )
        )
    }

    /// Reject RFC 057 `@rust.allow(...)` on declarations that do not own a supported Rust item boundary.
    ///
    /// Parser syntax allows decorators on several declaration forms. This helper keeps the semantic support matrix
    /// explicit so adding a new declaration kind does not silently inherit Rust lint suppression behavior.
    pub(crate) fn reject_rust_allow_on_unsupported_declaration(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
    ) {
        for dec in decorators {
            if self.decorator_id(&dec.node) == Some(DecoratorId::RustAllow) {
                self.errors
                    .push(errors::rust_allow_unsupported_attachment(kind, dec.span));
            }
        }
    }

    /// Return whether this decorator should be handled as an RFC 036 user-defined decorator candidate.
    ///
    /// Compiler-owned decorators and stdlib marker decorators keep their existing compiler semantics. Unknown paths in
    /// known compiler namespaces stay diagnostic-only rather than becoming user-defined decorators.
    pub(crate) fn is_user_defined_decorator_candidate(&mut self, dec: &Decorator) -> bool {
        if self.decorator_id(dec).is_some() {
            return false;
        }

        let resolved = decorator_resolution::resolve_decorator_path(dec, &self.symbols);
        if resolved
            .first()
            .is_some_and(|first| decorators::is_known_decorator_namespace(first))
        {
            return false;
        }

        let feature = self.surface_context.decorator_feature_for_path(&resolved);
        let is_stdlib_decorator_function = feature
            == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
            && resolved.len() >= 3
            && self
                .stdlib_cache
                .lookup_function_meta(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
                .is_some_and(|f| f.is_rust_extern && f.rust_module_path.is_some());
        !is_stdlib_decorator_function
    }

    /// Resolve a decorator identifier through checked source bindings.
    pub(crate) fn decorator_id(&self, dec: &Decorator) -> Option<DecoratorId> {
        let resolved = resolve_decorator_path(dec, &self.symbols);
        decorators::from_segments(&resolved)
    }

    /// Validate the source-owned descriptor attached to a checked C binding class.
    ///
    /// The import-activated `binding` vocabulary lowers to this ordinary decorator/class form, so the parser transports
    /// only vocabulary data while this language-layer check owns all explicit, inspectable ABI meaning.
    pub(crate) fn validate_c_binding_class(&mut self, class: &ClassDecl) {
        // A class must contribute one descriptor at most. Rejecting the duplicate before parsing its fields avoids
        // constructing a partial descriptor whose source authority would depend on decorator ordering.
        let bindings = class
            .decorators
            .iter()
            .filter(|decorator| self.decorator_id(&decorator.node) == Some(DecoratorId::CBinding))
            .collect::<Vec<_>>();
        if bindings.len() > 1 {
            self.errors.push(CompileError::type_error(
                "a class may declare only one @c.binding descriptor".to_string(),
                bindings[1].span,
            ));
            return;
        }
        let Some(decorator) = bindings.first() else {
            return;
        };
        let mut valid = true;
        if !self.c_interop_decorator_is_imported(&decorator.node) {
            self.errors.push(CompileError::type_error(
                "@c.binding requires `from std.interop import c` (or an alias of c)".to_string(),
                decorator.span,
            ));
            valid = false;
        }
        if class.extends.as_deref() != Some(c_abi::BINDING_DECLARATION_BASE) {
            self.errors.push(CompileError::type_error(
                "@c.binding classes must extend BindingDeclaration".to_string(),
                decorator.span,
            ));
            valid = false;
        }

        // The decorator is the declaration's outer envelope: header and link facts live here, while its vocabulary
        // members describe the named ABI surface below. Keep these channels separate so neither can imply the other.
        let mut header = None;
        let mut link = None;
        for argument in &decorator.node.args {
            let DecoratorArg::Named(name, DecoratorArgValue::Expr(value)) = argument else {
                self.errors.push(CompileError::type_error(
                    "@c.binding accepts only named `header` and `link` arguments".to_string(),
                    decorator.span,
                ));
                valid = false;
                continue;
            };
            let slot = match c_abi::binding_argument_from_str(name) {
                Some(BindingArgumentId::Header) => &mut header,
                Some(BindingArgumentId::Link) => &mut link,
                None => {
                    self.errors.push(CompileError::type_error(
                        format!("@c.binding does not accept argument `{name}`"),
                        value.span,
                    ));
                    valid = false;
                    continue;
                }
            };
            if slot.replace(value).is_some() {
                self.errors.push(CompileError::type_error(
                    format!("@c.binding repeats `{name}`"),
                    value.span,
                ));
                valid = false;
            }
        }

        let header = match header.and_then(|header| match &header.node {
            Expr::Literal(Literal::String(value)) if !value.is_empty() => Some(value.clone()),
            _ => None,
        }) {
            Some(header) => header,
            None => {
                self.errors.push(CompileError::type_error(
                    "@c.binding requires a named non-empty string-literal `header` argument".to_string(),
                    decorator.span,
                ));
                valid = false;
                String::new()
            }
        };
        let link = link.and_then(|value| self.c_system_library_name(value));
        let (system_library, link_capability) = match link {
            Some(link) => link,
            None => {
                self.errors.push(CompileError::type_error(
                    "@c.binding requires `link = c.system_library(\"name\")` or `c.framework(\"name\")`".to_string(),
                    decorator.span,
                ));
                valid = false;
                (String::new(), LinkCapabilityId::SystemLibrary)
            }
        };

        // Do not register an incomplete descriptor. Later access checking and lowering intentionally consume only this
        // complete checked product, never the raw class body or a best-effort subset of its declarations.
        let Some(members) = self.validate_c_binding_members(class, decorator.span) else {
            return;
        };
        if valid {
            self.type_info.c_abi.bindings.insert(
                class.name.clone(),
                CBindingDescriptor {
                    span: decorator.span,
                    class_name: class.name.clone(),
                    header,
                    system_library,
                    link_capability,
                    resources: members.resources,
                    symbols: members.symbols,
                    enums: members.enums,
                    structs: members.structs,
                },
            );
        }
    }

    /// Validate declarative C members retained by the ordinary lowered class.
    fn validate_c_binding_members(&mut self, class: &ClassDecl, fallback_span: Span) -> Option<CBindingMembers> {
        let mut valid = true;
        let mut resources = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut raw_symbols = Vec::<RawCBindingSymbol>::new();

        // Structures are collected first because the supported scalar call signatures may name a declared plain
        // structure. The validation pass remains order-independent even when a symbol precedes its structure body.
        let struct_names = class
            .declarative_members
            .iter()
            .filter(|member| c_abi::binding_member_from_str(&member.keyword) == Some(BindingMemberId::Struct))
            .filter_map(|member| member.head.name.clone())
            .collect::<HashSet<_>>();
        let resource_names = class
            .declarative_members
            .iter()
            .filter(|member| c_abi::binding_member_from_str(&member.keyword) == Some(BindingMemberId::Resource))
            .filter_map(|member| member.head.name.clone())
            .collect::<HashSet<_>>();

        for member in &class.declarative_members {
            let span = Self::c_member_span(member, fallback_span);
            let Some(kind) = c_abi::binding_member_from_str(&member.keyword) else {
                self.errors.push(CompileError::type_error(
                    format!("@c.binding does not accept `{}` declarations", member.keyword),
                    span,
                ));
                valid = false;
                continue;
            };
            let Some(name) = member.head.name.as_ref().filter(|name| !name.is_empty()) else {
                self.errors.push(CompileError::type_error(
                    format!("C {} declarations require a name", member.keyword),
                    span,
                ));
                valid = false;
                continue;
            };
            if !member.decorators.is_empty() || !member.head.header_args.is_empty() {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C {} declarations do not accept decorators or header arguments",
                        member.keyword
                    ),
                    span,
                ));
                valid = false;
                continue;
            }
            // Every accepted member becomes data in the descriptor. In particular, `symbol` owns no executable body:
            // it maps one Incan-facing signature to one explicit native spelling for later target verification.
            match kind {
                BindingMemberId::Resource => {
                    if !member.head.parameters.is_empty() || member.head.return_type.is_some() {
                        self.errors.push(CompileError::type_error(
                            "C resource declarations may not have a signature".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    }
                    let Some((native, release)) = Self::c_resource_fields(member, span, &mut self.errors) else {
                        valid = false;
                        continue;
                    };
                    resources.push(CBindingResource {
                        span,
                        name: name.clone(),
                        native,
                        release,
                    });
                }
                BindingMemberId::Symbol => {
                    let Some(return_type) = member.head.return_type.as_ref() else {
                        self.errors.push(CompileError::type_error(
                            "C symbols require an explicit return type".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    };
                    let Some(return_type) =
                        Self::c_binding_type(&return_type.source, &resource_names, &struct_names, false)
                    else {
                        self.errors.push(CompileError::type_error(
                            format!("C symbol `{name}` uses an unsupported return type"),
                            span,
                        ));
                        valid = false;
                        continue;
                    };
                    let mut parameters = Vec::new();
                    for parameter in &member.head.parameters {
                        let Some(parameter_type) = parameter.param_type.as_ref() else {
                            self.errors.push(CompileError::type_error(
                                format!("C symbol `{name}` parameters require explicit types"),
                                span,
                            ));
                            valid = false;
                            continue;
                        };
                        let Some(parameter_type) =
                            Self::c_binding_type(&parameter_type.source, &resource_names, &struct_names, true)
                        else {
                            self.errors.push(CompileError::type_error(
                                format!("C symbol `{name}` uses an unsupported parameter type"),
                                span,
                            ));
                            valid = false;
                            continue;
                        };
                        if parameter.default_value.is_some() || parameter.name.is_empty() {
                            self.errors.push(CompileError::type_error(
                                format!("C symbol `{name}` does not accept default or unnamed parameters"),
                                span,
                            ));
                            valid = false;
                            continue;
                        }
                        parameters.push(CBindingParameter {
                            name: parameter.name.clone(),
                            ty: parameter_type,
                        });
                    }
                    let Some((native, bounds, raw_outcomes)) = Self::c_symbol_data(member, span, &mut self.errors)
                    else {
                        valid = false;
                        continue;
                    };
                    raw_symbols.push(RawCBindingSymbol {
                        span,
                        name: name.clone(),
                        native,
                        parameters,
                        return_type,
                        bounds,
                        outcomes: raw_outcomes,
                    });
                }
                BindingMemberId::Enum => {
                    if !member.head.parameters.is_empty() || member.head.return_type.is_some() {
                        self.errors.push(CompileError::type_error(
                            "C enum declarations may not have a signature".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    }
                    let Some(enumeration) = Self::c_enum(member, name, span, &mut self.errors) else {
                        valid = false;
                        continue;
                    };
                    enums.push(enumeration);
                }
                BindingMemberId::Struct => {
                    if !member.head.parameters.is_empty() || member.head.return_type.is_some() {
                        self.errors.push(CompileError::type_error(
                            "C struct declarations may not have a signature".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    }
                    let Some(structure) =
                        Self::c_struct(member, name, span, &resource_names, &struct_names, &mut self.errors)
                    else {
                        valid = false;
                        continue;
                    };
                    structs.push(structure);
                }
            }
        }

        if raw_symbols
            .iter()
            .map(|symbol| &symbol.name)
            .collect::<HashSet<_>>()
            .len()
            != raw_symbols.len()
            || enums
                .iter()
                .map(|enumeration| &enumeration.name)
                .collect::<HashSet<_>>()
                .len()
                != enums.len()
            || resources
                .iter()
                .map(|resource| &resource.name)
                .collect::<HashSet<_>>()
                .len()
                != resources.len()
            || structs
                .iter()
                .map(|structure| &structure.name)
                .collect::<HashSet<_>>()
                .len()
                != structs.len()
        {
            self.errors.push(CompileError::type_error(
                "C binding members must have unique names within their declaration kind".to_string(),
                fallback_span,
            ));
            valid = false;
        }

        let mut symbols = Vec::with_capacity(raw_symbols.len());
        for raw_symbol in raw_symbols {
            let Some(outcomes) = Self::c_symbol_outcomes(
                raw_symbol.outcomes,
                &raw_symbol.name,
                &raw_symbol.return_type,
                &raw_symbol.parameters,
                &enums,
                raw_symbol.span,
                &mut self.errors,
            ) else {
                valid = false;
                continue;
            };
            let Some(buffers) = Self::c_symbol_buffer_contracts(
                raw_symbol.bounds,
                &raw_symbol.name,
                &raw_symbol.parameters,
                raw_symbol.span,
                &mut self.errors,
            ) else {
                valid = false;
                continue;
            };
            symbols.push(CBindingSymbol {
                name: raw_symbol.name,
                native: raw_symbol.native,
                parameters: raw_symbol.parameters,
                return_type: raw_symbol.return_type,
                buffers,
                outcomes,
            });
        }

        for resource in &resources {
            let release = symbols.iter().find(|symbol| symbol.name == resource.release);
            if !release.is_some_and(|symbol| {
                matches!(
                    symbol.parameters.as_slice(),
                    [CBindingParameter {
                        ty: CBindingType::Resource {
                            access: CResourceAccess::Owned,
                            resource: parameter_resource,
                        },
                        ..
                    }] if parameter_resource == &resource.name
                )
            }) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C resource `{}` release symbol `{}` must accept exactly c.Owned[{}]",
                        resource.name, resource.release, resource.name
                    ),
                    resource.span,
                ));
                valid = false;
            }
        }

        valid.then_some(CBindingMembers {
            resources,
            symbols,
            enums,
            structs,
        })
    }

    /// Prefer a vocabulary member's source span, retaining the binding span for synthesized members.
    fn c_member_span(member: &incan_vocab::VocabDeclaration, fallback: Span) -> Span {
        if member.span.end > member.span.start {
            Span::new(member.span.start, member.span.end)
        } else {
            fallback
        }
    }

    /// Extract the native opaque spelling and release symbol from one resource declaration.
    fn c_resource_fields(
        member: &incan_vocab::VocabDeclaration,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<(String, String)> {
        let mut native = None;
        let mut release = None;
        for item in &member.body {
            let incan_vocab::VocabBodyItem::Statement(statement) = item else {
                errors.push(CompileError::type_error(
                    "C resource declarations contain data fields only".to_string(),
                    span,
                ));
                return None;
            };
            let (field, value) = match statement {
                incan_vocab::IncanStatement::Let { name, value, .. } => (name, value),
                incan_vocab::IncanStatement::Assign { target, value } => (target, value),
                _ => {
                    errors.push(CompileError::type_error(
                        "C resource declarations contain data fields only".to_string(),
                        span,
                    ));
                    return None;
                }
            };
            let Some(argument) = c_abi::resource_argument_from_str(field) else {
                errors.push(CompileError::type_error(
                    format!("C resource does not accept `{field}`"),
                    span,
                ));
                return None;
            };
            match (argument, value) {
                (ResourceArgumentId::Native, incan_vocab::IncanExpr::Str(value))
                    if !value.is_empty() && native.replace(value.clone()).is_none() => {}
                (ResourceArgumentId::Release, incan_vocab::IncanExpr::Name(value))
                    if !value.is_empty() && release.replace(value.clone()).is_none() => {}
                _ => {
                    errors.push(CompileError::type_error(
                        format!("C resource `{field}` has the wrong value form or is repeated"),
                        span,
                    ));
                    return None;
                }
            }
        }
        match (native, release) {
            (Some(native), Some(release)) => Some((native, release)),
            _ => {
                errors.push(CompileError::type_error(
                    "C resource requires one non-empty `native` field and one `release` symbol".to_string(),
                    span,
                ));
                None
            }
        }
    }

    /// Extract the physical symbol spelling and declarative result outcomes from one raw C symbol.
    fn c_symbol_data(
        member: &incan_vocab::VocabDeclaration,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<RawCBindingSymbolData> {
        let mut native = None;
        let mut bounds = None;
        let mut outcomes = Vec::new();
        for item in &member.body {
            match item {
                incan_vocab::VocabBodyItem::Statement(statement) => {
                    let (field, value) = match statement {
                        incan_vocab::IncanStatement::Let { name, value, .. }
                        | incan_vocab::IncanStatement::Assign { target: name, value } => (name, value),
                        _ => {
                            errors.push(CompileError::type_error(
                                "C symbols contain only a `native` field and declarative outcomes".to_string(),
                                span,
                            ));
                            return None;
                        }
                    };
                    let Some(argument) = c_abi::symbol_argument_from_str(field) else {
                        errors.push(CompileError::type_error(
                            "C symbols accept only `native` and `bounds` data fields".to_string(),
                            span,
                        ));
                        return None;
                    };
                    match (argument, value) {
                        (SymbolArgumentId::Native, incan_vocab::IncanExpr::Str(value))
                            if !value.is_empty() && native.replace(value.clone()).is_none() => {}
                        (SymbolArgumentId::Native, _) => {
                            errors.push(CompileError::type_error(
                                "C symbol `native` must be one non-empty string literal".to_string(),
                                span,
                            ));
                            return None;
                        }
                        (SymbolArgumentId::Bounds, value) => {
                            let Some(parsed) = Self::c_symbol_bounds(value) else {
                                errors.push(CompileError::type_error(
                                    "C symbol `bounds` must be a non-empty dictionary of pointer parameter names to c.Size parameter names"
                                        .to_string(),
                                    span,
                                ));
                                return None;
                            };
                            if bounds.replace(parsed).is_some() {
                                errors.push(CompileError::type_error(
                                    "C symbols may state `bounds` once".to_string(),
                                    span,
                                ));
                                return None;
                            }
                        }
                    }
                }
                incan_vocab::VocabBodyItem::Declaration(outcome) => {
                    outcomes.push(Self::c_symbol_outcome(outcome, span, errors)?);
                }
                _ => {
                    errors.push(CompileError::type_error(
                        "C symbols contain only a `native` field and declarative outcomes".to_string(),
                        span,
                    ));
                    return None;
                }
            }
        }
        native
            .map(|native| (native, bounds.unwrap_or_default(), outcomes))
            .or_else(|| {
                errors.push(CompileError::type_error(
                    "C symbols require exactly one non-empty `native` field".to_string(),
                    span,
                ));
                None
            })
    }

    /// Parse the deliberately declarative pointer-to-length relation before checking it against one symbol signature.
    fn c_symbol_bounds(value: &incan_vocab::IncanExpr) -> Option<Vec<(String, String)>> {
        let incan_vocab::IncanExpr::Dict(entries) = value else {
            return None;
        };
        if entries.is_empty() {
            return None;
        }
        let mut pointer_names = HashSet::new();
        let mut result = Vec::with_capacity(entries.len());
        for (pointer, length) in entries {
            let (incan_vocab::IncanExpr::Name(pointer), incan_vocab::IncanExpr::Name(length)) = (pointer, length)
            else {
                return None;
            };
            if pointer.is_empty() || length.is_empty() || !pointer_names.insert(pointer.clone()) {
                return None;
            }
            result.push((pointer.clone(), length.clone()));
        }
        Some(result)
    }

    /// Check every typed-span declaration against the symbol it bounds before it can become a compiler fact.
    fn c_symbol_buffer_contracts(
        bounds: Vec<(String, String)>,
        symbol: &str,
        parameters: &[CBindingParameter],
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<Vec<CBindingBuffer>> {
        let pointer_parameters = parameters
            .iter()
            .filter_map(|parameter| match &parameter.ty {
                CBindingType::Pointer { pointee, .. }
                    if matches!(
                        pointee.as_ref(),
                        CBindingType::Scalar(c_abi::ScalarTypeId::U8 | c_abi::ScalarTypeId::F32)
                    ) =>
                {
                    let CBindingType::Scalar(element) = pointee.as_ref() else {
                        return None;
                    };
                    Some((parameter.name.as_str(), *element))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        if pointer_parameters.is_empty() {
            if bounds.is_empty() {
                return Some(Vec::new());
            }
            errors.push(CompileError::type_error(
                format!("C symbol `{symbol}` declares checked-span bounds but has no c.ConstPtr[c.u8|c.f32] or c.MutPtr[c.u8|c.f32] parameter"),
                span,
            ));
            return None;
        }
        let declared_pointers = bounds
            .iter()
            .map(|(pointer, _)| pointer.as_str())
            .collect::<HashSet<_>>();
        if declared_pointers != pointer_parameters.keys().copied().collect::<HashSet<_>>() {
            errors.push(CompileError::type_error(
                format!(
                    "C symbol `{symbol}` must declare `bounds` for every checked span-pointer parameter exactly once"
                ),
                span,
            ));
            return None;
        }
        let mut contracts = Vec::with_capacity(bounds.len());
        for (pointer_parameter, length_parameter) in bounds {
            let Some(length) = parameters.iter().find(|parameter| parameter.name == length_parameter) else {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol}` bounds `{pointer_parameter}` to unknown parameter `{length_parameter}`"
                    ),
                    span,
                ));
                return None;
            };
            if !matches!(length.ty, CBindingType::Scalar(c_abi::ScalarTypeId::Size)) {
                errors.push(CompileError::type_error(
                    format!("C symbol `{symbol}` bounds `{pointer_parameter}` with `{length_parameter}`, which must be c.Size"),
                    span,
                ));
                return None;
            }
            let Some(element) = pointer_parameters.get(pointer_parameter.as_str()).copied() else {
                errors.push(CompileError::type_error(
                    format!("C symbol `{symbol}` bounds unknown checked span-pointer `{pointer_parameter}`"),
                    span,
                ));
                return None;
            };
            contracts.push(CBindingBuffer {
                pointer_parameter,
                length_parameter,
                element,
            });
        }
        Some(contracts)
    }

    /// Extract one raw result declaration before checking it against a symbol's typed contract.
    fn c_symbol_outcome(
        outcome: &incan_vocab::VocabDeclaration,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<RawCBindingOutcome> {
        if outcome.keyword != c_abi::SYMBOL_OUTCOME_KEYWORD
            || outcome.head.name.is_some()
            || outcome.head.header_args.len() != 1
            || !outcome.head.parameters.is_empty()
            || outcome.head.return_type.is_some()
            || !outcome.decorators.is_empty()
        {
            errors.push(CompileError::type_error(
                "C symbol outcomes require one qualified result value and no signature".to_string(),
                span,
            ));
            return None;
        }
        let Some(result) = Self::c_native_reference(&outcome.head.header_args[0]) else {
            errors.push(CompileError::type_error(
                "C symbol outcomes require one qualified enum result value".to_string(),
                span,
            ));
            return None;
        };
        let mut initializes = None;
        let mut updates = None;
        let mut invalidates = None;
        for item in &outcome.body {
            let incan_vocab::VocabBodyItem::Statement(statement) = item else {
                errors.push(CompileError::type_error(
                    "C symbol outcomes contain data fields only".to_string(),
                    span,
                ));
                return None;
            };
            let (field, value) = match statement {
                incan_vocab::IncanStatement::Let { name, value, .. }
                | incan_vocab::IncanStatement::Assign { target: name, value } => (name, value),
                _ => {
                    errors.push(CompileError::type_error(
                        "C symbol outcomes contain data fields only".to_string(),
                        span,
                    ));
                    return None;
                }
            };
            let Some(field) = c_abi::symbol_outcome_argument_from_str(field) else {
                errors.push(CompileError::type_error(
                    "C symbol outcomes accept only initializes, updates, or invalidates".to_string(),
                    span,
                ));
                return None;
            };
            let Some(names) = Self::c_outcome_parameter_names(value) else {
                errors.push(CompileError::type_error(
                    "C symbol outcome fields require a list of unique parameter names".to_string(),
                    span,
                ));
                return None;
            };
            let slot = match field {
                SymbolOutcomeArgumentId::Initializes => &mut initializes,
                SymbolOutcomeArgumentId::Updates => &mut updates,
                SymbolOutcomeArgumentId::Invalidates => &mut invalidates,
            };
            if slot.replace(names).is_some() {
                errors.push(CompileError::type_error(
                    "C symbol outcomes may state each field once".to_string(),
                    span,
                ));
                return None;
            }
        }
        Some(RawCBindingOutcome {
            result,
            initializes: initializes.unwrap_or_default(),
            updates: updates.unwrap_or_default(),
            invalidates: invalidates.unwrap_or_default(),
        })
    }

    /// Extract an outcome's ordinary distinct parameter-name list.
    fn c_outcome_parameter_names(value: &incan_vocab::IncanExpr) -> Option<Vec<String>> {
        let incan_vocab::IncanExpr::List(values) = value else {
            return None;
        };
        let mut names = Vec::with_capacity(values.len());
        let mut seen = HashSet::new();
        for value in values {
            let incan_vocab::IncanExpr::Name(name) = value else {
                return None;
            };
            if name.is_empty() || !seen.insert(name.clone()) {
                return None;
            }
            names.push(name.clone());
        }
        Some(names)
    }

    /// Check raw outcome data against one C symbol's scalar result and output-slot contracts.
    #[allow(clippy::too_many_arguments)]
    fn c_symbol_outcomes(
        raw_outcomes: Vec<RawCBindingOutcome>,
        symbol_name: &str,
        return_type: &CBindingType,
        parameters: &[CBindingParameter],
        enums: &[CBindingEnum],
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<Vec<CBindingOutcome>> {
        let mut outcomes = Vec::with_capacity(raw_outcomes.len());
        let mut seen_results = HashSet::new();
        let mut initialized_outputs = HashSet::new();
        let mut valid = true;

        for raw in raw_outcomes {
            let Some((enum_name, variant_name)) = raw.result.split_once('.') else {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol_name}` outcome `{}` must name an enum variant",
                        raw.result
                    ),
                    span,
                ));
                valid = false;
                continue;
            };
            let Some(enumeration) = enums.iter().find(|enumeration| enumeration.name == enum_name) else {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol_name}` outcome `{}` names an unknown enum",
                        raw.result
                    ),
                    span,
                ));
                valid = false;
                continue;
            };
            let Some(_variant) = enumeration.variants.iter().find(|variant| variant.name == variant_name) else {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol_name}` outcome `{}` names an unknown enum variant",
                        raw.result
                    ),
                    span,
                ));
                valid = false;
                continue;
            };
            if !matches!(return_type, CBindingType::Scalar(carrier) if carrier == &enumeration.carrier) {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol_name}` outcome `{}` has a carrier incompatible with its return type",
                        raw.result
                    ),
                    span,
                ));
                valid = false;
                continue;
            }
            if !seen_results.insert(raw.result.clone()) {
                errors.push(CompileError::type_error(
                    format!("C symbol `{symbol_name}` repeats outcome `{}`", raw.result),
                    span,
                ));
                valid = false;
                continue;
            }

            let mut mentioned = HashSet::new();
            let mut outcome_valid = true;
            for (field, names, expected_mode) in [
                ("initializes", &raw.initializes, COutputMode::Out),
                ("updates", &raw.updates, COutputMode::InOut),
                ("invalidates", &raw.invalidates, COutputMode::InOut),
            ] {
                for name in names {
                    if !mentioned.insert(name.as_str()) {
                        errors.push(CompileError::type_error(
                            format!(
                                "C symbol `{symbol_name}` outcome `{}` repeats parameter `{name}`",
                                raw.result
                            ),
                            span,
                        ));
                        outcome_valid = false;
                        continue;
                    }
                    let Some(parameter) = parameters.iter().find(|parameter| parameter.name == *name) else {
                        errors.push(CompileError::type_error(
                            format!(
                                "C symbol `{symbol_name}` outcome `{}` names unknown parameter `{name}`",
                                raw.result
                            ),
                            span,
                        ));
                        outcome_valid = false;
                        continue;
                    };
                    if !matches!(&parameter.ty, CBindingType::Output { mode, .. } if *mode == expected_mode) {
                        let expected = match expected_mode {
                            COutputMode::Out => "c.Out[...]",
                            COutputMode::InOut => "c.InOut[...]",
                        };
                        errors.push(CompileError::type_error(
                            format!(
                                "C symbol `{symbol_name}` outcome `{}` `{field}` must name a {expected} parameter",
                                raw.result
                            ),
                            span,
                        ));
                        outcome_valid = false;
                        continue;
                    }
                    if expected_mode == COutputMode::Out {
                        initialized_outputs.insert(name.clone());
                    }
                }
            }
            if outcome_valid {
                outcomes.push(CBindingOutcome {
                    result: raw.result,
                    initializes: raw.initializes,
                    updates: raw.updates,
                    invalidates: raw.invalidates,
                });
            } else {
                valid = false;
            }
        }

        for parameter in parameters {
            if matches!(
                &parameter.ty,
                CBindingType::Output {
                    mode: COutputMode::Out,
                    ..
                }
            ) && !initialized_outputs.contains(&parameter.name)
            {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol_name}` must initialize c.Out parameter `{}` through an outcome",
                        parameter.name
                    ),
                    span,
                ));
                valid = false;
            }
        }

        valid.then_some(outcomes)
    }

    /// Collect one C enum and enforce its shared scalar carrier.
    fn c_enum(
        member: &incan_vocab::VocabDeclaration,
        name: &str,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<CBindingEnum> {
        let mut carrier = None;
        let mut variants = Vec::new();
        for item in &member.body {
            let incan_vocab::VocabBodyItem::Statement(incan_vocab::IncanStatement::TypedLet {
                name: variant,
                mutable: false,
                ty,
                value,
            }) = item
            else {
                errors.push(CompileError::type_error(
                    "C enum declarations require typed carrier assignments".to_string(),
                    span,
                ));
                return None;
            };
            let Some(this_carrier) = c_abi::scalar_type_from_str(&ty.source) else {
                errors.push(CompileError::type_error(
                    format!("C enum variant `{variant}` uses an unsupported carrier type"),
                    span,
                ));
                return None;
            };
            if carrier
                .replace(this_carrier)
                .is_some_and(|existing| existing != this_carrier)
            {
                errors.push(CompileError::type_error(
                    "C enum variants must use one shared carrier type".to_string(),
                    span,
                ));
                return None;
            }
            let Some(native) = Self::c_native_reference(value) else {
                errors.push(CompileError::type_error(
                    format!("C enum variant `{variant}` requires a native constant reference"),
                    span,
                ));
                return None;
            };
            variants.push(CBindingEnumVariant {
                name: variant.clone(),
                native,
            });
        }
        let Some(carrier) = carrier else {
            errors.push(CompileError::type_error(
                "C enum declarations require at least one variant".to_string(),
                span,
            ));
            return None;
        };
        Some(CBindingEnum {
            name: name.to_string(),
            carrier,
            variants,
        })
    }

    /// Collect one plain C structure and its declared native field layout.
    fn c_struct(
        member: &incan_vocab::VocabDeclaration,
        name: &str,
        span: Span,
        resources: &HashSet<String>,
        structs: &HashSet<String>,
        errors: &mut Vec<CompileError>,
    ) -> Option<CBindingStruct> {
        let mut native = None;
        let mut fields = Vec::new();
        for item in &member.body {
            let incan_vocab::VocabBodyItem::Statement(statement) = item else {
                errors.push(CompileError::type_error(
                    "C struct declarations contain fields only".to_string(),
                    span,
                ));
                return None;
            };
            match statement {
                incan_vocab::IncanStatement::Let { name: field, value, .. }
                | incan_vocab::IncanStatement::Assign { target: field, value }
                    if c_abi::plain_struct_argument_from_str(field).is_some() =>
                {
                    let incan_vocab::IncanExpr::Str(value) = value else {
                        errors.push(CompileError::type_error(
                            "C struct `native` must be a non-empty string literal".to_string(),
                            span,
                        ));
                        return None;
                    };
                    if value.is_empty() || native.replace(value.clone()).is_some() {
                        errors.push(CompileError::type_error(
                            "C struct requires exactly one non-empty `native` field".to_string(),
                            span,
                        ));
                        return None;
                    }
                }
                incan_vocab::IncanStatement::TypedLet {
                    name: field,
                    mutable: false,
                    ty,
                    value: incan_vocab::IncanExpr::Name(value),
                } if field == value => {
                    let Some(ty) = Self::c_binding_type(&ty.source, resources, structs, false) else {
                        errors.push(CompileError::type_error(
                            format!("C struct field `{field}` uses an unsupported C type"),
                            span,
                        ));
                        return None;
                    };
                    fields.push(CBindingStructField {
                        name: field.clone(),
                        ty,
                    });
                }
                _ => {
                    errors.push(CompileError::type_error(
                        "C struct fields must use `field: c.Type = field`".to_string(),
                        span,
                    ));
                    return None;
                }
            }
        }
        let Some(native) = native else {
            errors.push(CompileError::type_error(
                "C struct declarations require `native`".to_string(),
                span,
            ));
            return None;
        };
        if fields.is_empty() {
            errors.push(CompileError::type_error(
                "C struct declarations require at least one field".to_string(),
                span,
            ));
            return None;
        }
        Some(CBindingStruct {
            name: name.to_string(),
            native,
            fields,
        })
    }

    /// Resolve a supported C spelling into the checked binding type model.
    pub(crate) fn c_binding_type(
        source: &str,
        resources: &HashSet<String>,
        structs: &HashSet<String>,
        accepts_output: bool,
    ) -> Option<CBindingType> {
        let source = source.trim();
        if c_abi::is_void_type_spelling(source) {
            return Some(CBindingType::Void);
        }
        if let Some(scalar) = c_abi::scalar_type_from_str(source) {
            return Some(CBindingType::Scalar(scalar));
        }
        if let Some(inner) = source.strip_prefix("Option[").and_then(|rest| rest.strip_suffix(']')) {
            let value = Self::c_binding_type(inner, resources, structs, false)?;
            return matches!(
                value,
                CBindingType::Resource {
                    access: CResourceAccess::Owned,
                    ..
                }
            )
            .then(|| CBindingType::Nullable(Box::new(value)));
        }
        for (constructor, access) in [
            (ResourceTypeConstructorId::Owned, CResourceAccess::Owned),
            (ResourceTypeConstructorId::Borrowed, CResourceAccess::Borrowed),
            (ResourceTypeConstructorId::BorrowedMut, CResourceAccess::BorrowedMut),
        ] {
            if let Some(inner) = source
                .strip_prefix(c_abi::resource_type_constructor_as_str(constructor))
                .and_then(|rest| rest.strip_prefix('['))
                .and_then(|rest| rest.strip_suffix(']'))
            {
                return resources.contains(inner).then(|| CBindingType::Resource {
                    access,
                    resource: inner.to_string(),
                });
            }
        }
        if accepts_output {
            for (constructor, mode) in [
                (ResourceTypeConstructorId::Out, COutputMode::Out),
                (ResourceTypeConstructorId::InOut, COutputMode::InOut),
            ] {
                if let Some(inner) = source
                    .strip_prefix(c_abi::resource_type_constructor_as_str(constructor))
                    .and_then(|rest| rest.strip_prefix('['))
                    .and_then(|rest| rest.strip_suffix(']'))
                {
                    let value = Self::c_binding_type(inner, resources, structs, false)?;
                    if matches!(
                        (&mode, &value),
                        (
                            COutputMode::InOut,
                            CBindingType::Resource {
                                access: CResourceAccess::Owned,
                                ..
                            }
                        )
                    ) {
                        return None;
                    }
                    return Some(CBindingType::Output {
                        mode,
                        value: Box::new(value),
                    });
                }
            }
        }
        for (constructor, mutable) in [("c.ConstPtr", false), ("c.MutPtr", true)] {
            if let Some(inner) = source
                .strip_prefix(constructor)
                .and_then(|rest| rest.strip_prefix('['))
                .and_then(|rest| rest.strip_suffix(']'))
            {
                return Some(CBindingType::Pointer {
                    mutable,
                    pointee: Box::new(Self::c_binding_type(inner, resources, structs, false)?),
                });
            }
        }
        structs
            .contains(source)
            .then(|| CBindingType::Struct(source.to_string()))
    }

    /// Render a source name or qualified access as a native C constant reference.
    fn c_native_reference(value: &incan_vocab::IncanExpr) -> Option<String> {
        match value {
            incan_vocab::IncanExpr::Name(name) if !name.is_empty() => Some(name.clone()),
            incan_vocab::IncanExpr::Field { object, field } if !field.is_empty() => {
                Some(format!("{}.{}", Self::c_native_reference(object)?, field))
            }
            incan_vocab::IncanExpr::RelationField { relation, field } if !relation.is_empty() && !field.is_empty() => {
                Some(format!("{relation}.{field}"))
            }
            _ => None,
        }
    }

    /// Return whether the decorator's leading alias resolves to the activated C vocabulary.
    fn c_interop_decorator_is_imported(&self, decorator: &Decorator) -> bool {
        decorator.path.segments.first().is_some_and(|prefix| {
            self.import_binding_path(prefix)
                .is_some_and(|path| c_abi::is_interop_namespace_path(path.iter().map(String::as_str)))
        })
    }

    /// Extract the logical library name and exact link shape from an imported C link declaration.
    fn c_system_library_name(&self, value: &Spanned<Expr>) -> Option<(String, LinkCapabilityId)> {
        let (namespace, method, type_args, arguments) = match &value.node {
            Expr::MethodCall(namespace, method, type_args, arguments) => (
                namespace.as_ref(),
                method.as_str(),
                type_args.as_slice(),
                arguments.as_slice(),
            ),
            Expr::Call(callee, type_args, arguments) => {
                let Expr::Field(namespace, method) = &callee.node else {
                    return None;
                };
                (
                    namespace.as_ref(),
                    method.as_str(),
                    type_args.as_slice(),
                    arguments.as_slice(),
                )
            }
            _ => return None,
        };
        if !type_args.is_empty() {
            return None;
        }
        let link_capability = c_abi::link_capability_from_str(method)?;
        let Expr::Ident(namespace_name) = &namespace.node else {
            return None;
        };
        if !self
            .import_binding_path(namespace_name)
            .is_some_and(|path| c_abi::is_interop_namespace_path(path.iter().map(String::as_str)))
        {
            return None;
        }
        match arguments {
            [CallArg::Positional(value)] => match &value.node {
                Expr::Literal(Literal::String(value)) if !value.is_empty() => Some((value.clone(), link_capability)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Validate one lint name passed to `@rust.allow`.
    fn validate_single_rust_allow_lint(&mut self, name: &str, span: Span, seen: &mut HashSet<String>) {
        if name.is_empty() || name.trim() != name || !Self::is_valid_rust_lint_path(name) {
            self.errors.push(errors::rust_allow_invalid_lint_name(name, span));
            return;
        }

        if Self::is_broad_rust_lint_group(name) {
            self.errors.push(errors::rust_allow_broad_lint_group(name, span));
            return;
        }

        if !seen.insert(name.to_string()) {
            self.errors.push(errors::rust_allow_duplicate_lint(name, span));
        }
    }

    /// Return whether a Rust lint path has valid syntax.
    fn is_valid_rust_lint_path(name: &str) -> bool {
        name.split("::").all(Self::is_valid_rust_lint_segment)
    }

    /// Return whether one Rust lint path segment is valid.
    fn is_valid_rust_lint_segment(segment: &str) -> bool {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    /// Return whether a Rust lint group is too broad for `@rust.allow`.
    fn is_broad_rust_lint_group(name: &str) -> bool {
        matches!(
            name,
            "warnings"
                | "unused"
                | "clippy::all"
                | "clippy::pedantic"
                | "clippy::nursery"
                | "clippy::restriction"
                | "clippy::cargo"
        )
    }

    /// Validate @derive decorator arguments and report errors for unknown derives.
    pub(crate) fn validate_derives(&mut self, decorators: &[Spanned<Decorator>]) {
        let derive_items: Vec<_> = decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| {
                dec.node.args.iter().filter_map(|arg| match arg {
                    DecoratorArg::Positional(expr) => {
                        if let Expr::Ident(name) = &expr.node {
                            Some((name.clone(), expr.span))
                        } else {
                            None
                        }
                    }
                    DecoratorArg::Named(name, _) => {
                        // Named args not valid for derive, but report error on them.
                        Some((name.clone(), dec.span))
                    }
                })
            })
            .collect();

        for (name, span) in derive_items {
            if self.validate_single_derive(&name, span) {
                self.record_derive_argument_identity(&name, span);
            }
        }
    }

    /// Record the compiler-proven target selected by one accepted `@derive(...)` argument.
    fn record_derive_argument_identity(&mut self, name: &str, span: Span) {
        let identity = derives::from_str(name)
            .and_then(|id| {
                let canonical = derives::as_str(id);
                self.symbols
                    .lookup(canonical)
                    .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
                    .cloned()
                    .or_else(|| {
                        Some(CanonicalSymbolId {
                            namespace: SymbolNamespace::OrdinaryLexical,
                            origin: SymbolOrigin::Builtin,
                            declaration_name: canonical.to_string(),
                            kind: SemanticSourceTargetKind::Builtin,
                            scope_discriminant: None,
                            declaration_span: HirSourceSpan::new(0, 0),
                        })
                    })
            })
            .or_else(|| {
                self.symbols
                    .lookup(name)
                    .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
                    .cloned()
            });
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Extract derive names from @derive decorators.
    pub(crate) fn extract_derive_names(&self, decorators: &[Spanned<Decorator>]) -> Vec<String> {
        decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| positional_idents(&dec.node.args))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Record canonical Rust derive paths applied directly to one local concrete type.
    ///
    /// Unlike `@derive(...)`, RFC 043 `@rust.derive(...)` is emitted verbatim and does not adopt an Incan trait.
    /// Its inspected probe expansion can select a candidate Rust generic ABI, so retain the exact resolved macro
    /// namespace without mixing it into [`TypeInfo`](crate::frontend::symbols::TypeInfo) derive names. Native rustc
    /// compilation remains authoritative for the real generated declaration.
    pub(crate) fn record_local_rust_derive_paths(&mut self, type_name: &str, decorators: &[Spanned<Decorator>]) {
        let mut paths = Vec::new();
        for decorator in decorators_named(decorators, &self.symbols, DecoratorId::RustDerive) {
            for argument in &decorator.node.args {
                let DecoratorArg::Positional(expression) = argument else {
                    continue;
                };
                let path = match &expression.node {
                    Expr::Ident(name) => self.rust_import_path_for_local_name(name),
                    Expr::Literal(Literal::String(path)) if self.rust_derive_path_has_declared_crate(path) => {
                        Some(path.clone())
                    }
                    _ => None,
                };
                if let Some(path) = path
                    && !paths.contains(&path)
                {
                    paths.push(path);
                }
            }
        }
        if paths.is_empty() {
            self.local_rust_derive_paths.remove(type_name);
        } else {
            self.local_rust_derive_paths.insert(type_name.to_string(), paths);
        }
    }

    /// Extract `@requires` constraints from decorators as `(name, type)` pairs.
    pub(super) fn extract_requires(&mut self, decorators: &[Spanned<Decorator>]) -> Vec<(String, ResolvedType)> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut requires: Vec<(String, ResolvedType)> = Vec::new();

        for dec in decorators {
            if resolve_decorator_id(&dec.node, &self.symbols) != Some(DecoratorId::Requires) {
                continue;
            }
            for arg in &dec.node.args {
                if let DecoratorArg::Named(name, DecoratorArgValue::Type(ty)) = arg {
                    if !seen.insert(name.clone()) {
                        self.errors.push(errors::duplicate_trait_requires_field(name, ty.span));
                        continue;
                    }
                    requires.push((name.clone(), self.resolve_type_checked(ty)));
                }
            }
        }
        requires
    }

    /// Validate a single derive name, reporting appropriate errors.
    fn validate_single_derive(&mut self, name: &str, span: Span) -> bool {
        if derives::from_str(name).is_some() {
            return true;
        }

        if self
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        {
            return true;
        }

        if self
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Module(_)))
            && let Some(module_path) = self.module_path_for_imported_name(name)
        {
            if self.lookup_derivable_traits(&module_path).is_some() {
                return true;
            }
            self.errors.push(errors::derive_module_missing_derives(name, span));
            return false;
        }

        if let Some((canonical, info)) = self.resolve_qualified_trait(name) {
            self.define_hidden_trait_symbol(&canonical, info, span);
            return true;
        }

        // Allow custom derives imported from stdlib modules backed by rust.module(...).
        let resolved = self
            .import_binding_path(name)
            .map(<[String]>::to_vec)
            .unwrap_or_else(|| vec![name.to_string()]);
        if resolved.len() >= 2
            && self.imported_trait_is_derivable(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
        {
            return true;
        }

        // Check if the name refers to a type/function (wrong usage)
        if let Some(kind_name) = self.lookup_symbol_kind(name) {
            self.errors.push(errors::derive_wrong_kind(name, kind_name, span));
        } else {
            self.errors.push(errors::unknown_derive(name, span));
        }
        false
    }

    /// Look up what kind of symbol a name refers to, if any.
    fn lookup_symbol_kind(&self, name: &str) -> Option<&'static str> {
        let sym_id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(sym_id)?;

        match &sym.kind {
            SymbolKind::Type(TypeInfo::Model(_)) => Some("model"),
            SymbolKind::Type(TypeInfo::Class(_)) => Some("class"),
            SymbolKind::Type(TypeInfo::Enum(_)) => Some("enum"),
            SymbolKind::Function(_) => Some("function"),
            _ => None,
        }
    }
}
