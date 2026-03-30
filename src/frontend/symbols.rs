//! Symbol table and scope management for Incan
//!
//! Tracks all named entities (types, functions, variables, traits) and their scopes.

use std::collections::HashMap;

use crate::frontend::ast::{Receiver, Span, Type};
use incan_core::interop::RustItemMetadata;
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::conventions;
use incan_core::lang::surface::constructors;
use incan_core::lang::surface::types as surface_types;
use incan_core::lang::traits;
use incan_core::lang::types::collections;
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::numerics;
use incan_core::lang::types::numerics::NumericTypeId;
use incan_core::lang::types::stringlike;
use incan_core::lang::types::stringlike::StringLikeId;

/// Unique identifier for symbols
pub type SymbolId = usize;

/// Symbol table managing all named entities
#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
    scopes: Vec<Scope>,
    current_scope: usize,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut table = Self {
            symbols: Vec::new(),
            scopes: vec![Scope::new(None, ScopeKind::Module)],
            current_scope: 0,
        };

        // Add builtin types
        table.add_builtins();
        table
    }

    fn add_builtins(&mut self) {
        // Builtin types (from the canonical `incan_core::lang::types` registries).
        //
        // We define both canonical spellings and aliases so name lookup stays robust and we avoid
        // drift between the compiler and the language vocabulary registries.
        let mut builtin_types: Vec<&'static str> = Vec::new();
        for t in numerics::NUMERIC_TYPES {
            builtin_types.push(t.canonical);
            builtin_types.extend_from_slice(t.aliases);
        }
        for t in stringlike::STRING_LIKE_TYPES {
            builtin_types.push(t.canonical);
            builtin_types.extend_from_slice(t.aliases);
        }
        for t in collections::COLLECTION_TYPES {
            builtin_types.push(t.canonical);
            builtin_types.extend_from_slice(t.aliases);
        }
        for t in surface_types::SURFACE_TYPES {
            // RFC 022: stdlib-scoped types must be explicitly imported (e.g. `from std.web import App`).
            // Only truly global surface types (Rust interop helpers) are injected here.
            if surface_types::is_global(t.item.id) {
                builtin_types.push(t.item.canonical);
                builtin_types.extend_from_slice(t.item.aliases);
            }
        }
        // Unit-ish types that are not yet modeled in `incan_core::lang::types`.
        builtin_types.push(conventions::UNIT_TYPE_NAME);
        builtin_types.push(conventions::NONE_TYPE_NAME);

        // Deduplicate to avoid defining the same builtin twice.
        let mut seen: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
        for name in builtin_types.into_iter().filter(|n| seen.insert(*n)) {
            self.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
        }

        // Builtin traits
        for info in traits::TRAITS {
            self.define(Symbol {
                name: info.canonical.to_string(),
                kind: SymbolKind::Trait(TraitInfo {
                    type_params: vec![],
                    methods: HashMap::new(),
                    requires: vec![],
                    supertraits: vec![],
                }),
                span: Span::default(),
                scope: 0,
            });
        }

        // Builtin variants for Result and Option
        // Ok(T) and Err(E) for Result
        self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Ok).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                enum_name: collections::as_str(CollectionTypeId::Result).to_string(),
                fields: vec![ResolvedType::TypeVar("T".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Err).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                enum_name: collections::as_str(CollectionTypeId::Result).to_string(),
                fields: vec![ResolvedType::TypeVar("E".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        // Some(T) and None for Option
        self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Some).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                enum_name: collections::as_str(CollectionTypeId::Option).to_string(),
                fields: vec![ResolvedType::TypeVar("T".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::None).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                enum_name: collections::as_str(CollectionTypeId::Option).to_string(),
                fields: vec![],
            }),
            span: Span::default(),
            scope: 0,
        });

        // Builtin functions
        for name in std::iter::once(builtins::as_str(BuiltinFnId::Print))
            .chain(builtins::aliases(BuiltinFnId::Print).iter().copied())
        {
            self.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Function(FunctionInfo {
                    params: vec![("msg".to_string(), ResolvedType::Str)],
                    return_type: ResolvedType::Unit,
                    is_async: false,
                    type_params: vec![],
                    type_param_bounds: HashMap::new(),
                }),
                span: Span::default(),
                scope: 0,
            });
        }
        self.define(Symbol {
            name: builtins::as_str(BuiltinFnId::Len).to_string(),
            kind: SymbolKind::Function(FunctionInfo {
                params: vec![("collection".to_string(), ResolvedType::Unknown)],
                return_type: ResolvedType::Int,
                is_async: false,
                type_params: vec![],
                type_param_bounds: HashMap::new(),
            }),
            span: Span::default(),
            scope: 0,
        });
        // range() builtin - returns an iterator
        self.define(Symbol {
            name: builtins::as_str(BuiltinFnId::Range).to_string(),
            kind: SymbolKind::Function(FunctionInfo {
                params: vec![("n".to_string(), ResolvedType::Int)],
                return_type: ResolvedType::Named("Range".to_string()), // Iterator-like
                is_async: false,
                type_params: vec![],
                type_param_bounds: HashMap::new(),
            }),
            span: Span::default(),
            scope: 0,
        });
    }

    /// Enter a new scope
    pub fn enter_scope(&mut self, kind: ScopeKind) {
        let new_scope = Scope::new(Some(self.current_scope), kind);
        self.scopes.push(new_scope);
        self.current_scope = self.scopes.len() - 1;
    }

    /// Exit the current scope
    pub fn exit_scope(&mut self) {
        if let Some(parent) = self.scopes[self.current_scope].parent {
            self.current_scope = parent;
        }
    }

    /// Define a new symbol in the current scope
    pub fn define(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.scope = self.current_scope;
        let id = self.symbols.len();
        self.scopes[self.current_scope].symbols.insert(symbol.name.clone(), id);
        self.symbols.push(symbol);
        id
    }

    /// Look up a symbol by name in the current scope chain
    pub fn lookup(&self, name: &str) -> Option<SymbolId> {
        let mut scope_idx = self.current_scope;
        loop {
            if let Some(&id) = self.scopes[scope_idx].symbols.get(name) {
                return Some(id);
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        None
    }

    /// Look up a symbol only in the current scope (no parent lookup)
    pub fn lookup_local(&self, name: &str) -> Option<SymbolId> {
        self.scopes[self.current_scope].symbols.get(name).copied()
    }

    /// Get a symbol by ID
    pub fn get(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id)
    }

    /// Get a mutable symbol by ID
    pub fn get_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.get_mut(id)
    }

    /// All symbols in definition order (builtins first, then user declarations).
    ///
    /// Used for whole-program analyses such as supertrait graphs.
    pub(crate) fn all_symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// Get the current scope kind
    pub fn current_scope_kind(&self) -> ScopeKind {
        self.scopes[self.current_scope].kind
    }

    /// Check if we're inside a function/method
    pub fn in_function(&self) -> bool {
        let mut scope_idx = self.current_scope;
        loop {
            match self.scopes[scope_idx].kind {
                ScopeKind::Function | ScopeKind::Method { .. } => return true,
                _ => {}
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        false
    }

    /// Get the current function's return type (if in a function)
    pub fn current_return_type(&self) -> Option<&ResolvedType> {
        let mut scope_idx = self.current_scope;
        loop {
            match &self.scopes[scope_idx].kind {
                ScopeKind::Function | ScopeKind::Method { .. } => {
                    return self.scopes[scope_idx].return_type.as_ref();
                }
                _ => {}
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        None
    }

    /// Set the return type for the current function scope
    pub fn set_return_type(&mut self, ty: ResolvedType) {
        self.scopes[self.current_scope].return_type = Some(ty);
    }
}

/// A scope containing symbol definitions
#[derive(Debug)]
pub struct Scope {
    pub parent: Option<usize>,
    pub kind: ScopeKind,
    pub symbols: HashMap<String, SymbolId>,
    pub return_type: Option<ResolvedType>,
}

impl Scope {
    pub fn new(parent: Option<usize>, kind: ScopeKind) -> Self {
        Self {
            parent,
            kind,
            symbols: HashMap::new(),
            return_type: None,
        }
    }
}

/// Kind of scope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Module,
    Function,
    Method { receiver: Option<Receiver> },
    Class,
    Model,
    Trait,
    Block,
}

/// A symbol in the symbol table
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub scope: usize,
}

/// How a `rust::...` import binding relates to Rust’s module/type namespace (RFC 041).
///
/// Incan does not run the Rust type checker here; this classification is derived from import syntax only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustImportBindingKind {
    /// `import rust::crate_name` — binds the crate root as a namespace (not a concrete type).
    CrateRoot,
    /// `import rust::crate_name::a::b::...` with at least one path segment after the crate name.
    RootedPath,
    /// `from rust::... import item` — binds a single imported Rust item.
    FromImport,
}

