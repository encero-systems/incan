//! Union member identity for nominal types two modules of one crate declare under the same name (#1796).
//!
//! An anonymous union's generated Rust wrapper is named from its members' spellings, and every module of a crate shares
//! the crate-root wrappers. Two modules that each declare `Product` and write `Product | int` therefore spelled one
//! union, got one wrapper whose `Product` payload could name neither declaration, and a library publication refused
//! to bind that wrapper's `Product` to two declarations. The spelling alone cannot tell the two apart; the declaring
//! module can.
//!
//! Lowering therefore spells a union member nominal whose name is shared by several modules of the crate by the Rust
//! path of its declaring module (`crate::first::Product`), which names the wrapper and places its payload. A member
//! nobody else declares keeps its spelling, so every other wrapper name is unchanged. The frozen emitter and the
//! publication read the qualified spelling through `crate::types` helpers that relate it to the module-local spelling
//! values and published members use.
//!
//! A member's declaration is the one its spelling resolves to where the union is written: this module's own
//! declaration, the declaration an import resolves to (under the declaration's own name, also for an import under
//! another name), the declaration a module-qualified reference (`first.Product`) resolves to, the declaration the
//! checker named by module for a union that reached this module through another module's signature, or else the one
//! module of this module's check that declares that name. Every spelling of one declaration therefore names one
//! wrapper, and each wrapper's payload resolves at the crate root.

use std::collections::{HashMap, HashSet};

use incan_frontend::ast;
use incan_frontend::module::canonicalize_source_module_segments;
use incan_frontend::symbols::split_module_qualified_nominal_name;
use incan_lang::lang::stdlib::INCAN_STD_NAMESPACE;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

use super::super::types::{IR_UNION_TYPE_NAME, IrType};
use super::AstLowering;
use super::types::union_ir_type;

/// Crate-wide facts that decide how a union member names a nominal type (#1796).
///
/// The code generator computes these once per crate from every module it emits, and every module's lowering reads the
/// same facts, so each module spells a shared union member the same way.
#[derive(Debug, Clone, Default)]
pub struct CrateNominalContext {
    /// Type names (`model`, `class`, `enum`, `newtype` or `type` alias) that more than one module of the crate
    /// declares.
    ///
    /// The emitter places a type the crate root names by its spelling, which such a name cannot select: a union member
    /// of it is spelled by its declaring module, and an imported alias of it is lowered to its target.
    pub shared_spellings: HashSet<String>,
    /// The Rust module path each module of the crate is emitted at, keyed by its logical module path. The crate-root
    /// program is emitted at the empty path.
    pub module_rust_paths: HashMap<Vec<String>, Vec<String>>,
}

impl CrateNominalContext {
    /// Collect the context from each module's logical path, emitted Rust path and declarations.
    ///
    /// A module appears once per logical path it can be named by; the Rust path it maps to is where its declarations
    /// live in the generated crate.
    pub fn from_modules<'a>(
        modules: impl IntoIterator<Item = (Vec<Vec<String>>, Vec<String>, &'a ast::Program)>,
    ) -> Self {
        let mut declaring_modules: HashMap<String, HashSet<Vec<String>>> = HashMap::new();
        let mut module_rust_paths = HashMap::new();
        for (logical_paths, rust_path, program) in modules {
            for logical in logical_paths {
                module_rust_paths.insert(logical, rust_path.clone());
            }
            for name in declared_type_names(program) {
                declaring_modules.entry(name).or_default().insert(rust_path.clone());
            }
        }
        let shared_spellings = declaring_modules
            .into_iter()
            .filter(|(_, modules)| modules.len() > 1)
            .map(|(name, _)| name)
            .collect();
        Self {
            shared_spellings,
            module_rust_paths,
        }
    }
}

/// Return the type names one module declares: its nominal types and its type aliases.
fn declared_type_names(program: &ast::Program) -> HashSet<String> {
    let mut names = declared_nominal_names(program);
    names.extend(
        program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                ast::Declaration::TypeAlias(alias) => Some(alias.name.clone()),
                _ => None,
            }),
    );
    names
}

