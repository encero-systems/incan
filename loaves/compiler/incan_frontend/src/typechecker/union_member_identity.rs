//! Union member identity: a member that names a model, class, enum or newtype is the declaration its name resolves to
//! where the union is written (#1796).
//!
//! The checker spells a nominal type by its bare name, so `Product | int` written in two modules that each declare
//! `Product` was one union type, and a value of one module's union was accepted where the other's was expected. When
//! more than one module of a check declares a nominal name, a union member of that name is therefore spelled by its
//! declaring module (see [`module_qualified_nominal_name`]): members of two such unions compare by declaration, and the
//! spelling carries that declaration to lowering, which places the member's wrapper payload by it. Every name no two
//! modules of the check share keeps its bare spelling, so no other union changes.
//!
//! The qualification lives only inside unions. A member that becomes a value's own type again, through narrowing, a
//! type pattern or the last member left in an `else`, takes back its bare spelling, which is what the rest of the
//! checker looks nominal types up by.

use std::collections::{BTreeSet, HashMap, HashSet};

use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

use super::TypeChecker;
use crate::ast::{Declaration, Program};
use crate::module::canonicalize_source_module_segments;
use crate::symbols::{
    CallableParam, ResolvedType, SymbolKind, TypeInfo, UNION_TYPE_NAME, module_qualified_nominal_name,
    split_module_qualified_nominal_name, union_ty,
};

/// Which modules of one check declare each nominal type name, and which names the module being collected declares.
#[derive(Debug, Clone, Default)]
pub struct NominalDeclarationContext {
    /// The checked module and every dependency module of the check that declare each nominal name, by module path.
    declaring_modules: HashMap<String, BTreeSet<Vec<String>>>,
    /// Nominal names declared by the module whose declarations are being collected or checked.
    current_module_declared: HashSet<String>,
}

/// Return the nominal type names one module declares: its models, classes, enums and newtypes.
pub fn declared_nominal_names(program: &Program) -> HashSet<String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            Declaration::Model(model) => Some(model.name.clone()),
            Declaration::Class(class) => Some(class.name.clone()),
            Declaration::Enum(enumeration) => Some(enumeration.name.clone()),
            Declaration::Newtype(newtype) => Some(newtype.name.clone()),
            _ => None,
        })
        .collect()
}

/// Return the nominal name and type arguments of a named or applied non-union type.
fn nominal_parts(ty: &ResolvedType) -> Option<(&str, &[ResolvedType])> {
    match ty {
        ResolvedType::Named(name) => Some((name.as_str(), &[])),
        ResolvedType::Generic(name, args) if name != UNION_TYPE_NAME => Some((name.as_str(), args.as_slice())),
        _ => None,
    }
}

/// Return the module path a source declaration identity places its declaration in.
fn identity_module_path(identity: &CanonicalSymbolId) -> Option<Vec<String>> {
    match &identity.origin {
        SymbolOrigin::Module(module_path) | SymbolOrigin::Package { module_path, .. } => {
            Some(canonicalize_source_module_segments(module_path))
        }
        _ => None,
    }
}

impl TypeChecker {
    /// Record which modules of a check declare each nominal name: the checked program and every source dependency.
    ///
    /// Called before any dependency is imported, so every union a dependency's signatures or aliases spell is already
    /// qualified by the time the checked program reads it.
    pub(super) fn record_nominal_declaring_modules(&mut self, program: &Program, dependencies: &[(&str, &Program)]) {
        let mut declaring_modules: HashMap<String, BTreeSet<Vec<String>>> = HashMap::new();
        let root_path = canonicalize_source_module_segments(&self.current_module_path.clone().unwrap_or_default());
        for name in declared_nominal_names(program) {
            declaring_modules.entry(name).or_default().insert(root_path.clone());
        }
        for (module_name, dependency) in dependencies {
            if Self::is_generated_stdlib_dependency_module(module_name) {
                continue;
            }
            let module_path = canonicalize_source_module_segments(
                &self
                    .dependency_module_path_segments
                    .get(*module_name)
                    .cloned()
                    .unwrap_or_else(|| vec![(*module_name).to_string()]),
            );
            for name in declared_nominal_names(dependency) {
                declaring_modules.entry(name).or_default().insert(module_path.clone());
            }
        }
        self.nominal_declarations.declaring_modules = declaring_modules;
    }

    /// Forget the declaring modules of a previous check, for a check that has no dependencies.
    pub(super) fn clear_nominal_declaring_modules(&mut self) {
        self.nominal_declarations.declaring_modules.clear();
    }

