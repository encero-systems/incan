//! Module items that parameter defaults name.
//!
//! A parameter default is evaluated as if in the module that declares the callable, but a caller that omits the
//! argument receives the default at its own call site, in another module or another package. A default that names an
//! item of the declaring module -- a const, a static, a function, or a model, class, enum or newtype it constructs,
//! calls a method of or reads a variant of -- therefore reaches the caller as a path to that item. A private item is
//! still a valid default for a public callable, so the generated item such a path names is published beyond its
//! module: its Incan visibility is unchanged, and only the generated path a default takes becomes reachable. A model
//! or class a default constructs can be spelled at a caller in the same crate as a literal of every field, so its
//! fields are made reachable within the crate, never beyond it; a type a default only calls a method of keeps its
//! fields private. A default can also construct a model or class another module declares; that module publishes the
//! fields the same way once codegen hands it the fact (see [`AstLowering::default_constructed_foreign_types`]). A
//! method partial's presets are defaults of the method it generates, so they count the same way.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::Visibility;
use super::super::AstLowering;
use incan_frontend::ast;
use incan_frontend::ast_walk::any_expr_in_expr;
use incan_semantics_core::SemanticSourceTargetKind;

impl AstLowering {
    /// Record the module items that any parameter default or method-partial preset of this module names, and the
    /// models and classes such a default constructs.
    ///
    /// Every callable a module declares counts: functions, and the methods and method partials of its models, classes,
    /// traits and newtypes, plus the methods of its enums. A name only counts when the module declares a const,
    /// static, function, model, class, enum or newtype by that name, and a construction only when the callee names a
    /// model or class the module declares.
    pub(in crate::lower) fn collect_default_named_items(&mut self, program: &ast::Program) {
        let declared = program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                ast::Declaration::Const(konst) => Some(konst.name.as_str()),
                ast::Declaration::Static(static_decl) => Some(static_decl.name.as_str()),
                ast::Declaration::Function(function) => Some(function.name.as_str()),
                ast::Declaration::Model(model) => Some(model.name.as_str()),
                ast::Declaration::Class(class) => Some(class.name.as_str()),
                ast::Declaration::Enum(enum_decl) => Some(enum_decl.name.as_str()),
                ast::Declaration::Newtype(newtype) => Some(newtype.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut reads = HashMap::new();
        for declaration in &program.declarations {
            match &declaration.node {
                ast::Declaration::Function(function) => self.count_default_reads(&function.params, &mut reads),
                ast::Declaration::Model(model) => {
                    self.count_member_default_reads(&model.methods, &model.method_partials, &mut reads)
                }
                ast::Declaration::Class(class) => {
                    self.count_member_default_reads(&class.methods, &class.method_partials, &mut reads)
                }
                ast::Declaration::Trait(trait_decl) => {
                    self.count_member_default_reads(&trait_decl.methods, &trait_decl.method_partials, &mut reads)
                }
                ast::Declaration::Newtype(newtype) => {
                    self.count_member_default_reads(&newtype.methods, &newtype.method_partials, &mut reads)
                }
                ast::Declaration::Enum(enum_decl) => {
                    self.count_member_default_reads(&enum_decl.methods, &[], &mut reads)
                }
                _ => {}
            }
        }
        self.default_named_items = reads
            .into_keys()
            .filter(|name| declared.contains(name.as_str()))
            .collect();
        let structs = program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                ast::Declaration::Model(model) => Some(model.name.as_str()),
                ast::Declaration::Class(class) => Some(class.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut local = HashSet::new();
        let mut foreign = HashSet::new();
        for (name, span) in Self::default_constructions(program) {
            if structs.contains(name.as_str()) {
                local.insert(name);
            } else if let Some(owner) = self.constructed_type_owner(span) {
                foreign.insert(owner);
            }
        }
        self.default_constructed_types = local;
        self.default_constructed_foreign_types = foreign;
    }

    /// Return the Rust module path and name of the model or class another module declares that the callee at `span`
    /// constructs, when that module is compiled into this crate.
    fn constructed_type_owner(&self, span: ast::Span) -> Option<(Vec<String>, String)> {
        let identity = self.type_info.as_ref()?.resolved_identity(span)?;
        if !matches!(
            identity.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
        ) {
            return None;
        }
        let module_path = self.source_module_rust_paths.get(&identity.origin)?;
        Some((module_path.clone(), identity.declaration_name.clone()))
    }

    /// Return the models and classes of other modules in this crate that a default of this module constructs, each as
    /// its declaring module's Rust path and its name.
    ///
    /// Such a construction is spelled at a caller as a literal of every field, so codegen publishes those fields within
    /// the crate in the declaring module, which cannot see this module's defaults.
    pub fn default_constructed_foreign_types(&self) -> &HashSet<(Vec<String>, String)> {
        &self.default_constructed_foreign_types
    }

    /// Return the name and span of every callee a parameter default or method-partial preset of `program` calls by a
    /// bare name.
    ///
    /// A call through a bare name is how a default constructs a model or class; a method call on a type, such as
    /// `Settings.standard()`, and a variant read, such as `Mode.Fast`, are not calls through a bare name.
    fn default_constructions(program: &ast::Program) -> Vec<(String, ast::Span)> {
        let mut defaults: Vec<&ast::Expr> = Vec::new();
        for declaration in &program.declarations {
            match &declaration.node {
                ast::Declaration::Function(function) => defaults.extend(Self::param_defaults(&function.params)),
                ast::Declaration::Model(model) => {
                    defaults.extend(Self::member_defaults(&model.methods, &model.method_partials))
                }
                ast::Declaration::Class(class) => {
                    defaults.extend(Self::member_defaults(&class.methods, &class.method_partials))
                }
                ast::Declaration::Trait(trait_decl) => {
                    defaults.extend(Self::member_defaults(&trait_decl.methods, &trait_decl.method_partials))
                }
                ast::Declaration::Newtype(newtype) => {
                    defaults.extend(Self::member_defaults(&newtype.methods, &newtype.method_partials))
                }
                ast::Declaration::Enum(enum_decl) => defaults.extend(Self::member_defaults(&enum_decl.methods, &[])),
                _ => {}
            }
        }
        let mut constructed = Vec::new();
        for default in defaults {
            any_expr_in_expr(default, |expr| {
                if let ast::Expr::Call(callee, _, _) = expr
                    && let ast::Expr::Ident(name) = &callee.node
                {
                    constructed.push((name.clone(), callee.span));
                }
                false
            });
        }
        constructed
    }

    /// Return the default expressions of one parameter list.
    fn param_defaults(params: &[ast::Spanned<ast::Param>]) -> impl Iterator<Item = &ast::Expr> {
        params
            .iter()
            .filter_map(|param| param.node.default.as_ref().map(|default| &default.node))
    }

    /// Return the parameter defaults of one type's methods and the presets of its method partials.
    fn member_defaults<'p>(
        methods: &'p [ast::Spanned<ast::MethodDecl>],
        partials: &'p [ast::Spanned<ast::MethodPartialDecl>],
    ) -> impl Iterator<Item = &'p ast::Expr> {
        methods
            .iter()
            .flat_map(|method| Self::param_defaults(&method.node.params))
            .chain(
                partials
                    .iter()
                    .flat_map(|partial| &partial.node.args)
                    .map(|preset| &preset.value.node),
            )
    }