/// Return whether a checked identity names a model, class, enum or newtype.
fn is_nominal_identity(identity: &CanonicalSymbolId) -> bool {
    matches!(
        identity.kind,
        SemanticSourceTargetKind::Model
            | SemanticSourceTargetKind::Class
            | SemanticSourceTargetKind::Enum
            | SemanticSourceTargetKind::Newtype
    )
}

/// Return whether an IR type holds an anonymous union anywhere.
pub(in crate::lower) fn ir_type_contains_union(ty: &IrType) -> bool {
    match ty {
        _ if ty.is_union() => true,
        IrType::List(inner)
        | IrType::Set(inner)
        | IrType::Option(inner)
        | IrType::Ref(inner)
        | IrType::RefMut(inner)
        | IrType::TypeToken(inner) => ir_type_contains_union(inner),
        IrType::Dict(key, value) | IrType::Result(key, value) => {
            ir_type_contains_union(key) || ir_type_contains_union(value)
        }
        IrType::Tuple(items) | IrType::NamedGeneric(_, items) => items.iter().any(ir_type_contains_union),
        IrType::Function { params, ret } => params.iter().any(ir_type_contains_union) || ir_type_contains_union(ret),
        _ => false,
    }
}

/// Return the nominal type names one module declares: its models, classes, enums and newtypes.
pub fn declared_nominal_names(program: &ast::Program) -> HashSet<String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            ast::Declaration::Model(model) => Some(model.name.clone()),
            ast::Declaration::Class(class) => Some(class.name.clone()),
            ast::Declaration::Enum(enumeration) => Some(enumeration.name.clone()),
            ast::Declaration::Newtype(newtype) => Some(newtype.name.clone()),
            _ => None,
        })
        .collect()
}

impl AstLowering {
    /// Build the IR union for lowered members, spelling a member nominal the crate declares more than once by its
    /// declaring module.
    ///
    /// The union is first normalized from the members as written, so only a real union (two or more members, possibly
    /// under `Option`) is qualified: an `X | None` that normalizes to `Option[X]` keeps its member's spelling.
    pub(in crate::lower) fn lower_union_members(&self, members: Vec<IrType>) -> IrType {
        self.qualify_shared_union_members(union_ir_type(members))
    }

    /// Re-spell the members of a normalized union by the declarations they name.
    fn qualify_shared_union_members(&self, ty: IrType) -> IrType {
        let Some(context) = self.crate_nominal_context.as_deref() else {
            return ty;
        };
        match ty {
            IrType::NamedGeneric(name, members) if name == IR_UNION_TYPE_NAME => union_ir_type(
                members
                    .into_iter()
                    .map(|member| self.crate_qualified_nominals(member, context))
                    .collect(),
            ),
            IrType::Option(inner) if inner.is_union() => {
                IrType::Option(Box::new(self.qualify_shared_union_members(*inner)))
            }
            other => other,
        }
    }

