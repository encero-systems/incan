//! First-pass collection: register types, functions, and imports into the symbol table.

use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::freeze_const_type;

use super::TypeChecker;

mod decl_helpers;
pub(super) mod decorators;
mod stdlib_async;
mod stdlib_imports;

use self::decl_helpers::{collect_fields, collect_methods, inject_json_methods, inject_validate_methods};

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
            Declaration::Const(konst) => {
                self.validate_root_namespace(&konst.name, decl.span);
                self.collect_const(konst, decl.span);
            }
            Declaration::Model(model) => {
                self.validate_root_namespace(&model.name, decl.span);
                self.collect_model(model, decl.span);
            }
            Declaration::Class(class) => {
                self.validate_root_namespace(&class.name, decl.span);
                self.collect_class(class, decl.span);
            }
            Declaration::Trait(tr) => {
                self.validate_root_namespace(&tr.name, decl.span);
                self.collect_trait(tr, decl.span);
            }
            Declaration::Newtype(nt) => {
                self.validate_root_namespace(&nt.name, decl.span);
                self.collect_newtype(nt, decl.span);
            }
            Declaration::Enum(en) => {
                self.validate_root_namespace(&en.name, decl.span);
                self.collect_enum(en, decl.span);
            }
            Declaration::Function(func) => {
                self.validate_root_namespace(&func.name, decl.span);
                self.collect_function(func, decl.span);
            }
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
                let resolved = self.resolve_type_checked(t);
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

    /// Register a model declaration with its fields, methods, and derived traits.
    fn collect_model(&mut self, model: &ModelDecl, span: Span) {
        let fields = collect_fields(&model.fields, self);
        let mut methods = collect_methods(&model.methods, self);

        // Inject JSON methods based on derives
        let derives = self.extract_derive_names(&model.decorators);
        inject_json_methods(&mut methods, &model.name, &derives);
        let field_order: Vec<Ident> = model.fields.iter().map(|f| f.node.name.clone()).collect();
        inject_validate_methods(&mut methods, &model.name, &fields, &field_order, &derives);

        self.symbols.define(Symbol {
            name: model.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
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
        fields.extend(collect_fields(&class.fields, self));

        // Add own methods (can override inherited ones)
        methods.extend(collect_methods(&class.methods, self));

        // Inject JSON methods based on derives
        let derives = self.extract_derive_names(&class.decorators);
        inject_json_methods(&mut methods, &class.name, &derives);

        self.symbols.define(Symbol {
            name: class.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
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
        let Some(parent_name) = extends else {
            return (HashMap::new(), HashMap::new());
        };
        let Some(TypeInfo::Class(parent_info)) = self.lookup_type_info(parent_name) else {
            return (HashMap::new(), HashMap::new());
        };
        (parent_info.fields.clone(), parent_info.methods.clone())
    }

    /// Register a trait declaration with its method signatures and requirements.
    fn collect_trait(&mut self, tr: &TraitDecl, span: Span) {
        let methods = collect_methods(&tr.methods, self);
        let requires = self.extract_requires(&tr.decorators);

        self.symbols.define(Symbol {
            name: tr.name.clone(),
            kind: SymbolKind::Trait(TraitInfo {
                type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                methods,
                requires,
            }),
            span,
            scope: 0,
        });
    }

    /// Register a newtype declaration with its underlying type and methods.
    fn collect_newtype(&mut self, nt: &NewtypeDecl, span: Span) {
        let underlying = self.resolve_type_checked(&nt.underlying);

        // Define a placeholder symbol FIRST so methods can reference the newtype name
        self.symbols.define(Symbol {
            name: nt.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                underlying: underlying.clone(),
                methods: HashMap::new(), // Empty for now
            })),
            span,
            scope: 0,
        });

        // Now collect methods - they can reference the newtype name
        let methods = collect_methods(&nt.methods, self);

        // Update the symbol with the collected methods
        if let Some(sym_id) = self.symbols.lookup(&nt.name)
            && let Some(sym) = self.symbols.get_mut(sym_id)
            && let SymbolKind::Type(TypeInfo::Newtype(info)) = &mut sym.kind
        {
            info.methods = methods;
        }
    }

    /// Register an enum declaration and define symbols for each variant.
    fn collect_enum(&mut self, en: &EnumDecl, span: Span) {
        let variants: Vec<_> = en.variants.iter().map(|v| v.node.name.clone()).collect();

        self.symbols.define(Symbol {
            name: en.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
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
                .map(|f| self.resolve_type_checked(f))
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
        // Local declaration shadows any imported marker binding with the same name.
        self.testing_marker_import_bindings.remove(&func.name);

        let params: Vec<_> = func
            .params
            .iter()
            .map(|p| (p.node.name.clone(), self.resolve_type_checked(&p.node.ty)))
            .collect();
        let return_type = self.resolve_type_checked(&func.return_type);

        self.symbols.define(Symbol {
            name: func.name.clone(),
            kind: SymbolKind::Function(FunctionInfo {
                params,
                return_type,
                is_async: func.is_async(),
                type_params: Vec::new(),
            }),
            span,
            scope: 0,
        });
    }
}