    /// Make `program` the module whose own nominal declarations bare union members resolve to, returning the previous
    /// module's names for [`Self::restore_nominal_declaring_module`].
    pub(super) fn enter_nominal_declaring_module(&mut self, program: &Program) -> HashSet<String> {
        std::mem::replace(
            &mut self.nominal_declarations.current_module_declared,
            declared_nominal_names(program),
        )
    }

    /// Restore the nominal declarations of the module collected before a dependency import.
    pub(super) fn restore_nominal_declaring_module(&mut self, previous: HashSet<String>) {
        self.nominal_declarations.current_module_declared = previous;
    }

    /// Return whether more than one module of the current check declares a nominal type of this name.
    fn nominal_name_is_shared(&self, declaration_name: &str) -> bool {
        self.nominal_declarations
            .declaring_modules
            .get(declaration_name)
            .is_some_and(|modules| modules.len() > 1)
    }

    /// Return whether any nominal name is declared by more than one module of the current check.
    fn any_nominal_name_is_shared(&self) -> bool {
        self.nominal_declarations
            .declaring_modules
            .values()
            .any(|modules| modules.len() > 1)
    }

    /// Return the declaring module path and declaration name of the nominal type a bare name resolves to here.
    ///
    /// A name the module being collected declares is its own declaration; any other name is the model, class, enum or
    /// newtype its binding's proven identity names, which for an import under another name (`from first import
    /// Product as FirstProduct`) is the declaration's own name. `None` when the name binds no such declaration.
    fn nominal_declaration_in_scope(&self, name: &str) -> Option<(Vec<String>, String)> {
        if self.nominal_declarations.current_module_declared.contains(name) {
            let module_path =
                canonicalize_source_module_segments(&self.current_module_path.clone().unwrap_or_default());
            return Some((module_path, name.to_string()));
        }
        let id = self.symbols.lookup(name)?;
        let symbol = self.symbols.get(id)?;
        if !matches!(
            symbol.kind,
            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
        ) {
            return None;
        }
        let identity = self.symbols.identity_of(id)?;
        Some((identity_module_path(identity)?, identity.declaration_name.clone()))
    }

    /// Spell a bare nominal name by its declaring module when the current check shares its declaration name.
    ///
    /// Any other name, a name that resolves to no nominal declaration, and an already qualified spelling stay as they
    /// are.
    fn shared_nominal_spelling(&self, name: String) -> String {
        if split_module_qualified_nominal_name(&name).is_some() {
            return name;
        }
        match self.nominal_declaration_in_scope(&name) {
            Some((module_path, declaration_name)) if self.nominal_name_is_shared(&declaration_name) => {
                module_qualified_nominal_name(&module_path, &declaration_name)
            }
            _ => name,
        }
    }

    /// Spell the proven declaration of a module-qualified type reference (`first.Product`) by its declaring module,
    /// when the current check shares its declaration name.
    ///
    /// The qualified spelling survives only inside a union: [`Self::normalize_union_member_identity`] turns it back
    /// into the bare name everywhere else. A facade that re-exports a declaration under another name keeps the
    /// written name, as it always had.
    pub(super) fn qualified_reference_member_spelling(
        &self,
        resolved: ResolvedType,
        identity: &CanonicalSymbolId,
    ) -> ResolvedType {
        let ResolvedType::Named(name) = &resolved else {
            return resolved;
        };
        let is_nominal = matches!(
            identity.kind,
            SemanticSourceTargetKind::Model
                | SemanticSourceTargetKind::Class
                | SemanticSourceTargetKind::Enum
                | SemanticSourceTargetKind::Newtype
        );
        if !is_nominal || identity.declaration_name != *name || !self.nominal_name_is_shared(name) {
            return resolved;
        }
        match identity_module_path(identity) {
            Some(module_path) => ResolvedType::Named(module_qualified_nominal_name(&module_path, name)),
            None => resolved,
        }
    }

    /// Give each nominal inside a union of a resolved annotation the declaration it names where the annotation is
    /// written, and give every nominal outside a union its bare spelling.
    ///
    /// Only a name more than one module of the check declares is qualified, so a check without such a name keeps
    /// every type exactly as the shared resolver produced it.
    pub(super) fn normalize_union_member_identity(&self, ty: ResolvedType) -> ResolvedType {
        if !self.any_nominal_name_is_shared() {
            return ty;
        }
        self.normalize_member_identity_at(ty, false)
    }

