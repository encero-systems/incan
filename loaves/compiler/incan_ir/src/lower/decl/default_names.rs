//! Module items that parameter defaults name.
//!
//! A parameter default is evaluated as if in the module that declares the callable, but a caller that omits the
//! argument receives the default at its own call site, in another module or another package. A default that names an
//! item of the declaring module -- a const, a static, a function, or a model, class, enum or newtype it constructs or
//! reads a variant of -- therefore reaches the caller as a path to that item. A private item is still a valid default
//! for a public callable, so the generated item such a path names is published beyond its module, together with the
//! fields a construction spells: its Incan visibility is unchanged, and only the generated path a default takes becomes
//! reachable. A method partial's presets are defaults of the method it generates, so they count the same way.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::Visibility;
use super::super::AstLowering;
use incan_frontend::ast;

impl AstLowering {
    /// Record the module items that any parameter default or method-partial preset of this module names.
    ///
    /// Every callable a module declares counts: functions, and the methods and method partials of its models, classes,
    /// traits and newtypes, plus the methods of its enums. A name only counts when the module declares a const,
    /// static, function, model, class, enum or newtype by that name.
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
    }

    /// Return the generated visibility of a module item, published when a default names it.
    pub(in crate::lower) fn default_reachable_visibility(&self, name: &str, visibility: Visibility) -> Visibility {
        if visibility != Visibility::Public && self.default_named_items.contains(name) {
            Visibility::Public
        } else {
            visibility
        }
    }

    /// Return the generated visibility of a field of `owner`, published when a default constructs `owner`.
    ///
    /// A construction written in a default is spelled at the caller as a literal of every field, so each field of a
    /// type a default names must be reachable there, whatever its Incan visibility.
    pub(in crate::lower) fn default_reachable_field_visibility(
        &self,
        owner: &str,
        visibility: Visibility,
    ) -> Visibility {
        self.default_reachable_visibility(owner, visibility)
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
