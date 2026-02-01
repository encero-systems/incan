//! First-pass collection: register types, functions, and imports into the symbol table.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::module::ExportedSymbol;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::freeze_const_type;

use super::TypeChecker;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};

// ============================================================================
// Decorator Helpers
// ============================================================================

/// Find decorators by name.
fn decorators_named(decorators: &[Spanned<Decorator>], id: DecoratorId) -> impl Iterator<Item = &Spanned<Decorator>> {
    decorators
        .iter()
        .filter(move |d| decorators::from_str(d.node.name.as_str()) == Some(id))
}

/// Extract positional identifier names from decorator arguments.
fn positional_idents(args: &[DecoratorArg]) -> impl Iterator<Item = (&str, Span)> + '_ {
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

// ============================================================================
// Collection Helpers
// ============================================================================

/// Collect methods from method declarations into a HashMap.
fn collect_methods(methods: &[Spanned<MethodDecl>], symbols: &SymbolTable) -> HashMap<String, MethodInfo> {
    methods
        .iter()
        .map(|m| {
            let params = m
                .node
                .params
                .iter()
                .map(|p| (p.node.name.clone(), resolve_type(&p.node.ty.node, symbols)))
                .collect();
            let return_type = resolve_type(&m.node.return_type.node, symbols);
            (
                m.node.name.clone(),
                MethodInfo {
                    receiver: m.node.receiver,
                    params,
                    return_type,
                    is_async: m.node.is_async,
                    has_body: m.node.body.is_some(),
                },
            )
        })
        .collect()
}

/// Collect fields from field declarations into a HashMap.
fn collect_fields(fields: &[Spanned<FieldDecl>], symbols: &SymbolTable) -> HashMap<String, FieldInfo> {
    fields
        .iter()
        .map(|f| {
            (
                f.node.name.clone(),
                FieldInfo {
                    ty: resolve_type(&f.node.ty.node, symbols),
                    has_default: f.node.default.is_some(),
                    alias: f.node.metadata.alias.clone(),
                    description: f.node.metadata.description.clone(),
                },
            )
        })
        .collect()
}

/// Function signatures for `from testing import ...`.
fn testing_import_function_info(name: &str) -> Option<FunctionInfo> {
    match name {
        "assert" | "assert_true" | "assert_false" => Some(FunctionInfo {
            params: vec![("condition".to_string(), ResolvedType::Bool)],
            return_type: ResolvedType::Unit,
            is_async: false,
            type_params: vec![],
        }),
        "assert_eq" | "assert_ne" => Some(FunctionInfo {
            params: vec![
                ("left".to_string(), ResolvedType::TypeVar("T".to_string())),
                ("right".to_string(), ResolvedType::TypeVar("T".to_string())),
            ],
            return_type: ResolvedType::Unit,
            is_async: false,
            type_params: vec!["T".to_string()],
        }),
        "fail" => Some(FunctionInfo {
            params: vec![("msg".to_string(), ResolvedType::Str)],
            return_type: ResolvedType::Unit,
            is_async: false,
            type_params: vec![],
        }),
        _ => None,
    }
}

/// Inject to_json/from_json methods based on Serialize/Deserialize derives.
fn inject_json_methods(methods: &mut HashMap<String, MethodInfo>, type_name: &str, derives: &[String]) {
    if derives
        .iter()
        .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Serialize))
    {
        methods.insert(
            "to_json".to_string(),
            MethodInfo {
                receiver: Some(Receiver::Immutable),
                params: vec![],
                return_type: ResolvedType::Str,
                is_async: false,
                has_body: true,
            },
        );
    }
    if derives
        .iter()
        .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Deserialize))
    {
        methods.insert(
            "from_json".to_string(),
            MethodInfo {
                receiver: None, // Static method
                params: vec![("json_str".to_string(), ResolvedType::Str)],
                return_type: ResolvedType::Generic(
                    "Result".to_string(),
                    vec![ResolvedType::Named(type_name.to_string()), ResolvedType::Str],
                ),
                is_async: false,
                has_body: true,
            },
        );
    }
}

