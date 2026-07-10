//! RFC 023: Trait bound inference for generic functions.
//!
//! This module scans IR function bodies to infer which Rust trait bounds are required on each type parameter based on
//! how the parameter is used (e.g., `==` requires `PartialEq`, display f-string interpolation requires `Display`).
//!
//! ## Inference rules (from RFC 023)
//!
//! | Incan operation             | Inferred Rust trait bound      |
//! | --------------------------- | ------------------------------ |
//! | `==`, `!=`                  | `PartialEq`                    |
//! | `<`, `<=`, `>`, `>=`        | `PartialOrd`                   |
//! | f-string `{value}`          | `std::fmt::Display`            |
//! | f-string `{value:?}`        | `std::fmt::Debug`              |
//! | `+`                         | `std::ops::Add<Output = T>`    |
//! | `-`                         | `std::ops::Sub<Output = T>`    |
//! | `*`                         | `std::ops::Mul<Output = T>`    |
//! | `/`                         | `std::ops::Div<Output = T>`    |
//! | `%`                         | `std::ops::Rem<Output = T>`    |
//! | `clone()`                   | `Clone`                        |
//! | used as `Dict` key          | `Eq + Hash`                    |
//! | used as `Set` element       | `Eq + Hash`                    |
//!
//! ## Transitive inference
//!
//! If `foo[T]` calls `bar[T]` and `bar` requires `PartialEq`, then `foo` also requires `PartialEq` on `T`. This is
//! handled by collecting bounds from called generic functions.

use std::collections::{HashMap, HashSet};

use incan_core::lang::{magic_methods, trait_bounds::rust as tb};

use super::IrProgram;
use super::decl::{FunctionParam, IrDeclKind, IrFunction, IrTraitBound, IrTypeParam};
use super::expr::{
    BinOp, FormatPart, IrCallArg, IrDictEntry, IrExpr, IrExprKind, IrGeneratorClause, IrListEntry, MethodCallArgPolicy,
    VarRefKind,
};
use super::ownership::{
    RegularMethodArgumentContext, ValueUseSite, regular_method_argument_use_site, value_use_requires_clone_bound,
    value_use_site_target_ty,
};
use super::stmt::{IrStmt, IrStmtKind};
use super::types::IrType;

/// Run trait bound inference on an entire IR program.
///
/// This mutates the `type_params` of each generic function to include inferred bounds in addition to any explicit
/// `with` bounds from the source.
pub fn infer_trait_bounds(program: &mut IrProgram) {
    // ---- Pass 1: collect explicit + body-scanned bounds per function ----
    let mut function_bounds: HashMap<String, Vec<IrTypeParam>> = HashMap::new();
    let mut function_params: HashMap<String, Vec<FunctionParam>> = HashMap::new();
    let trait_decls: HashMap<String, super::decl::IrTrait> = program
        .declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            IrDeclKind::Trait(tr) => Some((tr.name.clone(), tr.clone())),
            _ => None,
        })
        .collect();

    for decl in &program.declarations {
        match &decl.kind {
            IrDeclKind::Function(func) => {
                collect_inferred_bounds_for_callable(
                    &func.name,
                    func,
                    &func.type_params,
                    &trait_decls,
                    &mut function_bounds,
                    &mut function_params,
                );
            }
            IrDeclKind::Trait(trait_decl) => {
                for (index, method) in trait_decl.methods.iter().enumerate() {
                    let key = format!("trait:{}:{}:{}", trait_decl.name, index, method.name);
                    collect_inferred_bounds_for_callable(
                        &key,
                        method,
                        &method.type_params,
                        &trait_decls,
                        &mut function_bounds,
                        &mut function_params,
                    );
                }
            }
            IrDeclKind::Impl(impl_block) => {
                for (index, method) in impl_block.methods.iter().enumerate() {
                    let type_params = callable_inference_type_params(method, Some(&impl_block.type_params));
                    let key = format!(
                        "impl:{}:{}:{}:{}",
                        impl_block.target_type,
                        impl_block.trait_name.as_deref().unwrap_or("<inherent>"),
                        index,
                        method.name
                    );
                    collect_inferred_bounds_for_callable(
                        &key,
                        method,
                        &type_params,
                        &trait_decls,
                        &mut function_bounds,
                        &mut function_params,
                    );
                }
            }
            _ => {}
        }
    }

    // ---- Pass 2: transitive inference (propagate bounds from called generic functions) ----
    // We iterate until a fixed point is reached (no new bounds added). Clone-per-iteration avoids borrow conflicts
    // between reading callee bounds and writing caller bounds.
    let max_iterations = 20; // safety cap
    for _ in 0..max_iterations {
        let mut changed = false;
        let snapshot = function_bounds.clone();

        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Function(func) => {
                    propagate_bounds_for_callable(
                        &func.name,
                        func,
                        &func.type_params,
                        &snapshot,
                        &function_params,
                        &mut function_bounds,
                        &mut changed,
                    );
                }
                IrDeclKind::Trait(trait_decl) => {
                    for (index, method) in trait_decl.methods.iter().enumerate() {
                        let key = format!("trait:{}:{}:{}", trait_decl.name, index, method.name);
                        propagate_bounds_for_callable(
                            &key,
                            method,
                            &method.type_params,
                            &snapshot,
                            &function_params,
                            &mut function_bounds,
                            &mut changed,
                        );
                    }
                }
                IrDeclKind::Impl(impl_block) => {
                    for (index, method) in impl_block.methods.iter().enumerate() {
                        let type_params = callable_inference_type_params(method, Some(&impl_block.type_params));
                        let key = format!(
                            "impl:{}:{}:{}:{}",
                            impl_block.target_type,
                            impl_block.trait_name.as_deref().unwrap_or("<inherent>"),
                            index,
                            method.name
                        );
                        propagate_bounds_for_callable(
                            &key,
                            method,
                            &type_params,
                            &snapshot,
                            &function_params,
                            &mut function_bounds,
                            &mut changed,
                        );
                    }
                }
                _ => {}
            }
        }

        if !changed {
            break;
        }
    }

    // ---- Pass 3: write inferred bounds back into the IR ----
    write_back_callable_bounds(program, &mut function_bounds);

    // ---- Pass 4: backend-synthesized clone bounds ----
    //
    // Ownership planning can introduce `.clone()` at emission time (for example `return self.value` from `&self` or
    // `return self` for `-> Self`). Those clones are invisible to the earlier IR-body scan, so generic impl/function
    // headers need a final pass that mirrors the backend's return-value ownership rules.
    infer_backend_clone_bounds(program);
}

