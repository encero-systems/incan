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

use std::collections::{HashMap, HashSet};

use incan_frontend::ast;
use incan_frontend::module::canonicalize_source_module_segments;
use incan_semantics_core::SymbolOrigin;

use super::super::types::{IR_UNION_TYPE_NAME, IrType};
use super::AstLowering;
use super::types::union_ir_type;

/// Crate-wide facts that decide how a union member names a nominal type (#1796).
///
/// The code generator computes these once per crate from every module it emits, and every module's lowering reads the
/// same facts, so each module spells a shared union member the same way.
#[derive(Debug, Clone, Default)]
pub struct CrateNominalContext {
    /// Nominal type names (`model`, `class`, `enum`, `newtype`) that more than one module of the crate declares.
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
            for name in declared_nominal_names(program) {
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

    /// Re-spell the members of a normalized union whose names another module of the crate also declares.
    fn qualify_shared_union_members(&self, ty: IrType) -> IrType {
        let Some(context) = self
            .crate_nominal_context
            .as_deref()
            .filter(|context| !context.shared_spellings.is_empty())
        else {
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

    /// Spell every shared nominal inside one union member by its declaring module, leaving all other types as they are.
    fn crate_qualified_nominals(&self, ty: IrType, context: &CrateNominalContext) -> IrType {
        let qualify = |ty: IrType| self.crate_qualified_nominals(ty, context);
        match ty {
            IrType::Struct(name) => IrType::Struct(self.crate_qualified_nominal(name, context)),
            IrType::Enum(name) => IrType::Enum(self.crate_qualified_nominal(name, context)),
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                self.crate_qualified_nominal(name, context),
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

    /// Spell one nominal name by its declaring module's Rust path when the crate declares that name more than once.
    ///
    /// The declaring module is this module for a name it declares, and the module the checker resolved the import to
    /// for a name it imports under that same name. A name this module neither declares nor imports, or imports under
    /// another local name, keeps its spelling.
    fn crate_qualified_nominal(&self, name: String, context: &CrateNominalContext) -> String {
        if !context.shared_spellings.contains(&name) {
            return name;
        }
        let Some(module_path) = self.declaring_module_rust_path(&name, context) else {
            return name;
        };
        std::iter::once("crate")
            .chain(module_path.iter().map(String::as_str))
            .chain(std::iter::once(name.as_str()))
            .collect::<Vec<_>>()
            .join("::")
    }

    /// Return the Rust module path of the module that declares `name` as this module sees it.
    fn declaring_module_rust_path(&self, name: &str, context: &CrateNominalContext) -> Option<Vec<String>> {
        if self.module_declared_nominals.contains(name) {
            let Some(module) = self.current_source_module_name.as_deref() else {
                return Some(Vec::new());
            };
            let logical =
                canonicalize_source_module_segments(&module.split('.').map(str::to_string).collect::<Vec<_>>());
            return context.module_rust_paths.get(&logical).cloned();
        }
        let identity = self.type_info.as_ref()?.resolved_import_identity(name)?;
        if identity.declaration_name != name {
            return None;
        }
        let SymbolOrigin::Module(logical) = &identity.origin else {
            return None;
        };
        context
            .module_rust_paths
            .get(&canonicalize_source_module_segments(logical))
            .cloned()
    }
}
