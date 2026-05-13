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

use std::collections::{HashMap, HashSet};

use super::TypedExpr;
use super::decl::{FunctionParam, IrDecl, IrDeclKind};
use super::expr::{IrCallArg, IrCallArgKind, IrExprKind, VarAccess, VarRefKind};
use super::stmt::{IrStmt, IrStmtKind};
use super::types::IrType;
use super::{FunctionSignature, IrProgram, Mutability};
use crate::frontend::ast;
use crate::frontend::decorator_resolution;
use crate::frontend::symbols::NewtypePrimitiveConstraint;
use crate::frontend::typechecker::TypeCheckInfo;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_core::lang::conventions;
use incan_core::lang::stdlib;
use incan_core::lang::traits::{self as core_traits, TraitId};
use incan_core::lang::types::collections::{self, CollectionTypeId};

// Re-export error types
pub use errors::{LoweringError, LoweringErrors};

pub(in crate::backend::ir::lower) struct TraitImplLoweringInput<'a> {
    pub type_name: &'a str,
    pub type_params: &'a [ast::TypeParam],
    pub trait_name: &'a str,
    pub trait_type_args: Vec<IrType>,
    pub impl_methods: &'a [ast::Spanned<ast::MethodDecl>],
    pub impl_properties: &'a [ast::Spanned<ast::PropertyDecl>],
    pub impl_associated_types: &'a [ast::Spanned<ast::AssociatedTypeDecl>],
}

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
    /// Scope chain for local bindings that preserve RFC 052 live static semantics.
    pub(super) static_binding_scopes: Vec<std::collections::HashSet<String>>,
    /// Scope chain for local callable signatures that carry default expressions not representable in [`IrType`].
    pub(super) local_callable_signature_scopes: Vec<HashMap<String, Option<FunctionSignature>>>,
    /// Callable signatures rehydrated while lowering local partial expressions, keyed by source span.
    pub(super) partial_expr_signatures: HashMap<(usize, usize), FunctionSignature>,
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
    /// Concrete nominal types that explicitly adopt the stdlib Iterator protocol.
    pub(super) iterator_adopter_names: HashSet<String>,
    /// Optional typechecker output used to drive lowering (avoid heuristics).
    pub(super) type_info: Option<TypeCheckInfo>,
    /// Newtype -> chosen validated constructor method name (e.g. "from_underlying", "from_str"),
    /// used for checked construction lowering of `T(x)` at call sites.
    pub(super) newtype_checked_ctor: HashMap<String, String>,
    /// Newtype -> generated constrained-primitive predicates for checked construction when no explicit hook exists.
    pub(super) newtype_constraints: HashMap<String, Vec<NewtypePrimitiveConstraint>>,
    /// When lowering methods inside an impl block, this tracks the current target type name.
    /// Used to avoid rewriting `T(x)` inside `impl T` bodies (e.g. inside `T.from_underlying`).
    pub(super) current_impl_type: Option<String>,
    /// Current classmethod constructor target exposed by source `cls(...)` calls.
    pub(super) current_classmethod_constructor: Option<String>,
    /// RFC 021: Map from (struct_name, alias) -> canonical_field_name for alias-aware resolution.
    ///
    /// Populated during model/class lowering; used to translate alias field names in:
    /// - Constructor args: `Account(type="x")` → `Account { type_: "x" }`
    /// - Field access: `a.type` → `a.type_`
    /// - Pattern fields: `Account(type=x)` → `Account { type_: x }`
    pub(super) struct_field_aliases: HashMap<String, HashMap<String, String>>,
    /// Remaining identifier reads for the currently-lowered statement block.
    ///
    /// This powers a local last-use heuristic: non-Copy vars are marked as `Move` only on their final read in a
    /// straight-line block.
    pub(super) remaining_ident_reads: Vec<HashMap<String, usize>>,
    /// Depth of non-linear execution contexts (loops/comprehensions/closures).
    ///
    /// While in a non-linear context, lowering avoids last-use moves.
    pub(super) non_linear_context_depth: usize,
    /// Import alias map for decorator/derive passthrough resolution.
    pub(super) import_aliases: HashMap<String, Vec<String>>,
    /// Direct Rust import aliases mapped to Rust path segments.
    pub(super) rust_import_aliases: HashMap<String, Vec<String>>,
    /// Function-typed parameters for the currently lowered callable body.
    pub(super) callable_param_scopes: Vec<HashSet<String>>,
    /// Module-level symbol aliases mapped from alias name to canonical target name.
    pub(super) symbol_aliases: HashMap<String, String>,
    /// Cached stdlib metadata used to resolve rust.module-backed decorators/derives.
    pub(super) stdlib_cache: StdlibAstCache,
    /// `rusttype` underlying Rust type lookup by alias name.
    pub(super) rusttype_underlying: HashMap<String, IrType>,
    /// Raw `interop:` edge declarations keyed by rusttype alias name.
    pub(super) rusttype_interop_edges: HashMap<String, Vec<ast::InteropEdgeDecl>>,
    /// Method rebinding aliases keyed by type alias/newtype name (`alias -> target_method`).
    pub(super) type_method_rebindings: HashMap<String, HashMap<String, String>>,
    /// Best-effort source module name for compiler-provided call-site metadata.
    pub(super) current_source_module_name: Option<String>,
}

impl AstLowering {
    /// Convert a declared callable parameter element type into its runtime parameter type.
    pub(super) fn lower_param_container_type(kind: ast::ParamKind, base_ty: IrType) -> IrType {
        match kind {
            ast::ParamKind::Normal => base_ty,
            ast::ParamKind::RestPositional => IrType::List(Box::new(base_ty)),
            ast::ParamKind::RestKeyword => IrType::Dict(Box::new(IrType::String), Box::new(base_ty)),
        }
    }

    /// Select the canonical RFC 017 checked-construction hook for a newtype.
    fn select_newtype_checked_ctor(n: &ast::NewtypeDecl) -> Option<String> {
        /// Return whether an AST type is `Result[Newtype, ValidationError]`.
        fn is_result_of_newtype_validation_error(ty: &ast::Type, newtype_name: &str) -> bool {
            let ast::Type::Generic(name, args) = ty else {
                return false;
            };
            if collections::from_str(name.as_str()) != Some(CollectionTypeId::Result) || args.len() != 2 {
                return false;
            }
            matches!(&args[0].node, ast::Type::Simple(t) if t == newtype_name || t == "Self")
                && matches!(&args[1].node, ast::Type::Simple(t) if t == "ValidationError")
        }

        fn matches_underlying_param(m: &ast::MethodDecl, underlying: &ast::Type) -> bool {
            if m.params.len() != 1 {
                return false;
            }
            m.params[0].node.ty.node == *underlying
        }

        n.methods.iter().find_map(|m| {
            let md = &m.node;
            if md.receiver.is_some() {
                return None;
            }
            if md.name != conventions::NEWTYPE_FROM_UNDERLYING_METHOD {
                return None;
            }
            if !matches_underlying_param(md, &n.underlying.node) {
                return None;
            }
            if !is_result_of_newtype_validation_error(&md.return_type.node, &n.name) {
                return None;
            }
            Some(md.name.clone())
        })
    }

