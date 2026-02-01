//! AST to IR lowering pass.
//!
//! This module converts the Incan frontend AST to the typed IR representation.
//! The lowering pass:
//!
//! 1. Resolves types from AST type annotations
//! 2. Determines ownership/borrowing semantics
//! 3. Converts AST nodes to their IR equivalents
//!
//! # Architecture
//!
//! The lowering module is split into submodules for maintainability:
//!
//! - `errors` - Error types (`LoweringError`, `LoweringErrors`)
//! - `types` - Type lowering utilities
//! - `decl` - Declaration lowering (functions, models, classes, enums, etc.)
//! - `stmt` - Statement lowering
//! - `expr` - Expression lowering
//!
//! # Usage
//!
//! ```rust,ignore
//! use incan::backend::ir::lower::AstLowering;
//!
//! let mut lowering = AstLowering::new();
//! let ir_program = lowering.lower_program(&ast_program)?;
//! ```

mod decl;
mod errors;
mod expr;
mod stmt;
mod types;

use std::collections::HashMap;

use super::decl::{FunctionParam, IrDecl, IrDeclKind};
use super::types::IrType;
use super::{IrProgram, Mutability};
use crate::frontend::ast;
use crate::frontend::typechecker::TypeCheckInfo;
use incan_core::lang::conventions;
use incan_core::lang::types::collections::{self, CollectionTypeId};

// Re-export error types
pub use errors::{LoweringError, LoweringErrors};

/// AST to IR lowering context.
///
/// Maintains state needed during the lowering pass:
/// - Scope chain for variable type lookups
/// - Registered struct/enum names for constructor detection
/// - Mutable variable tracking for borrow insertion
/// - Class declarations for inheritance resolution
/// - Trait method names for impl filtering
///
/// # Examples
///
/// ```rust,ignore
/// use incan::backend::ir::lower::AstLowering;
///
/// let mut lowering = AstLowering::new();
/// let ir_program = lowering.lower_program(&ast_program)?;
/// ```
pub struct AstLowering {
    /// Scope chain for variable type lookups (innermost last)
    pub(super) scopes: Vec<HashMap<String, IrType>>,
    /// Track declared structs/models/classes for constructor detection
    pub(super) struct_names: HashMap<String, IrType>,
    /// Track declared enums for type resolution
    pub(super) enum_names: HashMap<String, IrType>,
    /// Track mutable variables for auto-borrow at call sites
    pub(super) mutable_vars: HashMap<String, bool>,
    /// Track class declarations for inheritance resolution
    pub(super) class_decls: HashMap<String, ast::ClassDecl>,
    /// Track trait method names for filtering trait impls
    pub(super) trait_methods: HashMap<String, Vec<String>>,
    /// Track full trait declarations for default-method expansion into impl blocks.
    pub(super) trait_decls: HashMap<String, ast::TraitDecl>,
    /// Optional typechecker output used to drive lowering (avoid heuristics).
    pub(super) type_info: Option<TypeCheckInfo>,
    /// Newtype -> chosen validated constructor method name (e.g. "from_underlying", "from_str"),
    /// used for checked construction lowering of `T(x)` at call sites.
    pub(super) newtype_checked_ctor: HashMap<String, String>,
    /// When lowering methods inside an impl block, this tracks the current target type name.
    /// Used to avoid rewriting `T(x)` inside `impl T` bodies (e.g. inside `T.from_underlying`).
    pub(super) current_impl_type: Option<String>,
    /// RFC 021: Map from (struct_name, alias) -> canonical_field_name for alias-aware resolution.
    ///
    /// Populated during model/class lowering; used to translate alias field names in:
    /// - Constructor args: `Account(type="x")` → `Account { type_: "x" }`
    /// - Field access: `a.type` → `a.type_`
    /// - Pattern fields: `Account(type=x)` → `Account { type_: x }`
    pub(super) struct_field_aliases: HashMap<String, HashMap<String, String>>,
}