    /// Spell every nominal inside one union member by the declaration it names, leaving all other types as they are.
    ///
    /// A generic nominal's base is spelled the same way as a plain one, and its arguments in turn.
    fn crate_qualified_nominals(&self, ty: IrType, context: &CrateNominalContext) -> IrType {
        let qualify = |ty: IrType| self.crate_qualified_nominals(ty, context);
        match ty {
            IrType::Struct(name) => IrType::Struct(self.union_member_spelling(name, context)),
            IrType::Enum(name) => IrType::Enum(self.union_member_spelling(name, context)),
            IrType::NamedGeneric(name, args) if name == IR_UNION_TYPE_NAME => {
                self.qualify_shared_union_members(IrType::NamedGeneric(name, args))
            }
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                self.union_member_spelling(name, context),
                args.into_iter().map(qualify).collect(),
            ),
            IrType::List(inner) => IrType::List(Box::new(qualify(*inner))),
            IrType::Set(inner) => IrType::Set(Box::new(qualify(*inner))),
            IrType::Option(inner) => IrType::Option(Box::new(qualify(*inner))),
            IrType::Ref(inner) => IrType::Ref(Box::new(qualify(*inner))),
            IrType::RefMut(inner) => IrType::RefMut(Box::new(qualify(*inner))),
            IrType::TypeToken(inner) => IrType::TypeToken(Box::new(qualify(*inner))),
            IrType::Dict(key, value) => IrType::Dict(Box::new(qualify(*key)), Box::new(qualify(*value))),
            IrType::Result(ok, err) => IrType::Result(Box::new(qualify(*ok)), Box::new(qualify(*err))),
            IrType::Tuple(items) => IrType::Tuple(items.into_iter().map(qualify).collect()),
            IrType::Function { params, ret } => IrType::Function {
                params: params.into_iter().map(qualify).collect(),
                ret: Box::new(qualify(*ret)),
            },
            other => other,
        }
    }

    /// Spell one union member nominal by the declaration it names.
    ///
    /// A declaration whose name another module of the crate also declares is spelled by its declaring module's Rust
    /// path; any other declaration by its own name, which the crate root places by that name. A name that names no
    /// declaration of this crate keeps its spelling.
    fn union_member_spelling(&self, name: String, context: &CrateNominalContext) -> String {
        let Some((rust_module_path, declaration_name)) = self.union_member_declaration(&name, context) else {
            return name;
        };
        if context.shared_spellings.contains(&declaration_name) {
            Self::crate_item_path(&rust_module_path, &declaration_name)
        } else {
            declaration_name
        }
    }

    /// Return the Rust module path and declaration name of the crate declaration a union member spelling names here.
    ///
    /// A crate path (a module-qualified reference such as `first.Product`, or a member already spelled by its module)
    /// names the declaration at that path. A bare name is this module's own declaration, the model, class, enum or
    /// newtype an import resolved to (under its own name, also for an import under another name), or, when this module
    /// binds no such name, the one module of this module's check that declares it: a union that reached this module
    /// through another module's signature names that module's declaration.
    fn union_member_declaration(&self, name: &str, context: &CrateNominalContext) -> Option<(Vec<String>, String)> {
        // ---- A crate path ----
        if let Some(path) = name.strip_prefix("crate::") {
            let mut segments = path.split("::").map(str::to_string).collect::<Vec<_>>();
            let declaration_name = segments.pop()?;
            let is_crate_module = segments.first().map(String::as_str) != Some(INCAN_STD_NAMESPACE)
                && context
                    .module_rust_paths
                    .values()
                    .any(|rust_path| *rust_path == segments);
            return is_crate_module.then_some((segments, declaration_name));
        }
        if name.contains("::") {
            return None;
        }

        // ---- This module's own declaration ----
        if self.module_declared_nominals.contains(name) {
            return Some((self.crate_module_rust_path(&[], context)?, name.to_string()));
        }

        // ---- An import ----
        let info = self.type_info.as_ref()?;
        if let Some(identity) = info.resolved_import_identity(name) {
            if !is_nominal_identity(identity) {
                return None;
            }
            let rust_module_path = self.crate_declaration_rust_path(identity, context)?;
            return Some((rust_module_path, identity.declaration_name.clone()));
        }

        // ---- A name this module does not bind ----
        if self.import_aliases.contains_key(name)
            || self.rust_import_aliases.contains_key(name)
            || self.source_type_alias_targets.contains_key(name)
        {
            return None;
        }
        let logical_path = info.unique_nominal_declaring_module(name)?;
        let rust_module_path = self.crate_module_rust_path(logical_path, context)?;
        Some((rust_module_path, name.to_string()))
    }

    /// Return the Rust module path of the crate module a checked declaration identity places its declaration in.
    ///
    /// A package build gives its own modules' declarations a package origin; another package's declaration is not a
    /// module of this crate.
    fn crate_declaration_rust_path(
        &self,
        identity: &CanonicalSymbolId,
        context: &CrateNominalContext,
    ) -> Option<Vec<String>> {
        let logical_path = match &identity.origin {
            SymbolOrigin::Module(logical_path) => logical_path,
            SymbolOrigin::Package { library, module_path }
                if Some(library.as_str()) == self.produced_library_identity() =>
            {
                module_path
            }
            _ => return None,
        };
        context
            .module_rust_paths
            .get(&canonicalize_source_module_segments(logical_path))
            .cloned()
    }

    /// Return the IR spelling of a nominal the checker spelled by its declaring module, or `None` for any other name.
    ///
    /// The checker spells a union member by its declaring module when several modules of one check declare its name
    /// (`incan_frontend::symbols::module_qualified_nominal_name`), which is the fact that tells two modules'
    /// `Product` apart where no binding of the lowered module names either. The member is spelled by the Rust path of
    /// that module when the crate declares the name more than once, and by the declaration's own name otherwise, the
    /// way [`Self::lower_union_members`] spells any other member of that declaration.
    pub(in crate::lower) fn lower_module_qualified_nominal(&self, name: &str) -> Option<String> {
        let (module_path, declaration_name) = split_module_qualified_nominal_name(name)?;
        let crate_path = self
            .crate_nominal_context
            .as_deref()
            .filter(|context| context.shared_spellings.contains(declaration_name))
            .and_then(|context| self.crate_module_rust_path(&module_path, context));
        Some(match crate_path {
            Some(rust_path) => Self::crate_item_path(&rust_path, declaration_name),
            None => declaration_name.to_string(),
        })
    }

    /// Return the Rust module path of a module of this crate by its logical path; an empty logical path names the
    /// module being lowered, which the checker spells that way when it checks the module without a module path.
    fn crate_module_rust_path(&self, logical_path: &[String], context: &CrateNominalContext) -> Option<Vec<String>> {
        if !logical_path.is_empty() {
            return context
                .module_rust_paths
                .get(&canonicalize_source_module_segments(logical_path))
                .cloned();
        }
        let Some(module) = self.current_source_module_name.as_deref() else {
            return Some(Vec::new());
        };
        let logical = canonicalize_source_module_segments(&module.split('.').map(str::to_string).collect::<Vec<_>>());
        context.module_rust_paths.get(&logical).cloned()
    }

    /// Spell an item of this crate by its Rust module path.
    fn crate_item_path(rust_module_path: &[String], name: &str) -> String {
        std::iter::once("crate")
            .chain(rust_module_path.iter().map(String::as_str))
            .chain(std::iter::once(name))
            .collect::<Vec<_>>()
            .join("::")
    }

    /// Collect the import aliases this module writes as union members, each mapped to its declaration's crate path.
    ///
    /// `from first import Product as FirstProduct` binds a name the crate root cannot resolve, so a union written with
    /// `FirstProduct` spells that member by the declaration (`Product`) instead. The values that inhabit the member
    /// are typed by the alias, so this module types every use of such an alias by the declaration's crate path, which
    /// the member matches by its declaration name and which Rust resolves in any module. Only aliases this module
    /// writes inside a union annotation are collected, so every other module keeps its alias spellings.
    pub(in crate::lower) fn collect_union_member_import_aliases(
        &self,
        program: &ast::Program,
    ) -> HashMap<String, String> {
        let Some(context) = self.crate_nominal_context.as_deref() else {
            return HashMap::new();
        };
        let mut written = HashSet::new();
        for declaration in &program.declarations {
            match &declaration.node {
                ast::Declaration::Function(function) => {
                    Self::collect_signature_union_names(&function.params, &function.return_type, &mut written);
                }
                ast::Declaration::Model(model) => {
                    for field in &model.fields {
                        collect_union_names(&field.node.ty.node, false, &mut written);
                    }
                    for method in &model.methods {
                        Self::collect_signature_union_names(
                            &method.node.params,
                            &method.node.return_type,
                            &mut written,
                        );
                    }
                }
                ast::Declaration::Class(class) => {
                    for field in &class.fields {
                        collect_union_names(&field.node.ty.node, false, &mut written);
                    }
                    for method in &class.methods {
                        Self::collect_signature_union_names(
                            &method.node.params,
                            &method.node.return_type,
                            &mut written,
                        );
                    }
                }
                ast::Declaration::TypeAlias(alias) => collect_union_names(&alias.target.node, false, &mut written),
                _ => {}
            }
        }
        let Some(info) = self.type_info.as_ref() else {
            return HashMap::new();
        };
        written
            .into_iter()
            .filter_map(|name| {
                let identity = info.resolved_import_identity(&name)?;
                if !is_nominal_identity(identity) || identity.declaration_name == name {
                    return None;
                }
                let rust_module_path = self.crate_declaration_rust_path(identity, context)?;
                let canonical = Self::crate_item_path(&rust_module_path, &identity.declaration_name);
                Some((name, canonical))
            })
            .collect()
    }

    /// Collect the simple names written inside unions of one callable signature.
    fn collect_signature_union_names(
        params: &[ast::Spanned<ast::Param>],
        return_type: &ast::Spanned<ast::Type>,
        written: &mut HashSet<String>,
    ) {
        for param in params {
            collect_union_names(&param.node.ty.node, false, written);
        }
        collect_union_names(&return_type.node, false, written);
    }

    /// Return the declaration crate path a union-member import alias of this module is typed by, if it is one.
    pub(in crate::lower) fn union_member_import_alias(&self, name: &str) -> Option<&str> {
        self.union_member_import_aliases.get(name).map(String::as_str)
    }

    /// Lower an imported source type alias whose target holds a union and whose name another module of the crate also
    /// declares, to that target.
    ///
    /// The emitter resolves an alias it places by its name, which such an alias's name cannot select, so a value
    /// returned or passed into it was never wrapped into the union (#1796). The target is the same Rust type as the
    /// alias; an alias whose name is unique keeps its name.
    pub(in crate::lower) fn lower_shared_imported_type_alias(&self, name: &str) -> Option<IrType> {
        let context = self.crate_nominal_context.as_deref()?;
        if !context.shared_spellings.contains(name) {
            return None;
        }
        let info = self.type_info.as_ref()?;
        let identity = info.resolved_import_identity(name)?;
        if !matches!(identity.kind, SemanticSourceTargetKind::TypeAlias) {
            return None;
        }
        self.crate_declaration_rust_path(identity, context)?;
        let target = self.lower_resolved_type(info.type_alias_target(name)?);
        ir_type_contains_union(&target).then_some(target)
    }
}