/// Infer `Clone` bounds required by backend-inserted ownership materialization.
///
/// Today this mirrors `ConversionContext::ReturnValue`: non-`Copy` vars returned without a move and non-`Copy` field
/// reads returned from a borrowed context materialize owned values via `.clone()`. It also mirrors ordinary
/// Incan-owned method-call argument lowering, where non-`Copy` vars/field reads are cloned at by-value call
/// boundaries.
fn infer_backend_clone_bounds(program: &mut IrProgram) {
    let clone_derived_self_params = collect_clone_derived_self_params(program);
    let clone_context = BackendCloneInferenceContext::from_program(program);

    for decl in &mut program.declarations {
        match &mut decl.kind {
            IrDeclKind::Function(func) => augment_callable_type_params_for_backend_return_clones(
                &mut func.type_params,
                &func.body,
                None,
                &clone_context,
            ),
            IrDeclKind::Impl(impl_block) => {
                let self_clone_params = clone_derived_self_params.get(&impl_block.target_type);
                for method in &impl_block.methods {
                    augment_callable_type_params_for_backend_return_clones(
                        &mut impl_block.type_params,
                        &method.body,
                        self_clone_params,
                        &clone_context,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Return type parameters visible to callable-bound inference.
fn callable_inference_type_params(func: &IrFunction, owner_type_params: Option<&[IrTypeParam]>) -> Vec<IrTypeParam> {
    let mut type_params = owner_type_params.map_or_else(Vec::new, |params| params.to_vec());
    for type_param in &func.type_params {
        if !type_params.iter().any(|existing| existing.name == type_param.name) {
            type_params.push(type_param.clone());
        }
    }
    type_params
}

/// Propagate bounds into one program using already-inferred callable signatures from external programs.
///
/// This is used after separate IR programs have already run local bound inference. Imported generic call targets can
/// still force additional bounds on the current program's type parameters, so their signatures are gathered as
/// read-only propagation inputs.
pub fn propagate_trait_bounds_from_programs(program: &mut IrProgram, externals: &[&IrProgram]) {
    let mut external_bounds: HashMap<String, Vec<IrTypeParam>> = HashMap::new();
    let mut external_params: HashMap<String, Vec<FunctionParam>> = HashMap::new();
    for external in externals {
        collect_current_callable_signature_maps(external, &mut external_bounds, &mut external_params);
    }
    propagate_trait_bounds_from_signature_maps(program, &external_bounds, &external_params);
}

/// Propagate generic bounds using local callable signatures plus externally supplied callable signatures.
///
/// The current program supplies the mutable destination signatures, while `external_bounds` and `external_params`
/// provide already-inferred signatures for imported call targets that may appear in this program's call graph. The pass
/// iterates to a fixed point because generic functions can forward type parameters through chains of calls.
fn propagate_trait_bounds_from_signature_maps(
    program: &mut IrProgram,
    external_bounds: &HashMap<String, Vec<IrTypeParam>>,
    external_params: &HashMap<String, Vec<FunctionParam>>,
) {
    let mut function_bounds: HashMap<String, Vec<IrTypeParam>> = HashMap::new();
    let mut function_params: HashMap<String, Vec<FunctionParam>> = HashMap::new();
    collect_current_callable_signature_maps(program, &mut function_bounds, &mut function_params);
    let local_callable_keys = collect_current_callable_keys(program);

    for (key, bounds) in external_bounds {
        if !local_callable_keys.contains(key) {
            function_bounds.entry(key.clone()).or_insert_with(|| bounds.clone());
        }
    }
    for (key, params) in external_params {
        if !local_callable_keys.contains(key) {
            function_params.entry(key.clone()).or_insert_with(|| params.clone());
        }
    }

    let max_iterations = 20;
    for _ in 0..max_iterations {
        let mut changed = false;
        let snapshot = function_bounds.clone();

        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Function(func) => {
                    propagate_bounds_for_callable(
                        &func.name,
                        func,
                        &func.type_params,
                        &snapshot,
                        &function_params,
                        &mut function_bounds,
                        &mut changed,
                    );
                }
                IrDeclKind::Trait(trait_decl) => {
                    for (index, method) in trait_decl.methods.iter().enumerate() {
                        let key = format!("trait:{}:{}:{}", trait_decl.name, index, method.name);
                        propagate_bounds_for_callable(
                            &key,
                            method,
                            &method.type_params,
                            &snapshot,
                            &function_params,
                            &mut function_bounds,
                            &mut changed,
                        );
                    }
                }
                IrDeclKind::Impl(impl_block) => {
                    for (index, method) in impl_block.methods.iter().enumerate() {
                        let type_params = callable_inference_type_params(method, Some(&impl_block.type_params));
                        let key = format!(
                            "impl:{}:{}:{}:{}",
                            impl_block.target_type,
                            impl_block.trait_name.as_deref().unwrap_or("<inherent>"),
                            index,
                            method.name
                        );
                        propagate_bounds_for_callable(
                            &key,
                            method,
                            &type_params,
                            &snapshot,
                            &function_params,
                            &mut function_bounds,
                            &mut changed,
                        );
                    }
                }
                _ => {}
            }
        }

        if !changed {
            break;
        }
    }

    write_back_callable_bounds(program, &mut function_bounds);
}

/// Collect every local callable key, including non-generic functions.
///
/// External signatures are keyed by callable name for legacy cross-module propagation. Keeping a complete local key set
/// prevents a same-named external generic helper from rewriting a local non-generic declaration's signature.
fn collect_current_callable_keys(program: &IrProgram) -> HashSet<String> {
    let mut keys = HashSet::new();
    for decl in &program.declarations {
        match &decl.kind {
            IrDeclKind::Function(func) => {
                keys.insert(func.name.clone());
            }
            IrDeclKind::Trait(trait_decl) => {
                for (index, method) in trait_decl.methods.iter().enumerate() {
                    keys.insert(format!("trait:{}:{}:{}", trait_decl.name, index, method.name));
                }
            }
            IrDeclKind::Impl(impl_block) => {
                for (index, method) in impl_block.methods.iter().enumerate() {
                    keys.insert(format!(
                        "impl:{}:{}:{}:{}",
                        impl_block.target_type,
                        impl_block.trait_name.as_deref().unwrap_or("<inherent>"),
                        index,
                        method.name
                    ));
                }
            }
            _ => {}
        }
    }
    keys
}

/// Collect the callable signatures that are already present on an IR program.
///
/// The propagation pass needs two parallel maps: type-parameter bounds for each generic callable, and parameter types
/// for mapping callee type parameters back to caller type parameters at call sites. Impl methods are keyed by their
/// owner and method identity because multiple impl blocks can contain same-named methods.
fn collect_current_callable_signature_maps(
    program: &IrProgram,
    function_bounds: &mut HashMap<String, Vec<IrTypeParam>>,
    function_params: &mut HashMap<String, Vec<FunctionParam>>,
) {
    for decl in &program.declarations {
        match &decl.kind {
            IrDeclKind::Function(func) if !func.type_params.is_empty() => {
                function_bounds.insert(func.name.clone(), func.type_params.clone());
                function_params.insert(func.name.clone(), func.params.clone());
            }
            IrDeclKind::Trait(trait_decl) => {
                for (index, method) in trait_decl.methods.iter().enumerate() {
                    if method.type_params.is_empty() {
                        continue;
                    }
                    let key = format!("trait:{}:{}:{}", trait_decl.name, index, method.name);
                    function_bounds.insert(key.clone(), method.type_params.clone());
                    function_params.insert(key, method.params.clone());
                }
            }
            IrDeclKind::Impl(impl_block) if !impl_block.type_params.is_empty() => {
                for (index, method) in impl_block.methods.iter().enumerate() {
                    let key = format!(
                        "impl:{}:{}:{}:{}",
                        impl_block.target_type,
                        impl_block.trait_name.as_deref().unwrap_or("<inherent>"),
                        index,
                        method.name
                    );
                    function_bounds.insert(key.clone(), impl_block.type_params.clone());
                    function_params.insert(key, method.params.clone());
                }
            }
            _ => {}
        }
    }
}

/// Write propagated bounds back into the program's callable declarations.
///
/// Free functions and trait methods own their own type parameters, so their inferred signature can be replaced
/// directly. Impl methods borrow the impl block's generic parameter list, so method-level propagation is merged back
/// into `impl_block.type_params` instead of replacing each method signature independently.
fn write_back_callable_bounds(program: &mut IrProgram, function_bounds: &mut HashMap<String, Vec<IrTypeParam>>) {
    for decl in &mut program.declarations {
        match &mut decl.kind {
            IrDeclKind::Function(func) => {
                if let Some(inferred) = function_bounds.remove(&func.name) {
                    func.type_params = inferred;
                }
            }
            IrDeclKind::Trait(trait_decl) => {
                for (index, method) in trait_decl.methods.iter_mut().enumerate() {
                    let key = format!("trait:{}:{}:{}", trait_decl.name, index, method.name);
                    if let Some(inferred) = function_bounds.remove(&key) {
                        method.type_params = inferred;
                    }
                }
            }
            IrDeclKind::Impl(impl_block) => {
                let target_type = impl_block.target_type.clone();
                let trait_name = impl_block.trait_name.clone();
                let owner_type_params = &mut impl_block.type_params;
                let methods = &mut impl_block.methods;
                for (index, method) in methods.iter_mut().enumerate() {
                    let key = format!(
                        "impl:{}:{}:{}:{}",
                        target_type,
                        trait_name.as_deref().unwrap_or("<inherent>"),
                        index,
                        method.name
                    );
                    if let Some(inferred) = function_bounds.remove(&key) {
                        for source in inferred.iter() {
                            if let Some(target) = owner_type_params
                                .iter_mut()
                                .find(|type_param| type_param.name == source.name)
                            {
                                merge_inferred_bounds(target, source);
                            } else if let Some(target) = method
                                .type_params
                                .iter_mut()
                                .find(|type_param| type_param.name == source.name)
                            {
                                merge_inferred_bounds(target, source);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn merge_inferred_bounds(target: &mut IrTypeParam, source: &IrTypeParam) {
    target.bounds.extend(source.bounds.iter().cloned());
    target.bounds = deduplicate_bounds(std::mem::take(&mut target.bounds));
}

/// Collect owner type parameters that participate in a derived `Clone` implementation.
///
/// `return self` lowers to `self.clone()`, which is only available when the owner type itself implements `Clone`.
/// Rust's derive machinery adds those bounds based on field/variant payload usage, so we mirror that dependency here
/// for impl headers that call `self.clone()`.
fn collect_clone_derived_self_params(program: &IrProgram) -> HashMap<String, HashSet<String>> {
    let mut result = HashMap::new();

    for decl in &program.declarations {
        match &decl.kind {
            IrDeclKind::Struct(s) if s.derives.iter().any(|derive| derive == tb::CLONE) => {
                let type_param_names: HashSet<&str> = s.type_params.iter().map(|tp| tp.name.as_str()).collect();
                let mut used = HashSet::new();
                for field in &s.fields {
                    collect_generic_type_param_names(&field.ty, &type_param_names, &mut used);
                }
                result.insert(s.name.clone(), used);
            }
            IrDeclKind::Enum(e) if e.derives.iter().any(|derive| derive == tb::CLONE) => {
                let type_param_names: HashSet<&str> = e.type_params.iter().map(|tp| tp.name.as_str()).collect();
                let mut used = HashSet::new();
                for variant in &e.variants {
                    match &variant.fields {
                        super::decl::VariantFields::Unit => {}
                        super::decl::VariantFields::Tuple(items) => {
                            for item in items {
                                collect_generic_type_param_names(item, &type_param_names, &mut used);
                            }
                        }
                        super::decl::VariantFields::Struct(fields) => {
                            for field in fields {
                                collect_generic_type_param_names(&field.ty, &type_param_names, &mut used);
                            }
                        }
                    }
                }
                result.insert(e.name.clone(), used);
            }
            _ => {}
        }
    }

    result
}

/// Receiver ownership facts used to mirror method-call argument planning during clone-bound inference.
struct BackendCloneInferenceContext {
    incan_nominal_names: HashSet<String>,
    rusttype_alias_names: HashSet<String>,
}

#[derive(Clone, Copy)]
struct BackendCallCloneContext<'a> {
    callable_signature: Option<&'a super::FunctionSignature>,
    in_return: bool,
}

impl BackendCloneInferenceContext {
    /// Build clone-bound inference context from an IR program.
    fn from_program(program: &IrProgram) -> Self {
        let mut incan_nominal_names = HashSet::new();
        let mut rusttype_alias_names = HashSet::new();
        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Struct(s) => {
                    incan_nominal_names.insert(s.name.clone());
                }
                IrDeclKind::Enum(e) => {
                    incan_nominal_names.insert(e.name.clone());
                }
                IrDeclKind::Trait(trait_decl) => {
                    incan_nominal_names.insert(trait_decl.name.clone());
                }
                IrDeclKind::TypeAlias {
                    name,
                    is_rusttype: true,
                    ..
                } => {
                    incan_nominal_names.insert(name.clone());
                    rusttype_alias_names.insert(name.clone());
                }
                _ => {}
            }
        }
        Self {
            incan_nominal_names,
            rusttype_alias_names,
        }
    }

    /// Return whether a receiver is an Incan-owned nominal type.
    fn is_incan_owned_nominal_receiver(&self, receiver_ty: &IrType) -> bool {
        match receiver_type_for_method_dispatch(receiver_ty) {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) | IrType::Enum(name) => {
                self.name_matches(name, &self.incan_nominal_names)
            }
            IrType::Trait(_) => true,
            _ => false,
        }
    }

    /// Return whether a receiver is a rusttype alias.
    fn is_rusttype_alias_receiver(&self, receiver_ty: &IrType) -> bool {
        match receiver_type_for_method_dispatch(receiver_ty) {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => self.name_matches(name, &self.rusttype_alias_names),
            _ => false,
        }
    }

    /// Return whether a fully qualified or short name is in the provided name set.
    fn name_matches(&self, name: &str, names: &HashSet<String>) -> bool {
        let short_name = name.rsplit("::").next().unwrap_or(name);
        names.contains(name) || names.contains(short_name)
    }
}

/// Return the receiver type used for method-dispatch analysis.
fn receiver_type_for_method_dispatch(receiver_ty: &IrType) -> &IrType {
    let mut receiver_ty = receiver_ty;
    while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
        receiver_ty = inner.as_ref();
    }
    receiver_ty
}

/// Add backend clone bounds required by callable return values.
fn augment_callable_type_params_for_backend_return_clones(
    type_params: &mut [IrTypeParam],
    body: &[IrStmt],
    self_clone_params: Option<&HashSet<String>>,
    clone_context: &BackendCloneInferenceContext,
) {
    if type_params.is_empty() {
        return;
    }

    let type_param_names: HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
    let mut clone_params = HashSet::new();
    for stmt in body {
        collect_backend_clone_bounds_in_stmt(
            stmt,
            &type_param_names,
            self_clone_params,
            clone_context,
            &mut clone_params,
        );
    }

    for tp in type_params {
        if clone_params.contains(&tp.name) && !tp.bounds.iter().any(|bound| bound.trait_path == tb::CLONE) {
            tp.bounds.push(IrTraitBound::simple(tb::CLONE));
        }
        tp.bounds = deduplicate_bounds(std::mem::take(&mut tp.bounds));
    }
}

/// Scan one statement for generic type parameters that need `Clone` because ownership planning will clone them.
///
/// This pass intentionally mirrors codegen use sites instead of source operations. It catches clones inserted for
/// returns, matches, owned collection elements, struct fields, and nested call arguments after the normal trait-bound
/// scan has already run.
fn collect_backend_clone_bounds_in_stmt(
    stmt: &IrStmt,
    type_param_names: &HashSet<&str>,
    self_clone_params: Option<&HashSet<String>>,
    clone_context: &BackendCloneInferenceContext,
    clone_params: &mut HashSet<String>,
) {
    match &stmt.kind {
        IrStmtKind::Return(Some(expr)) | IrStmtKind::Yield(expr) => {
            collect_backend_clone_bounds_for_value_use(
                expr,
                ValueUseSite::ReturnValue {
                    target_ty: Some(&expr.ty),
                },
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            if let IrExprKind::Call {
                func,
                args,
                callable_signature,
                ..
            } = &expr.kind
            {
                collect_backend_clone_bounds_in_call(
                    func,
                    args,
                    BackendCallCloneContext {
                        callable_signature: callable_signature.as_ref(),
                        in_return: true,
                    },
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            } else {
                collect_backend_clone_bounds_in_expr(
                    expr,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrStmtKind::Expr(expr) => {
            collect_backend_clone_bounds_in_expr(
                expr,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrStmtKind::Let { value, .. } | IrStmtKind::Assign { value, .. } | IrStmtKind::CompoundAssign { value, .. } => {
            collect_backend_clone_bounds_in_expr(
                value,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrStmtKind::While { body, .. } | IrStmtKind::Loop { body, .. } => {
            for stmt in body {
                collect_backend_clone_bounds_in_stmt(
                    stmt,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrStmtKind::For { body, .. } => {
            for stmt in body {
                collect_backend_clone_bounds_in_stmt(
                    stmt,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrStmtKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            for stmt in then_branch {
                collect_backend_clone_bounds_in_stmt(
                    stmt,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
            if let Some(else_branch) = else_branch {
                for stmt in else_branch {
                    collect_backend_clone_bounds_in_stmt(
                        stmt,
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    );
                }
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            collect_backend_clone_bounds_for_value_use(
                scrutinee,
                ValueUseSite::MatchScrutinee {
                    target_ty: Some(&scrutinee.ty),
                },
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            for arm in arms {
                for binding in &arm.bindings {
                    collect_backend_clone_bounds_in_expr(
                        &binding.value,
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    );
                    if let Some(guard_value) = &binding.guard_value {
                        collect_backend_clone_bounds_in_expr(
                            guard_value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
                if let IrExprKind::Block { stmts, .. } = &arm.body.kind {
                    for stmt in stmts {
                        collect_backend_clone_bounds_in_stmt(
                            stmt,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
                collect_backend_clone_bounds_in_expr(
                    &arm.body,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
                if let Some(guard) = &arm.guard {
                    collect_backend_clone_bounds_in_expr(
                        guard,
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    );
                }
            }
        }
        IrStmtKind::Block(stmts) => {
            for stmt in stmts {
                collect_backend_clone_bounds_in_stmt(
                    stmt,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrStmtKind::Break { value: Some(expr), .. } => {
            collect_backend_clone_bounds_for_value_use(
                expr,
                ValueUseSite::ReturnValue {
                    target_ty: Some(&expr.ty),
                },
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            collect_backend_clone_bounds_in_expr(
                expr,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrStmtKind::Return(None) | IrStmtKind::Break { label: _, value: None } | IrStmtKind::Continue(_) => {}
    }
}

/// Mirror `emit_expr_for_use` for trait-bound inference.
///
/// If a value-use site plans a backend `.clone()`, this records any generic type parameters that must receive a
/// generated `Clone` bound. For recursive owned containers, the same target-type propagation rules as expression
/// emission are applied so tuple/list/set/dict elements are checked against their element use sites.
fn collect_backend_clone_bounds_for_value_use<'a>(
    expr: &'a IrExpr,
    site: ValueUseSite<'a>,
    type_param_names: &HashSet<&str>,
    self_clone_params: Option<&HashSet<String>>,
    clone_context: &BackendCloneInferenceContext,
    clone_params: &mut HashSet<String>,
) {
    if value_use_requires_clone_bound(expr, site) {
        add_backend_clone_bounds_for_cloned_expr(expr, type_param_names, self_clone_params, clone_params);
    }

    match &expr.kind {
        IrExprKind::Tuple(items) | IrExprKind::Set(items) => {
            let item_target_ty = match &expr.kind {
                IrExprKind::Tuple(_) => None,
                IrExprKind::Set(_) => match value_use_site_target_ty(site) {
                    Some(IrType::Set(elem)) => Some(elem.as_ref()),
                    _ => match &expr.ty {
                        IrType::Set(elem) => Some(elem.as_ref()),
                        _ => None,
                    },
                },
                _ => None,
            };
            for (idx, item) in items.iter().enumerate() {
                let item_site = match &expr.kind {
                    IrExprKind::Tuple(_) => {
                        let tuple_target_items = match value_use_site_target_ty(site) {
                            Some(IrType::Tuple(items)) => Some(items.as_slice()),
                            _ => match &expr.ty {
                                IrType::Tuple(items) => Some(items.as_slice()),
                                _ => None,
                            },
                        };
                        tuple_item_use_site(site, tuple_target_items.and_then(|items| items.get(idx)))
                    }
                    _ => ValueUseSite::CollectionElement {
                        target_ty: item_target_ty,
                    },
                };
                collect_backend_clone_bounds_for_value_use(
                    item,
                    item_site,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::List(items) => {
            let item_target_ty = match value_use_site_target_ty(site) {
                Some(IrType::List(elem)) => Some(elem.as_ref()),
                _ => match &expr.ty {
                    IrType::List(elem) => Some(elem.as_ref()),
                    _ => None,
                },
            };
            for item in items {
                match item {
                    IrListEntry::Element(value) => collect_backend_clone_bounds_for_value_use(
                        value,
                        ValueUseSite::CollectionElement {
                            target_ty: item_target_ty,
                        },
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    ),
                    IrListEntry::Spread(value) => {
                        collect_backend_clone_bounds_in_expr(
                            value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
            }
        }
        IrExprKind::Dict(entries) => {
            let (key_target_ty, value_target_ty) = match value_use_site_target_ty(site) {
                Some(IrType::Dict(key, value)) => (Some(key.as_ref()), Some(value.as_ref())),
                _ => match &expr.ty {
                    IrType::Dict(key, value) => (Some(key.as_ref()), Some(value.as_ref())),
                    _ => (None, None),
                },
            };
            for entry in entries {
                match entry {
                    IrDictEntry::Pair(key, value) => {
                        collect_backend_clone_bounds_for_value_use(
                            key,
                            ValueUseSite::CollectionElement {
                                target_ty: key_target_ty,
                            },
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                        collect_backend_clone_bounds_for_value_use(
                            value,
                            ValueUseSite::CollectionElement {
                                target_ty: value_target_ty,
                            },
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                    IrDictEntry::Spread(value) => {
                        collect_backend_clone_bounds_in_expr(
                            value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
            }
        }
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                collect_backend_clone_bounds_for_value_use(
                    value,
                    ValueUseSite::StructField { target_ty: None },
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        _ => {}
    }
}

/// Walk an expression tree for nested ownership-planned clones that are not represented as explicit source calls.
///
/// Ordinary trait-bound inference catches user-written operations. This pass catches clones introduced by backend call
/// argument and owned-sink planning, while preserving external Rust call shapes that should not use Incan clone policy.
fn collect_backend_clone_bounds_in_call(
    func: &IrExpr,
    args: &[IrCallArg],
    call_context: BackendCallCloneContext<'_>,
    type_param_names: &HashSet<&str>,
    self_clone_params: Option<&HashSet<String>>,
    clone_context: &BackendCloneInferenceContext,
    clone_params: &mut HashSet<String>,
) {
    if call_args_use_incan_clone_policy(func) {
        for (idx, arg) in args.iter().enumerate() {
            let sig_param = call_context.callable_signature.and_then(|sig| sig.params.get(idx));
            let target_ty = sig_param.map(|param| &param.ty).or_else(|| match &func.ty {
                IrType::Function { params, .. } => params.get(idx),
                _ => None,
            });
            let requires_clone = value_use_requires_clone_bound(
                &arg.expr,
                ValueUseSite::IncanCallArg {
                    target_ty,
                    callee_param: sig_param,
                    in_return: call_context.in_return,
                },
            );
            if requires_clone {
                add_backend_clone_bounds_for_cloned_expr(&arg.expr, type_param_names, self_clone_params, clone_params);
            }
            collect_backend_clone_bounds_in_expr(
                &arg.expr,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
    } else {
        for arg in args {
            collect_backend_clone_bounds_in_expr(
                &arg.expr,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
    }
    collect_backend_clone_bounds_in_expr(func, type_param_names, self_clone_params, clone_context, clone_params);
}

/// Walk an expression tree for backend-planned clones and explicit clone calls that affect generic bounds.
fn collect_backend_clone_bounds_in_expr(
    expr: &IrExpr,
    type_param_names: &HashSet<&str>,
    self_clone_params: Option<&HashSet<String>>,
    clone_context: &BackendCloneInferenceContext,
    clone_params: &mut HashSet<String>,
) {
    match &expr.kind {
        IrExprKind::MethodCall {
            receiver,
            args,
            arg_policy,
            callable_signature,
            ..
        } => {
            let callable_signature = callable_signature.as_ref();
            for (idx, arg) in args.iter().enumerate() {
                let sig_param = callable_signature.and_then(|sig| sig.params.get(idx));
                let use_site = regular_method_argument_use_site(
                    RegularMethodArgumentContext {
                        arg_policy: *arg_policy,
                        receiver_ref_kind: receiver_ref_kind(receiver),
                        has_incan_method_signature: callable_signature.is_some(),
                        is_incan_owned_nominal_receiver: clone_context.is_incan_owned_nominal_receiver(&receiver.ty),
                        is_rusttype_alias_receiver: clone_context.is_rusttype_alias_receiver(&receiver.ty),
                        preserves_lookup_arg_shape: matches!(arg_policy, MethodCallArgPolicy::PreserveShape),
                        in_return: false,
                    },
                    sig_param,
                );
                if value_use_requires_clone_bound(&arg.expr, use_site) {
                    add_backend_clone_bounds_for_cloned_expr(
                        &arg.expr,
                        type_param_names,
                        self_clone_params,
                        clone_params,
                    );
                }
                collect_backend_clone_bounds_in_expr(
                    &arg.expr,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
            collect_backend_clone_bounds_in_expr(
                receiver,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            collect_backend_clone_bounds_in_expr(
                receiver,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            for arg in args {
                collect_backend_clone_bounds_in_expr(
                    &arg.expr,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Call {
            func,
            args,
            callable_signature,
            ..
        } => collect_backend_clone_bounds_in_call(
            func,
            args,
            BackendCallCloneContext {
                callable_signature: callable_signature.as_ref(),
                in_return: false,
            },
            type_param_names,
            self_clone_params,
            clone_context,
            clone_params,
        ),
        IrExprKind::BuiltinCall { args, .. } | IrExprKind::Tuple(args) => {
            for arg in args {
                collect_backend_clone_bounds_in_expr(
                    arg,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::List(args) => {
            for arg in args {
                match arg {
                    IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                        collect_backend_clone_bounds_in_expr(
                            value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
            }
        }
        IrExprKind::Dict(entries) => {
            for entry in entries {
                match entry {
                    IrDictEntry::Pair(key, value) => {
                        collect_backend_clone_bounds_in_expr(
                            key,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                        collect_backend_clone_bounds_in_expr(
                            value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                    IrDictEntry::Spread(value) => {
                        collect_backend_clone_bounds_in_expr(
                            value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
            }
        }
        IrExprKind::Set(items) => {
            for item in items {
                collect_backend_clone_bounds_in_expr(
                    item,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                collect_backend_clone_bounds_in_expr(
                    value,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Field { object, .. }
        | IrExprKind::Await(object)
        | IrExprKind::Try(object)
        | IrExprKind::Cast { expr: object, .. }
        | IrExprKind::NumericResize { expr: object, .. }
        | IrExprKind::InteropCoerce { expr: object, .. }
        | IrExprKind::UnaryOp { operand: object, .. } => {
            collect_backend_clone_bounds_in_expr(
                object,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrExprKind::BinOp { left, right, .. }
        | IrExprKind::Index {
            object: left,
            index: right,
        } => {
            collect_backend_clone_bounds_in_expr(
                left,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            collect_backend_clone_bounds_in_expr(
                right,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            collect_backend_clone_bounds_in_expr(
                target,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            if let Some(start) = start {
                collect_backend_clone_bounds_in_expr(
                    start,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
            if let Some(end) = end {
                collect_backend_clone_bounds_in_expr(
                    end,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
            if let Some(step) = step {
                collect_backend_clone_bounds_in_expr(
                    step,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_backend_clone_bounds_in_expr(
                condition,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            collect_backend_clone_bounds_in_expr(
                then_branch,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            if let Some(else_branch) = else_branch {
                collect_backend_clone_bounds_in_expr(
                    else_branch,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Block { stmts, value } => {
            for stmt in stmts {
                collect_backend_clone_bounds_in_stmt(
                    stmt,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
            if let Some(value) = value {
                collect_backend_clone_bounds_in_expr(
                    value,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Loop { body } => {
            for stmt in body {
                collect_backend_clone_bounds_in_stmt(
                    stmt,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Race { arms, .. } => {
            for arm in arms {
                collect_backend_clone_bounds_in_expr(
                    &arm.awaitable,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
                collect_backend_clone_bounds_in_expr(
                    &arm.body,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Match { scrutinee, arms } => {
            collect_backend_clone_bounds_for_value_use(
                scrutinee,
                ValueUseSite::MatchScrutinee {
                    target_ty: Some(&scrutinee.ty),
                },
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            for arm in arms {
                for binding in &arm.bindings {
                    collect_backend_clone_bounds_in_expr(
                        &binding.value,
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    );
                    if let Some(guard_value) = &binding.guard_value {
                        collect_backend_clone_bounds_in_expr(
                            guard_value,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
                collect_backend_clone_bounds_in_expr(
                    &arm.body,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
                if let Some(guard) = &arm.guard {
                    collect_backend_clone_bounds_in_expr(
                        guard,
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    );
                }
            }
        }
        IrExprKind::Closure { body, .. } => {
            collect_backend_clone_bounds_in_expr(
                body,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            collect_backend_clone_bounds_in_expr(
                element,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            collect_backend_clone_bounds_in_expr(
                iterable,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            if let Some(filter) = filter {
                collect_backend_clone_bounds_in_expr(
                    filter,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            collect_backend_clone_bounds_in_expr(key, type_param_names, self_clone_params, clone_context, clone_params);
            collect_backend_clone_bounds_in_expr(
                value,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            collect_backend_clone_bounds_in_expr(
                iterable,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            if let Some(filter) = filter {
                collect_backend_clone_bounds_in_expr(
                    filter,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Generator { element, clauses } => {
            collect_backend_clone_bounds_in_expr(
                element,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
            for clause in clauses {
                match clause {
                    IrGeneratorClause::For { iterable, .. } => {
                        collect_backend_clone_bounds_in_expr(
                            iterable,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                    IrGeneratorClause::If(condition) => {
                        collect_backend_clone_bounds_in_expr(
                            condition,
                            type_param_names,
                            self_clone_params,
                            clone_context,
                            clone_params,
                        );
                    }
                }
            }
        }
        IrExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                collect_backend_clone_bounds_in_expr(
                    start,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
            if let Some(end) = end {
                collect_backend_clone_bounds_in_expr(
                    end,
                    type_param_names,
                    self_clone_params,
                    clone_context,
                    clone_params,
                );
            }
        }
        IrExprKind::Format { parts } => {
            for part in parts {
                if let FormatPart::Expr { expr, .. } = part {
                    collect_backend_clone_bounds_in_expr(
                        expr,
                        type_param_names,
                        self_clone_params,
                        clone_context,
                        clone_params,
                    );
                }
            }
        }
        IrExprKind::RegisterCallableName { callable, .. } => {
            collect_backend_clone_bounds_in_expr(
                callable,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
            collect_backend_clone_bounds_in_expr(
                value,
                type_param_names,
                self_clone_params,
                clone_context,
                clone_params,
            );
        }
        IrExprKind::Var { .. }
        | IrExprKind::StaticRead { .. }
        | IrExprKind::StaticBinding { .. }
        | IrExprKind::AssociatedFunction { .. }
        | IrExprKind::TypeToken { .. }
        | IrExprKind::FunctionItem { .. }
        | IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Decimal(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => {}
    }
}

/// Return the reference kind used by a receiver expression.
fn receiver_ref_kind(receiver: &IrExpr) -> Option<VarRefKind> {
    match &receiver.kind {
        IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
        _ => None,
    }
}

/// Return whether a call expression targets an Incan callable rather than an external Rust symbol.
fn call_args_use_incan_clone_policy(func: &IrExpr) -> bool {
    !matches!(
        &func.kind,
        IrExprKind::Var {
            ref_kind: VarRefKind::ExternalRustName,
            ..
        }
    )
}

/// Extract the borrowed inner type from method chains that erase the concrete borrow in the outer expression type.
///
/// This is primarily for `as_ref()` over generic wrappers. The outer expression may look erased, but the inner generic
/// type still determines whether a backend clone requires a `Clone` bound.
fn borrowed_method_inner_ty(expr: &IrExpr) -> Option<&IrType> {
    match &expr.kind {
        IrExprKind::MethodCall { receiver, method, .. } if method == "as_ref" => match &receiver.ty {
            IrType::NamedGeneric(_, args) if args.len() == 1 => args.first(),
            _ => None,
        },
        _ => None,
    }
}

/// Rebuild a parent value-use site for one tuple item while preserving the parent ownership context.
///
/// Tuple elements can be planned as call arguments, return values, collection elements, and match scrutinees. This
/// helper keeps that outer context while swapping in the tuple slot target type.
fn tuple_item_use_site<'a>(site: ValueUseSite<'a>, target_ty: Option<&'a IrType>) -> ValueUseSite<'a> {
    match site {
        ValueUseSite::IncanCallArg {
            in_return,
            callee_param,
            ..
        } => ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return,
        },
        ValueUseSite::ExternalCallArg { .. } => ValueUseSite::ExternalCallArg { target_ty },
        ValueUseSite::ExternalInferredGenericArg { .. } => ValueUseSite::ExternalInferredGenericArg { target_ty },
        ValueUseSite::StructField { .. } => ValueUseSite::StructField { target_ty },
        ValueUseSite::CollectionElement { .. } => ValueUseSite::CollectionElement { target_ty },
        ValueUseSite::Assignment { .. } => ValueUseSite::Assignment { target_ty },
        ValueUseSite::ReturnValue { .. } => ValueUseSite::ReturnValue { target_ty },
        ValueUseSite::MatchScrutinee { .. } => ValueUseSite::MatchScrutinee { target_ty },
        ValueUseSite::MethodArg => ValueUseSite::MethodArg,
    }
}

/// Record generic type parameters that need `Clone` because `expr` is cloned by backend ownership planning.
///
/// `self` is special: cloning `self` can imply all or a subset of the impl's type parameters, depending on the derived
/// `Clone` implementation for the receiver type. For erased borrowed method results, inspect the borrowed inner type in
/// addition to the expression's outer type.
fn add_backend_clone_bounds_for_cloned_expr(
    expr: &IrExpr,
    type_param_names: &HashSet<&str>,
    self_clone_params: Option<&HashSet<String>>,
    clone_params: &mut HashSet<String>,
) {
    if matches!(&expr.kind, IrExprKind::Var { name, .. } if name == "self") {
        if let Some(self_clone_params) = self_clone_params {
            if self_clone_params.is_empty() {
                clone_params.extend(type_param_names.iter().map(|name| (*name).to_string()));
            } else {
                clone_params.extend(self_clone_params.iter().cloned());
            }
        } else {
            clone_params.extend(type_param_names.iter().map(|name| (*name).to_string()));
        }
        if matches!(expr.ty, IrType::SelfType) {
            return;
        }
    }

    let before = clone_params.len();
    if let Some(inner_ty) = borrowed_method_inner_ty(expr) {
        collect_generic_type_param_names(inner_ty, type_param_names, clone_params);
    }
    collect_generic_type_param_names(&expr.ty, type_param_names, clone_params);
    if clone_params.len() == before
        && matches!(&expr.kind, IrExprKind::Var { .. } | IrExprKind::Field { .. })
        && !type_param_names.is_empty()
    {
        clone_params.extend(type_param_names.iter().map(|name| (*name).to_string()));
    }
}

/// Collect generic parameter names nested anywhere inside an IR type.
fn collect_generic_type_param_names(ty: &IrType, type_param_names: &HashSet<&str>, out: &mut HashSet<String>) {
    match ty {
        IrType::Generic(name) => {
            if type_param_names.contains(name.as_str()) {
                out.insert(name.clone());
            }
        }
        IrType::List(inner)
        | IrType::Set(inner)
        | IrType::Option(inner)
        | IrType::Ref(inner)
        | IrType::RefMut(inner)
        | IrType::TypeToken(inner) => {
            collect_generic_type_param_names(inner, type_param_names, out);
        }
        IrType::Dict(key, value) | IrType::Result(key, value) => {
            collect_generic_type_param_names(key, type_param_names, out);
            collect_generic_type_param_names(value, type_param_names, out);
        }
        IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
            for item in items {
                collect_generic_type_param_names(item, type_param_names, out);
            }
        }
        IrType::ExternalUnion { union, .. } => collect_generic_type_param_names(union, type_param_names, out),
        IrType::Function { params, ret } => {
            for param in params {
                collect_generic_type_param_names(param, type_param_names, out);
            }
            collect_generic_type_param_names(ret, type_param_names, out);
        }
        IrType::ImplTrait(bound) => {
            for arg in &bound.type_args {
                collect_generic_type_param_names(arg, type_param_names, out);
            }
            for (_, ty) in &bound.assoc_types {
                collect_generic_type_param_names(ty, type_param_names, out);
            }
        }
        IrType::Struct(_)
        | IrType::Enum(_)
        | IrType::Trait(_)
        | IrType::Unit
        | IrType::Bool
        | IrType::Int
        | IrType::Float
        | IrType::Numeric(_)
        | IrType::Decimal { .. }
        | IrType::String
        | IrType::Bytes
        | IrType::StaticStr
        | IrType::StaticBytes
        | IrType::FrozenStr
        | IrType::FrozenBytes
        | IrType::StrRef
        | IrType::RustDisplay(_)
        | IrType::SelfType
        | IrType::Unknown => {}
    }
}

/// Collect inferred bounds for a callable (function, trait method, or impl method).
///
/// Scans the callable's body to infer trait bounds on its type parameters, including bounds required by the return
/// type, and stores them in the bounds map for later transitive propagation.
fn collect_inferred_bounds_for_callable(
    key: &str,
    func: &IrFunction,
    type_params: &[IrTypeParam],
    trait_decls: &HashMap<String, super::decl::IrTrait>,
    function_bounds: &mut HashMap<String, Vec<IrTypeParam>>,
    function_params: &mut HashMap<String, Vec<FunctionParam>>,
) {
    if type_params.is_empty() {
        return;
    }

    let mut inferred = infer_function_bounds(func, type_params);

    // Also check return types like `-> DataSet[T]` / `-> BoundedDataSet[T]`, which lower to `impl Trait` and
    // must carry through any bounds required by the returned trait's generic arguments.
    add_bounds_from_return_type(&func.return_type, type_params, trait_decls, &mut inferred);

    function_bounds.insert(key.to_string(), inferred);
    function_params.insert(key.to_string(), func.params.clone());
}

/// Propagate bounds for a callable by transitive inference from called generic functions.
///
/// Checks if the callable uses any generic functions and propagates their trait bounds to the caller's type
/// parameters using the type argument mapping.
fn propagate_bounds_for_callable(
    key: &str,
    func: &IrFunction,
    type_params: &[IrTypeParam],
    snapshot: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
    function_bounds: &mut HashMap<String, Vec<IrTypeParam>>,
    changed: &mut bool,
) {
    if type_params.is_empty() {
        return;
    }

    let called_generics = collect_called_generic_functions(func, type_params, snapshot, function_params);
    if let Some(current_bounds) = function_bounds.get_mut(key) {
        for (callee_name, type_arg_mapping) in &called_generics {
            if let Some(callee_bounds) = snapshot.get(callee_name)
                && propagate_transitive_bounds(current_bounds, callee_bounds, type_arg_mapping)
            {
                *changed = true;
            }
        }
    }
}

/// Infer trait bounds for a single function by scanning its body.
fn infer_function_bounds(func: &IrFunction, type_params: &[IrTypeParam]) -> Vec<IrTypeParam> {
    let type_param_names: HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
    let mut bounds_map: HashMap<String, Vec<IrTraitBound>> = HashMap::new();

    // Start with explicit bounds from `with` clauses.
    for tp in type_params {
        bounds_map.insert(tp.name.clone(), tp.bounds.clone());
    }

    // Scan body statements for operations on type parameters.
    for stmt in &func.body {
        scan_stmt_for_bounds(stmt, &type_param_names, &func.params, &mut bounds_map);
    }

    // Rebuild type params with combined bounds.
    type_params
        .iter()
        .map(|tp| {
            let bounds = bounds_map.remove(&tp.name).unwrap_or_default();
            IrTypeParam {
                name: tp.name.clone(),
                bounds: deduplicate_bounds(bounds),
            }
        })
        .collect()
}

/// Scan a statement for trait-bound-relevant operations on type parameters.
fn scan_stmt_for_bounds(
    stmt: &IrStmt,
    type_params: &HashSet<&str>,
    params: &[super::decl::FunctionParam],
    bounds_map: &mut HashMap<String, Vec<IrTraitBound>>,
) {
    match &stmt.kind {
        IrStmtKind::Expr(expr) | IrStmtKind::Yield(expr) => scan_expr_for_bounds(expr, type_params, params, bounds_map),
        IrStmtKind::Let { value, .. } => scan_expr_for_bounds(value, type_params, params, bounds_map),
        IrStmtKind::Assign { value, .. } => scan_expr_for_bounds(value, type_params, params, bounds_map),
        IrStmtKind::CompoundAssign { value, .. } => {
            scan_expr_for_bounds(value, type_params, params, bounds_map);
        }
        IrStmtKind::Return(Some(expr)) => scan_expr_for_bounds(expr, type_params, params, bounds_map),
        IrStmtKind::Break { label: _, value } => {
            if let Some(expr) = value {
                scan_expr_for_bounds(expr, type_params, params, bounds_map);
            }
        }
        IrStmtKind::Return(None) | IrStmtKind::Continue(_) => {}
        IrStmtKind::While { condition, body, .. } => {
            scan_expr_for_bounds(condition, type_params, params, bounds_map);
            for s in body {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
        IrStmtKind::For { iterable, body, .. } => {
            scan_expr_for_bounds(iterable, type_params, params, bounds_map);
            for s in body {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
        IrStmtKind::Loop { body, .. } => {
            for s in body {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            scan_expr_for_bounds(condition, type_params, params, bounds_map);
            for s in then_branch {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(else_stmts) = else_branch {
                for s in else_stmts {
                    scan_stmt_for_bounds(s, type_params, params, bounds_map);
                }
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            scan_expr_for_bounds(scrutinee, type_params, params, bounds_map);
            for arm in arms {
                for binding in &arm.bindings {
                    scan_expr_for_bounds(&binding.value, type_params, params, bounds_map);
                    if let Some(guard_value) = &binding.guard_value {
                        scan_expr_for_bounds(guard_value, type_params, params, bounds_map);
                    }
                }
                scan_expr_for_bounds(&arm.body, type_params, params, bounds_map);
                if let Some(guard) = &arm.guard {
                    scan_expr_for_bounds(guard, type_params, params, bounds_map);
                }
            }
        }
        IrStmtKind::Block(stmts) => {
            for s in stmts {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
    }
}

/// Return the trait bound implied by a value-level reflection magic method.
fn reflection_magic_trait_bound(method: &str) -> Option<&'static str> {
    match magic_methods::from_str(method) {
        Some(magic_methods::MagicMethodId::ClassName) => Some(tb::INCAN_CLASS_NAME),
        Some(magic_methods::MagicMethodId::Fields) => Some(tb::INCAN_FIELD_METADATA),
        Some(magic_methods::MagicMethodId::FieldValue | magic_methods::MagicMethodId::FieldItems) => {
            Some(tb::INCAN_FIELD_VALUE_REFLECTION)
        }
        _ => None,
    }
}

/// Return the trait bound implied by a type-level reflection magic method.
fn type_reflection_magic_trait_bound(method: &str) -> Option<&'static str> {
    match magic_methods::from_str(method) {
        Some(magic_methods::MagicMethodId::ClassName) => Some(tb::INCAN_TYPE_CLASS_NAME),
        Some(magic_methods::MagicMethodId::Fields) => Some(tb::INCAN_TYPE_FIELD_METADATA),
        _ => None,
    }
}

/// Scan an expression for trait-bound-relevant operations on type parameters.
fn scan_expr_for_bounds(
    expr: &IrExpr,
    type_params: &HashSet<&str>,
    params: &[super::decl::FunctionParam],
    bounds_map: &mut HashMap<String, Vec<IrTraitBound>>,
) {
    match &expr.kind {
        // ---- Binary operations: check if either operand is a type parameter ----
        IrExprKind::BinOp { op, left, right } => {
            let left_tp = expr_type_param_name(left, type_params, params);
            let right_tp = expr_type_param_name(right, type_params, params);

            for tp_name in left_tp.iter().chain(right_tp.iter()) {
                if let Some(bound) = binop_to_trait_bound(op, tp_name) {
                    add_bound(bounds_map, tp_name, bound);
                }
            }

            scan_expr_for_bounds(left, type_params, params, bounds_map);
            scan_expr_for_bounds(right, type_params, params, bounds_map);
        }

        // ---- f-string interpolation: expressions used in format require the matching formatting trait ----
        IrExprKind::Format { parts } => {
            for part in parts {
                if let FormatPart::Expr { expr: inner, style } = part {
                    let bound = if style.emits_rust_debug(&inner.ty) {
                        tb::DEBUG
                    } else {
                        tb::DISPLAY
                    };
                    let mut formatted_type_params = HashSet::new();
                    if let Some(tp_name) = expr_type_param_name(inner, type_params, params) {
                        formatted_type_params.insert(tp_name);
                    }
                    if style.emits_rust_debug(&inner.ty) {
                        collect_generic_type_param_names(&inner.ty, type_params, &mut formatted_type_params);
                    }
                    for tp_name in formatted_type_params {
                        add_bound(bounds_map, &tp_name, IrTraitBound::simple(bound));
                    }
                    scan_expr_for_bounds(inner, type_params, params, bounds_map);
                }
            }
        }

        // ---- Method call: `x.clone()` on a generic param requires Clone ----
        IrExprKind::MethodCall {
            receiver, method, args, ..
        } => {
            let receiver_is_type_name = matches!(
                receiver.kind,
                IrExprKind::Var {
                    ref_kind: VarRefKind::TypeName,
                    ..
                }
            );
            if let Some(tp_name) = expr_type_param_name(receiver, type_params, params) {
                if method == "clone" && !receiver_is_type_name {
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::CLONE));
                }
                let reflection_bound = if receiver_is_type_name {
                    type_reflection_magic_trait_bound(method)
                } else {
                    reflection_magic_trait_bound(method)
                };
                if let Some(bound) = reflection_bound {
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(bound));
                }
            } else if receiver_is_type_name
                && let Some(tp_name) = type_name_expr_type_param_name(receiver, type_params)
                && let Some(bound) = type_reflection_magic_trait_bound(method)
            {
                add_bound(bounds_map, &tp_name, IrTraitBound::simple(bound));
            } else if method == "clone"
                && matches!(receiver.ty, IrType::Unknown)
                && matches!(&receiver.kind, IrExprKind::Var { .. } | IrExprKind::Field { .. })
            {
                for tp_name in type_params {
                    add_bound(bounds_map, tp_name, IrTraitBound::simple(tb::CLONE));
                }
            }
            scan_expr_for_bounds(receiver, type_params, params, bounds_map);
            for arg in args {
                scan_expr_for_bounds(&arg.expr, type_params, params, bounds_map);
            }
        }

        // ---- Function call: recurse into args ----
        IrExprKind::Call { func, args, .. } => {
            scan_expr_for_bounds(func, type_params, params, bounds_map);
            for arg in args {
                scan_expr_for_bounds(&arg.expr, type_params, params, bounds_map);
            }
        }

        // ---- Known method calls: recurse ----
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            scan_expr_for_bounds(receiver, type_params, params, bounds_map);
            for arg in args {
                scan_expr_for_bounds(&arg.expr, type_params, params, bounds_map);
            }
        }

        // ---- Builtin calls: recurse ----
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                scan_expr_for_bounds(arg, type_params, params, bounds_map);
            }
        }

        // ---- Dict literal: keys that are generic require Eq + Hash ----
        // Note: `Eq: PartialEq` in Rust, so we only need `Eq` (not redundant `PartialEq`).
        IrExprKind::Dict(entries) => {
            for entry in entries {
                match entry {
                    IrDictEntry::Pair(key, value) => {
                        if let Some(tp_name) = expr_type_param_name(key, type_params, params) {
                            add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::EQ));
                            add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::HASH));
                        }
                        scan_expr_for_bounds(key, type_params, params, bounds_map);
                        scan_expr_for_bounds(value, type_params, params, bounds_map);
                    }
                    IrDictEntry::Spread(value) => {
                        scan_expr_for_bounds(value, type_params, params, bounds_map);
                    }
                }
            }
        }

        // ---- Set literal: elements that are generic require Eq + Hash ----
        IrExprKind::Set(elems) => {
            for elem in elems {
                if let Some(tp_name) = expr_type_param_name(elem, type_params, params) {
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::EQ));
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::HASH));
                }
                scan_expr_for_bounds(elem, type_params, params, bounds_map);
            }
        }

        // ---- If expression: recurse ----
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            scan_expr_for_bounds(condition, type_params, params, bounds_map);
            scan_expr_for_bounds(then_branch, type_params, params, bounds_map);
            if let Some(e) = else_branch {
                scan_expr_for_bounds(e, type_params, params, bounds_map);
            }
        }

        // ---- Unary: recurse ----
        IrExprKind::UnaryOp { operand, .. } => {
            scan_expr_for_bounds(operand, type_params, params, bounds_map);
        }

        // ---- Field/Index: recurse ----
        IrExprKind::Field { object, .. } => scan_expr_for_bounds(object, type_params, params, bounds_map),
        IrExprKind::Index { object, index } => {
            scan_expr_for_bounds(object, type_params, params, bounds_map);
            scan_expr_for_bounds(index, type_params, params, bounds_map);
        }

        // ---- Collections: recurse ----
        IrExprKind::Tuple(elems) => {
            for elem in elems {
                scan_expr_for_bounds(elem, type_params, params, bounds_map);
            }
        }
        IrExprKind::List(elems) => {
            for elem in elems {
                match elem {
                    IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                        scan_expr_for_bounds(value, type_params, params, bounds_map);
                    }
                }
            }
        }

        // ---- Block: recurse into stmts and value ----
        IrExprKind::Block { stmts, value } => {
            for s in stmts {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(v) = value {
                scan_expr_for_bounds(v, type_params, params, bounds_map);
            }
        }
        IrExprKind::Loop { body } => {
            for stmt in body {
                scan_stmt_for_bounds(stmt, type_params, params, bounds_map);
            }
        }

        // ---- Match: recurse ----
        IrExprKind::Match { scrutinee, arms } => {
            scan_expr_for_bounds(scrutinee, type_params, params, bounds_map);
            for arm in arms {
                for binding in &arm.bindings {
                    scan_expr_for_bounds(&binding.value, type_params, params, bounds_map);
                    if let Some(guard_value) = &binding.guard_value {
                        scan_expr_for_bounds(guard_value, type_params, params, bounds_map);
                    }
                }
                scan_expr_for_bounds(&arm.body, type_params, params, bounds_map);
                if let Some(guard) = &arm.guard {
                    scan_expr_for_bounds(guard, type_params, params, bounds_map);
                }
            }
        }

        // ---- Closure: recurse into body ----
        IrExprKind::Closure { body, .. } => {
            scan_expr_for_bounds(body, type_params, params, bounds_map);
        }

        // ---- ListComp / DictComp ----
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            scan_expr_for_bounds(element, type_params, params, bounds_map);
            scan_expr_for_bounds(iterable, type_params, params, bounds_map);
            if let Some(f) = filter {
                scan_expr_for_bounds(f, type_params, params, bounds_map);
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            scan_expr_for_bounds(key, type_params, params, bounds_map);
            scan_expr_for_bounds(value, type_params, params, bounds_map);
            scan_expr_for_bounds(iterable, type_params, params, bounds_map);
            if let Some(f) = filter {
                scan_expr_for_bounds(f, type_params, params, bounds_map);
            }
        }
        IrExprKind::Generator { element, clauses } => {
            scan_expr_for_bounds(element, type_params, params, bounds_map);
            for clause in clauses {
                match clause {
                    IrGeneratorClause::For { iterable, .. } => {
                        scan_expr_for_bounds(iterable, type_params, params, bounds_map);
                    }
                    IrGeneratorClause::If(condition) => {
                        scan_expr_for_bounds(condition, type_params, params, bounds_map);
                    }
                }
            }
        }

        // ---- Struct construction: recurse into field values ----
        IrExprKind::Struct { fields, .. } => {
            for (_, val) in fields {
                scan_expr_for_bounds(val, type_params, params, bounds_map);
            }
        }

        // ---- Await/Try: recurse ----
        IrExprKind::Await(inner) | IrExprKind::Try(inner) => {
            scan_expr_for_bounds(inner, type_params, params, bounds_map);
        }

        IrExprKind::Race { arms, .. } => {
            for arm in arms {
                scan_expr_for_bounds(&arm.awaitable, type_params, params, bounds_map);
                scan_expr_for_bounds(&arm.body, type_params, params, bounds_map);
            }
        }

        // ---- Slice: recurse ----
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            scan_expr_for_bounds(target, type_params, params, bounds_map);
            if let Some(s) = start {
                scan_expr_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(e) = end {
                scan_expr_for_bounds(e, type_params, params, bounds_map);
            }
            if let Some(s) = step {
                scan_expr_for_bounds(s, type_params, params, bounds_map);
            }
        }

        // ---- Cast: recurse ----
        IrExprKind::Cast { expr, .. } => {
            scan_expr_for_bounds(expr, type_params, params, bounds_map);
        }

        IrExprKind::NumericResize { expr, .. } => {
            scan_expr_for_bounds(expr, type_params, params, bounds_map);
        }

        IrExprKind::InteropCoerce { expr, .. } => {
            scan_expr_for_bounds(expr, type_params, params, bounds_map);
        }

        IrExprKind::RegisterCallableName { callable, .. } => {
            scan_expr_for_bounds(callable, type_params, params, bounds_map);
        }
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
            scan_expr_for_bounds(value, type_params, params, bounds_map);
        }

        // ---- Range: recurse ----
        IrExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                scan_expr_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(e) = end {
                scan_expr_for_bounds(e, type_params, params, bounds_map);
            }
        }

        // ---- Leaf nodes: no sub-expressions to scan ----
        IrExprKind::Var { .. }
        | IrExprKind::StaticRead { .. }
        | IrExprKind::StaticBinding { .. }
        | IrExprKind::AssociatedFunction { .. }
        | IrExprKind::TypeToken { .. }
        | IrExprKind::FunctionItem { .. }
        | IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Decimal(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => {}
    }
}

/// Determine if an expression refers to a variable whose type is a type parameter.
///
/// Returns the type parameter name if so.
fn expr_type_param_name(
    expr: &IrExpr,
    type_params: &HashSet<&str>,
    params: &[super::decl::FunctionParam],
) -> Option<String> {
    match &expr.kind {
        IrExprKind::InteropCoerce { expr, .. }
        | IrExprKind::Cast { expr, .. }
        | IrExprKind::Await(expr)
        | IrExprKind::Try(expr)
        | IrExprKind::UnaryOp { operand: expr, .. } => {
            return expr_type_param_name(expr, type_params, params);
        }
        _ => {}
    }

    // Check the resolved type on the expression.
    if let IrType::Generic(ref name) = expr.ty
        && type_params.contains(name.as_str())
    {
        return Some(name.clone());
    }

    // Also check if it's a Var referencing a param whose type is Generic.
    if let IrExprKind::Var { name, .. } = &expr.kind {
        for p in params {
            if &p.name == name
                && let IrType::Generic(ref tp_name) = p.ty
                && type_params.contains(tp_name.as_str())
            {
                return Some(tp_name.clone());
            }
        }
    }

    None
}

/// Return the type parameter named by a type-name expression.
fn type_name_expr_type_param_name(expr: &IrExpr, type_params: &HashSet<&str>) -> Option<String> {
    let IrExprKind::Var {
        name,
        ref_kind: VarRefKind::TypeName,
        ..
    } = &expr.kind
    else {
        return None;
    };
    type_params.contains(name.as_str()).then(|| name.clone())
}

/// Extract a type parameter name from an IR type.
fn type_param_name_from_ir_type(ty: &IrType, type_params: &HashSet<&str>) -> Option<String> {
    match ty {
        IrType::Generic(name) if type_params.contains(name.as_str()) => Some(name.clone()),
        IrType::Struct(name) if type_params.contains(name.as_str()) => Some(name.clone()),
        _ => None,
    }
}

/// Collect type-parameter mappings between callee and caller types.
fn collect_type_param_mapping(
    callee_ty: &IrType,
    caller_ty: &IrType,
    caller_type_params: &HashSet<&str>,
    mapping: &mut HashMap<String, String>,
) {
    match (callee_ty, caller_ty) {
        (IrType::Generic(callee_tp), caller_ty) => {
            if let Some(caller_tp) = type_param_name_from_ir_type(caller_ty, caller_type_params) {
                mapping.insert(callee_tp.clone(), caller_tp);
            }
        }
        (IrType::List(callee), IrType::List(caller))
        | (IrType::Set(callee), IrType::Set(caller))
        | (IrType::Option(callee), IrType::Option(caller))
        | (IrType::Ref(callee), IrType::Ref(caller))
        | (IrType::RefMut(callee), IrType::RefMut(caller)) => {
            collect_type_param_mapping(callee, caller, caller_type_params, mapping);
        }
        (IrType::Dict(callee_key, callee_value), IrType::Dict(caller_key, caller_value))
        | (IrType::Result(callee_key, callee_value), IrType::Result(caller_key, caller_value)) => {
            collect_type_param_mapping(callee_key, caller_key, caller_type_params, mapping);
            collect_type_param_mapping(callee_value, caller_value, caller_type_params, mapping);
        }
        (IrType::Tuple(callee_items), IrType::Tuple(caller_items))
        | (IrType::NamedGeneric(_, callee_items), IrType::NamedGeneric(_, caller_items))
            if callee_items.len() == caller_items.len() =>
        {
            for (callee_item, caller_item) in callee_items.iter().zip(caller_items.iter()) {
                collect_type_param_mapping(callee_item, caller_item, caller_type_params, mapping);
            }
        }
        _ => {}
    }
}

/// Resolve the generic function key for a call target.
fn resolve_called_generic_key(
    local_name: &str,
    canonical_path: Option<&[String]>,
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
) -> Option<String> {
    if function_bounds.contains_key(local_name) {
        return Some(local_name.to_string());
    }

    let canonical_path = canonical_path?;
    if let Some(last) = canonical_path.last()
        && function_bounds.contains_key(last)
    {
        return Some(last.clone());
    }

    let joined = canonical_path.join("::");
    function_bounds.contains_key(&joined).then_some(joined)
}

/// Map a binary operator to the required trait bound on the type parameter.
fn binop_to_trait_bound(op: &BinOp, tp_name: &str) -> Option<IrTraitBound> {
    match op {
        // Comparison
        BinOp::Eq | BinOp::Ne => Some(IrTraitBound::simple(tb::PARTIAL_EQ)),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Some(IrTraitBound::simple(tb::PARTIAL_ORD)),

        // Arithmetic
        BinOp::Add => Some(IrTraitBound::with_output(tb::ADD, IrType::Generic(tp_name.to_string()))),
        BinOp::Sub => Some(IrTraitBound::with_output(tb::SUB, IrType::Generic(tp_name.to_string()))),
        BinOp::Mul => Some(IrTraitBound::with_output(tb::MUL, IrType::Generic(tp_name.to_string()))),
        BinOp::Div => Some(IrTraitBound::with_output(tb::DIV, IrType::Generic(tp_name.to_string()))),
        BinOp::Mod => Some(IrTraitBound::with_output(tb::REM, IrType::Generic(tp_name.to_string()))),

        // Logical, bitwise, etc. — no trait bound inferred for these.
        BinOp::FloorDiv
        | BinOp::Pow
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => None,
    }
}

/// Add a trait bound to a type parameter, avoiding duplicates.
fn add_bound(bounds_map: &mut HashMap<String, Vec<IrTraitBound>>, tp_name: &str, bound: IrTraitBound) {
    let bounds = bounds_map.entry(tp_name.to_string()).or_default();
    if !bounds.contains(&bound) {
        bounds.push(bound);
    }
}

/// Remove duplicate bounds (by trait path).
fn deduplicate_bounds(bounds: Vec<IrTraitBound>) -> Vec<IrTraitBound> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for bound in bounds {
        if seen.insert(bound.trait_path.clone()) {
            result.push(bound);
        }
    }
    result
}

/// Add trait bounds from the return type of a function.
///
/// When a function returns a trait type like `impl BoundedDataSet<T>`, the type parameter `T` must satisfy the bounds
/// required by that trait (e.g., `T: Clone` if `BoundedDataSet` requires `T with Clone`).
fn add_bounds_from_return_type(
    return_type: &IrType,
    type_params: &[IrTypeParam],
    trait_decls: &HashMap<String, super::decl::IrTrait>,
    bounds: &mut [IrTypeParam],
) {
    let type_param_names: HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();

    // Collect bounds from the return type
    let mut temp_bounds: HashMap<String, Vec<IrTraitBound>> = HashMap::new();
    let mut visited_traits = HashSet::new();
    add_bounds_from_type(
        return_type,
        &type_param_names,
        trait_decls,
        &mut temp_bounds,
        &mut visited_traits,
    );

    // Merge into the existing bounds for each type parameter
    for tp in bounds.iter_mut() {
        if let Some(new_bounds) = temp_bounds.remove(&tp.name) {
            for new_bound in new_bounds {
                if !tp.bounds.contains(&new_bound) {
                    tp.bounds.push(new_bound);
                }
            }
        }
    }
}

/// Recursively add trait bounds from a type for any type parameters in scope.
fn add_bounds_from_type(
    ty: &IrType,
    type_params: &HashSet<&str>,
    trait_decls: &HashMap<String, super::decl::IrTrait>,
    bounds_map: &mut HashMap<String, Vec<IrTraitBound>>,
    visited_traits: &mut HashSet<String>,
) {
    match ty {
        IrType::Generic(_) => {}
        IrType::NamedGeneric(_, args) => {
            for arg in args {
                add_bounds_from_type(arg, type_params, trait_decls, bounds_map, visited_traits);
            }
        }
        IrType::ExternalUnion { union, .. } => {
            add_bounds_from_type(union, type_params, trait_decls, bounds_map, visited_traits);
        }
        IrType::ImplTrait(bound) => {
            add_bounds_from_trait_bound(bound, type_params, trait_decls, bounds_map, visited_traits);
        }
        IrType::List(elem)
        | IrType::Option(elem)
        | IrType::Ref(elem)
        | IrType::RefMut(elem)
        | IrType::Set(elem)
        | IrType::TypeToken(elem) => {
            add_bounds_from_type(elem, type_params, trait_decls, bounds_map, visited_traits);
        }
        IrType::Dict(key, value) | IrType::Result(key, value) => {
            add_bounds_from_type(key, type_params, trait_decls, bounds_map, visited_traits);
            add_bounds_from_type(value, type_params, trait_decls, bounds_map, visited_traits);
        }
        IrType::Tuple(elems) => {
            for elem in elems {
                add_bounds_from_type(elem, type_params, trait_decls, bounds_map, visited_traits);
            }
        }
        IrType::Function { params, ret } => {
            for param in params {
                add_bounds_from_type(param, type_params, trait_decls, bounds_map, visited_traits);
            }
            add_bounds_from_type(ret, type_params, trait_decls, bounds_map, visited_traits);
        }
        // Struct, Enum, Trait, Unit, Bool, Int, Float, String, etc. don't require bounds
        IrType::Struct(_)
        | IrType::Enum(_)
        | IrType::Trait(_)
        | IrType::Unit
        | IrType::Bool
        | IrType::Int
        | IrType::Float
        | IrType::Numeric(_)
        | IrType::Decimal { .. }
        | IrType::String
        | IrType::Bytes
        | IrType::StaticStr
        | IrType::StaticBytes
        | IrType::FrozenStr
        | IrType::FrozenBytes
        | IrType::StrRef
        | IrType::RustDisplay(_)
        | IrType::SelfType
        | IrType::Unknown => {}
    }
}

fn add_bounds_from_trait_bound(
    bound: &IrTraitBound,
    type_params: &HashSet<&str>,
    trait_decls: &HashMap<String, super::decl::IrTrait>,
    bounds_map: &mut HashMap<String, Vec<IrTraitBound>>,
    visited_traits: &mut HashSet<String>,
) {
    for arg in &bound.type_args {
        add_bounds_from_type(arg, type_params, trait_decls, bounds_map, visited_traits);
    }

    let Some(trait_decl) = trait_decls.get(&bound.trait_path) else {
        return;
    };
    if !visited_traits.insert(bound.trait_path.clone()) {
        return;
    }

    let type_arg_subst: HashMap<&str, &IrType> = trait_decl
        .type_params
        .iter()
        .zip(bound.type_args.iter())
        .map(|(param, arg)| (param.name.as_str(), arg))
        .collect();

    for type_param in &trait_decl.type_params {
        let Some(actual_arg) = type_arg_subst.get(type_param.name.as_str()) else {
            continue;
        };

        if let IrType::Generic(actual_name) = actual_arg
            && type_params.contains(actual_name.as_str())
        {
            for required_bound in &type_param.bounds {
                add_bound(
                    bounds_map,
                    actual_name,
                    substitute_trait_bound(required_bound, &type_arg_subst),
                );
            }
        }
    }

    for (supertrait_name, supertrait_args) in &trait_decl.supertraits {
        let supertrait_bound = IrTraitBound::with_type_args(
            supertrait_name.clone(),
            supertrait_args
                .iter()
                .map(|arg| substitute_ir_type(arg, &type_arg_subst))
                .collect(),
        );
        add_bounds_from_trait_bound(&supertrait_bound, type_params, trait_decls, bounds_map, visited_traits);
    }

    visited_traits.remove(&bound.trait_path);
}

fn substitute_trait_bound(bound: &IrTraitBound, subst: &HashMap<&str, &IrType>) -> IrTraitBound {
    IrTraitBound {
        trait_path: bound.trait_path.clone(),
        type_args: bound
            .type_args
            .iter()
            .map(|arg| substitute_ir_type(arg, subst))
            .collect(),
        assoc_types: bound
            .assoc_types
            .iter()
            .map(|(name, ty)| (name.clone(), substitute_ir_type(ty, subst)))
            .collect(),
        origin: bound.origin,
    }
}

fn substitute_ir_type(ty: &IrType, subst: &HashMap<&str, &IrType>) -> IrType {
    match ty {
        IrType::Generic(name) => subst.get(name.as_str()).cloned().cloned().unwrap_or_else(|| ty.clone()),
        IrType::List(elem) => IrType::List(Box::new(substitute_ir_type(elem, subst))),
        IrType::Dict(key, value) => IrType::Dict(
            Box::new(substitute_ir_type(key, subst)),
            Box::new(substitute_ir_type(value, subst)),
        ),
        IrType::Set(elem) => IrType::Set(Box::new(substitute_ir_type(elem, subst))),
        IrType::Tuple(elems) => IrType::Tuple(elems.iter().map(|elem| substitute_ir_type(elem, subst)).collect()),
        IrType::Option(elem) => IrType::Option(Box::new(substitute_ir_type(elem, subst))),
        IrType::Result(ok, err) => IrType::Result(
            Box::new(substitute_ir_type(ok, subst)),
            Box::new(substitute_ir_type(err, subst)),
        ),
        IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
            name.clone(),
            args.iter().map(|arg| substitute_ir_type(arg, subst)).collect(),
        ),
        IrType::ImplTrait(bound) => IrType::ImplTrait(substitute_trait_bound(bound, subst)),
        IrType::Function { params, ret } => IrType::Function {
            params: params.iter().map(|param| substitute_ir_type(param, subst)).collect(),
            ret: Box::new(substitute_ir_type(ret, subst)),
        },
        IrType::Ref(inner) => IrType::Ref(Box::new(substitute_ir_type(inner, subst))),
        IrType::RefMut(inner) => IrType::RefMut(Box::new(substitute_ir_type(inner, subst))),
        _ => ty.clone(),
    }
}

/// Collect calls to generic functions and their type argument mappings.
///
/// Returns a list of (callee name, type arg mapping) pairs. Each mapping connects the callee's type parameter names to
/// the caller's type parameter names when the argument is a direct type parameter pass-through.
fn collect_called_generic_functions(
    func: &IrFunction,
    type_params: &[IrTypeParam],
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
) -> Vec<(String, HashMap<String, String>)> {
    let type_param_names: HashSet<&str> = type_params.iter().map(|tp| tp.name.as_str()).collect();
    let mut result = Vec::new();

    for stmt in &func.body {
        collect_calls_in_stmt(
            stmt,
            &type_param_names,
            &func.params,
            function_bounds,
            function_params,
            &mut result,
        );
    }

    result
}

/// Recursively collect generic function calls from a statement.
fn collect_calls_in_stmt(
    stmt: &IrStmt,
    type_params: &HashSet<&str>,
    params: &[FunctionParam],
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
    result: &mut Vec<(String, HashMap<String, String>)>,
) {
    let recurse_expr = |e: &IrExpr, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_expr(e, type_params, params, function_bounds, function_params, r);
    };
    let recurse_stmt = |s: &IrStmt, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_stmt(s, type_params, params, function_bounds, function_params, r);
    };

    match &stmt.kind {
        IrStmtKind::Expr(expr) | IrStmtKind::Yield(expr) => recurse_expr(expr, result),
        IrStmtKind::Let { value, .. } | IrStmtKind::Assign { value, .. } | IrStmtKind::CompoundAssign { value, .. } => {
            recurse_expr(value, result)
        }
        IrStmtKind::Return(Some(expr)) => recurse_expr(expr, result),
        IrStmtKind::Break { label: _, value } => {
            if let Some(expr) = value {
                recurse_expr(expr, result);
            }
        }
        IrStmtKind::Return(None) | IrStmtKind::Continue(_) => {}
        IrStmtKind::While { condition, body, .. } => {
            recurse_expr(condition, result);
            for s in body {
                recurse_stmt(s, result);
            }
        }
        IrStmtKind::For { iterable, body, .. } => {
            recurse_expr(iterable, result);
            for s in body {
                recurse_stmt(s, result);
            }
        }
        IrStmtKind::Loop { body, .. } => {
            for s in body {
                recurse_stmt(s, result);
            }
        }
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            recurse_expr(condition, result);
            for s in then_branch {
                recurse_stmt(s, result);
            }
            if let Some(else_stmts) = else_branch {
                for s in else_stmts {
                    recurse_stmt(s, result);
                }
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            recurse_expr(scrutinee, result);
            for arm in arms {
                for binding in &arm.bindings {
                    recurse_expr(&binding.value, result);
                    if let Some(guard_value) = &binding.guard_value {
                        recurse_expr(guard_value, result);
                    }
                }
                recurse_expr(&arm.body, result);
                if let Some(guard) = &arm.guard {
                    recurse_expr(guard, result);
                }
            }
        }
        IrStmtKind::Block(stmts) => {
            for s in stmts {
                recurse_stmt(s, result);
            }
        }
    }
}

/// Recursively collect generic function calls from an expression.
fn collect_calls_in_expr(
    expr: &IrExpr,
    type_params: &HashSet<&str>,
    params: &[FunctionParam],
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
    result: &mut Vec<(String, HashMap<String, String>)>,
) {
    let recurse_expr = |e: &IrExpr, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_expr(e, type_params, params, function_bounds, function_params, r);
    };
    let recurse_stmt = |s: &IrStmt, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_stmt(s, type_params, params, function_bounds, function_params, r);
    };

    match &expr.kind {
        IrExprKind::Call {
            func,
            args,
            type_args,
            canonical_path,
            ..
        } => {
            // ---- Check if the called function is a generic function we know about ----
            if let IrExprKind::Var { name, .. } = &func.kind
                && let Some(callee_key) = resolve_called_generic_key(name, canonical_path.as_deref(), function_bounds)
            {
                let mut mapping = HashMap::new();

                // Explicit call-site type arguments are the strongest signal for how callee generics map back to
                // the caller.
                if let Some(callee_type_params) = function_bounds.get(callee_key.as_str()) {
                    for (callee_tp, caller_ty) in callee_type_params.iter().zip(type_args.iter()) {
                        if let Some(caller_tp) = type_param_name_from_ir_type(caller_ty, type_params) {
                            mapping.insert(callee_tp.name.clone(), caller_tp);
                        }
                    }
                }

                // Use the callee's parameter types to determine which type parameter each argument corresponds to.
                // Named arguments (`foo(b=x)`) are matched by name; positional arguments by index.
                if let Some(callee_params) = function_params.get(callee_key.as_str()) {
                    for (i, arg) in args.iter().enumerate() {
                        // Resolve the callee parameter: by name if the arg is named, by position otherwise.
                        let callee_param = if let Some(arg_name) = &arg.name {
                            callee_params.iter().find(|p| &p.name == arg_name)
                        } else {
                            callee_params.get(i)
                        };
                        if let Some(cp) = callee_param {
                            collect_type_param_mapping(&cp.ty, &arg.expr.ty, type_params, &mut mapping);
                        }
                    }
                }

                if !mapping.is_empty() {
                    result.push((callee_key, mapping));
                }
            }

            // Recurse.
            recurse_expr(func, result);
            for arg in args {
                recurse_expr(&arg.expr, result);
            }
        }
        IrExprKind::FunctionItem { name, type_args } => {
            if let Some(callee_key) = resolve_called_generic_key(name, None, function_bounds) {
                let mut mapping = HashMap::new();
                if let Some(callee_type_params) = function_bounds.get(callee_key.as_str()) {
                    for (callee_tp, caller_ty) in callee_type_params.iter().zip(type_args.iter()) {
                        if let Some(caller_tp) = type_param_name_from_ir_type(caller_ty, type_params) {
                            mapping.insert(callee_tp.name.clone(), caller_tp);
                        }
                    }
                }
                if !mapping.is_empty() {
                    result.push((callee_key, mapping));
                }
            }
        }
        IrExprKind::BinOp { left, right, .. } => {
            recurse_expr(left, result);
            recurse_expr(right, result);
        }
        IrExprKind::UnaryOp { operand, .. } => {
            recurse_expr(operand, result);
        }
        IrExprKind::MethodCall { receiver, args, .. } => {
            recurse_expr(receiver, result);
            for arg in args {
                recurse_expr(&arg.expr, result);
            }
        }
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            recurse_expr(receiver, result);
            for arg in args {
                recurse_expr(&arg.expr, result);
            }
        }
        IrExprKind::BuiltinCall { args, .. } | IrExprKind::Tuple(args) | IrExprKind::Set(args) => {
            for arg in args {
                recurse_expr(arg, result);
            }
        }
        IrExprKind::Field { object, .. } => {
            recurse_expr(object, result);
        }
        IrExprKind::Index { object, index } => {
            recurse_expr(object, result);
            recurse_expr(index, result);
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            recurse_expr(target, result);
            if let Some(start) = start {
                recurse_expr(start, result);
            }
            if let Some(end) = end {
                recurse_expr(end, result);
            }
            if let Some(step) = step {
                recurse_expr(step, result);
            }
        }
        IrExprKind::List(items) => {
            for item in items {
                match item {
                    IrListEntry::Element(value) | IrListEntry::Spread(value) => recurse_expr(value, result),
                }
            }
        }
        IrExprKind::Dict(entries) => {
            for entry in entries {
                match entry {
                    IrDictEntry::Pair(key, value) => {
                        recurse_expr(key, result);
                        recurse_expr(value, result);
                    }
                    IrDictEntry::Spread(value) => recurse_expr(value, result),
                }
            }
        }
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                recurse_expr(value, result);
            }
        }
        IrExprKind::Format { parts } => {
            for part in parts {
                if let FormatPart::Expr { expr, .. } = part {
                    recurse_expr(expr, result);
                }
            }
        }
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            recurse_expr(condition, result);
            recurse_expr(then_branch, result);
            if let Some(e) = else_branch {
                recurse_expr(e, result);
            }
        }
        IrExprKind::Block { stmts, value } => {
            for s in stmts {
                recurse_stmt(s, result);
            }
            if let Some(v) = value {
                recurse_expr(v, result);
            }
        }
        IrExprKind::Loop { body } => {
            for stmt in body {
                recurse_stmt(stmt, result);
            }
        }
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            recurse_expr(element, result);
            recurse_expr(iterable, result);
            if let Some(filter) = filter {
                recurse_expr(filter, result);
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            recurse_expr(key, result);
            recurse_expr(value, result);
            recurse_expr(iterable, result);
            if let Some(filter) = filter {
                recurse_expr(filter, result);
            }
        }
        IrExprKind::Generator { element, clauses } => {
            recurse_expr(element, result);
            for clause in clauses {
                match clause {
                    IrGeneratorClause::For { iterable, .. } => recurse_expr(iterable, result),
                    IrGeneratorClause::If(filter) => recurse_expr(filter, result),
                }
            }
        }
        IrExprKind::Match { scrutinee, arms } => {
            recurse_expr(scrutinee, result);
            for arm in arms {
                for binding in &arm.bindings {
                    recurse_expr(&binding.value, result);
                    if let Some(guard_value) = &binding.guard_value {
                        recurse_expr(guard_value, result);
                    }
                }
                recurse_expr(&arm.body, result);
                if let Some(guard) = &arm.guard {
                    recurse_expr(guard, result);
                }
            }
        }
        IrExprKind::Closure { body, .. } => {
            recurse_expr(body, result);
        }
        IrExprKind::Race { arms, .. } => {
            for arm in arms {
                recurse_expr(&arm.awaitable, result);
                recurse_expr(&arm.body, result);
            }
        }
        IrExprKind::Await(expr) | IrExprKind::Try(expr) => {
            recurse_expr(expr, result);
        }
        IrExprKind::Cast { expr, .. }
        | IrExprKind::NumericResize { expr, .. }
        | IrExprKind::InteropCoerce { expr, .. } => {
            recurse_expr(expr, result);
        }
        IrExprKind::RegisterCallableName { callable, .. } => {
            recurse_expr(callable, result);
        }
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
            recurse_expr(value, result);
        }
        // Other expression kinds are not recursed into for transitive inference.
        // The primary call pattern (direct function calls) is covered above.
        _ => {}
    }
}

/// Propagate bounds from a callee to a caller using the type argument mapping.
///
/// Returns `true` if any new bounds were added.
fn propagate_transitive_bounds(
    caller_bounds: &mut [IrTypeParam],
    callee_bounds: &[IrTypeParam],
    type_arg_mapping: &HashMap<String, String>,
) -> bool {
    let mut changed = false;

    for callee_tp in callee_bounds {
        // Check if this callee type param is mapped to a caller type param.
        if let Some(caller_tp_name) = type_arg_mapping.get(&callee_tp.name) {
            // Find the corresponding caller type param.
            if let Some(caller_tp) = caller_bounds.iter_mut().find(|tp| &tp.name == caller_tp_name) {
                for bound in &callee_tp.bounds {
                    if !caller_tp.bounds.contains(bound) {
                        caller_tp.bounds.push(bound.clone());
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::decl::{FunctionParam, IrDecl, IrDeclKind, IrImpl, Visibility};
    use crate::backend::ir::expr::{FormatStyle, IrCallArgKind, Literal, MethodCallArgPolicy, VarAccess};
    use crate::backend::ir::{FunctionRegistry, FunctionSignature, Mutability, TypedExpr};

    fn function(name: &str, type_params: Vec<IrTypeParam>) -> IrFunction {
        IrFunction {
            name: name.to_string(),
            docstring: None,
            params: Vec::new(),
            return_type: IrType::Unit,
            body: Vec::new(),
            is_async: false,
            is_generator: false,
            visibility: Visibility::Public,
            type_params,
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        }
    }

    fn program(functions: Vec<IrFunction>) -> IrProgram {
        IrProgram {
            declarations: functions
                .into_iter()
                .map(|func| IrDecl::new(IrDeclKind::Function(func)))
                .collect(),
            source_module_name: None,
            entry_point: None,
            function_registry: FunctionRegistry::new(),
            function_reexports: Vec::new(),
            rust_module_path: None,
            newtype_checked_ctor: Default::default(),
        }
    }

    #[test]
    fn impl_owner_generic_bounds_are_written_to_impl_header() -> Result<(), Box<dyn std::error::Error>> {
        let method = IrFunction {
            name: "render".to_string(),
            docstring: None,
            params: Vec::new(),
            return_type: IrType::Unit,
            body: vec![IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::Format {
                    parts: vec![FormatPart::Expr {
                        expr: TypedExpr::new(
                            IrExprKind::Var {
                                name: "value".to_string(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::Generic("T".to_string()),
                        ),
                        style: FormatStyle::Display,
                    }],
                },
                IrType::String,
            )))],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Public,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        };
        let mut program = IrProgram {
            declarations: vec![IrDecl::new(IrDeclKind::Impl(IrImpl {
                target_type: "Boxed".to_string(),
                type_params: vec![IrTypeParam::bare("T")],
                trait_name: None,
                trait_type_args: Vec::new(),
                associated_types: Vec::new(),
                methods: vec![method],
            }))],
            source_module_name: None,
            entry_point: None,
            function_registry: FunctionRegistry::new(),
            function_reexports: Vec::new(),
            rust_module_path: None,
            newtype_checked_ctor: Default::default(),
        };

        infer_trait_bounds(&mut program);

        let decl = program
            .declarations
            .first()
            .ok_or_else(|| std::io::Error::other("expected impl declaration"))?;
        let IrDecl {
            kind: IrDeclKind::Impl(impl_block),
            ..
        } = decl
        else {
            return Err(std::io::Error::other("expected impl declaration").into());
        };
        let bounds = &impl_block.type_params[0].bounds;
        assert!(
            bounds.contains(&IrTraitBound::simple(tb::DISPLAY)),
            "owner generic T should receive Display bound from impl method body, got {bounds:?}"
        );
        assert!(
            impl_block.methods[0].type_params.is_empty(),
            "impl-owner generics must stay on the impl header, not the method signature"
        );
        Ok(())
    }

    #[test]
    fn generic_value_field_reflection_methods_infer_field_value_bound() -> Result<(), Box<dyn std::error::Error>> {
        let generic_receiver = || {
            Box::new(TypedExpr::new(
                IrExprKind::Var {
                    name: "value".to_string(),
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                IrType::Generic("T".to_string()),
            ))
        };
        let mut func = function("reflect_values", vec![IrTypeParam::bare("T")]);
        func.body = vec![
            IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: generic_receiver(),
                    method: "__field_value__".to_string(),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Literal(Literal::StaticStr("name".to_string())),
                            IrType::StaticStr,
                        ),
                    }],
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Option(Box::new(IrType::String)),
            ))),
            IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: generic_receiver(),
                    method: "__field_items__".to_string(),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::List(Box::new(IrType::Tuple(vec![IrType::String, IrType::String]))),
            ))),
        ];
        let mut program = program(vec![func]);

        infer_trait_bounds(&mut program);

        let decl = program
            .declarations
            .first()
            .ok_or_else(|| std::io::Error::other("expected function declaration"))?;
        let IrDecl {
            kind: IrDeclKind::Function(func),
            ..
        } = decl
        else {
            return Err(std::io::Error::other("expected function declaration").into());
        };
        let bounds = &func.type_params[0].bounds;
        assert_eq!(
            bounds,
            &[IrTraitBound::simple(tb::INCAN_FIELD_VALUE_REFLECTION)],
            "generic field value reflection should infer only the field-value reflection bound, got {bounds:?}"
        );
        Ok(())
    }

    #[test]
    fn backend_clone_bounds_do_not_use_incan_policy_for_external_nominal_methods()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut func = function("send", vec![IrTypeParam::bare("T")]);
        func.body = vec![IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "client".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("external_crate::Client".to_string()),
                )),
                method: "send".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "value".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Generic("T".to_string()),
                    ),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unit,
        )))];
        let mut program = program(vec![func]);

        infer_trait_bounds(&mut program);

        let decl = program
            .declarations
            .first()
            .ok_or_else(|| std::io::Error::other("expected function declaration"))?;
        let IrDecl {
            kind: IrDeclKind::Function(func),
            ..
        } = decl
        else {
            return Err(std::io::Error::other("expected function declaration").into());
        };
        assert!(
            func.type_params[0].bounds.is_empty(),
            "external nominal method args should not inherit Incan clone policy, got {:?}",
            func.type_params[0].bounds
        );
        Ok(())
    }

    #[test]
    fn backend_clone_bounds_use_incan_policy_for_methods_with_signatures() -> Result<(), Box<dyn std::error::Error>> {
        let mut func = function("send", vec![IrTypeParam::bare("T")]);
        func.body = vec![IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "client".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Client".to_string()),
                )),
                method: "send".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "value".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Generic("T".to_string()),
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "value".to_string(),
                        ty: IrType::Generic("T".to_string()),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Unit,
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unit,
        )))];
        let mut program = program(vec![func]);

        infer_trait_bounds(&mut program);

        let decl = program
            .declarations
            .first()
            .ok_or_else(|| std::io::Error::other("expected function declaration"))?;
        let IrDecl {
            kind: IrDeclKind::Function(func),
            ..
        } = decl
        else {
            return Err(std::io::Error::other("expected function declaration").into());
        };
        assert!(
            func.type_params[0].bounds.contains(&IrTraitBound::simple(tb::CLONE)),
            "Incan method signatures should keep clone-bound inference aligned with emission, got {:?}",
            func.type_params[0].bounds
        );
        Ok(())
    }

    #[test]
    fn external_generic_bounds_do_not_rewrite_same_named_local_non_generic_function()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut local = program(vec![function("timeout", Vec::new())]);
        let external = program(vec![function(
            "timeout",
            vec![
                IrTypeParam::bare("T"),
                IrTypeParam {
                    name: "TaskFuture".to_string(),
                    bounds: vec![IrTraitBound::with_type_args_classified(
                        "RuntimeFuture".to_string(),
                        vec![IrType::Generic("T".to_string())],
                    )],
                },
            ],
        )]);

        propagate_trait_bounds_from_programs(&mut local, &[&external]);

        let decl = local
            .declarations
            .first()
            .ok_or_else(|| std::io::Error::other("expected function declaration"))?;
        let IrDecl {
            kind: IrDeclKind::Function(func),
            ..
        } = decl
        else {
            return Err(std::io::Error::other("expected function declaration").into());
        };
        assert!(
            func.type_params.is_empty(),
            "local non-generic timeout should not inherit external generic bounds"
        );
        Ok(())
    }
}