impl AstLowering {
    /// Select a validated constructor method for a newtype for v0.1 checked construction.
    ///
    /// Heuristic (minimal hardening for #44, RFC runway):
    /// - Prefer a static `from_underlying(underlying) -> Result[T, E]` if present and well-shaped.
    /// - Otherwise, if there is exactly one static `from_*` method with, use it when:
    ///     - exactly 1 parameter whose type matches the newtype underlying type (syntactic match), and
    ///     - return type `Result[T, E]`,
    fn select_newtype_checked_ctor(n: &ast::NewtypeDecl) -> Option<String> {
        fn is_result_of_newtype(ty: &ast::Type, newtype_name: &str) -> bool {
            let ast::Type::Generic(name, args) = ty else {
                return false;
            };
            if collections::from_str(name.as_str()) != Some(CollectionTypeId::Result) || args.is_empty() {
                return false;
            }
            matches!(&args[0].node, ast::Type::Simple(t) if t == newtype_name)
        }

        fn matches_underlying_param(m: &ast::MethodDecl, underlying: &ast::Type) -> bool {
            if m.params.len() != 1 {
                return false;
            }
            m.params[0].node.ty.node == *underlying
        }

        // Candidate: static method named from_* with (underlying) -> Result[T, E]
        let mut candidates: Vec<&ast::MethodDecl> = n
            .methods
            .iter()
            .filter_map(|m| {
                let md = &m.node;
                if md.receiver.is_some() {
                    return None;
                }
                if !md.name.starts_with("from_") {
                    return None;
                }
                if !matches_underlying_param(md, &n.underlying.node) {
                    return None;
                }
                if !is_result_of_newtype(&md.return_type.node, &n.name) {
                    return None;
                }
                Some(md)
            })
            .collect();

        // Prefer from_underlying
        if let Some(m) = candidates
            .iter()
            .find(|m| m.name == conventions::NEWTYPE_FROM_UNDERLYING_METHOD)
        {
            return Some(m.name.clone());
        }

        if candidates.len() == 1 {
            // Safe: we just checked len() == 1
            return candidates.pop().map(|m| m.name.clone());
        }

        if candidates.len() > 1 {
            tracing::warn!(
                newtype = %n.name,
                candidates = ?candidates.iter().map(|m| &m.name).collect::<Vec<_>>(),
                "newtype has multiple from_* methods; define explicit from_underlying for checked construction"
            );
        }

        None
    }