/// Inject a `TypeName.new(...) -> Result[TypeName, E]` constructor for `@derive(Validate)` models.
///
/// This is a *typechecker-only* method injection to allow `User.new(...)` calls to typecheck even though the backend
/// generates the actual Rust implementation.
fn inject_validate_methods(
    methods: &mut HashMap<String, MethodInfo>,
    _type_name: &str,
    fields: &HashMap<String, FieldInfo>,
    field_order: &[Ident],
    derives: &[String],
) {
    let has_validate = derives
        .iter()
        .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));
    if !has_validate {
        return;
    }

    // Only inject if the user didn't already define it.
    if methods.contains_key("new") {
        return;
    }

    // Use the return type of validate() if present; otherwise use Unknown (second pass will report a better error).
    let return_type = methods
        .get("validate")
        .map(|m| m.return_type.clone())
        .unwrap_or(ResolvedType::Unknown);

    // Prefer required fields only (no defaults). This keeps the signature stable and avoids needing default args.
    let mut params: Vec<(String, ResolvedType)> = Vec::new();
    for field_name in field_order {
        if let Some(info) = fields.get(field_name) {
            if !info.has_default {
                params.push((field_name.clone(), info.ty.clone()));
            }
        }
    }

    methods.insert(
        "new".to_string(),
        MethodInfo {
            receiver: None, // associated function via `TypeName.new(...)`
            params,
            return_type,
            is_async: false,
            has_body: true,
        },
    );
}

impl TypeChecker {
    // ========================================================================
    // First pass: collect declarations
    // ========================================================================