/// Provenance for a symbol that refers into a Rust dependency via `rust::` (RFC 041).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustItemInfo {
    /// Crate name (first segment after `rust::` in the import source).
    pub crate_name: String,
    /// Canonical path used for diagnostics and future lowering: `crate::module::Item` (same string the import
    /// collector already built, joined with `::`).
    pub path: String,
    pub binding: RustImportBindingKind,
    /// Optional extracted Rust semantic metadata (RFC 041).
    pub metadata: Option<RustItemMetadata>,
}

/// Kind of symbol
#[derive(Debug, Clone)]
pub enum SymbolKind {
    /// Variable/binding
    Variable(VariableInfo),
    /// Function
    Function(FunctionInfo),
    /// Type (class, model, newtype, enum, builtin)
    Type(TypeInfo),
    /// Trait
    Trait(TraitInfo),
    /// Module/import
    Module(ModuleInfo),
    /// Enum variant
    Variant(VariantInfo),
    /// Field
    Field(FieldInfo),
    /// Rust dependency import (`import rust::...` / `from rust::... import ...`, RFC 005 / RFC 041).
    RustItem(RustItemInfo),
}

/// Variable information
#[derive(Debug, Clone)]
pub struct VariableInfo {
    pub ty: ResolvedType,
    pub is_mutable: bool,
    pub is_used: bool,
}