    /// Normalize one type position, `in_union` telling whether it lies inside a union member.
    fn normalize_member_identity_at(&self, ty: ResolvedType, in_union: bool) -> ResolvedType {
        let spell = |name: String| {
            if in_union {
                self.shared_nominal_spelling(name)
            } else {
                Self::bare_nominal_spelling(name)
            }
        };
        match ty {
            ResolvedType::Named(name) => ResolvedType::Named(spell(name)),
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => {
                let normalized = union_ty(
                    args.into_iter()
                        .map(|arg| self.normalize_member_identity_at(arg, true))
                        .collect(),
                );
                // `X | None` is `Option[X]`, not a union, so its member keeps its bare spelling.
                if normalized.is_union() || normalized.option_inner_type().is_some_and(ResolvedType::is_union) {
                    normalized
                } else {
                    self.normalize_member_identity_at(normalized, false)
                }
            }
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                spell(name),
                args.into_iter()
                    .map(|arg| self.normalize_member_identity_at(arg, in_union))
                    .collect(),
            ),
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .into_iter()
                    .map(|param| CallableParam {
                        ty: self.normalize_member_identity_at(param.ty, in_union),
                        ..param
                    })
                    .collect(),
                Box::new(self.normalize_member_identity_at(*ret, in_union)),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|item| self.normalize_member_identity_at(item, in_union))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(self.normalize_member_identity_at(*inner, in_union)))
            }
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(self.normalize_member_identity_at(*inner, in_union)))
            }
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(self.normalize_member_identity_at(*key, in_union)),
                Box::new(self.normalize_member_identity_at(*value, in_union)),
            ),
            ResolvedType::TypeToken(inner) => {
                ResolvedType::TypeToken(Box::new(self.normalize_member_identity_at(*inner, in_union)))
            }
            ResolvedType::Ref(inner) => {
                ResolvedType::Ref(Box::new(self.normalize_member_identity_at(*inner, in_union)))
            }
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(self.normalize_member_identity_at(*inner, in_union)))
            }
            other => other,
        }
    }

    /// Return the bare spelling of a possibly module-qualified nominal name.
    fn bare_nominal_spelling(name: String) -> String {
        match split_module_qualified_nominal_name(&name) {
            Some((_, declaration_name)) => declaration_name.to_string(),
            None => name,
        }
    }

    /// Spell a nominal target written in this scope, such as an `isinstance` type or a type pattern, the way a union
    /// member of the same declaration is spelled, so it selects that member and no other of the same name.
    pub(super) fn union_member_target_spelling(&self, ty: ResolvedType) -> ResolvedType {
        if !self.any_nominal_name_is_shared() {
            return ty;
        }
        match ty {
            ResolvedType::Named(name) => ResolvedType::Named(self.shared_nominal_spelling(name)),
            ResolvedType::Generic(name, args) if name != UNION_TYPE_NAME => {
                ResolvedType::Generic(self.shared_nominal_spelling(name), args)
            }
            other => other,
        }
    }

    /// Return the type a value has once narrowing selects one union member for a target written in this scope.
    ///
    /// A member of an unshared name is its own type, as it always was. A member spelled by its declaring module takes
    /// the target's spelling instead, which names the same declaration here (an import alias included) and is what
    /// the checker looks the type's fields and methods up by; a generic member keeps its type arguments.
    pub(super) fn narrowed_union_member_type(member: &ResolvedType, target: &ResolvedType) -> ResolvedType {
        let Some((member_name, member_args)) = nominal_parts(member) else {
            return member.clone();
        };
        if split_module_qualified_nominal_name(member_name).is_none() {
            return member.clone();
        }
        match nominal_parts(target) {
            Some((target_name, _)) if member_args.is_empty() => ResolvedType::Named(target_name.to_string()),
            Some((target_name, _)) => ResolvedType::Generic(target_name.to_string(), member_args.to_vec()),
            None => Self::localize_union_member(member.clone()),
        }
    }

    /// Give a union member that becomes a value's own type again its bare spelling.
    pub(super) fn localize_union_member(ty: ResolvedType) -> ResolvedType {
        match ty {
            ResolvedType::Named(name) => ResolvedType::Named(Self::bare_nominal_spelling(name)),
            ResolvedType::Generic(name, args) if name != UNION_TYPE_NAME => {
                ResolvedType::Generic(Self::bare_nominal_spelling(name), args)
            }
            other => other,
        }
    }

    /// Compare two nominal types when at least one is spelled by its declaring module.
    ///
    /// Two qualified spellings are one type exactly when they name one declaration. A bare spelling carries no
    /// declaration of its own: it is the declaration this scope binds to it, or, for a type that reached this module
    /// through another module's signature, a declaration of that name, so it names a qualified declaration that has
    /// its name or that this scope binds it to. `None` leaves any other pair, and a bare spelling that names neither,
    /// to the ordinary rules.
    pub(super) fn module_qualified_nominals_compatible(
        &self,
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> Option<bool> {
        let (actual_name, actual_args) = nominal_parts(actual)?;
        let (expected_name, expected_args) = nominal_parts(expected)?;
        let same_declaration = match (
            split_module_qualified_nominal_name(actual_name),
            split_module_qualified_nominal_name(expected_name),
        ) {
            (None, None) => return None,
            (Some(actual_declaration), Some(expected_declaration)) => actual_declaration == expected_declaration,
            (Some((module_path, declaration_name)), None) => {
                self.bare_name_names_declaration(expected_name, &module_path, declaration_name)
            }
            (None, Some((module_path, declaration_name))) => {
                self.bare_name_names_declaration(actual_name, &module_path, declaration_name)
            }
        };
        if !same_declaration {
            let both_qualified = split_module_qualified_nominal_name(actual_name).is_some()
                && split_module_qualified_nominal_name(expected_name).is_some();
            return both_qualified.then_some(false);
        }
        if actual_args.is_empty() || expected_args.is_empty() {
            return Some(true);
        }
        Some(
            actual_args.len() == expected_args.len()
                && actual_args
                    .iter()
                    .zip(expected_args)
                    .all(|(actual, expected)| self.types_compatible(actual, expected)),
        )
    }

    /// Return whether a bare nominal spelling names the declaration `declaration_name` of `module_path`.
    fn bare_name_names_declaration(&self, bare: &str, module_path: &[String], declaration_name: &str) -> bool {
        match self.nominal_declaration_in_scope(bare) {
            Some((module, declaration)) => module == module_path && declaration == declaration_name,
            None => bare == declaration_name,
        }
    }

    /// Return the metadata of the declaration a module-qualified union member names.
    ///
    /// That is the declaration this scope binds under the member's own name when it is the same one, and otherwise the
    /// member the declaring dependency module exports.
    pub(super) fn module_qualified_nominal_type_info(&self, name: &str) -> Option<&TypeInfo> {
        let (module_path, declaration_name) = split_module_qualified_nominal_name(name)?;
        if self
            .nominal_declaration_in_scope(declaration_name)
            .is_some_and(|(module, declaration)| module == module_path && declaration == declaration_name)
        {
            let id = self.symbols.lookup(declaration_name)?;
            return match &self.symbols.get(id)?.kind {
                SymbolKind::Type(info) => Some(info),
                _ => None,
            };
        }
        let module_key = self.dependency_direct_member_symbols.keys().find(|key| {
            let path = self
                .dependency_module_path_segments
                .get(*key)
                .cloned()
                .unwrap_or_else(|| vec![(*key).clone()]);
            canonicalize_source_module_segments(&path) == module_path
        })?;
        match self
            .dependency_direct_member_symbols
            .get(module_key)?
            .get(declaration_name)?
        {
            SymbolKind::Type(info) => Some(info),
            _ => None,
        }
    }

    /// Retain, for lowering, the target of each non-generic type alias this module declares or imports.
    pub(super) fn export_type_alias_targets(&mut self) {
        self.type_info.declarations.type_alias_targets = self
            .type_aliases
            .iter()
            .filter(|(_, alias)| alias.type_params.is_empty())
            .map(|(name, alias)| (name.clone(), alias.target.clone()))
            .collect();
    }

    /// Retain, for lowering, the one module of this check that declares each nominal name no other module declares.
    ///
    /// A union that reached this module through another module's signature spells such a member bare, and lowering
    /// places its wrapper payload at this declaring module even where no binding of this module names it.
    pub(super) fn export_unique_nominal_declaring_modules(&mut self) {
        self.type_info.declarations.unique_nominal_declaring_modules = self
            .nominal_declarations
            .declaring_modules
            .iter()
            .filter_map(|(name, modules)| {
                let [module_path] = modules.iter().collect::<Vec<_>>()[..] else {
                    return None;
                };
                (!module_path.is_empty()).then(|| (name.clone(), module_path.clone()))
            })
            .collect();
    }
}
