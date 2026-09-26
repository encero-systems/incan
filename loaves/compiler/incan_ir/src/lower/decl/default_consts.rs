//! Module consts that parameter defaults name.
//!
//! A parameter default is evaluated as if in the module that declares the callable, but a caller that omits the
//! argument receives the default at its own call site, in another module or another package. A default that names a
//! const of the declaring module therefore reaches the caller as a path to that const. A private const is still a valid
//! default for a public callable, so the generated item such a path names is published beyond its module: its Incan
//! visibility is unchanged, and only the generated path a default takes becomes reachable.

use std::collections::{HashMap, HashSet};

use super::super::AstLowering;
use incan_frontend::ast;

impl AstLowering {
    /// Record the module consts that any parameter default of this module names.
    ///
    /// Every callable a module declares counts: functions, and the methods of its models, classes, traits, newtypes
    /// and enums. A name only counts when the module declares a const by that name.
    pub(in crate::lower) fn collect_default_named_consts(&mut self, program: &ast::Program) {
        let consts = program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                ast::Declaration::Const(konst) => Some(konst.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut reads = HashMap::new();
        for declaration in &program.declarations {
            match &declaration.node {
                ast::Declaration::Function(function) => self.count_default_reads(&function.params, &mut reads),
                ast::Declaration::Model(model) => self.count_method_default_reads(&model.methods, &mut reads),
                ast::Declaration::Class(class) => self.count_method_default_reads(&class.methods, &mut reads),
                ast::Declaration::Trait(trait_decl) => self.count_method_default_reads(&trait_decl.methods, &mut reads),
                ast::Declaration::Newtype(newtype) => self.count_method_default_reads(&newtype.methods, &mut reads),
                ast::Declaration::Enum(enum_decl) => self.count_method_default_reads(&enum_decl.methods, &mut reads),
                _ => {}
            }
        }
        self.default_named_consts = reads
            .into_keys()
            .filter(|name| consts.contains(name.as_str()))
            .collect();
    }

    /// Return whether a parameter default of this module names the const.
    pub(in crate::lower) fn const_is_named_by_a_default(&self, name: &str) -> bool {
        self.default_named_consts.contains(name)
    }

    /// Count the names every parameter default of one method list reads.
    fn count_method_default_reads(
        &self,
        methods: &[ast::Spanned<ast::MethodDecl>],
        reads: &mut HashMap<String, usize>,
    ) {
        for method in methods {
            self.count_default_reads(&method.node.params, reads);
        }
    }

    /// Count the names every default of one parameter list reads.
    fn count_default_reads(&self, params: &[ast::Spanned<ast::Param>], reads: &mut HashMap<String, usize>) {
        for default in params.iter().filter_map(|param| param.node.default.as_ref()) {
            self.count_expr_ident_reads(&default.node, reads);
        }
    }
}