/// Function information
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub params: Vec<(String, ResolvedType)>,
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub type_params: Vec<String>,
    /// Explicit source-declared bounds per type parameter (RFC 023), keyed by type parameter name.
    pub type_param_bounds: HashMap<String, Vec<String>>,
}

/// Type information
#[derive(Debug, Clone)]
pub enum TypeInfo {
    Builtin,
    Class(ClassInfo),
    Model(ModelInfo),
    TypeAlias,
    Newtype(NewtypeInfo),
    Enum(EnumInfo),
}

/// Class information
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub type_params: Vec<String>,
    pub extends: Option<String>,
    pub traits: Vec<String>,
    pub derives: Vec<String>,
    pub fields: HashMap<String, FieldInfo>,
    pub methods: HashMap<String, MethodInfo>,
}

/// Model information
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub type_params: Vec<String>,
    pub traits: Vec<String>,
    pub derives: Vec<String>,
    pub fields: HashMap<String, FieldInfo>,
    pub methods: HashMap<String, MethodInfo>,
}

/// Newtype information
#[derive(Debug, Clone)]
pub struct NewtypeInfo {
    pub type_params: Vec<String>,
    pub is_rusttype: bool,
    /// Set when this `rusttype` declares at least one `interop:` edge (used by later pipeline stages).
    pub has_interop: bool,
    pub underlying: ResolvedType,
    /// Alias-to-target method rebinding map declared inside the type body (`alias = target`).
    ///
    /// Example: `send_now = try_send` is stored as `"send_now" -> "try_send"`.
    pub method_rebindings: HashMap<String, String>,
    pub methods: HashMap<String, MethodInfo>,
}

/// Enum information
#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub type_params: Vec<String>,
    pub variants: Vec<String>,
}

/// Trait information
#[derive(Debug, Clone)]
pub struct TraitInfo {
    pub type_params: Vec<String>,
    /// Direct supertraits from `with Trait, Other[T]` (RFC 042), after resolving type arguments.
    ///
    /// Each entry is `(trait_name, type_arguments)`; use an empty `type_arguments` list for a non-generic supertrait.
    pub supertraits: Vec<(String, Vec<ResolvedType>)>,
    pub methods: HashMap<String, MethodInfo>,
    pub requires: Vec<(String, ResolvedType)>, // Required fields
}