    /// Return the generated visibility of a module item, published when a default names it.
    pub(in crate::lower) fn default_reachable_visibility(&self, name: &str, visibility: Visibility) -> Visibility {
        if visibility != Visibility::Public && self.default_named_items.contains(name) {
            Visibility::Public
        } else {
            visibility
        }
    }

    /// Return the generated visibility of a field of `owner`, reachable within the crate when a default constructs
    /// `owner`.
    ///
    /// A construction written in a default can be spelled at a caller in another module of the crate as a literal of
    /// every field, so each private field of a type a default constructs must be reachable there. A caller in another
    /// package constructs the type through its generated constructor instead, so the field stays out of the library's
    /// public Rust surface. A default that only calls a method of `owner` spells no field and publishes none.
    pub(in crate::lower) fn default_reachable_field_visibility(
        &self,
        owner: &str,
        visibility: Visibility,
    ) -> Visibility {
        if visibility == Visibility::Private && self.default_constructed_types.contains(owner) {
            Visibility::Crate
        } else {
            visibility
        }
    }

    /// Count the names every parameter default and method-partial preset of one type's members reads.
    fn count_member_default_reads(
        &self,
        methods: &[ast::Spanned<ast::MethodDecl>],
        partials: &[ast::Spanned<ast::MethodPartialDecl>],
        reads: &mut HashMap<String, usize>,
    ) {
        for method in methods {
            self.count_default_reads(&method.node.params, reads);
        }
        for preset in partials.iter().flat_map(|partial| &partial.node.args) {
            self.count_expr_ident_reads(&preset.value.node, reads);
        }
    }

    /// Count the names every default of one parameter list reads.
    fn count_default_reads(&self, params: &[ast::Spanned<ast::Param>], reads: &mut HashMap<String, usize>) {
        for default in params.iter().filter_map(|param| param.node.default.as_ref()) {
            self.count_expr_ident_reads(&default.node, reads);
        }
    }
}
