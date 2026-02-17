//! Visitor trait for AST traversal.

use super::*;

// ============================================================================
// Visitor trait for AST traversal
// ============================================================================

pub trait Visitor {
    fn visit_program(&mut self, program: &Program) {
        for decl in &program.declarations {
            self.visit_declaration(decl);
        }
    }

    fn visit_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(i) => self.visit_import(i),
            Declaration::Const(c) => self.visit_const(c),
            Declaration::Model(m) => self.visit_model(m),
            Declaration::Class(c) => self.visit_class(c),
            Declaration::Trait(t) => self.visit_trait(t),
            Declaration::Newtype(n) => self.visit_newtype(n),
            Declaration::Enum(e) => self.visit_enum(e),
            Declaration::Function(f) => self.visit_function(f),
            Declaration::Docstring(d) => self.visit_docstring(d),
        }
    }

    fn visit_import(&mut self, _import: &ImportDecl) {}
    fn visit_const(&mut self, _const_decl: &ConstDecl) {}
    fn visit_docstring(&mut self, _doc: &str) {}
    fn visit_model(&mut self, _model: &ModelDecl) {}
    fn visit_class(&mut self, _class: &ClassDecl) {}
    fn visit_trait(&mut self, _trait: &TraitDecl) {}
    fn visit_newtype(&mut self, _newtype: &NewtypeDecl) {}
    fn visit_enum(&mut self, _enum: &EnumDecl) {}
    fn visit_function(&mut self, _func: &FunctionDecl) {}
    fn visit_statement(&mut self, _stmt: &Spanned<Statement>) {}
    fn visit_expr(&mut self, _expr: &Spanned<Expr>) {}
    fn visit_type(&mut self, _ty: &Spanned<Type>) {}
    fn visit_pattern(&mut self, _pat: &Spanned<Pattern>) {}
}