/// Collect the simple names written inside unions of one type annotation.
fn collect_union_names(ty: &ast::Type, in_union: bool, written: &mut HashSet<String>) {
    match ty {
        ast::Type::Simple(name) => {
            if in_union {
                written.insert(name.clone());
            }
        }
        ast::Type::Generic(base, args) => {
            let in_union = in_union || base == IR_UNION_TYPE_NAME;
            if in_union && base != IR_UNION_TYPE_NAME {
                written.insert(base.clone());
            }
            for arg in args {
                collect_union_names(&arg.node, in_union, written);
            }
        }
        ast::Type::DottedGeneric(_, args) | ast::Type::Tuple(args) => {
            for arg in args {
                collect_union_names(&arg.node, in_union, written);
            }
        }
        ast::Type::Function(params, ret) => {
            for param in params {
                collect_union_names(&param.node, in_union, written);
            }
            collect_union_names(&ret.node, in_union, written);
        }
        ast::Type::Ref(inner) | ast::Type::RefMut(inner) | ast::Type::MutParam(inner) => {
            collect_union_names(&inner.node, in_union, written);
        }
        ast::Type::Qualified(_)
        | ast::Type::Dotted(_)
        | ast::Type::ConstrainedPrimitive(..)
        | ast::Type::IntLiteral(_)
        | ast::Type::Unit
        | ast::Type::SelfType
        | ast::Type::Infer => {}
    }
}