    /// Create a new lowering context.
    ///
    /// Initializes an empty scope chain and type registries.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            struct_names: HashMap::new(),
            enum_names: HashMap::new(),
            mutable_vars: HashMap::new(),
            class_decls: HashMap::new(),
            trait_methods: HashMap::new(),
            trait_decls: HashMap::new(),
            type_info: None,
            newtype_checked_ctor: HashMap::new(),
            current_impl_type: None,
            struct_field_aliases: HashMap::new(),
        }
    }

    /// Create a lowering context that uses typechecker output for more accurate lowering.
    pub fn new_with_type_info(type_info: TypeCheckInfo) -> Self {
        let mut s = Self::new();
        s.type_info = Some(type_info);
        s
    }

    /// Seed alias maps for types that may be referenced from other modules.
    ///
    /// This is used by multi-file codegen so alias-aware lowering works when a module
    /// references a `model` defined in a different module (e.g. `a.type` or `Account(type="x")`).
    pub fn seed_struct_field_aliases(&mut self, aliases: HashMap<String, HashMap<String, String>>) {
        for (struct_name, map) in aliases {
            self.struct_field_aliases.entry(struct_name).or_default().extend(map);
        }
    }

    /// RFC 021: Resolve a field name through alias mapping.
    ///
    /// If `field_name` is an alias for a field on `struct_name`, returns the canonical field name.
    /// Otherwise returns the original `field_name`.
    ///
    /// This is used to translate alias-based field references in:
    /// - Constructor args: `Account(type="x")` → uses canonical `type_`
    /// - Field access: `a.type` → accesses canonical `type_`
    /// - Pattern fields: `Account(type=x)` → matches canonical `type_`
    pub(super) fn resolve_field_alias(&self, struct_name: &str, field_name: &str) -> String {
        self.struct_field_aliases
            .get(struct_name)
            .and_then(|aliases| aliases.get(field_name))
            .cloned()
            .unwrap_or_else(|| field_name.to_string())
    }

    /// RFC 021: Register field aliases for a struct/model/class.
    ///
    /// Called during model/class lowering to populate `struct_field_aliases`.
    pub(super) fn register_field_aliases(&mut self, struct_name: &str, fields: &[ast::Spanned<ast::FieldDecl>]) {
        let mut aliases = HashMap::new();
        for field in fields {
            if let Some(alias) = &field.node.metadata.alias {
                aliases.insert(alias.clone(), field.node.name.clone());
            }
        }
        if !aliases.is_empty() {
            self.struct_field_aliases.insert(struct_name.to_string(), aliases);
        }
    }

    /// RFC 021: Register imported struct aliases that map to known model names.
    ///
    /// This enables alias-aware lowering when a module imports a model under an alias:
    /// `from db.schema import Account as A` should resolve `A(type=...)` and `a.type`.
    pub(super) fn register_imported_struct_aliases(&mut self, program: &ast::Program) {
        for decl in &program.declarations {
            let ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            let ast::ImportKind::From { items, .. } = &import.kind else {
                continue;
            };

            for item in items {
                let Some(alias) = &item.alias else {
                    continue;
                };
                if self.struct_field_aliases.contains_key(alias) {
                    continue;
                }
                if let Some(map) = self.struct_field_aliases.get(&item.name) {
                    self.struct_field_aliases.insert(alias.clone(), map.clone());
                }
            }
        }
    }

    /// Lower a complete AST program to IR.
    ///
    /// This is the main entry point for the lowering pass. It performs:
    ///
    /// 1. First pass: Collect class declarations and trait method names
    /// 2. Second pass: Collect function signatures for the registry
    /// 3. Third pass: Lower all declarations to IR
    ///
    /// # Parameters
    ///
    /// * `program` - The AST program to lower
    ///
    /// # Returns
    ///
    /// An `IrProgram` containing all lowered declarations.
    ///
    /// # Errors
    ///
    /// Returns `LoweringErrors` containing all errors encountered during lowering.
    /// This allows callers to display multiple errors to the user at once.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
    pub fn lower_program(&mut self, program: &ast::Program) -> Result<IrProgram, LoweringErrors> {
        let mut ir_program = IrProgram::new();
        let mut errors: Vec<LoweringError> = Vec::new();

        // Seed alias maps for imported model aliases before lowering expressions.
        self.register_imported_struct_aliases(program);

        // First pass: collect class declarations, trait decls, and newtype ctor selection.
        for decl in &program.declarations {
            if let ast::Declaration::Class(ref c) = decl.node {
                self.class_decls.insert(c.name.clone(), c.clone());
            }
            if let ast::Declaration::Trait(ref t) = decl.node {
                let method_names: Vec<String> = t.methods.iter().map(|m| m.node.name.clone()).collect();
                self.trait_methods.insert(t.name.clone(), method_names);
                self.trait_decls.insert(t.name.clone(), t.clone());
            }
            if let ast::Declaration::Newtype(ref n) = decl.node {
                // Track validation hook selection for checked construction lowering.
                if let Some(ctor) = Self::select_newtype_checked_ctor(n) {
                    self.newtype_checked_ctor.insert(n.name.clone(), ctor);
                }
            }
        }

        // Pass 1.5: register module-level const names into the root scope for lookups.
        // (Type inference/refinement happens later; Unknown is fine for non-const contexts.)
        for decl in &program.declarations {
            if let ast::Declaration::Const(ref c) = decl.node {
                let ty = if let Some(ann) = &c.ty {
                    self.lower_type(&ann.node)
                } else {
                    IrType::Unknown
                };
                if let Some(scope) = self.scopes.first_mut() {
                    scope.insert(c.name.clone(), ty);
                }
            }
        }

        // Second pass: collect all function signatures
        for decl in &program.declarations {
            if let ast::Declaration::Function(ref f) = decl.node {
                let params: Vec<FunctionParam> = f
                    .params
                    .iter()
                    .map(|p| {
                        let base_ty = self.lower_type(&p.node.ty.node);
                        FunctionParam {
                            name: p.node.name.clone(),
                            ty: base_ty,
                            mutability: if p.node.is_mut {
                                Mutability::Mutable
                            } else {
                                Mutability::Immutable
                            },
                            is_self: false,
                        }
                    })
                    .collect();
                let return_type = self.lower_type(&f.return_type.node);
                ir_program
                    .function_registry
                    .register(f.name.clone(), params, return_type);
            }
        }

        // Third pass: lower declarations
        for decl in &program.declarations {
            // Handle models - generate both struct and impl
            // Models always get impl blocks (for serde methods even if no user methods)
            match &decl.node {
                ast::Declaration::Model(m) => {
                    // Generate struct
                    match self.lower_model(m) {
                        Ok(struct_ir) => {
                            self.struct_names
                                .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                            ir_program
                                .declarations
                                .push(IrDecl::new(IrDeclKind::Struct(struct_ir.clone())));

                            // Generate impl block (may be empty if no methods, serde methods added during emission)
                            match self.lower_model_methods(&struct_ir.name, &m.methods) {
                                Ok(impl_ir) => {
                                    ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                }
                                Err(e) => errors.push(e),
                            }

                            // Generate trait impls for each trait this model implements
                            for trait_ref in &m.traits {
                                match self.lower_trait_impl(&struct_ir.name, trait_ref.node.as_str(), &m.methods) {
                                    Ok(trait_impl) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }
                        }
                        Err(e) => errors.push(e),
                    }
                }
                ast::Declaration::Docstring(_) => {
                    // Module-level docstrings are not part of IR; ignore silently
                    continue;
                }
                ast::Declaration::Class(c) => {
                    // Generate struct
                    match self.lower_class(c) {
                        Ok(struct_ir) => {
                            self.struct_names
                                .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                            ir_program
                                .declarations
                                .push(IrDecl::new(IrDeclKind::Struct(struct_ir.clone())));

                            // Collect methods from this class and all parent classes
                            let mut all_methods = Vec::new();
                            if let Err(e) = self.collect_inherited_methods(&c.name, &mut all_methods) {
                                errors.push(e);
                            }

                            // Generate impl block for all methods (inherited + own)
                            if !all_methods.is_empty() {
                                match self.lower_class_methods(&struct_ir.name, &all_methods) {
                                    Ok(impl_ir) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }

                            // Generate trait impls for each trait this class implements
                            for trait_ref in &c.traits {
                                match self.lower_trait_impl(&struct_ir.name, trait_ref.node.as_str(), &all_methods) {
                                    Ok(trait_impl) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }
                        }
                        Err(e) => errors.push(e),
                    }
                }
                ast::Declaration::Newtype(n) => {
                    // Generate struct
                    match self.lower_newtype(n) {
                        Ok(struct_ir) => {
                            self.struct_names
                                .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                            ir_program
                                .declarations
                                .push(IrDecl::new(IrDeclKind::Struct(struct_ir.clone())));

                            // Generate impl block for newtype methods (if any).
                            if !n.methods.is_empty() {
                                match self.lower_model_methods(&struct_ir.name, &n.methods) {
                                    Ok(impl_ir) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }
                        }
                        Err(e) => errors.push(e),
                    }
                }
                _ => {
                    // Regular declaration lowering
                    match self.lower_declaration(&decl.node) {
                        Ok(ir_decl) => {
                            if let IrDeclKind::Function(ref func) = ir_decl.kind {
                                if func.name == conventions::ENTRYPOINT_NAME {
                                    ir_program.entry_point = Some(conventions::ENTRYPOINT_NAME.to_string());
                                }
                            }
                            ir_program.declarations.push(ir_decl);
                        }
                        Err(e) => errors.push(e),
                    }
                }
            }
        }
        // Propagate Serialize/Deserialize derives from structs to their field types (enums).
        // This allows users to only annotate the top-level model with @derive(Serialize, Deserialize)
        // and have it automatically apply to nested user-defined enums.
        Self::propagate_serde_derives(&mut ir_program);

        if errors.is_empty() {
            Ok(ir_program)
        } else {
            // Return all collected errors
            Err(LoweringErrors(errors))
        }
    }

    /// Propagate Serialize/Deserialize derives from structs to enum/newtype field types.
    ///
    /// When a struct has Serialize or Deserialize derives and contains fields of enum types,
    /// those enums also need the same derives for the generated Rust code to compile.
    /// This function automatically adds those derives to avoid requiring users to manually
    /// annotate every nested enum.
    fn propagate_serde_derives(ir_program: &mut IrProgram) {
        use super::decl::IrDeclKind;
        use incan_core::lang::derives::{self, DeriveId};

        let serialize = derives::as_str(DeriveId::Serialize);
        let deserialize = derives::as_str(DeriveId::Deserialize);

        // Collect enum/newtype names that need Serialize/Deserialize
        let mut enums_need_serialize: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut enums_need_deserialize: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut structs_need_serialize: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut structs_need_deserialize: std::collections::HashSet<String> = std::collections::HashSet::new();

        let mut newtype_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut enum_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for decl in &ir_program.declarations {
            if let IrDeclKind::Struct(s) = &decl.kind {
                if s.fields.len() == 1 && s.fields[0].name == "0" {
                    newtype_names.insert(s.name.clone());
                }
            }
            if let IrDeclKind::Enum(e) = &decl.kind {
                enum_names.insert(e.name.clone());
            }
        }

        // First pass: find all structs with Serialize/Deserialize and collect their enum/newtype field types
        for decl in &ir_program.declarations {
            if let IrDeclKind::Struct(s) = &decl.kind {
                let has_serialize = s.derives.iter().any(|d| d == serialize);
                let has_deserialize = s.derives.iter().any(|d| d == deserialize);

                if has_serialize {
                    for field in &s.fields {
                        Self::collect_enum_and_struct_types_from_ir_type(
                            &field.ty,
                            &mut enums_need_serialize,
                            &mut structs_need_serialize,
                        );
                    }
                }
                if has_deserialize {
                    for field in &s.fields {
                        Self::collect_enum_and_struct_types_from_ir_type(
                            &field.ty,
                            &mut enums_need_deserialize,
                            &mut structs_need_deserialize,
                        );
                    }
                }
            }
        }

        for name in structs_need_serialize.iter() {
            if enum_names.contains(name) {
                enums_need_serialize.insert(name.clone());
            }
        }
        for name in structs_need_deserialize.iter() {
            if enum_names.contains(name) {
                enums_need_deserialize.insert(name.clone());
            }
        }

        // Second pass: add Serialize/Deserialize to enums/newtypes that need them
        for decl in &mut ir_program.declarations {
            if let IrDeclKind::Enum(e) = &mut decl.kind {
                if enums_need_serialize.contains(&e.name) && !e.derives.iter().any(|d| d == serialize) {
                    e.derives.push(serialize.to_string());
                }
                if enums_need_deserialize.contains(&e.name) && !e.derives.iter().any(|d| d == deserialize) {
                    e.derives.push(deserialize.to_string());
                }
            }
            if let IrDeclKind::Struct(s) = &mut decl.kind {
                if newtype_names.contains(&s.name) {
                    if structs_need_serialize.contains(&s.name) && !s.derives.iter().any(|d| d == serialize) {
                        s.derives.push(serialize.to_string());
                    }
                    if structs_need_deserialize.contains(&s.name) && !s.derives.iter().any(|d| d == deserialize) {
                        s.derives.push(deserialize.to_string());
                    }
                }
            }
        }
    }

    /// Recursively collect enum and struct type names from an IR type.
    fn collect_enum_and_struct_types_from_ir_type(
        ty: &IrType,
        enums: &mut std::collections::HashSet<String>,
        structs: &mut std::collections::HashSet<String>,
    ) {
        match ty {
            IrType::Enum(name) => {
                enums.insert(name.clone());
            }
            IrType::Struct(name) => {
                structs.insert(name.clone());
            }
            IrType::Option(inner) => {
                Self::collect_enum_and_struct_types_from_ir_type(inner, enums, structs);
            }
            IrType::List(inner) => {
                Self::collect_enum_and_struct_types_from_ir_type(inner, enums, structs);
            }
            IrType::Dict(k, v) => {
                Self::collect_enum_and_struct_types_from_ir_type(k, enums, structs);
                Self::collect_enum_and_struct_types_from_ir_type(v, enums, structs);
            }
            IrType::Set(inner) => {
                Self::collect_enum_and_struct_types_from_ir_type(inner, enums, structs);
            }
            IrType::Result(ok, err) => {
                Self::collect_enum_and_struct_types_from_ir_type(ok, enums, structs);
                Self::collect_enum_and_struct_types_from_ir_type(err, enums, structs);
            }
            IrType::Tuple(elems) => {
                for elem in elems {
                    Self::collect_enum_and_struct_types_from_ir_type(elem, enums, structs);
                }
            }
            IrType::NamedGeneric(_, args) => {
                for arg in args {
                    Self::collect_enum_and_struct_types_from_ir_type(arg, enums, structs);
                }
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => {
                Self::collect_enum_and_struct_types_from_ir_type(inner, enums, structs);
            }
            // Primitive types and other types don't contain enums
            _ => {}
        }
    }
}

impl Default for AstLowering {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::frontend::{lexer, parser};
    use incan_core::lang::derives::{self, DeriveId};

    fn lower_source(source: &str) -> Result<IrProgram, LoweringErrors> {
        let tokens = lexer::lex(source).unwrap_or_else(|errs| {
            panic!("lexer failed: {errs:?}");
        });
        let ast = parser::parse(&tokens).unwrap_or_else(|errs| {
            panic!("parser failed: {errs:?}");
        });
        let mut lowering = AstLowering::new();
        lowering.lower_program(&ast)
    }

    #[test]
    fn test_lower_simple_function() {
        let ir = lower_source(
            r#"
def add(a: int, b: int) -> int:
    return a + b
"#,
        )
        .unwrap();
        assert_eq!(ir.declarations.len(), 1);
        if let IrDeclKind::Function(f) = &ir.declarations[0].kind {
            assert_eq!(f.name, "add");
            assert_eq!(f.params.len(), 2);
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn test_lower_model() {
        let ir = lower_source(
            r#"
model User:
    name: str
    age: int
"#,
        )
        .unwrap();
        // Model generates both struct and impl
        assert_eq!(ir.declarations.len(), 2);
        if let IrDeclKind::Struct(s) = &ir.declarations[0].kind {
            assert_eq!(s.name, "User");
            assert_eq!(s.fields.len(), 2);
        } else {
            panic!("Expected struct declaration");
        }
    }

    #[test]
    fn test_lower_main_entry() {
        let ir = lower_source(
            r#"
def main() -> None:
    pass
"#,
        )
        .unwrap();
        assert_eq!(ir.entry_point, Some("main".to_string()));
    }

    #[test]
    fn test_lower_if_statement() {
        let ir = lower_source(
            r#"
def check(x: int) -> str:
    if x > 0:
        return "positive"
    elif x < 0:
        return "negative"
    else:
        return "zero"
"#,
        )
        .unwrap();
        assert_eq!(ir.declarations.len(), 1);
        if let IrDeclKind::Function(f) = &ir.declarations[0].kind {
            assert!(!f.body.is_empty());
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn test_lower_for_loop() {
        let ir = lower_source(
            r#"
def count() -> None:
    for i in range(10):
        print(i)
"#,
        )
        .unwrap();
        assert_eq!(ir.declarations.len(), 1);
    }

    #[test]
    fn test_lower_binary_expressions() {
        let ir = lower_source(
            r#"
def math(a: int, b: int) -> int:
    x = a + b
    y = a * b
    z = a - b
    return x + y + z
"#,
        )
        .unwrap();
        assert_eq!(ir.declarations.len(), 1);
    }

    #[test]
    fn test_lower_list_literal() {
        let ir = lower_source(
            r#"
def get_list() -> List[int]:
    return [1, 2, 3]
"#,
        )
        .unwrap();
        assert_eq!(ir.declarations.len(), 1);
    }

    #[test]
    fn test_lower_enum() {
        let ir = lower_source(
            r#"
enum Color:
    Red
    Green
    Blue
"#,
        )
        .unwrap();
        assert_eq!(ir.declarations.len(), 1);
        if let IrDeclKind::Enum(e) = &ir.declarations[0].kind {
            assert_eq!(e.name, "Color");
            assert_eq!(e.variants.len(), 3);
        } else {
            panic!("Expected enum declaration");
        }
    }

    #[test]
    fn test_inferred_reassign_mutable() {
        // `mut x = 1; x = 2` should succeed because x is mutable.
        let source = r#"
def test() -> int:
    mut x = 1
    x = 2
    return x
"#;
        let ir = lower_source(source).unwrap();
        assert_eq!(ir.declarations.len(), 1);
        if let IrDeclKind::Function(f) = &ir.declarations[0].kind {
            // Expected: Let, Assign, Return (3 statements)
            assert_eq!(f.body.len(), 3, "Expected 3 statements");
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn test_inferred_reassign_immutable_error() {
        // `x = 1; x = 2` should fail because x is immutable.
        let source = r#"
def test() -> int:
    x = 1
    x = 2
    return x
"#;
        let result = lower_source(source);
        assert!(result.is_err(), "Expected error for immutable reassignment");
        let errors = result.unwrap_err();
        assert!(
            errors.0[0].message.contains("immutable"),
            "Error should mention immutable"
        );
    }

    #[test]
    fn test_serde_propagation_respects_derives_and_containers() {
        let ir = lower_source(
            r#"
@derive(Serialize)
model Payload:
  tags: set[Tag]
  id: UserId

enum Tag:
  A
  B

type UserId = newtype int
"#,
        )
        .unwrap();

        let serialize = derives::as_str(DeriveId::Serialize).to_string();
        let deserialize = derives::as_str(DeriveId::Deserialize).to_string();

        let mut tag_derives: Option<Vec<String>> = None;
        let mut user_id_derives: Option<Vec<String>> = None;
        for decl in &ir.declarations {
            match &decl.kind {
                IrDeclKind::Enum(e) if e.name == "Tag" => tag_derives = Some(e.derives.clone()),
                IrDeclKind::Struct(s) if s.name == "UserId" => user_id_derives = Some(s.derives.clone()),
                _ => {}
            }
        }

        let tag_derives = match tag_derives {
            Some(derives) => derives,
            None => panic!("Tag enum not found"),
        };
        let user_id_derives = match user_id_derives {
            Some(derives) => derives,
            None => panic!("UserId newtype not found"),
        };
        assert!(tag_derives.contains(&serialize));
        assert!(!tag_derives.contains(&deserialize));
        assert!(user_id_derives.contains(&serialize));
        assert!(!user_id_derives.contains(&deserialize));
    }
}