/// Module/import information
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub path: Vec<String>,
    pub is_python: bool,
}

/// Variant information
#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub enum_name: String,
    pub fields: Vec<ResolvedType>,
}

/// Field information
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub ty: ResolvedType,
    pub has_default: bool,
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// Method information
#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub receiver: Option<Receiver>,
    pub params: Vec<(String, ResolvedType)>,
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub has_body: bool, // false for abstract methods (...)
}

/// Resolved type (after type checking)
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedType {
    /// Primitive types
    Int,
    Float,
    Bool,
    Str,
    Bytes,
    FrozenStr,
    FrozenBytes,
    FrozenList(Box<ResolvedType>),
    FrozenDict(Box<ResolvedType>, Box<ResolvedType>),
    FrozenSet(Box<ResolvedType>),
    /// Unit type
    Unit,
    /// Named type (class, model, newtype, enum)
    Named(String),
    /// Generic type with arguments
    Generic(String, Vec<ResolvedType>),
    /// Function type
    Function(Vec<ResolvedType>, Box<ResolvedType>),
    /// Tuple type
    Tuple(Vec<ResolvedType>),
    /// Type variable (for generics)
    TypeVar(String),
    /// Self type (resolved to the implementing type in traits)
    SelfType,
    /// Internal reference type (borrowed `&T`).
    ///
    /// ## Notes
    /// - This is currently compiler-internal (not a user-spellable surface type).
    /// - It exists to model Rust interop semantics like `HashMap::get` returning `Option<&V>`.
    Ref(Box<ResolvedType>),
    /// Rust import with a known canonical path (`crate::...` string), RFC 041.
    ///
    /// Lowers to backend `IrType::Unknown` until dedicated IR typing exists; provenance also lives on
    /// [`SymbolKind::RustItem`].
    RustPath(String),
    /// Unknown/error type
    Unknown,
}

impl ResolvedType {
    /// Check if this is a Result type
    pub fn is_result(&self) -> bool {
        matches!(
            self,
            ResolvedType::Generic(name, _) if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result)
        )
    }

    /// Check if this is an Option type
    pub fn is_option(&self) -> bool {
        matches!(
            self,
            ResolvedType::Generic(name, _) if collections::from_str(name.as_str()) == Some(CollectionTypeId::Option)
        )
    }

    /// Get the Ok type from Result[T, E]
    pub fn result_ok_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Get the Err type from Result[T, E]
    pub fn result_err_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result) && args.len() >= 2 =>
            {
                Some(&args[1])
            }
            _ => None,
        }
    }

    /// Get the inner type from Option[T]
    pub fn option_inner_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Option) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for ResolvedType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolvedType::Int => write!(f, "int"),
            ResolvedType::Float => write!(f, "float"),
            ResolvedType::Bool => write!(f, "bool"),
            ResolvedType::Str => write!(f, "str"),
            ResolvedType::Bytes => write!(f, "bytes"),
            ResolvedType::FrozenStr => write!(f, "FrozenStr"),
            ResolvedType::FrozenBytes => write!(f, "FrozenBytes"),
            ResolvedType::FrozenList(elem) => write!(f, "FrozenList[{}]", elem),
            ResolvedType::FrozenDict(k, v) => write!(f, "FrozenDict[{}, {}]", k, v),
            ResolvedType::FrozenSet(elem) => write!(f, "FrozenSet[{}]", elem),
            ResolvedType::Unit => write!(f, "Unit"),
            ResolvedType::Named(name) => write!(f, "{}", name),
            ResolvedType::Generic(name, args) => {
                write!(f, "{}[", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, "]")
            }
            ResolvedType::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            ResolvedType::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            ResolvedType::TypeVar(name) => write!(f, "{}", name),
            ResolvedType::SelfType => write!(f, "Self"),
            ResolvedType::Ref(inner) => write!(f, "&{}", inner),
            ResolvedType::RustPath(path) => write!(f, "rust::{}", path),
            ResolvedType::Unknown => write!(f, "?"),
        }
    }
}

