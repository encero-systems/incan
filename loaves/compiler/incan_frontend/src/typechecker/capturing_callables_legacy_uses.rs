//! The original whole-source use scan, retained only for the cold checker oracle.

use super::{TypeChecker, ValueReads};
use crate::ast::{Declaration, Decorator, Expr, MethodDecl, Program, Spanned};
use crate::ast_walk::any_expr_in_program;
use std::collections::{HashMap, HashSet};

/// Scan source uses with the pre-split traversal and run the same compiler-owned eligibility rules.
fn legacy_value_reads(program: &Program) -> ValueReads {
    let mut callees = HashMap::<String, usize>::new();
    let mut idents = HashMap::<String, usize>::new();
    let mut members = HashSet::new();
    any_expr_in_program(program, |expr| {
        match expr {
            Expr::Call(callee, _, _) => {
                if let Expr::Ident(name) = &callee.node {
                    *callees.entry(name.clone()).or_default() += 1;
                }
            }
            Expr::Ident(name) => *idents.entry(name.clone()).or_default() += 1,
            Expr::Field(_, member) => {
                members.insert(member.clone());
            }
            _ => {}
        }
        false
    });
    let mut names = idents
        .into_iter()
        .filter(|(name, count)| callees.get(name).copied().unwrap_or(0) < *count)
        .map(|(name, _)| name)
        .collect::<HashSet<_>>();
    for decl in &program.declarations {
        let (decorators, methods): (&[Spanned<Decorator>], &[Spanned<MethodDecl>]) = match &decl.node {
            Declaration::Function(function) => (&function.decorators, &[]),
            Declaration::Model(model) => (&model.decorators, &model.methods),
            Declaration::Class(class) => (&class.decorators, &class.methods),
            Declaration::Newtype(newtype) => (&newtype.decorators, &newtype.methods),
            Declaration::Enum(enum_decl) => (&enum_decl.decorators, &enum_decl.methods),
            Declaration::Trait(trait_decl) => (&trait_decl.decorators, &trait_decl.methods),
            _ => continue,
        };
        let method_decorators = methods.iter().flat_map(|method| method.node.decorators.iter());
        names.extend(
            decorators
                .iter()
                .chain(method_decorators)
                .map(|decorator| decorator.node.name.clone()),
        );
    }
    ValueReads { names, members }
}

impl TypeChecker {
    /// Retain the legacy source-use selection only in the test oracle; production always consumes contributions.
    pub(in crate::typechecker) fn legacy_plan_closure_holding_callables(&mut self, program: &Program) {
        self.capturing_callables = super::CapturingCallables::default();
        if !self.checks_standard_library_source() {
            self.plan_closure_holding_callables_from_uses(program, legacy_value_reads(program));
        }
    }
}