    /// Register a declaration in the symbol table (first pass).
    ///
    /// Dispatches to `collect_import`, `collect_model`, etc. to populate the [`SymbolTable`] with type, function,
    /// and trait definitions. Bodies are **not** validated here; that happens in
    /// [`check_declaration`](Self::check_declaration) in the second pass.
    pub(crate) fn collect_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(import) => self.collect_import(import, decl.span),
            Declaration::Const(konst) => self.collect_const(konst, decl.span),
            Declaration::Model(model) => self.collect_model(model, decl.span),
            Declaration::Class(class) => self.collect_class(class, decl.span),
            Declaration::Trait(tr) => self.collect_trait(tr, decl.span),
            Declaration::Newtype(nt) => self.collect_newtype(nt, decl.span),
            Declaration::Enum(en) => self.collect_enum(en, decl.span),
            Declaration::Function(func) => self.collect_function(func, decl.span),
            Declaration::Docstring(_) => {} // Docstrings don't need collection
        }
    }

    /// Register a module-level const binding (first pass).
    ///
    /// Note: the initializer is validated in the second pass.
    fn collect_const(&mut self, konst: &ConstDecl, span: Span) {
        // Remember for const-eval (cycle detection / evaluation).
        self.const_decls.insert(konst.name.clone(), (konst.clone(), span));

        // Best-effort type from annotation; refined during const-eval in second pass.
        let ty = konst
            .ty
            .as_ref()
            .map(|t| {
                // `const` implies deep immutability; map common container annotations to frozen equivalents.
                let resolved = resolve_type(&t.node, &self.symbols);
                freeze_const_type(resolved)
            })
            .unwrap_or(ResolvedType::Unknown);

        // Define as an immutable variable-like symbol for name resolution.
        self.symbols.define(Symbol {
            name: konst.name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
    }

    /// Register an import declaration in the symbol table.
    fn collect_import(&mut self, import: &ImportDecl, span: Span) {
        self.validate_import_visibility(import, span);
        match &import.kind {
            ImportKind::Module(path) => {
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.segments.last().cloned().unwrap_or_else(|| "module".to_string()));
                self.define_import_symbol(name, path.segments.clone(), false, span);
            }
            ImportKind::From { module, items } => {
                // Special-case stdlib testing API:
                // `from testing import assert_eq, ...` should work as normal function imports (LSP/typechecker),
                // while backend codegen maps these to `incan_stdlib::testing::*`.
                if module.parent_levels == 0 && !module.is_absolute && module.segments == vec!["testing".to_string()] {
                    for item in items {
                        let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                        if let Some(info) = testing_import_function_info(&item.name) {
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Function(info),
                                span,
                                scope: 0,
                            });
                        } else {
                            let mut path = module.segments.clone();
                            path.push(item.name.clone());
                            self.define_import_symbol(local_name, path, false, span);
                        }
                    }
                    return;
                }

                // For each item in `from module import item1, item2, ...`
                // create a symbol as if it were `import module::item`
                for item in items {
                    let aliased_type = item.alias.as_ref().and_then(|alias| {
                        if self.symbols.lookup(alias).is_some() {
                            return None;
                        }
                        let id = self.symbols.lookup(&item.name)?;
                        let sym = self.symbols.get(id)?;
                        let SymbolKind::Type(info) = &sym.kind else {
                            return None;
                        };
                        Some((alias.clone(), info.clone()))
                    });

                    if let Some((alias, info)) = aliased_type {
                        self.symbols.define(Symbol {
                            name: alias,
                            kind: SymbolKind::Type(info),
                            span,
                            scope: 0,
                        });
                        continue;
                    }
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    let mut path = module.segments.clone();
                    path.push(item.name.clone());
                    self.define_import_symbol(name, path, false, span);
                }
            }
            ImportKind::Python(pkg) => {
                let name = import.alias.clone().unwrap_or_else(|| pkg.clone());
                self.define_import_symbol(name, vec![pkg.clone()], true, span);
            }
            ImportKind::RustCrate { crate_name, path } => {
                // Rust crate import: import rust::serde_json or import rust::serde_json::Value
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.clone()));
                let mut full_path = vec![crate_name.clone()];
                full_path.extend(path.clone());
                // Mark as "rust" import type for codegen
                self.define_rust_import_symbol(name, crate_name.clone(), full_path, span);
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
            } => {
                // from rust::time import Instant, Duration
                for item in items {
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    let mut full_path = vec![crate_name.clone()];
                    full_path.extend(path.clone());
                    full_path.push(item.name.clone());
                    self.define_rust_import_symbol(name, crate_name.clone(), full_path, span);
                }
            }
        }
    }

    /// Ensure imported items are public in the dependency module.
    fn validate_import_visibility(&mut self, import: &ImportDecl, span: Span) {
        let ImportKind::From { module, items } = &import.kind else {
            return;
        };

        // Only check modules that were pre-imported; skip std and unresolved ones.
        let module_name = module.segments.join("_");
        let Some(exports) = self.dependency_exports.get(&module_name) else {
            return;
        };

        let mut exported_names: HashSet<String> = HashSet::new();
        for sym in exports {
            match sym {
                ExportedSymbol::Const(name)
                | ExportedSymbol::Type(name)
                | ExportedSymbol::Trait(name)
                | ExportedSymbol::Function(name) => {
                    exported_names.insert(name.clone());
                }
                ExportedSymbol::Variant { variant_name, .. } => {
                    exported_names.insert(variant_name.clone());
                }
            }
        }

        for item in items {
            if !exported_names.contains(&item.name) {
                let message = format!(
                    "Cannot import `{}` from `{}`: it is private or not exported. Mark it `pub` in that module.",
                    item.name,
                    module.to_rust_path()
                );
                let hint = format!(
                    "Public exports from `{}`: {}",
                    module.to_rust_path(),
                    if exported_names.is_empty() {
                        "<none>".to_string()
                    } else {
                        let mut names: Vec<_> = exported_names.iter().cloned().collect();
                        names.sort();
                        names.join(", ")
                    }
                );

                self.errors.push(CompileError::new(message, span).with_hint(hint));
            }
        }
    }

    /// Define a symbol for a Rust crate import, skipping if a real definition exists.
    fn define_rust_import_symbol(&mut self, name: Ident, crate_name: String, path: Vec<Ident>, span: Span) {
        if let Some(id) = self.symbols.lookup(&name) {
            if let Some(sym) = self.symbols.get(id) {
                match &sym.kind {
                    SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::Trait(_) | SymbolKind::Variant(_) => {
                        return;
                    }
                    _ => {}
                }
            }
        }

        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::RustModule {
                crate_name,
                path: path.join("::"),
            },
            span,
            scope: 0, // Will be set by define()
        });
    }

    /// Define a symbol for a module import, skipping if a real definition exists.
    fn define_import_symbol(&mut self, name: Ident, path: Vec<Ident>, is_python: bool, span: Span) {
        if let Some(id) = self.symbols.lookup(&name) {
            if let Some(sym) = self.symbols.get(id) {
                match &sym.kind {
                    SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::Trait(_) | SymbolKind::Variant(_) => {
                        // Already have a real definition, don't overwrite with Module placeholder
                        return;
                    }
                    _ => {}
                }
            }
        }

        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::Module(ModuleInfo { path, is_python }),
            span,
            scope: 0,
        });
    }

    /// Register a model declaration with its fields, methods, and derived traits.
    fn collect_model(&mut self, model: &ModelDecl, span: Span) {
        let fields = collect_fields(&model.fields, &self.symbols);
        let mut methods = collect_methods(&model.methods, &self.symbols);

        // Inject JSON methods based on derives
        let derives = Self::extract_derive_names(&model.decorators);
        inject_json_methods(&mut methods, &model.name, &derives);
        let field_order: Vec<Ident> = model.fields.iter().map(|f| f.node.name.clone()).collect();
        inject_validate_methods(&mut methods, &model.name, &fields, &field_order, &derives);

        self.symbols.define(Symbol {
            name: model.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                type_params: model.type_params.clone(),
                traits: model.traits.iter().map(|t| t.node.clone()).collect(),
                derives,
                fields,
                methods,
            })),
            span,
            scope: 0,
        });
    }

    /// Register a class declaration, inheriting from parent if present.
    fn collect_class(&mut self, class: &ClassDecl, span: Span) {
        let (mut fields, mut methods) = self.inherit_from_parent(&class.extends);

        // Add own fields (can override inherited ones)
        fields.extend(collect_fields(&class.fields, &self.symbols));

        // Add own methods (can override inherited ones)
        methods.extend(collect_methods(&class.methods, &self.symbols));

        // Inject JSON methods based on derives
        let derives = Self::extract_derive_names(&class.decorators);
        inject_json_methods(&mut methods, &class.name, &derives);

        self.symbols.define(Symbol {
            name: class.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                type_params: class.type_params.clone(),
                extends: class.extends.clone(),
                traits: class.traits.iter().map(|t| t.node.clone()).collect(),
                derives,
                fields,
                methods,
            })),
            span,
            scope: 0,
        });
    }

    /// Inherit fields and methods from a parent class if present.
    fn inherit_from_parent(
        &self,
        extends: &Option<String>,
    ) -> (HashMap<String, FieldInfo>, HashMap<String, MethodInfo>) {
        let mut fields = HashMap::new();
        let mut methods = HashMap::new();

        if let Some(parent_name) = extends {
            if let Some(parent_id) = self.symbols.lookup(parent_name) {
                if let Some(parent_sym) = self.symbols.get(parent_id) {
                    if let SymbolKind::Type(TypeInfo::Class(parent_info)) = &parent_sym.kind {
                        fields = parent_info.fields.clone();
                        methods = parent_info.methods.clone();
                    }
                }
            }
        }

        (fields, methods)
    }

    /// Register a trait declaration with its method signatures and requirements.
    fn collect_trait(&mut self, tr: &TraitDecl, span: Span) {
        let methods = collect_methods(&tr.methods, &self.symbols);
        let requires = self.extract_requires(&tr.decorators);

        self.symbols.define(Symbol {
            name: tr.name.clone(),
            kind: SymbolKind::Trait(TraitInfo {
                type_params: tr.type_params.clone(),
                methods,
                requires,
            }),
            span,
            scope: 0,
        });
    }

    /// Validate @derive decorator arguments and report errors for unknown derives.
    pub(crate) fn validate_derives(&mut self, decorators: &[Spanned<Decorator>]) {
        for dec in decorators_named(decorators, DecoratorId::Derive) {
            // Collect all derive names with their spans
            let derive_items: Vec<_> = dec
                .node
                .args
                .iter()
                .filter_map(|arg| {
                    match arg {
                        DecoratorArg::Positional(expr) => {
                            if let Expr::Ident(name) = &expr.node {
                                Some((name.clone(), expr.span))
                            } else {
                                None
                            }
                        }
                        DecoratorArg::Named(name, _) => {
                            // Named args not valid for derive, but report error on them
                            Some((name.clone(), dec.span))
                        }
                    }
                })
                .collect();

            for (name, span) in derive_items {
                self.validate_single_derive(&name, span);
            }
        }
    }

    /// Validate a single derive name, reporting appropriate errors.
    fn validate_single_derive(&mut self, name: &str, span: Span) {
        if derives::from_str(name).is_some() {
            return;
        }

        // Check if the name refers to a type/function (wrong usage)
        if let Some(kind_name) = self.lookup_symbol_kind(name) {
            self.errors.push(errors::derive_wrong_kind(name, kind_name, span));
        } else {
            self.errors.push(errors::unknown_derive(name, span));
        }
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

    /// Extract `@requires` constraints from decorators as `(name, type)` pairs.
    fn extract_requires(&mut self, decorators: &[Spanned<Decorator>]) -> Vec<(String, ResolvedType)> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut requires: Vec<(String, ResolvedType)> = Vec::new();

        for dec in decorators_named(decorators, DecoratorId::Requires) {
            for arg in &dec.node.args {
                if let DecoratorArg::Named(name, DecoratorArgValue::Type(ty)) = arg {
                    if !seen.insert(name.clone()) {
                        self.errors.push(errors::duplicate_trait_requires_field(name, ty.span));
                        continue;
                    }
                    requires.push((name.clone(), resolve_type(&ty.node, &self.symbols)));
                }
            }
        }
        requires
    }

    /// Extract derive names from @derive decorators.
    pub(crate) fn extract_derive_names(decorators: &[Spanned<Decorator>]) -> Vec<String> {
        decorators_named(decorators, DecoratorId::Derive)
            .flat_map(|dec| positional_idents(&dec.node.args))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Register a newtype declaration with its underlying type and methods.
    fn collect_newtype(&mut self, nt: &NewtypeDecl, span: Span) {
        let underlying = resolve_type(&nt.underlying.node, &self.symbols);
        let methods = collect_methods(&nt.methods, &self.symbols);

        self.symbols.define(Symbol {
            name: nt.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo { underlying, methods })),
            span,
            scope: 0,
        });
    }

    /// Register an enum declaration and define symbols for each variant.
    fn collect_enum(&mut self, en: &EnumDecl, span: Span) {
        let variants: Vec<_> = en.variants.iter().map(|v| v.node.name.clone()).collect();

        self.symbols.define(Symbol {
            name: en.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                type_params: en.type_params.clone(),
                variants: variants.clone(),
            })),
            span,
            scope: 0,
        });

        // Also define each variant as a symbol
        for variant in &en.variants {
            let fields: Vec<_> = variant
                .node
                .fields
                .iter()
                .map(|f| resolve_type(&f.node, &self.symbols))
                .collect();
            self.symbols.define(Symbol {
                name: variant.node.name.clone(),
                kind: SymbolKind::Variant(VariantInfo {
                    enum_name: en.name.clone(),
                    fields,
                }),
                span: variant.span,
                scope: 0,
            });
        }
    }

    /// Register a top-level function declaration.
    fn collect_function(&mut self, func: &FunctionDecl, span: Span) {
        let params: Vec<_> = func
            .params
            .iter()
            .map(|p| (p.node.name.clone(), resolve_type(&p.node.ty.node, &self.symbols)))
            .collect();
        let return_type = resolve_type(&func.return_type.node, &self.symbols);

        self.symbols.define(Symbol {
            name: func.name.clone(),
            kind: SymbolKind::Function(FunctionInfo {
                params,
                return_type,
                is_async: func.is_async,
                type_params: Vec::new(),
            }),
            span,
            scope: 0,
        });
    }
}