    /// Create a new lowering context.
    ///
    /// Initializes an empty scope chain and type registries.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            static_binding_scopes: vec![HashSet::new()],
            local_callable_signature_scopes: vec![HashMap::new()],
            partial_expr_signatures: HashMap::new(),
            struct_names: HashMap::new(),
            enum_names: HashMap::new(),
            mutable_vars: HashMap::new(),
            class_decls: HashMap::new(),
            trait_methods: HashMap::new(),
            trait_decls: HashMap::new(),
            iterator_adopter_names: HashSet::new(),
            type_info: None,
            newtype_checked_ctor: HashMap::new(),
            newtype_constraints: HashMap::new(),
            current_impl_type: None,
            current_classmethod_constructor: None,
            struct_field_aliases: HashMap::new(),
            remaining_ident_reads: Vec::new(),
            non_linear_context_depth: 0,
            import_aliases: HashMap::new(),
            rust_import_aliases: HashMap::new(),
            callable_param_scopes: Vec::new(),
            symbol_aliases: HashMap::new(),
            stdlib_cache: StdlibAstCache::new(),
            rusttype_underlying: HashMap::new(),
            rusttype_interop_edges: HashMap::new(),
            type_method_rebindings: HashMap::new(),
            current_source_module_name: None,
        }
    }

    /// Override the source module name used for compiler-provided call-site metadata.
    pub fn set_current_source_module_name(&mut self, name: Option<String>) {
        self.current_source_module_name = name;
    }

    /// Return the logger name supplied to default `std.logging.get_logger()` calls.
    pub(super) fn current_default_logger_name(&self) -> String {
        self.current_source_module_name
            .clone()
            .unwrap_or_else(|| "root".to_string())
    }

    /// Extract generated validation constraints from a newtype underlying annotation.
    fn newtype_constraints_from_ast(ty: &ast::Type) -> Vec<NewtypePrimitiveConstraint> {
        let ast::Type::ConstrainedPrimitive(_, constraints) = ty else {
            return Vec::new();
        };
        constraints
            .iter()
            .map(|constraint| NewtypePrimitiveConstraint {
                key: constraint.node.key,
                value: constraint.node.value.value,
                repr: constraint.node.value.repr.clone(),
            })
            .collect()
    }

    /// Build a keyword-to-expression map for one partial preset argument list.
    fn partial_arg_map(args: &[ast::PartialArg]) -> HashMap<String, ast::Spanned<ast::Expr>> {
        args.iter().map(|arg| (arg.name.clone(), arg.value.clone())).collect()
    }

    /// Construct a spanned identifier expression for synthetic wrapper bodies.
    fn ident_expr(name: impl Into<String>, span: ast::Span) -> ast::Spanned<ast::Expr> {
        ast::Spanned::new(ast::Expr::Ident(name.into()), span)
    }

    /// Construct a spanned simple type annotation for synthetic constructor wrappers.
    fn simple_type(name: impl Into<String>, span: ast::Span) -> ast::Spanned<ast::Type> {
        ast::Spanned::new(ast::Type::Simple(name.into()), span)
    }

    /// Clone target parameters and replace preset parameters with partial-provided defaults.
    fn partial_projected_params(
        params: &[ast::Spanned<ast::Param>],
        presets: &HashMap<String, ast::Spanned<ast::Expr>>,
    ) -> Vec<ast::Spanned<ast::Param>> {
        params
            .iter()
            .map(|param| {
                let mut projected = param.clone();
                if let Some(default) = presets.get(&projected.node.name) {
                    projected.node.default = Some(default.clone());
                }
                projected
            })
            .collect()
    }

    /// Build named forwarding arguments from a projected wrapper parameter list.
    fn partial_forward_args(params: &[ast::Spanned<ast::Param>], span: ast::Span) -> Vec<ast::CallArg> {
        params
            .iter()
            .map(|param| ast::CallArg::Named(param.node.name.clone(), Self::ident_expr(param.node.name.clone(), span)))
            .collect()
    }

    /// Build a synthetic function declaration that forwards a top-level function partial to its target.
    fn function_partial_wrapper(
        partial: &ast::PartialDecl,
        target: &ast::FunctionDecl,
        span: ast::Span,
    ) -> ast::FunctionDecl {
        let presets = Self::partial_arg_map(&partial.args);
        let params = Self::partial_projected_params(&target.params, &presets);
        let callee = Self::ident_expr(target.name.clone(), span);
        let call = ast::Spanned::new(
            ast::Expr::Call(Box::new(callee), Vec::new(), Self::partial_forward_args(&params, span)),
            span,
        );
        ast::FunctionDecl {
            visibility: partial.visibility,
            decorators: Vec::new(),
            surface_modifiers: target.surface_modifiers.clone(),
            name: partial.name.clone(),
            type_params: target.type_params.clone(),
            params,
            return_type: target.return_type.clone(),
            body: vec![ast::Spanned::new(ast::Statement::Return(Some(call)), span)],
        }
    }

    /// Build a synthetic function declaration that forwards a constructor partial to its target type.
    fn constructor_partial_wrapper(
        partial: &ast::PartialDecl,
        target_name: &str,
        target_params: Vec<ast::Spanned<ast::Param>>,
        return_type: ast::Spanned<ast::Type>,
        span: ast::Span,
    ) -> ast::FunctionDecl {
        let presets = Self::partial_arg_map(&partial.args);
        let params = Self::partial_projected_params(&target_params, &presets);
        let callee = Self::ident_expr(target_name.to_string(), span);
        let call = ast::Spanned::new(
            ast::Expr::Call(Box::new(callee), Vec::new(), Self::partial_forward_args(&params, span)),
            span,
        );
        ast::FunctionDecl {
            visibility: partial.visibility,
            decorators: Vec::new(),
            surface_modifiers: Vec::new(),
            name: partial.name.clone(),
            type_params: Vec::new(),
            params,
            return_type,
            body: vec![ast::Spanned::new(ast::Statement::Return(Some(call)), span)],
        }
    }

    /// Convert model or class fields into constructor-style wrapper parameters.
    fn model_constructor_params(fields: &[ast::Spanned<ast::FieldDecl>]) -> Vec<ast::Spanned<ast::Param>> {
        fields
            .iter()
            .map(|field| {
                ast::Spanned::new(
                    ast::Param {
                        is_mut: false,
                        kind: ast::ParamKind::Normal,
                        name: field.node.name.clone(),
                        ty: field.node.ty.clone(),
                        default: field.node.default.clone(),
                    },
                    field.span,
                )
            })
            .collect()
    }

    /// Build the single `value` parameter surface for a newtype constructor partial.
    fn newtype_constructor_params(nt: &ast::NewtypeDecl) -> Vec<ast::Spanned<ast::Param>> {
        vec![ast::Spanned::new(
            ast::Param {
                is_mut: false,
                kind: ast::ParamKind::Normal,
                name: "value".to_string(),
                ty: nt.underlying.clone(),
                default: None,
            },
            nt.underlying.span,
        )]
    }

    /// Resolve a top-level partial declaration to the synthetic wrapper function used by IR lowering.
    fn partial_wrapper_function(
        &self,
        program: &ast::Program,
        partial: &ast::PartialDecl,
        span: ast::Span,
    ) -> Result<ast::FunctionDecl, LoweringError> {
        let [target_name] = partial.target.segments.as_slice() else {
            return Err(LoweringError {
                message: format!(
                    "Partial '{}' lowers only for local callable targets in this implementation",
                    partial.name
                ),
                span: span.into(),
            });
        };
        for decl in &program.declarations {
            match &decl.node {
                ast::Declaration::Function(func) if &func.name == target_name => {
                    return Ok(Self::function_partial_wrapper(partial, func, span));
                }
                ast::Declaration::Model(model) if &model.name == target_name => {
                    return Ok(Self::constructor_partial_wrapper(
                        partial,
                        &model.name,
                        Self::model_constructor_params(&model.fields),
                        Self::simple_type(model.name.clone(), span),
                        span,
                    ));
                }
                ast::Declaration::Class(class) if &class.name == target_name => {
                    return Ok(Self::constructor_partial_wrapper(
                        partial,
                        &class.name,
                        Self::model_constructor_params(&class.fields),
                        Self::simple_type(class.name.clone(), span),
                        span,
                    ));
                }
                ast::Declaration::Newtype(nt) if &nt.name == target_name => {
                    return Ok(Self::constructor_partial_wrapper(
                        partial,
                        &nt.name,
                        Self::newtype_constructor_params(nt),
                        Self::simple_type(nt.name.clone(), span),
                        span,
                    ));
                }
                _ => {}
            }
        }
        Err(LoweringError {
            message: format!("Partial '{}' targets unknown callable '{}'", partial.name, target_name),
            span: span.into(),
        })
    }

    /// Build synthetic same-type method wrappers for method partial declarations.
    fn method_partial_wrappers(
        methods: &[ast::Spanned<ast::MethodDecl>],
        aliases: &[ast::Spanned<ast::MethodAliasDecl>],
        partials: &[ast::Spanned<ast::MethodPartialDecl>],
        span: ast::Span,
    ) -> Vec<ast::Spanned<ast::MethodDecl>> {
        let mut out = Vec::new();
        let aliases = Self::method_alias_rebindings(aliases);
        for partial in partials {
            let target_name = aliases
                .get(&partial.node.target)
                .map(String::as_str)
                .unwrap_or(partial.node.target.as_str());
            let Some(target) = methods
                .iter()
                .chain(out.iter())
                .find(|method| method.node.name == target_name)
            else {
                continue;
            };
            let presets = Self::partial_arg_map(&partial.node.args);
            let params = Self::partial_projected_params(&target.node.params, &presets);
            let receiver = ast::Spanned::new(ast::Expr::SelfExpr, span);
            let call = ast::Spanned::new(
                ast::Expr::MethodCall(
                    Box::new(receiver),
                    target.node.name.clone(),
                    Vec::new(),
                    Self::partial_forward_args(&params, span),
                ),
                span,
            );
            out.push(ast::Spanned::new(
                ast::MethodDecl {
                    decorators: Vec::new(),
                    surface_modifiers: target.node.surface_modifiers.clone(),
                    name: partial.node.name.clone(),
                    type_params: target.node.type_params.clone(),
                    trait_target: target.node.trait_target.clone(),
                    receiver: target.node.receiver,
                    params,
                    return_type: target.node.return_type.clone(),
                    body: Some(vec![ast::Spanned::new(ast::Statement::Return(Some(call)), span)]),
                },
                partial.span,
            ));
        }
        out
    }

    /// Return authored methods plus synthetic method partial wrappers in declaration order.
    fn methods_with_partials(
        methods: &[ast::Spanned<ast::MethodDecl>],
        aliases: &[ast::Spanned<ast::MethodAliasDecl>],
        partials: &[ast::Spanned<ast::MethodPartialDecl>],
        span: ast::Span,
    ) -> Vec<ast::Spanned<ast::MethodDecl>> {
        let mut all = methods.to_vec();
        all.extend(Self::method_partial_wrappers(methods, aliases, partials, span));
        all
    }

    /// Create a lowering context that uses typechecker output for more accurate lowering.
    pub fn new_with_type_info(type_info: TypeCheckInfo) -> Self {
        let mut s = Self::new();
        s.type_info = Some(type_info);
        s
    }

    /// Seed trait declarations from imported source modules so RFC 024 default methods can be expanded into adopter
    /// impls.
    pub fn seed_dependency_trait_decls(&mut self, dependency_modules: &[(&str, &ast::Program, Option<Vec<String>>)]) {
        for (module_name, module_ast, path_segments) in dependency_modules {
            let mut module_keys = vec![(*module_name).to_string()];
            if let Some(path_segments) = path_segments {
                let dotted = path_segments.join(".");
                if !module_keys.iter().any(|key| key == &dotted) {
                    module_keys.push(dotted);
                }
            }
            for decl in &module_ast.declarations {
                let ast::Declaration::Trait(tr) = &decl.node else {
                    continue;
                };
                let mut trait_decl = tr.clone();
                trait_decl.methods =
                    Self::methods_with_partials(&tr.methods, &tr.method_aliases, &tr.method_partials, decl.span);
                for module_key in &module_keys {
                    self.trait_decls
                        .insert(format!("{module_key}.{}", tr.name), trait_decl.clone());
                }
            }
        }
    }

    /// Seed alias maps for types that may be referenced from other modules.
    ///
    /// This is used by multi-file codegen so alias-aware lowering works when a module references a `model` defined in
    /// a different module (e.g. `a.type` or `Account(type="x")`).
    pub fn seed_struct_field_aliases(&mut self, aliases: HashMap<String, HashMap<String, String>>) {
        for (struct_name, map) in aliases {
            self.struct_field_aliases.entry(struct_name).or_default().extend(map);
        }
    }

    /// Record one identifier read and report whether this was the last read in the current statement block.
    pub(super) fn consume_ident_read(&mut self, name: &str) -> bool {
        if self.remaining_ident_reads.is_empty() {
            return false;
        }

        // Keep parent block counters in sync with nested-block reads: counters are precomputed per block and include
        // nested reads.
        let last_idx = self.remaining_ident_reads.len() - 1;
        let mut is_last_in_current_block = false;
        for (idx, reads) in self.remaining_ident_reads.iter_mut().enumerate() {
            if let Some(remaining) = reads.get_mut(name) {
                if *remaining > 0 {
                    *remaining -= 1;
                }
                if idx == last_idx {
                    is_last_in_current_block = *remaining == 0;
                }
            }
        }
        is_last_in_current_block
    }

    /// Choose variable access mode for an identifier read.
    ///
    /// This implements a local #121-style heuristic:
    /// - copy types stay `Copy`,
    /// - mutable/non-linear/non-tracked reads stay non-consuming (`Read`),
    /// - immutable last reads in straight-line blocks become `Move`.
    pub(super) fn select_var_access_for_ident(&mut self, name: &str, ty: &IrType) -> VarAccess {
        if ty.is_copy() {
            return VarAccess::Copy;
        }

        let has_tracking = !self.remaining_ident_reads.is_empty();
        if !has_tracking {
            // Outside statement-block tracking (e.g. some declaration lowering), keep the historical move-default
            // behavior.
            return VarAccess::Move;
        }

        // Keep counters in sync even when we intentionally disable moves.
        let is_last_use_here = self.consume_ident_read(name);

        let is_mutable = self.mutable_vars.get(name).copied().unwrap_or(false);
        if self.non_linear_context_depth > 0 || is_mutable || !is_last_use_here {
            return VarAccess::Read;
        }

        // In nested blocks, only move when every tracked parent block also sees no future reads for this binding.
        if self.remaining_ident_reads.len() > 1 {
            let has_future_parent_read = self
                .remaining_ident_reads
                .iter()
                .take(self.remaining_ident_reads.len() - 1)
                .any(|reads| reads.get(name).is_some_and(|remaining| *remaining > 0));
            if has_future_parent_read {
                return VarAccess::Read;
            }
        }

        VarAccess::Move
    }

    /// Enter a nested lowering scope for locals, live static bindings, and local callable signatures.
    pub(super) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.static_binding_scopes.push(HashSet::new());
        self.local_callable_signature_scopes.push(HashMap::new());
    }

    /// Leave the current lowering scope and discard scoped local binding metadata.
    pub(super) fn pop_scope(&mut self) {
        let _ = self.scopes.pop();
        let _ = self.static_binding_scopes.pop();
        let _ = self.local_callable_signature_scopes.pop();
    }

    pub(super) fn define_local_binding(&mut self, name: String, ty: IrType, is_static_binding: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.clone(), ty);
        }
        if is_static_binding && let Some(scope) = self.static_binding_scopes.last_mut() {
            scope.insert(name);
        }
    }

    /// Define or shadow the callable signature associated with a local binding in the current scope.
    pub(super) fn define_local_callable_signature(&mut self, name: String, signature: Option<FunctionSignature>) {
        if let Some(scope) = self.local_callable_signature_scopes.last_mut() {
            scope.insert(name, signature);
        }
    }

    /// Update the nearest existing local binding's callable signature after reassignment.
    pub(super) fn update_local_callable_signature(&mut self, name: &str, signature: Option<FunctionSignature>) {
        if let Some(index) = self.scopes.iter().rposition(|scope| scope.contains_key(name))
            && let Some(scope) = self.local_callable_signature_scopes.get_mut(index)
        {
            scope.insert(name.to_string(), signature);
        }
    }

    /// Look up the callable signature associated with a local binding, respecting shadowing.
    pub(super) fn lookup_local_callable_signature(&self, name: &str) -> Option<FunctionSignature> {
        self.local_callable_signature_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).map(|signature| signature.as_ref().cloned()))?
    }

    /// Return the callable signature recorded while lowering a local partial expression.
    pub(super) fn partial_expr_signature_for_span(&self, span: ast::Span) -> Option<FunctionSignature> {
        self.partial_expr_signatures.get(&(span.start, span.end)).cloned()
    }

    /// Rehydrate a local partial expression into an IR callable signature so default values survive calls through
    /// function-typed locals.
    pub(super) fn partial_expr_callable_signature(
        &mut self,
        partial: &ast::PartialExpr,
        span: ast::Span,
    ) -> Result<Option<FunctionSignature>, LoweringError> {
        let Some(crate::frontend::symbols::ResolvedType::Function(params, ret)) =
            self.type_info.as_ref().and_then(|info| info.expr_type(span).cloned())
        else {
            return Ok(None);
        };

        let mut defaults = HashMap::new();
        for arg in &partial.args {
            defaults.insert(arg.name.clone(), self.lower_expr_spanned(&arg.value)?);
        }

        let signature = FunctionSignature {
            params: params
                .iter()
                .enumerate()
                .map(|(idx, param)| {
                    let base_ty = self.lower_resolved_type(&param.ty);
                    let ty = Self::lower_param_container_type(param.kind, base_ty);
                    FunctionParam {
                        name: param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                        ty,
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: param.kind,
                        default: param.name.as_ref().and_then(|name| defaults.get(name).cloned()),
                    }
                })
                .collect(),
            return_type: self.lower_resolved_type(ret.as_ref()),
        };
        self.partial_expr_signatures
            .insert((span.start, span.end), signature.clone());
        Ok(Some(signature))
    }

    /// Replace the nearest local binding type for `name` after lowering refines it.
    pub(super) fn update_local_binding(&mut self, name: &str, ty: IrType) {
        if let Some(scope) = self.scopes.iter_mut().rev().find(|scope| scope.contains_key(name)) {
            scope.insert(name.to_string(), ty);
        }
    }

    /// Track function-typed parameters for the callable body currently being lowered.
    pub(super) fn push_callable_param_scope(&mut self, params: &[FunctionParam]) {
        self.callable_param_scopes.push(
            params
                .iter()
                .filter(|param| matches!(param.ty, IrType::Function { .. }))
                .map(|param| param.name.clone())
                .collect(),
        );
    }

    /// Drop the current callable parameter tracking scope.
    pub(super) fn pop_callable_param_scope(&mut self) {
        let _ = self.callable_param_scopes.pop();
    }

    /// Return whether the current callable body has a function-typed parameter named `name`.
    pub(super) fn current_callable_param_scope_contains(&self, name: &str) -> bool {
        self.callable_param_scopes
            .last()
            .is_some_and(|params| params.contains(name))
    }

    /// Refresh the root-scope function binding after lowering has refined the function signature.
    fn update_root_function_binding(&mut self, name: &str, params: &[FunctionParam], return_type: &IrType) {
        if let Some(scope) = self.scopes.first_mut() {
            scope.insert(
                name.to_string(),
                IrType::Function {
                    params: params.iter().map(|param| param.ty.clone()).collect(),
                    ret: Box::new(return_type.clone()),
                },
            );
        }
    }

    /// Return whether `name` resolves to a source-level static binding in an active scope.
    pub(super) fn is_static_binding(&self, name: &str) -> bool {
        self.static_binding_scopes
            .iter()
            .rev()
            .any(|scope| scope.contains(name))
    }

    pub(super) fn is_direct_static_ident(&self, expr: &ast::Spanned<ast::Expr>) -> Option<String> {
        let ast::Expr::Ident(name) = &expr.node else {
            return None;
        };

        self.type_info
            .as_ref()
            .and_then(|info| info.ident_kind(expr.span))
            .filter(|kind| matches!(kind, crate::frontend::typechecker::IdentKind::Static))
            .map(|_| name.clone())
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

    /// Extract a method name from a rebinding target expression.
    ///
    /// Supports:
    /// - `alias = method_name`
    /// - `alias = TypeOrValue.method_name`
    fn rebinding_target_method_name(target: &ast::Expr) -> Option<String> {
        match target {
            ast::Expr::Ident(name) => Some(name.clone()),
            ast::Expr::Field(_, member) => Some(member.clone()),
            _ => None,
        }
    }

    /// Resolve a method name through per-type rebinding aliases.
    pub(super) fn resolve_method_rebinding(&self, receiver_ty: &IrType, method_name: &str) -> String {
        let Some(type_name) = receiver_ty.nominal_type_name() else {
            return method_name.to_string();
        };
        self.type_method_rebindings
            .get(type_name)
            .and_then(|aliases| aliases.get(method_name))
            .cloned()
            .unwrap_or_else(|| method_name.to_string())
    }

    /// Convert parsed same-type method aliases into lowering-time alias-to-target maps.
    fn method_alias_rebindings(aliases: &[ast::Spanned<ast::MethodAliasDecl>]) -> HashMap<String, String> {
        aliases
            .iter()
            .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
            .collect()
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
        self.import_aliases = decorator_resolution::collect_import_aliases(program);
        self.rust_import_aliases = decorator_resolution::collect_rust_import_aliases(program);
        self.seed_imported_stdlib_trait_decls(program);
        self.alias_imported_dependency_trait_decls();
        self.symbol_aliases = program
            .declarations
            .iter()
            .filter_map(|decl| {
                let ast::Declaration::Alias(alias) = &decl.node else {
                    return None;
                };
                let [target] = alias.target.segments.as_slice() else {
                    return None;
                };
                Some((alias.name.clone(), target.clone()))
            })
            .collect();

        // RFC 023: propagate rust.module() path from AST to IR.
        ir_program.rust_module_path = program.rust_module_path.as_ref().map(|sp| sp.node.clone());
        // Seed alias maps for imported model aliases before lowering expressions.
        self.register_imported_struct_aliases(program);

        // First pass: collect class declarations, trait decls, and newtype ctor selection.
        for decl in &program.declarations {
            if let ast::Declaration::Class(ref c) = decl.node {
                let mut class_decl = c.clone();
                class_decl.methods =
                    Self::methods_with_partials(&c.methods, &c.method_aliases, &c.method_partials, decl.span);
                self.class_decls.insert(c.name.clone(), class_decl);
            }
            if let ast::Declaration::Trait(ref t) = decl.node {
                let trait_methods =
                    Self::methods_with_partials(&t.methods, &t.method_aliases, &t.method_partials, decl.span);
                let method_names: Vec<String> = trait_methods.iter().map(|m| m.node.name.clone()).collect();
                self.trait_methods.insert(t.name.clone(), method_names);
                let mut trait_decl = t.clone();
                trait_decl.methods = trait_methods;
                self.trait_decls.insert(t.name.clone(), trait_decl);
                let aliases = Self::method_alias_rebindings(&t.method_aliases);
                if !aliases.is_empty() {
                    self.type_method_rebindings.insert(t.name.clone(), aliases);
                }
            }
            if let ast::Declaration::Model(ref m) = decl.node {
                if m.traits
                    .iter()
                    .any(|bound| bound.node.name == core_traits::as_str(TraitId::Iterator))
                {
                    self.iterator_adopter_names.insert(m.name.clone());
                }
                let aliases = Self::method_alias_rebindings(&m.method_aliases);
                if !aliases.is_empty() {
                    self.type_method_rebindings.insert(m.name.clone(), aliases);
                }
            }
            if let ast::Declaration::Class(ref c) = decl.node {
                if c.traits
                    .iter()
                    .any(|bound| bound.node.name == core_traits::as_str(TraitId::Iterator))
                {
                    self.iterator_adopter_names.insert(c.name.clone());
                }
                let aliases = Self::method_alias_rebindings(&c.method_aliases);
                if !aliases.is_empty() {
                    self.type_method_rebindings.insert(c.name.clone(), aliases);
                }
            }
            if let ast::Declaration::Newtype(ref n) = decl.node {
                let rebindings: HashMap<String, String> = n
                    .rebindings
                    .iter()
                    .filter_map(|rebinding| {
                        Self::rebinding_target_method_name(&rebinding.node.target.node)
                            .map(|target| (rebinding.node.name.clone(), target))
                    })
                    .collect();
                let mut rebindings = rebindings;
                rebindings.extend(Self::method_alias_rebindings(&n.method_aliases));
                if !rebindings.is_empty() {
                    self.type_method_rebindings.insert(n.name.clone(), rebindings);
                }
                if n.is_rusttype {
                    let ir_underlying = self
                        .type_info
                        .as_ref()
                        .and_then(|ti| ti.rust.rusttype_canonical_paths.get(&n.name))
                        .cloned()
                        .map(IrType::Struct)
                        .unwrap_or_else(|| self.lower_type(&n.underlying.node));
                    self.rusttype_underlying.insert(n.name.clone(), ir_underlying);
                    self.rusttype_interop_edges.insert(
                        n.name.clone(),
                        n.interop_edges.iter().map(|edge| edge.node.clone()).collect(),
                    );
                }
                if n.is_rusttype {
                    continue;
                }
                // Track validation hook selection for checked construction lowering.
                if let Some(ctor) = Self::select_newtype_checked_ctor(n) {
                    self.newtype_checked_ctor.insert(n.name.clone(), ctor);
                } else {
                    let constraints = Self::newtype_constraints_from_ast(&n.underlying.node);
                    if !constraints.is_empty() {
                        self.newtype_constraints.insert(n.name.clone(), constraints);
                    }
                }
            }
        }
        ir_program.newtype_checked_ctor = self.newtype_checked_ctor.clone();

        // Pass 1.5: register module-level const names into the root scope for lookups.
        // (Type inference/refinement happens later; Unknown is fine for non-const contexts.)
        for decl in &program.declarations {
            if let ast::Declaration::Const(ref c) = decl.node {
                if c.name == "__derives__" {
                    continue;
                }
                let ty = if let Some(ann) = &c.ty {
                    self.lower_const_annotation_type(&ann.node)
                } else {
                    IrType::Unknown
                };
                if let Some(scope) = self.scopes.first_mut() {
                    scope.insert(c.name.clone(), ty);
                }
            } else if let ast::Declaration::Static(ref s) = decl.node {
                let ty = self.lower_type(&s.ty.node);
                if let Some(scope) = self.scopes.first_mut() {
                    scope.insert(s.name.clone(), ty);
                }
            }
        }

        // Second pass: collect all function signatures
        for decl in &program.declarations {
            if let ast::Declaration::Function(ref f) = decl.node {
                let type_param_names: std::collections::HashSet<&str> =
                    f.type_params.iter().map(|tp| tp.name.as_str()).collect();
                let params: Vec<FunctionParam> = f
                    .params
                    .iter()
                    .map(|p| {
                        let base_ty = self.lower_type_with_type_params(&p.node.ty.node, Some(&type_param_names));
                        let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                        FunctionParam {
                            name: p.node.name.clone(),
                            ty: param_ty,
                            mutability: if p.node.is_mut {
                                Mutability::Mutable
                            } else {
                                Mutability::Immutable
                            },
                            is_self: false,
                            kind: p.node.kind,
                            default: match &p.node.default {
                                Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                                None => None,
                            },
                        }
                    })
                    .collect();
                let return_type = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.declarations.decorated_function_bindings.get(&f.name))
                    .and_then(|binding| match &binding.ty {
                        crate::frontend::symbols::ResolvedType::Function(_, ret) => Some(self.lower_resolved_type(ret)),
                        _ => None,
                    })
                    .unwrap_or_else(|| self.lower_type_with_type_params(&f.return_type.node, Some(&type_param_names)));
                ir_program
                    .function_registry
                    .register(f.name.clone(), params.clone(), return_type.clone());
                if let Some(signature) = ir_program.function_registry.get(&f.name).cloned() {
                    self.update_root_function_binding(&f.name, &signature.params, &signature.return_type);
                }
                if self
                    .type_info
                    .as_ref()
                    .is_some_and(|info| info.declarations.decorated_function_bindings.contains_key(&f.name))
                {
                    let original_name = Self::decorator_original_function_name(&f.name);
                    let original_return_type =
                        self.lower_type_with_type_params(&f.return_type.node, Some(&type_param_names));
                    ir_program
                        .function_registry
                        .register(original_name, params, original_return_type);
                }
            } else if let ast::Declaration::Alias(ref alias) = decl.node
                && let [target] = alias.target.segments.as_slice()
                && let Some(signature) = ir_program.function_registry.get(target).cloned()
            {
                ir_program
                    .function_registry
                    .register(alias.name.clone(), signature.params, signature.return_type);
            } else if let ast::Declaration::Partial(ref partial) = decl.node {
                match self.partial_wrapper_function(program, partial, decl.span) {
                    Ok(wrapper) => {
                        let type_param_names: std::collections::HashSet<&str> =
                            wrapper.type_params.iter().map(|tp| tp.name.as_str()).collect();
                        let params: Vec<FunctionParam> = wrapper
                            .params
                            .iter()
                            .map(|p| {
                                let base_ty =
                                    self.lower_type_with_type_params(&p.node.ty.node, Some(&type_param_names));
                                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                                FunctionParam {
                                    name: p.node.name.clone(),
                                    ty: param_ty,
                                    mutability: if p.node.is_mut {
                                        Mutability::Mutable
                                    } else {
                                        Mutability::Immutable
                                    },
                                    is_self: false,
                                    kind: p.node.kind,
                                    default: p
                                        .node
                                        .default
                                        .as_ref()
                                        .and_then(|default_expr| self.lower_expr_spanned(default_expr).ok()),
                                }
                            })
                            .collect();
                        let return_type =
                            self.lower_type_with_type_params(&wrapper.return_type.node, Some(&type_param_names));
                        ir_program.function_registry.register(
                            wrapper.name.clone(),
                            params.clone(),
                            return_type.clone(),
                        );
                        self.update_root_function_binding(&wrapper.name, &params, &return_type);
                    }
                    Err(e) => errors.push(e),
                }
            }
        }

        // Third pass: lower declarations
        for decl in &program.declarations {
            // Handle models - generate both struct and impl
            // Models always get impl blocks (for serde methods even if no user methods)
            match &decl.node {
                ast::Declaration::Model(m) => {
                    let model_methods =
                        Self::methods_with_partials(&m.methods, &m.method_aliases, &m.method_partials, decl.span);
                    // Generate struct
                    match self.lower_model(m) {
                        Ok(struct_ir) => {
                            self.struct_names
                                .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                            ir_program
                                .declarations
                                .push(IrDecl::new(IrDeclKind::Struct(struct_ir.clone())));
                            match self.lower_decorated_method_statics(&struct_ir.name, &model_methods) {
                                Ok(statics) => ir_program.declarations.extend(statics),
                                Err(e) => errors.push(e),
                            }

                            // Generate impl block (may be empty if no methods, serde methods added during emission)
                            match self.lower_model_methods(
                                &struct_ir.name,
                                &m.type_params,
                                &model_methods,
                                &m.properties,
                                &m.traits,
                            ) {
                                Ok(impl_ir) => {
                                    ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                }
                                Err(e) => errors.push(e),
                            }

                            // Generate trait impls for each trait this model implements
                            for trait_ref in &m.traits {
                                for (trait_name, trait_type_args) in
                                    self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, &m.type_params)
                                {
                                    match self.lower_trait_impl(TraitImplLoweringInput {
                                        type_name: &struct_ir.name,
                                        type_params: &m.type_params,
                                        trait_name: &trait_name,
                                        trait_type_args,
                                        impl_methods: &model_methods,
                                        impl_properties: &m.properties,
                                        impl_associated_types: &[],
                                    }) {
                                        Ok(trait_impl) => {
                                            ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                        }
                                        Err(e) => errors.push(e),
                                    }
                                }
                            }
                            for (trait_name, trait_type_args) in self.derive_trait_impl_targets(&m.decorators) {
                                match self.lower_trait_impl(TraitImplLoweringInput {
                                    type_name: &struct_ir.name,
                                    type_params: &m.type_params,
                                    trait_name: &trait_name,
                                    trait_type_args,
                                    impl_methods: &model_methods,
                                    impl_properties: &m.properties,
                                    impl_associated_types: &[],
                                }) {
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
                ast::Declaration::Docstring(_) | ast::Declaration::TestModule(_) => {
                    // Module-level docstrings and inline test modules are not part of production IR.
                    continue;
                }
                ast::Declaration::Const(c) if c.name == "__derives__" => {
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
                            let mut all_properties = Vec::new();
                            if let Err(e) = self.collect_inherited_properties(&c.name, &mut all_properties) {
                                errors.push(e);
                            }

                            // Generate impl block for all methods/properties (inherited + own)
                            if !all_methods.is_empty() || !all_properties.is_empty() {
                                match self.lower_decorated_method_statics(&struct_ir.name, &all_methods) {
                                    Ok(statics) => ir_program.declarations.extend(statics),
                                    Err(e) => errors.push(e),
                                }
                                match self.lower_class_methods(
                                    &struct_ir.name,
                                    &c.type_params,
                                    &all_methods,
                                    &all_properties,
                                    &c.traits,
                                ) {
                                    Ok(impl_ir) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }

                            // Generate trait impls for each trait this class implements
                            for trait_ref in &c.traits {
                                for (trait_name, trait_type_args) in
                                    self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, &c.type_params)
                                {
                                    match self.lower_trait_impl(TraitImplLoweringInput {
                                        type_name: &struct_ir.name,
                                        type_params: &c.type_params,
                                        trait_name: &trait_name,
                                        trait_type_args,
                                        impl_methods: &all_methods,
                                        impl_properties: &all_properties,
                                        impl_associated_types: &[],
                                    }) {
                                        Ok(trait_impl) => {
                                            ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                        }
                                        Err(e) => errors.push(e),
                                    }
                                }
                            }
                            for (trait_name, trait_type_args) in self.derive_trait_impl_targets(&c.decorators) {
                                match self.lower_trait_impl(TraitImplLoweringInput {
                                    type_name: &struct_ir.name,
                                    type_params: &c.type_params,
                                    trait_name: &trait_name,
                                    trait_type_args,
                                    impl_methods: &all_methods,
                                    impl_properties: &all_properties,
                                    impl_associated_types: &[],
                                }) {
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
                    let newtype_methods =
                        Self::methods_with_partials(&n.methods, &n.method_aliases, &n.method_partials, decl.span);
                    if n.is_rusttype {
                        match self.lower_declaration(&ast::Declaration::Newtype(n.clone())) {
                            Ok(ir_decl) => {
                                ir_program.declarations.push(ir_decl);
                            }
                            Err(e) => errors.push(e),
                        }
                        for trait_ref in &n.traits {
                            if self.rusttype_forwarding_satisfied_by_alias(&n.name, &trait_ref.node.name) {
                                continue;
                            }
                            for (trait_name, trait_type_args) in
                                self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, &n.type_params)
                            {
                                match self.lower_trait_impl(TraitImplLoweringInput {
                                    type_name: &n.name,
                                    type_params: &n.type_params,
                                    trait_name: &trait_name,
                                    trait_type_args,
                                    impl_methods: &n.methods,
                                    impl_properties: &[],
                                    impl_associated_types: &n.associated_types,
                                }) {
                                    Ok(trait_impl) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }
                        }
                        continue;
                    }
                    // Generate struct
                    match self.lower_newtype(n) {
                        Ok(struct_ir) => {
                            self.struct_names
                                .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                            ir_program
                                .declarations
                                .push(IrDecl::new(IrDeclKind::Struct(struct_ir.clone())));

                            // Generate impl block for newtype methods (if any).
                            if !newtype_methods.is_empty() {
                                match self.lower_decorated_method_statics(&struct_ir.name, &newtype_methods) {
                                    Ok(statics) => ir_program.declarations.extend(statics),
                                    Err(e) => errors.push(e),
                                }
                                match self.lower_model_methods(
                                    &struct_ir.name,
                                    &n.type_params,
                                    &newtype_methods,
                                    &[],
                                    &n.traits,
                                ) {
                                    Ok(impl_ir) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }
                            for trait_ref in &n.traits {
                                for (trait_name, trait_type_args) in
                                    self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, &n.type_params)
                                {
                                    match self.lower_trait_impl(TraitImplLoweringInput {
                                        type_name: &struct_ir.name,
                                        type_params: &n.type_params,
                                        trait_name: &trait_name,
                                        trait_type_args,
                                        impl_methods: &n.methods,
                                        impl_properties: &[],
                                        impl_associated_types: &n.associated_types,
                                    }) {
                                        Ok(trait_impl) => {
                                            ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                        }
                                        Err(e) => errors.push(e),
                                    }
                                }
                            }
                        }
                        Err(e) => errors.push(e),
                    }
                }
                ast::Declaration::Enum(e) => match self.lower_enum(e) {
                    Ok(enum_ir) => {
                        self.enum_names
                            .insert(enum_ir.name.clone(), IrType::Enum(enum_ir.name.clone()));
                        ir_program
                            .declarations
                            .push(IrDecl::new(IrDeclKind::Enum(enum_ir.clone())));

                        if !e.methods.is_empty() {
                            match self.lower_decorated_method_statics(&enum_ir.name, &e.methods) {
                                Ok(statics) => ir_program.declarations.extend(statics),
                                Err(e) => errors.push(e),
                            }
                            match self.lower_enum_methods(&enum_ir.name, &e.type_params, &e.methods, &e.traits) {
                                Ok(impl_ir) => {
                                    ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(impl_ir)));
                                }
                                Err(e) => errors.push(e),
                            }
                        }

                        for trait_ref in &e.traits {
                            for (trait_name, trait_type_args) in
                                self.trait_impl_targets_for_adopted_trait_bound(&trait_ref.node, &e.type_params)
                            {
                                match self.lower_trait_impl(TraitImplLoweringInput {
                                    type_name: &enum_ir.name,
                                    type_params: &e.type_params,
                                    trait_name: &trait_name,
                                    trait_type_args,
                                    impl_methods: &e.methods,
                                    impl_properties: &[],
                                    impl_associated_types: &[],
                                }) {
                                    Ok(trait_impl) => {
                                        ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                    }
                                    Err(e) => errors.push(e),
                                }
                            }
                        }
                        for (trait_name, trait_type_args) in self.derive_trait_impl_targets(&e.decorators) {
                            match self.lower_trait_impl(TraitImplLoweringInput {
                                type_name: &enum_ir.name,
                                type_params: &e.type_params,
                                trait_name: &trait_name,
                                trait_type_args,
                                impl_methods: &e.methods,
                                impl_properties: &[],
                                impl_associated_types: &[],
                            }) {
                                Ok(trait_impl) => {
                                    ir_program.declarations.push(IrDecl::new(IrDeclKind::Impl(trait_impl)));
                                }
                                Err(e) => errors.push(e),
                            }
                        }
                    }
                    Err(e) => errors.push(e),
                },
                ast::Declaration::Function(f) => match self.lower_decorated_function_declarations(f) {
                    Ok(decls) => {
                        if f.name == conventions::ENTRYPOINT_NAME {
                            ir_program.entry_point = Some(conventions::ENTRYPOINT_NAME.to_string());
                        }
                        for decl in &decls {
                            if let IrDeclKind::Function(func) = &decl.kind {
                                ir_program.function_registry.register(
                                    func.name.clone(),
                                    func.params.clone(),
                                    func.return_type.clone(),
                                );
                                self.update_root_function_binding(&func.name, &func.params, &func.return_type);
                            }
                        }
                        ir_program.declarations.extend(decls);
                    }
                    Err(e) => errors.push(e),
                },
                ast::Declaration::Partial(partial) => {
                    match self.partial_wrapper_function(program, partial, decl.span) {
                        Ok(wrapper) => match self.lower_declaration(&ast::Declaration::Function(wrapper)) {
                            Ok(ir_decl) => {
                                if let IrDeclKind::Function(ref func) = ir_decl.kind {
                                    ir_program.function_registry.register(
                                        func.name.clone(),
                                        func.params.clone(),
                                        func.return_type.clone(),
                                    );
                                    self.update_root_function_binding(&func.name, &func.params, &func.return_type);
                                }
                                ir_program.declarations.push(ir_decl);
                            }
                            Err(e) => errors.push(e),
                        },
                        Err(e) => errors.push(e),
                    }
                }
                _ => {
                    // Regular declaration lowering
                    match self.lower_declaration(&decl.node) {
                        Ok(ir_decl) => {
                            if let IrDeclKind::Function(ref func) = ir_decl.kind
                                && func.name == conventions::ENTRYPOINT_NAME
                            {
                                ir_program.entry_point = Some(conventions::ENTRYPOINT_NAME.to_string());
                            }
                            if let IrDeclKind::Function(ref func) = ir_decl.kind {
                                ir_program.function_registry.register(
                                    func.name.clone(),
                                    func.params.clone(),
                                    func.return_type.clone(),
                                );
                                self.update_root_function_binding(&func.name, &func.params, &func.return_type);
                            }
                            ir_program.declarations.push(ir_decl);
                        }
                        Err(e) => errors.push(e),
                    }
                }
            }
        }
        // Propagate serde derives from structs to their field types (enums). This allows users to only annotate the
        // top-level model with @derive(json) and have it automatically apply to nested user-defined enums.
        Self::propagate_serde_derives(&mut ir_program);

        if errors.is_empty() {
            Ok(ir_program)
        } else {
            // Return all collected errors
            Err(LoweringErrors(errors))
        }
    }

    /// Lower a function declaration, expanding RFC 036 decorated functions into original/static/wrapper items.
    fn lower_decorated_function_declarations(&mut self, f: &ast::FunctionDecl) -> Result<Vec<IrDecl>, LoweringError> {
        let Some(binding) = self
            .type_info
            .as_ref()
            .and_then(|info| info.declarations.decorated_function_bindings.get(&f.name).cloned())
        else {
            return Ok(vec![self.lower_declaration(&ast::Declaration::Function(f.clone()))?]);
        };
        let crate::frontend::symbols::ResolvedType::Function(callable_params, callable_ret) = binding.ty else {
            return Err(LoweringError {
                message: format!(
                    "decorated function '{}' lowers only when the decorated binding remains callable",
                    f.name
                ),
                span: ast::Span::default().into(),
            });
        };

        let original_name = Self::decorator_original_function_name(&f.name);
        let original = self.lower_function_named(f, original_name.clone(), super::decl::Visibility::Private)?;
        let decorated_ty = IrType::Function {
            params: callable_params
                .iter()
                .map(|param| self.lower_resolved_type(&param.ty))
                .collect(),
            ret: Box::new(self.lower_resolved_type(&callable_ret)),
        };

        let decorator_expr = self.decorator_application_expr(&f.name, &f.decorators)?;
        let mut value = self.lower_expr_spanned(&decorator_expr)?;
        value.ty = decorated_ty.clone();
        let static_name = Self::decorator_static_binding_name(&f.name);
        let wrapper = self.decorated_function_wrapper(f, &static_name, &callable_params, callable_ret.as_ref());

        Ok(vec![
            IrDecl::new(IrDeclKind::Function(original)),
            IrDecl::new(IrDeclKind::Static {
                visibility: super::decl::Visibility::Private,
                name: static_name,
                ty: decorated_ty,
                value,
            }),
            IrDecl::new(IrDeclKind::Function(wrapper)),
        ])
    }

    /// Lower the public function wrapper that dispatches through the decorated callable static.
    fn decorated_function_wrapper(
        &mut self,
        f: &ast::FunctionDecl,
        static_name: &str,
        callable_params: &[crate::frontend::symbols::CallableParam],
        callable_ret: &crate::frontend::symbols::ResolvedType,
    ) -> super::decl::IrFunction {
        let params: Vec<FunctionParam> = callable_params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let base_ty = self.lower_resolved_type(&param.ty);
                FunctionParam {
                    name: param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                    ty: Self::lower_param_container_type(param.kind, base_ty),
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: param.kind,
                    default: None,
                }
            })
            .collect();
        let return_type = self.lower_resolved_type(callable_ret);
        let static_func = TypedExpr::new(
            IrExprKind::StaticRead {
                name: static_name.to_string(),
            },
            IrType::Function {
                params: params.iter().map(|param| param.ty.clone()).collect(),
                ret: Box::new(return_type.clone()),
            },
        );
        let args = params
            .iter()
            .map(|param| IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: TypedExpr::new(
                    IrExprKind::Var {
                        name: param.name.clone(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    param.ty.clone(),
                ),
            })
            .collect();
        let call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(static_func),
                type_args: Vec::new(),
                args,
                callable_signature: None,
                canonical_path: None,
            },
            return_type.clone(),
        );

        super::decl::IrFunction {
            name: f.name.clone(),
            params,
            return_type,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(call)))],
            is_async: f.is_async(),
            is_generator: false,
            visibility: Self::map_visibility(f.visibility),
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        }
    }

    /// Add alias-qualified dependency trait declarations so default methods can expand for imported derive aliases.
    fn alias_imported_dependency_trait_decls(&mut self) {
        let existing = self.trait_decls.clone();
        for (alias, path) in self.import_aliases.clone() {
            let mut canonical_path = crate::frontend::module::canonicalize_source_module_segments(&path);
            if canonical_path
                .first()
                .is_some_and(|segment| segment == stdlib::STDLIB_ROOT)
            {
                canonical_path[0] = stdlib::INCAN_STD_NAMESPACE.to_string();
            }
            let module_key = canonical_path.join(".");
            if let Some(decl) = existing
                .get(&module_key)
                .filter(|decl| Self::trait_decl_has_lowerable_defaults(decl))
            {
                self.trait_decls.entry(alias.clone()).or_insert_with(|| decl.clone());
            }
            let prefix = format!("{module_key}.");
            for (qualified, decl) in &existing {
                let Some(trait_name) = qualified.strip_prefix(&prefix) else {
                    continue;
                };
                if !Self::trait_decl_has_lowerable_defaults(decl) {
                    continue;
                }
                self.trait_decls
                    .entry(format!("{alias}.{trait_name}"))
                    .or_insert_with(|| decl.clone());
            }
        }
    }

    /// Seed trait declarations imported from stdlib modules.
    ///
    /// Lowering needs the source trait body to decide which methods belong in generated `impl Trait for Type` blocks.
    /// The typechecker already validates the import; this pass follows the same stdlib namespace graph so imported
    /// traits such as `std.io.BinaryReader` lower without hardcoded method lists.
    fn seed_imported_stdlib_trait_decls(&mut self, program: &ast::Program) {
        for decl in &program.declarations {
            let ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            let ast::ImportKind::From { module, items } = &import.kind else {
                continue;
            };
            if module.segments.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT) {
                continue;
            }

            for item in items {
                let Some(mut trait_decl) = self.stdlib_cache.lookup_trait_decl(&module.segments, &item.name) else {
                    continue;
                };
                let local_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                trait_decl.name = local_name.clone();
                trait_decl.methods = Self::methods_with_partials(
                    &trait_decl.methods,
                    &trait_decl.method_aliases,
                    &trait_decl.method_partials,
                    decl.span,
                );
                let method_names = trait_decl
                    .methods
                    .iter()
                    .map(|method| method.node.name.clone())
                    .collect();
                self.trait_methods.entry(local_name.clone()).or_insert(method_names);
                self.trait_decls.entry(local_name).or_insert(trait_decl);
            }
        }
    }

    /// Return whether an imported trait declaration needs aliasing for default-body expansion.
    fn trait_decl_has_lowerable_defaults(decl: &ast::TraitDecl) -> bool {
        decl.methods.iter().any(|method| method.node.body.is_some())
    }

    /// Propagate serde Rust derives from structs to enum/newtype field types.
    ///
    /// When a struct has `serde::Serialize` or `serde::Deserialize` derives and contains fields of enum types, those
    /// enums also need the same derives for the generated Rust code to compile. This function automatically adds those
    /// derives to avoid requiring users to manually annotate every nested enum.
    fn propagate_serde_derives(ir_program: &mut IrProgram) {
        use super::decl::IrDeclKind;

        const SERDE_SERIALIZE_DERIVE: &str = "serde::Serialize";
        const SERDE_DESERIALIZE_DERIVE: &str = "serde::Deserialize";

        // Collect enum/newtype names that need serde derives.
        let mut enums_need_serialize: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut enums_need_deserialize: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut structs_need_serialize: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut structs_need_deserialize: std::collections::HashSet<String> = std::collections::HashSet::new();

        let mut newtype_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut enum_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for decl in &ir_program.declarations {
            if let IrDeclKind::Struct(s) = &decl.kind
                && s.fields.len() == 1
                && s.fields[0].name == "0"
            {
                newtype_names.insert(s.name.clone());
            }
            if let IrDeclKind::Enum(e) = &decl.kind {
                enum_names.insert(e.name.clone());
            }
        }

        // First pass: find all structs with serde derives and collect their enum/newtype field types.
        for decl in &ir_program.declarations {
            if let IrDeclKind::Struct(s) = &decl.kind {
                let has_serialize = s.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE);
                let has_deserialize = s.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE);

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

        // Second pass: add serde derives to enums/newtypes that need them.
        for decl in &mut ir_program.declarations {
            if let IrDeclKind::Enum(e) = &mut decl.kind {
                if enums_need_serialize.contains(&e.name) && !e.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE) {
                    e.derives.push(SERDE_SERIALIZE_DERIVE.to_string());
                }
                if enums_need_deserialize.contains(&e.name) && !e.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE)
                {
                    e.derives.push(SERDE_DESERIALIZE_DERIVE.to_string());
                }
            }
            if let IrDeclKind::Struct(s) = &mut decl.kind
                && newtype_names.contains(&s.name)
            {
                if structs_need_serialize.contains(&s.name) && !s.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE) {
                    s.derives.push(SERDE_SERIALIZE_DERIVE.to_string());
                }
                if structs_need_deserialize.contains(&s.name)
                    && !s.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE)
                {
                    s.derives.push(SERDE_DESERIALIZE_DERIVE.to_string());
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
mod tests {
    use super::*;
    use crate::backend::ir::expr::{CollectionMethodKind, IrExprKind, MethodKind, StringMethodKind, UnaryOp};
    use crate::backend::ir::stmt::IrStmtKind;
    use crate::frontend::{lexer, parser};

    fn must_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

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
        let ir = must_ok(lower_source(
            r#"
def add(a: int, b: int) -> int:
    return a + b
"#,
        ));
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
        let ir = must_ok(lower_source(
            r#"
model User:
    name: str
    age: int
"#,
        ));
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
        let ir = must_ok(lower_source(
            r#"
def main() -> None:
    pass
"#,
        ));
        assert_eq!(ir.entry_point, Some("main".to_string()));
    }

    #[test]
    fn test_lower_rfc018_assert_is_some_binding_to_helper_let() -> Result<(), Box<dyn std::error::Error>> {
        let ir = must_ok(lower_source(
            r#"
import std.testing

def unwrap_value(value: Option[int]) -> int:
    assert value is Some(inner)
    return inner
"#,
        ));
        let function = ir
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "unwrap_value" => Some(function),
                _ => None,
            })
            .ok_or_else(|| std::io::Error::other("Expected unwrap_value function"))?;
        let first_stmt = function
            .body
            .first()
            .ok_or_else(|| std::io::Error::other("Expected assert lowering statement"))?;
        let IrStmtKind::Let { name, value, .. } = &first_stmt.kind else {
            return Err(std::io::Error::other("Expected assert Some binding to lower as let").into());
        };
        assert_eq!(name, "inner");
        let IrExprKind::Call {
            canonical_path: Some(path),
            ..
        } = &value.kind
        else {
            return Err(std::io::Error::other("Expected canonical assert_is_some helper call").into());
        };
        assert_eq!(
            path,
            &vec!["std".to_string(), "testing".to_string(), "assert_is_some".to_string()]
        );
        Ok(())
    }

    #[test]
    fn test_lower_rfc018_assert_raises_to_canonical_helper_call() -> Result<(), Box<dyn std::error::Error>> {
        let ir = must_ok(lower_source(
            r#"
def explode() -> None:
    pass

def check() -> None:
    assert explode() raises ValueError, "expected failure"
"#,
        ));
        let function = ir
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "check" => Some(function),
                _ => None,
            })
            .ok_or_else(|| std::io::Error::other("Expected check function"))?;
        let first_stmt = function
            .body
            .first()
            .ok_or_else(|| std::io::Error::other("Expected assert raises lowering statement"))?;
        let IrStmtKind::Expr(expr) = &first_stmt.kind else {
            return Err(std::io::Error::other("Expected assert raises to lower as an expression statement").into());
        };
        let IrExprKind::Call {
            canonical_path: Some(path),
            args,
            ..
        } = &expr.kind
        else {
            return Err(std::io::Error::other("Expected canonical assert_raises helper call").into());
        };
        assert_eq!(
            path,
            &vec!["std".to_string(), "testing".to_string(), "assert_raises".to_string()]
        );
        assert_eq!(args.len(), 3);
        Ok(())
    }

    #[test]
    fn test_lower_imported_assert_raises_injects_error_type_name() -> Result<(), Box<dyn std::error::Error>> {
        let ir = must_ok(lower_source(
            r#"
from std.testing import assert_raises

def explode() -> None:
    pass

def check() -> None:
    assert_raises[ValueError](explode, "expected failure")
"#,
        ));
        let function = ir
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "check" => Some(function),
                _ => None,
            })
            .ok_or_else(|| std::io::Error::other("Expected check function"))?;
        let first_stmt = function
            .body
            .first()
            .ok_or_else(|| std::io::Error::other("Expected assert_raises call statement"))?;
        let IrStmtKind::Expr(expr) = &first_stmt.kind else {
            return Err(std::io::Error::other("Expected assert_raises to lower as an expression statement").into());
        };
        let IrExprKind::Call {
            canonical_path: Some(path),
            args,
            ..
        } = &expr.kind
        else {
            return Err(std::io::Error::other("Expected canonical assert_raises helper call").into());
        };
        assert_eq!(
            path,
            &vec!["std".to_string(), "testing".to_string(), "assert_raises".to_string()]
        );
        assert_eq!(args.len(), 3);
        assert!(matches!(
            args.get(1).map(|arg| &arg.expr.kind),
            Some(IrExprKind::Literal(crate::backend::ir::expr::Literal::StaticStr(name))) if name == "ValueError"
        ));
        Ok(())
    }

    #[test]
    fn test_lower_if_statement() {
        let ir = must_ok(lower_source(
            r#"
def check(x: int) -> str:
    if x > 0:
        return "positive"
    elif x < 0:
        return "negative"
    else:
        return "zero"
"#,
        ));
        assert_eq!(ir.declarations.len(), 1);
        if let IrDeclKind::Function(f) = &ir.declarations[0].kind {
            assert!(!f.body.is_empty());
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn test_lower_for_loop() {
        let ir = must_ok(lower_source(
            r#"
def count() -> None:
    for i in range(10):
        print(i)
"#,
        ));
        assert_eq!(ir.declarations.len(), 1);
    }

    #[test]
    fn test_lower_binary_expressions() {
        let ir = must_ok(lower_source(
            r#"
def math(a: int, b: int) -> int:
    x = a + b
    y = a * b
    z = a - b
    return x + y + z
"#,
        ));
        assert_eq!(ir.declarations.len(), 1);
    }

    #[test]
    fn test_lower_list_literal() {
        let ir = must_ok(lower_source(
            r#"
def get_list() -> List[int]:
    return [1, 2, 3]
"#,
        ));
        assert_eq!(ir.declarations.len(), 1);
    }

    #[test]
    fn test_lower_enum() {
        let ir = must_ok(lower_source(
            r#"
enum Color:
    Red
    Green
    Blue
"#,
        ));
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
        let ir = must_ok(lower_source(source));
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
        let errors = match result {
            Ok(_) => panic!("Expected lowering error for immutable reassignment"),
            Err(errs) => errs,
        };
        assert!(
            errors.0[0].message.contains("immutable"),
            "Error should mention immutable"
        );
    }

    #[test]
    fn test_serde_propagation_respects_derives_and_containers() {
        let ir = must_ok(lower_source(
            r#"
from std.serde.json import Serialize

@derive(Serialize)
model Payload:
  tags: set[Tag]
  id: UserId

enum Tag:
  A
  B

type UserId = newtype int
"#,
        ));

        let serialize = "serde::Serialize".to_string();
        let deserialize = "serde::Deserialize".to_string();

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

    #[test]
    fn method_kind_for_receiver() {
        assert_eq!(
            MethodKind::for_receiver(&IrType::String, "join"),
            Some(MethodKind::String(StringMethodKind::Join))
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::Struct("Dataset".to_string()), "join"),
            None
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::String, "split"),
            Some(MethodKind::String(StringMethodKind::Split))
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::String, "replace"),
            Some(MethodKind::String(StringMethodKind::Replace))
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::Struct("Dataset".to_string()), "split"),
            None
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::Struct("Dataset".to_string()), "replace"),
            None
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::RefMut(Box::new(IrType::List(Box::new(IrType::Int)))), "swap"),
            Some(MethodKind::Collection(CollectionMethodKind::Swap))
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::RefMut(Box::new(IrType::List(Box::new(IrType::Int)))), "extend"),
            Some(MethodKind::Collection(CollectionMethodKind::Extend))
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::List(Box::new(IrType::Int)), "count"),
            Some(MethodKind::Collection(CollectionMethodKind::Count))
        );
        assert_eq!(
            MethodKind::for_receiver(&IrType::List(Box::new(IrType::Int)), "index"),
            Some(MethodKind::Collection(CollectionMethodKind::Index))
        );
    }

    #[test]
    fn membership_operators_lower_with_receiver_aware_known_methods() {
        let ir = must_ok(lower_source(
            r#"
def in_list(items: List[int]) -> bool:
    return 1 in items

def in_set(items: Set[str]) -> bool:
    return "a" in items

def in_dict(items: Dict[str, int]) -> bool:
    return "key" in items

def in_text(text: str) -> bool:
    return "e" in text

def not_in_list(items: List[int]) -> bool:
    return 2 not in items
"#,
        ));

        let returned_expr = |name: &str| -> &crate::backend::ir::expr::TypedExpr {
            let function = ir
                .declarations
                .iter()
                .find_map(|decl| match &decl.kind {
                    IrDeclKind::Function(f) if f.name == name => Some(f),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing function `{name}`"));
            match function.body.last() {
                Some(crate::backend::ir::stmt::IrStmt {
                    kind: IrStmtKind::Return(Some(expr)),
                    ..
                }) => expr,
                other => panic!("expected trailing return expression for `{name}`, got {other:?}"),
            }
        };

        for (name, expected_kind) in [
            ("in_list", MethodKind::Collection(CollectionMethodKind::Contains)),
            ("in_set", MethodKind::Collection(CollectionMethodKind::Contains)),
            ("in_dict", MethodKind::Collection(CollectionMethodKind::Contains)),
            ("in_text", MethodKind::String(StringMethodKind::Contains)),
        ] {
            match &returned_expr(name).kind {
                IrExprKind::KnownMethodCall { kind, .. } => {
                    assert_eq!(*kind, expected_kind, "unexpected known method kind for `{name}`");
                }
                other => panic!("expected known-method lowering for `{name}`, got {other:?}"),
            }
        }

        match &returned_expr("not_in_list").kind {
            IrExprKind::UnaryOp {
                op: UnaryOp::Not,
                operand,
            } => match &operand.kind {
                IrExprKind::KnownMethodCall { kind, .. } => {
                    assert_eq!(*kind, MethodKind::Collection(CollectionMethodKind::Contains));
                }
                other => panic!("expected negated known-method call for `not_in_list`, got {other:?}"),
            },
            other => panic!("expected unary negation for `not_in_list`, got {other:?}"),
        }
    }
}