/// Convert AST Type to ResolvedType
/// Normalize type name to canonical form (uppercase for built-in generics)
fn normalize_type_name(name: &str) -> String {
    // Generic base normalization: prefer the canonical spelling from `incan_core` for all builtin
    // collection/generic-base types (and their aliases).
    if let Some(id) = collections::from_str(name) {
        return collections::as_str(id).to_string();
    }
    name.to_string()
}

/// Resolve `a::b::c` in type position when `a` is a `rust::` import binding (module or item).
fn resolve_qualified_rust_type_path(segments: &[String], symbols: &SymbolTable) -> ResolvedType {
    if segments.len() < 2 {
        return ResolvedType::Unknown;
    }
    let Some(root) = segments.first() else {
        return ResolvedType::Unknown;
    };
    let Some(id) = symbols.lookup(root) else {
        return ResolvedType::Unknown;
    };
    let Some(sym) = symbols.get(id) else {
        return ResolvedType::Unknown;
    };
    let SymbolKind::RustItem(info) = &sym.kind else {
        return ResolvedType::Unknown;
    };
    let mut path = info.path.clone();
    for seg in segments.iter().skip(1) {
        path.push_str("::");
        path.push_str(seg);
    }
    ResolvedType::RustPath(path)
}

pub fn resolve_type(ty: &Type, symbols: &SymbolTable) -> ResolvedType {
    match ty {
        Type::Qualified(segments) => resolve_qualified_rust_type_path(segments, symbols),
        Type::Simple(name) => {
            if let Some(id) = numerics::from_str(name.as_str()) {
                return match id {
                    NumericTypeId::Int => ResolvedType::Int,
                    NumericTypeId::Float => ResolvedType::Float,
                    NumericTypeId::Bool => ResolvedType::Bool,
                };
            }
            if let Some(id) = stringlike::from_str(name.as_str()) {
                return match id {
                    StringLikeId::Str => ResolvedType::Str,
                    StringLikeId::Bytes => ResolvedType::Bytes,
                    StringLikeId::FrozenStr => ResolvedType::FrozenStr,
                    StringLikeId::FrozenBytes => ResolvedType::FrozenBytes,
                    // We currently treat f-strings as a regular string type at the type level.
                    StringLikeId::FString => ResolvedType::Str,
                };
            }
            if let Some(id) = collections::from_str(name.as_str()) {
                // `List`/`Dict`/... can appear in type position without parameters (e.g. `Tuple` as "any tuple").
                // Preserve it as a named type, but normalize to the canonical spelling from `incan_core`.
                return ResolvedType::Named(collections::as_str(id).to_string());
            }

            match name.as_str() {
                conventions::UNIT_TYPE_NAME | conventions::NONE_TYPE_NAME => ResolvedType::Unit,
                _ => {
                    if let Some(id) = symbols.lookup(name)
                        && let Some(sym) = symbols.get(id)
                        && let SymbolKind::RustItem(info) = &sym.kind
                    {
                        return match info.binding {
                            RustImportBindingKind::CrateRoot => ResolvedType::Unknown,
                            RustImportBindingKind::RootedPath | RustImportBindingKind::FromImport => {
                                ResolvedType::RustPath(info.path.clone())
                            }
                        };
                    }
                    // Check if it's a known type
                    if symbols.lookup(name).is_some() {
                        ResolvedType::Named(name.clone())
                    } else {
                        // Could be a type variable
                        ResolvedType::TypeVar(name.clone())
                    }
                }
            }
        }
        Type::Generic(name, args) => {
            let resolved_args: Vec<_> = args.iter().map(|a| resolve_type(&a.node, symbols)).collect();
            // Normalize type name for built-in generics (aliases → canonical spellings).
            let id = collections::from_str(name.as_str());
            let normalized_name = id
                .map(|id| collections::as_str(id).to_string())
                .unwrap_or_else(|| normalize_type_name(name));

            match id {
                Some(CollectionTypeId::FrozenList) => {
                    let elem = resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                    ResolvedType::FrozenList(Box::new(elem))
                }
                Some(CollectionTypeId::FrozenSet) => {
                    let elem = resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                    ResolvedType::FrozenSet(Box::new(elem))
                }
                Some(CollectionTypeId::FrozenDict) => {
                    let k = resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                    let v = resolved_args.get(1).cloned().unwrap_or(ResolvedType::Unknown);
                    ResolvedType::FrozenDict(Box::new(k), Box::new(v))
                }
                _ => ResolvedType::Generic(normalized_name, resolved_args),
            }
        }
        Type::Function(params, ret) => {
            let resolved_params: Vec<_> = params.iter().map(|p| resolve_type(&p.node, symbols)).collect();
            let resolved_ret = resolve_type(&ret.node, symbols);
            ResolvedType::Function(resolved_params, Box::new(resolved_ret))
        }
        Type::Unit => ResolvedType::Unit,
        Type::Tuple(elems) => {
            let resolved_elems: Vec<_> = elems.iter().map(|e| resolve_type(&e.node, symbols)).collect();
            ResolvedType::Tuple(resolved_elems)
        }
        Type::SelfType => ResolvedType::SelfType,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Span, Spanned, Type};

    #[test]
    fn test_scope_lookup() {
        let mut table = SymbolTable::new();

        // Define in global scope
        table.define(Symbol {
            name: "x".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        // Enter a new scope
        table.enter_scope(ScopeKind::Function);

        // Should still find x
        assert!(table.lookup("x").is_some());

        // Define y in inner scope
        table.define(Symbol {
            name: "y".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        assert!(table.lookup("y").is_some());

        // Exit scope
        table.exit_scope();

        // x still visible, y not
        assert!(table.lookup("x").is_some());
        assert!(table.lookup("y").is_none());
    }

    #[test]
    fn test_result_type_helpers() {
        let result_type = ResolvedType::Generic(
            "Result".to_string(),
            vec![ResolvedType::Int, ResolvedType::Named("AppError".to_string())],
        );

        assert!(result_type.is_result());
        assert_eq!(result_type.result_ok_type(), Some(&ResolvedType::Int));
        assert_eq!(
            result_type.result_err_type(),
            Some(&ResolvedType::Named("AppError".to_string()))
        );
    }

    #[test]
    fn test_function_type_resolution() {
        let symbols = SymbolTable::new();

        // The parser desugars Callable[(), int] → Type::Function([], int).
        // Verify that resolve_type handles the desugared form correctly.

        // () -> int (zero params)
        let fn_zero = Type::Function(
            vec![],
            Box::new(Spanned::new(Type::Simple("int".to_string()), Span::default())),
        );
        let ty = resolve_type(&fn_zero, &symbols);
        assert_eq!(ty, ResolvedType::Function(vec![], Box::new(ResolvedType::Int)));

        // (int) -> int (single param)
        let fn_single = Type::Function(
            vec![Spanned::new(Type::Simple("int".to_string()), Span::default())],
            Box::new(Spanned::new(Type::Simple("int".to_string()), Span::default())),
        );
        let ty = resolve_type(&fn_single, &symbols);
        assert_eq!(
            ty,
            ResolvedType::Function(vec![ResolvedType::Int], Box::new(ResolvedType::Int))
        );

        // (int, str) -> bool (multi param)
        let fn_multi = Type::Function(
            vec![
                Spanned::new(Type::Simple("int".to_string()), Span::default()),
                Spanned::new(Type::Simple("str".to_string()), Span::default()),
            ],
            Box::new(Spanned::new(Type::Simple("bool".to_string()), Span::default())),
        );
        let ty = resolve_type(&fn_multi, &symbols);
        assert_eq!(
            ty,
            ResolvedType::Function(vec![ResolvedType::Int, ResolvedType::Str], Box::new(ResolvedType::Bool))
        );
    }

    #[test]
    fn resolve_type_qualified_rust_module_item() {
        let mut table = SymbolTable::new();
        table.define(Symbol {
            name: "proto_type".to_string(),
            kind: SymbolKind::RustItem(RustItemInfo {
                crate_name: "substrait".to_string(),
                path: "substrait::proto::type".to_string(),
                binding: RustImportBindingKind::FromImport,
                metadata: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        let ty = Type::Qualified(vec!["proto_type".to_string(), "Binary".to_string()]);
        let r = resolve_type(&ty, &table);
        assert_eq!(r, ResolvedType::RustPath("substrait::proto::type::Binary".to_string()));
    }
}
