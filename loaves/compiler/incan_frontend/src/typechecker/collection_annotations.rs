//! Refusal of builtin collection annotations written without their type arguments (#1717, #1749).
//!
//! `list`, `dict`, `set`, `tuple`, `Option`, `Result`, the frozen collections and `Generator` each name a family of
//! types, one per argument list. Written bare, the shared resolver keeps the word as a nominal type that no backend can
//! spell, so the build stopped on it. The checker refuses the bare word wherever it stands in an annotation, and leaves
//! alone a spelling the program or an import takes for a type of its own.

use std::collections::HashSet;

use super::TypeChecker;
use crate::ast::{Declaration, Spanned, Type};
use crate::diagnostics::errors;
use crate::symbols::TypeInfo;
use crate::typechecker::helpers::collection_type_id;

impl TypeChecker {
    /// Report every bare builtin collection family in the annotation `ty` and return whether there was one.
    ///
    /// The bare word is refused wherever it stands: alone, as a generic argument (`list[Dict]`, `Option[List]`,
    /// `dict[str, Tuple]`), inside a function type, behind a reference or a `mut` parameter marker, or as an element of
    /// a written tuple type. Every occurrence gets its own report, so an annotation that spells two bare words
    /// names both places. The collection pass and the body check can both resolve one annotation, so each
    /// occurrence is reported once, keyed the way unknown annotation names are. The caller yields `Unknown` for the
    /// whole annotation, since the shared resolver would otherwise hand lowering a bare family nominal that no
    /// backend can spell, nested or not.
    pub(super) fn report_bare_builtin_collection_annotations(&mut self, ty: &Spanned<Type>) -> bool {
        match &ty.node {
            Type::Simple(name) => {
                if !self.is_bare_builtin_collection_annotation(name) {
                    return false;
                }
                if self
                    .unknown_source_type_names_emitted
                    .insert((name.clone(), ty.span.start, ty.span.end))
                {
                    self.errors
                        .push(errors::collection_annotation_requires_type_arguments(name, ty.span));
                }
                true
            }
            Type::Generic(_, args) | Type::DottedGeneric(_, args) | Type::Tuple(args) => {
                self.report_bare_builtin_collection_annotations_in(args)
            }
            Type::Function(params, ret) => {
                // Every parameter is walked before the result, so no occurrence is skipped by an earlier find.
                let in_params = self.report_bare_builtin_collection_annotations_in(params);
                self.report_bare_builtin_collection_annotations(ret) || in_params
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.report_bare_builtin_collection_annotations(inner)
            }
            Type::Qualified(_)
            | Type::Dotted(_)
            | Type::ConstrainedPrimitive(_, _)
            | Type::IntLiteral(_)
            | Type::Unit
            | Type::SelfType
            | Type::Infer => false,
        }
    }

    /// Walk every annotation in `types` for [`Self::report_bare_builtin_collection_annotations`], reporting each bare
    /// occurrence, and return whether there was one.
    fn report_bare_builtin_collection_annotations_in(&mut self, types: &[Spanned<Type>]) -> bool {
        let mut found = false;
        for ty in types {
            found |= self.report_bare_builtin_collection_annotations(ty);
        }
        found
    }

    /// Return whether a simple annotation is a builtin collection family written without type arguments.
    ///
    /// Only the prelude builtin counts. A spelling the symbol table binds to anything else (a source type, an import)
    /// is an ordinary nominal and resolves as one, and so is a spelling the program declares as a type of its own,
    /// even where an annotation meets it before the collection pass has registered the declaration.
    fn is_bare_builtin_collection_annotation(&self, name: &str) -> bool {
        collection_type_id(name).is_some()
            && !self.source_type_declaration_names.contains(name)
            && matches!(self.lookup_type_info(name), Some(TypeInfo::Builtin))
    }

    /// Collect the names of the types a program declares itself, including those inside an inline test module.
    ///
    /// Models, classes, traits, enums, newtypes and type aliases name types; functions, bindings and imports do not,
    /// and an import that binds a type spelling is found through the symbol table once it is collected.
    pub(super) fn collect_source_type_declaration_names(declarations: &[Spanned<Declaration>]) -> HashSet<String> {
        let mut names = HashSet::new();
        for declaration in declarations {
            match &declaration.node {
                Declaration::Model(model) => {
                    names.insert(model.name.clone());
                }
                Declaration::Class(class) => {
                    names.insert(class.name.clone());
                }
                Declaration::Trait(trait_decl) => {
                    names.insert(trait_decl.name.clone());
                }
                Declaration::Enum(enum_decl) => {
                    names.insert(enum_decl.name.clone());
                }
                Declaration::Newtype(newtype) => {
                    names.insert(newtype.name.clone());
                }
                Declaration::TypeAlias(alias) => {
                    names.insert(alias.name.clone());
                }
                Declaration::TestModule(module) => {
                    names.extend(Self::collect_source_type_declaration_names(&module.body));
                }
                Declaration::Import(_)
                | Declaration::Const(_)
                | Declaration::Static(_)
                | Declaration::Capability(_)
                | Declaration::Alias(_)
                | Declaration::Partial(_)
                | Declaration::Function(_)
                | Declaration::VocabBlock(_)
                | Declaration::Docstring(_) => {}
            }
        }
        names
    }
}
