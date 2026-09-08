//! Conservative shared-parameter inference for closed, directly called helpers.
//!
//! This pass runs before emission and changes declaration and call signatures together. Public functions, trait
//! slots, escaping callable values, and unsupported uses retain their authored ABI. Borrow selection never follows
//! a Rust method spelling: foreign calls require the receiver contract retained by checking.

use std::collections::{HashMap, HashSet};

use incan_core::lang::keywords::{self, KeywordId};

use super::decl::Visibility;
use super::expr::{MatchArm, Pattern};
use super::scanners::expr_uses_binding_name;
use super::stmt::AssignTarget;
use super::visit::{self, Visitor};
use super::{
    FunctionSignature, IrDeclKind, IrExpr, IrExprKind, IrFunction, IrProgram, IrStmt, IrStmtKind, IrType, Mutability,
};

/// Infer closed helper signatures to a fixed point, then publish the same signatures to every direct call.
pub(super) fn infer_shared_helpers(
    program: &mut IrProgram,
    contracts: &HashMap<(usize, usize), incan_core::interop::RustReceiverContract>,
) {
    let mut escapes = Escapes::default();
    visit_program(program, &mut escapes);
    for reexport in &program.function_reexports {
        if let Some(name) = reexport.target_path.last() {
            escapes.names.insert(name.clone());
        }
    }
    let mut changed = HashMap::new();
    loop {
        let mut progress = false;
        for decl in &mut program.declarations {
            let IrDeclKind::Function(function) = &mut decl.kind else {
                continue;
            };
            if !matches!(function.visibility, Visibility::Private)
                || function.is_extern
                || function.is_async
                || function.is_generator
                || !function.rust_attributes.is_empty()
                || escapes.names.contains(&function.name)
                || program
                    .function_registry
                    .emitted_projection(&function.name)
                    .is_some_and(|name| escapes.names.contains(&name))
                || program
                    .function_registry
                    .source_name(&function.name)
                    .is_some_and(|name| escapes.names.contains(name))
            {
                continue;
            }
            progress |= infer_parameters(function, contracts);
            if function.params.iter().any(|param| matches!(param.ty, IrType::Ref(_))) {
                changed.insert(
                    function.name.clone(),
                    FunctionSignature {
                        params: function.params.clone(),
                        return_type: function.return_type.clone(),
                    },
                );
            }
        }
        if !progress {
            break;
        }
        let projections = program
            .function_registry
            .iter()
            .filter_map(|(name, _)| {
                let projected = program.function_registry.emitted_projection(name)?;
                changed.get(name).cloned().map(|signature| (projected, signature))
            })
            .collect::<Vec<_>>();
        changed.extend(projections);
        for (name, signature) in &changed {
            program
                .function_registry
                .register(name.clone(), signature.params.clone(), signature.return_type.clone());
        }
        visit_program(program, &mut Calls { signatures: &changed });
    }
    infer_crate_helpers(program, contracts);
    infer_method_helpers(program, contracts);
    for decl in &mut program.declarations {
        match &mut decl.kind {
            IrDeclKind::Function(function) => infer_local_cursors(function, contracts),
            IrDeclKind::Impl(implementation) => {
                for method in &mut implementation.methods {
                    infer_local_cursors(method, contracts);
                }
            }
            _ => {}
        }
    }
}

/// Preserve crate-visible stdlib entry points while refining closed direct calls inside their owning module.
/// Sibling modules and compiled-provider consumers continue to call the authored owned ABI.
fn infer_crate_helpers(
    program: &mut IrProgram,
    contracts: &HashMap<(usize, usize), incan_core::interop::RustReceiverContract>,
) {
    let mut planned = HashSet::new();
    let mut names: HashSet<String> = program.function_registry.iter().map(|(name, _)| name.clone()).collect();
    loop {
        let mut helpers = Vec::new();
        let mut plans = HashMap::new();
        for declaration in &mut program.declarations {
            let IrDeclKind::Function(function) = &mut declaration.kind else {
                continue;
            };
            if !matches!(function.visibility, Visibility::Crate)
                || planned.contains(&function.name)
                || function.is_extern
                || function.is_async
                || function.is_generator
                || !function.rust_attributes.is_empty()
            {
                continue;
            }
            let mut helper = function.clone();
            if !infer_parameters(&mut helper, contracts) {
                continue;
            }
            let mut index = names.len();
            let name = loop {
                let candidate = format!("__incan_shared_function_{index}");
                if names.insert(candidate.clone()) {
                    break candidate;
                }
                index += 1;
            };
            helper.name = name.clone();
            helper.visibility = Visibility::Private;
            let signature = FunctionSignature {
                params: helper.params.clone(),
                return_type: helper.return_type.clone(),
            };
            let call = IrExpr::new(
                IrExprKind::Call {
                    func: Box::new(IrExpr::new(
                        IrExprKind::Var {
                            name: name.clone(),
                            access: super::expr::VarAccess::Read,
                            ref_kind: super::expr::VarRefKind::Value,
                        },
                        IrType::Unknown,
                    )),
                    type_args: function
                        .type_params
                        .iter()
                        .map(|param| IrType::Generic(param.name.clone()))
                        .collect(),
                    args: forwarding_arguments(function),
                    callable_signature: Some(signature.clone()),
                    canonical_path: None,
                },
                function.return_type.clone(),
            );
            function.body = vec![IrStmt::new(IrStmtKind::Return(Some(call)))];
            planned.insert(function.name.clone());
            plans.insert(function.name.clone(), (name.clone(), signature.clone()));
            if let Some(projection) = program.function_registry.emitted_projection(&function.name) {
                plans.insert(projection, (name.clone(), signature.clone()));
            }
            program
                .function_registry
                .register(name, signature.params, signature.return_type);
            let mut helper_declaration = declaration.clone();
            helper_declaration.kind = IrDeclKind::Function(helper);
            helpers.push(helper_declaration);
        }
        if helpers.is_empty() {
            break;
        }
        program.declarations.extend(helpers);
        visit_program(program, &mut SharedCalls { plans: &plans });
    }
}

/// Forward wrapper arguments through the existing call ownership planner using their authored types.
fn forwarding_arguments(function: &IrFunction) -> Vec<super::expr::IrCallArg> {
    function
        .params
        .iter()
        .filter(|param| !param.is_self)
        .map(|param| super::expr::IrCallArg {
            name: None,
            kind: super::expr::IrCallArgKind::Positional,
            expr: IrExpr::new(
                IrExprKind::Var {
                    name: param.name.clone(),
                    access: super::expr::VarAccess::Move,
                    ref_kind: super::expr::VarRefKind::Value,
                },
                param.ty.clone(),
            ),
        })
        .collect()
}

/// Redirect only direct calls; callback values and reexports continue to name the original ABI.
struct SharedCalls<'a> {
    plans: &'a HashMap<String, (String, FunctionSignature)>,
}

impl Visitor for SharedCalls<'_> {
    /// Select a private implementation only when sibling argument evaluation cannot invalidate its borrow.
    fn expr(&mut self, expr: &mut IrExpr) {
        if let IrExprKind::Call {
            func,
            args,
            callable_signature,
            canonical_path,
            ..
        } = &mut expr.kind
            && independent_arguments(args)
            && let IrExprKind::Var { name, .. } = &mut func.kind
            && let Some((helper, signature)) = self.plans.get(name)
        {
            *name = helper.clone();
            *canonical_path = None;
            *callable_signature = Some(signature.clone());
        }
        visit::walk_expr(expr, self);
    }
}

/// Prove and refine candidate parameters without changing the callable's externally visible identity.
fn infer_parameters(
    function: &mut IrFunction,
    contracts: &HashMap<(usize, usize), incan_core::interop::RustReceiverContract>,
) -> bool {
    if function
        .params
        .iter()
        .any(|param| param.kind != crate::frontend::ast::ParamKind::Normal)
    {
        return false;
    }
    let mut changed = false;
    let mut counts = BindingCounts::default();
    visit_function(function, &mut counts);
    for param in &function.params {
        *counts.names.entry(param.name.clone()).or_default() += 1;
    }
    for param in &mut function.params {
        if param.is_self
            || param.mutability != Mutability::Immutable
            || param.default.is_some()
            || !matches!(
                param.ty,
                IrType::Struct(_) | IrType::Enum(_) | IrType::NamedGeneric(_, _)
            )
        {
            continue;
        }
        let mut proof = SharedUse {
            root: &param.name,
            aliases: HashSet::from([param.name.clone()]),
            valid: true,
            observed: false,
            contracts,
            allow_root_reassign: false,
        };
        for stmt in &mut function.body {
            proof.stmt(stmt);
        }
        if !proof.valid
            || !proof.observed
            || proof
                .aliases
                .iter()
                .any(|name| counts.names.get(name).copied() != Some(1))
        {
            continue;
        }
        param.ty = IrType::Ref(Box::new(param.ty.clone()));
        let mut rewrite = BorrowBinding {
            aliases: &proof.aliases,
        };
        for stmt in &mut function.body {
            rewrite.stmt(stmt);
        }
        changed = true;
    }
    changed
}

/// Preserve inherent method ABI by emitting a private borrowed implementation for proven local direct calls.
fn infer_method_helpers(
    program: &mut IrProgram,
    contracts: &HashMap<(usize, usize), incan_core::interop::RustReceiverContract>,
) {
    let mut plans = HashMap::new();
    for decl in &mut program.declarations {
        let IrDeclKind::Impl(implementation) = &mut decl.kind else {
            continue;
        };
        if implementation.trait_name.is_some() {
            continue;
        }
        let mut names: HashSet<String> = implementation
            .methods
            .iter()
            .map(|method| method.name.clone())
            .collect();
        names.extend(
            implementation
                .source_method_projections
                .iter()
                .map(|projection| projection.source_name.clone()),
        );
        let mut helpers = Vec::new();
        for method in &mut implementation.methods {
            if method.is_extern
                || method.is_async
                || method.is_generator
                || !method.rust_attributes.is_empty()
                || !method
                    .params
                    .iter()
                    .any(|param| param.is_self && param.mutability == Mutability::Immutable)
            {
                continue;
            }
            let mut helper = method.clone();
            if !infer_parameters(&mut helper, contracts) {
                continue;
            }
            let mut index = helpers.len();
            let name = loop {
                let candidate = format!("__incan_shared_method_{index}");
                if names.insert(candidate.clone()) {
                    break candidate;
                }
                index += 1;
            };
            helper.name = name.clone();
            helper.visibility = Visibility::Private;
            let signature = FunctionSignature {
                params: helper.params.iter().filter(|param| !param.is_self).cloned().collect(),
                return_type: helper.return_type.clone(),
            };
            let receiver = IrExpr::new(
                IrExprKind::Var {
                    name: keywords::as_str(KeywordId::SelfKw).to_string(),
                    access: super::expr::VarAccess::Read,
                    ref_kind: super::expr::VarRefKind::Value,
                },
                IrType::Ref(Box::new(IrType::Struct(implementation.target_type.clone()))),
            );
            let args = forwarding_arguments(method);
            let call = IrExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: name.clone(),
                    dispatch: None,
                    type_args: method
                        .type_params
                        .iter()
                        .map(|param| IrType::Generic(param.name.clone()))
                        .collect(),
                    args,
                    callable_signature: Some(signature.clone()),
                    arg_policy: super::expr::MethodCallArgPolicy::Default,
                },
                method.return_type.clone(),
            );
            method.body = vec![IrStmt::new(IrStmtKind::Return(Some(call)))];
            plans.insert(
                (implementation.target_type.clone(), method.name.clone()),
                (name, signature),
            );
            helpers.push(helper);
        }
        implementation.methods.extend(helpers);
    }
    for decl in &mut program.declarations {
        match &mut decl.kind {
            IrDeclKind::Function(function) => visit_function(
                function,
                &mut MethodCalls {
                    owner: None,
                    plans: &plans,
                },
            ),
            IrDeclKind::Impl(implementation) => {
                for method in &mut implementation.methods {
                    visit_function(
                        method,
                        &mut MethodCalls {
                            owner: Some(&implementation.target_type),
                            plans: &plans,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

/// Direct-call remapping is confined to a proven nominal receiver in this module; trait/callback surfaces keep ABI.
struct MethodCalls<'a> {
    owner: Option<&'a str>,
    plans: &'a HashMap<(String, String), (String, FunctionSignature)>,
}

impl Visitor for MethodCalls<'_> {
    /// Resolve a local method by owner and exact lowered method identity before selecting its borrowed helper.
    fn expr(&mut self, expr: &mut IrExpr) {
        if let IrExprKind::MethodCall {
            receiver,
            method,
            dispatch: None,
            callable_signature,
            args,
            ..
        } = &mut expr.kind
        {
            if !independent_arguments(args) {
                visit::walk_expr(expr, self);
                return;
            }
            let owner = if matches!(&receiver.kind, IrExprKind::Var { name, .. } if name == keywords::as_str(KeywordId::SelfKw))
            {
                self.owner
            } else {
                receiver.ty.nominal_type_name()
            };
            if let Some(owner) = owner
                && let Some((helper, signature)) = self.plans.get(&(owner.to_string(), method.clone()))
            {
                *method = helper.clone();
                *callable_signature = Some(signature.clone());
            }
        }
        visit::walk_expr(expr, self);
    }
}

/// Borrow a local tree place when all later uses are shared and its owner is not accessed again in the body.
/// This deliberately excludes temporaries and overlapping owner uses; the source retains owned semantics there.
fn infer_local_cursors(
    function: &mut IrFunction,
    contracts: &HashMap<(usize, usize), incan_core::interop::RustReceiverContract>,
) {
    if function.is_async || function.is_generator || function.is_extern {
        return;
    }
    let mut counts = BindingCounts::default();
    visit_function(function, &mut counts);
    for param in &function.params {
        *counts.names.entry(param.name.clone()).or_default() += 1;
    }
    for index in 0..function.body.len() {
        let (prefix, tail) = function.body.split_at_mut(index + 1);
        let Some(statement) = prefix.last_mut() else {
            continue;
        };
        let IrStmtKind::Let {
            name,
            ty,
            type_annotation: None,
            value,
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        if !matches!(ty, IrType::Struct(_) | IrType::Enum(_) | IrType::NamedGeneric(_, _)) {
            continue;
        }
        let owner = match &value.kind {
            IrExprKind::Var { name, .. } => name.as_str(),
            IrExprKind::Field { object, .. } => match &object.kind {
                IrExprKind::Var { name, .. } => name.as_str(),
                _ => continue,
            },
            _ => continue,
        };
        let mut root_uses = RootUses {
            name: owner,
            used: false,
        };
        for stmt in tail.iter_mut() {
            root_uses.stmt(stmt);
        }
        if root_uses.used {
            continue;
        }
        let mut proof = SharedUse {
            root: name,
            aliases: HashSet::from([name.clone()]),
            valid: true,
            observed: false,
            contracts,
            allow_root_reassign: true,
        };
        for stmt in tail.iter_mut() {
            proof.stmt(stmt);
        }
        if !proof.valid
            || !proof.observed
            || proof
                .aliases
                .iter()
                .any(|name| counts.names.get(name).copied() != Some(1))
        {
            continue;
        }
        borrow_type(ty);
        let mut rewrite = BorrowBinding {
            aliases: &proof.aliases,
        };
        for stmt in tail {
            rewrite.stmt(stmt);
        }
    }
}

/// Reject inferred local aliasing when the owner is subsequently read, moved, rebound or mutated.
struct RootUses<'a> {
    name: &'a str,
    used: bool,
}

impl Visitor for RootUses<'_> {
    /// Assignment targets are uses of storage even when the target variable has no expression node.
    fn stmt(&mut self, stmt: &mut IrStmt) {
        match &stmt.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Var { name, .. },
                ..
            }
            | IrStmtKind::CompoundAssign {
                target: AssignTarget::Var { name, .. },
                ..
            } if name == self.name => self.used = true,
            IrStmtKind::Let { name, .. } if name == self.name => self.used = true,
            _ => {}
        }
        visit::walk_stmt(stmt, self);
    }
    /// Include nested callback/closure capture and place expressions without relying on last-use heuristics.
    fn expr(&mut self, expr: &mut IrExpr) {
        if expr_uses_binding_name(expr, self.name) {
            self.used = true;
        }
        visit::walk_expr(expr, self);
    }
}

/// Visit executable bodies and initializer expressions that can retain a function value.
fn visit_program(program: &mut IrProgram, visitor: &mut impl Visitor) {
    for stmt in &mut program.module_init {
        visitor.stmt(stmt);
    }
    for decl in &mut program.declarations {
        match &mut decl.kind {
            IrDeclKind::Function(function) => visit_function(function, visitor),
            IrDeclKind::Impl(implementation) => {
                for function in &mut implementation.methods {
                    visit_function(function, visitor);
                }
            }
            IrDeclKind::Trait(declaration) => {
                for method in &mut declaration.methods {
                    visit_function(method, visitor);
                }
            }
            IrDeclKind::Struct(declaration) => {
                for field in &mut declaration.fields {
                    if let Some(value) = &mut field.default {
                        visitor.expr(value);
                    }
                }
            }
            IrDeclKind::SymbolAlias { target_path, .. } => {
                if let Some(name) = target_path.last() {
                    visitor.expr(&mut IrExpr::new(
                        IrExprKind::FunctionItem {
                            name: name.clone(),
                            type_args: Vec::new(),
                        },
                        IrType::Unknown,
                    ));
                }
            }
            IrDeclKind::Const { value, .. } | IrDeclKind::Static { value, .. } => visitor.expr(value),
            _ => {}
        }
    }
}

/// Include default expressions, which are callable-value escape sites just like function bodies.
fn visit_function(function: &mut IrFunction, visitor: &mut impl Visitor) {
    for param in &mut function.params {
        if let Some(super::decl::FunctionParamDefault::Source(value)) = &mut param.default {
            visitor.expr(value);
        }
    }
    for stmt in &mut function.body {
        visitor.stmt(stmt);
    }
}

/// Function identifiers outside a direct callee position must keep their existing callable ABI.
#[derive(Default)]
struct Escapes {
    names: HashSet<String>,
}

impl Visitor for Escapes {
    /// Skip only direct callee identifiers; closure captures, partials and callback arguments remain escapes.
    fn expr(&mut self, expr: &mut IrExpr) {
        match &mut expr.kind {
            IrExprKind::Call { func, args, .. } if matches!(func.kind, IrExprKind::Var { .. }) => {
                if !independent_arguments(args) {
                    self.expr(func);
                }
                for arg in args {
                    self.expr(&mut arg.expr);
                }
            }
            IrExprKind::Var { name, .. } | IrExprKind::FunctionItem { name, .. } => {
                self.names.insert(name.clone());
            }
            _ => visit::walk_expr(expr, self),
        }
    }
}

/// Keep owned call ABI when another argument may consume the same storage or mutate it during evaluation.
/// Simple independent places are enough for helper and tree traversal calls; more complex calls fail closed.
fn independent_arguments(args: &[super::expr::IrCallArg]) -> bool {
    if args.len() < 2 {
        return true;
    }
    let mut roots = HashSet::new();
    for arg in args {
        let mut argument_roots = HashSet::new();
        if !simple_argument_roots(&arg.expr, &mut argument_roots)
            || argument_roots.iter().any(|name| roots.contains(name))
        {
            return false;
        }
        roots.extend(argument_roots);
    }
    true
}

/// Collect storage roots only through pure place and literal construction; calls and operators remain unknown.
fn simple_argument_roots<'a>(value: &'a IrExpr, roots: &mut HashSet<&'a str>) -> bool {
    match &value.kind {
        IrExprKind::Var { name, .. } => {
            roots.insert(name);
            true
        }
        IrExprKind::Field { object, .. } => simple_argument_roots(object, roots),
        IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Bool(_)
        | IrExprKind::String(_)
        | IrExprKind::Literal(_)
        | IrExprKind::Unit => true,
        IrExprKind::List(items) => items.iter().all(|item| match item {
            super::expr::IrListEntry::Element(value) => simple_argument_roots(value, roots),
            _ => false,
        }),
        _ => false,
    }
}

/// Count declarations before inference so a later alias never rewrites an earlier same-spelled local.
#[derive(Default)]
struct BindingCounts {
    names: HashMap<String, usize>,
}

impl Visitor for BindingCounts {
    /// Count statement-owned declarations before visiting nested bodies.
    fn stmt(&mut self, stmt: &mut IrStmt) {
        if let IrStmtKind::Let { name, .. } = &stmt.kind {
            *self.names.entry(name.clone()).or_default() += 1;
        }
        visit::walk_stmt(stmt, self);
    }

    /// Count closure parameters as well as their body-local declarations.
    fn expr(&mut self, expr: &mut IrExpr) {
        if let IrExprKind::Closure { params, .. } = &expr.kind {
            for (name, _) in params {
                *self.names.entry(name.clone()).or_default() += 1;
            }
        }
        visit::walk_expr(expr, self);
    }

    /// Include bindings in matches, loops, comprehensions, and generator clauses.
    fn pattern(&mut self, pattern: &mut Pattern) {
        if let Pattern::Var(name) = pattern {
            *self.names.entry(name.clone()).or_default() += 1;
        }
        visit::walk_pattern(pattern, self);
    }
}

/// A proof for one root and its aliases. Every accepted alias stays within the root's lifetime.
struct SharedUse<'a> {
    root: &'a str,
    allow_root_reassign: bool,
    aliases: HashSet<String>,
    valid: bool,
    observed: bool,
    contracts: &'a HashMap<(usize, usize), incan_core::interop::RustReceiverContract>,
}

impl SharedUse<'_> {
    /// Check all currently known aliases, including borrowed payloads introduced by a match.
    fn uses(&self, expr: &IrExpr) -> bool {
        self.aliases.iter().any(|name| expr_uses_binding_name(expr, name))
    }

    /// A direct alias preserves a known root; arbitrary field/method chains do not establish lifetime equality.
    fn alias(&self, expr: &IrExpr) -> bool {
        matches!(&expr.kind, IrExprKind::Var { name, .. } if self.aliases.contains(name))
    }

    /// A returned child may replace a cursor only when checking retained its receiver lifetime relationship.
    fn descendant(&self, expr: &IrExpr) -> bool {
        matches!(&expr.kind, IrExprKind::MethodCall { receiver, .. } if self.alias(receiver))
            && (matches!(&expr.ty, IrType::Ref(_))
                || matches!(&expr.ty, IrType::Option(inner) if matches!(inner.as_ref(), IrType::Ref(_))))
            && self
                .contracts
                .get(&(expr.span.start, expr.span.end))
                .is_some_and(|contract| contract.shared && contract.returns_receiver_borrow)
    }

    /// Match ergonomics bind payloads by reference when the scrutinee is a proven root or descendant.
    fn classify(&mut self, scrutinee: &mut IrExpr, arms: &mut [MatchArm]) {
        let borrowed = self.alias(scrutinee) || self.descendant(scrutinee);
        if self.uses(scrutinee) && !borrowed {
            // A shared observation can produce an independent enum such as an optional source span.
            self.expr(scrutinee);
            if !self.valid {
                return;
            }
        }
        if borrowed {
            self.observed = true;
        }
        // Method arguments can themselves consume an alias even when the receiver is shared.
        if borrowed && let IrExprKind::MethodCall { args, .. } = &mut scrutinee.kind {
            for arg in args {
                self.expr(&mut arg.expr);
            }
        }
        for arm in arms {
            if !arm.bindings.is_empty() {
                self.valid = false;
                return;
            }
            let mut names = Vec::new();
            pattern_names(&arm.pattern, &mut names);
            if names.iter().any(|name| self.aliases.contains(name)) {
                self.valid = false;
                return;
            }
            if borrowed {
                self.aliases.extend(names);
            }
            if let Some(guard) = &mut arm.guard {
                self.expr(guard);
            }
            self.expr(&mut arm.body);
        }
    }
}

impl Visitor for SharedUse<'_> {
    /// Accept cursor reassignments only between aliases of the same root. Rebinding the root stays owned.
    fn stmt(&mut self, stmt: &mut IrStmt) {
        match &mut stmt.kind {
            IrStmtKind::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
                if self.aliases.contains(name) {
                    self.valid = false;
                    return;
                }
                if self.alias(value) && type_annotation.is_none() {
                    self.aliases.insert(name.clone());
                    self.observed = true;
                } else {
                    self.expr(value);
                }
            }
            IrStmtKind::Assign {
                target: AssignTarget::Var { name, .. },
                value,
            } if self.aliases.contains(name) => {
                if (name == self.root && !self.allow_root_reassign) || !self.alias(value) {
                    self.valid = false;
                }
            }
            IrStmtKind::CompoundAssign {
                target: AssignTarget::Var { name, .. },
                ..
            } if self.aliases.contains(name) => self.valid = false,
            IrStmtKind::For { pattern, .. } => {
                let mut names = Vec::new();
                pattern_names(pattern, &mut names);
                if names.iter().any(|name| self.aliases.contains(name)) {
                    self.valid = false;
                }
                visit::walk_stmt(stmt, self);
            }
            IrStmtKind::Match { scrutinee, arms } => self.classify(scrutinee, arms),
            _ => visit::walk_stmt(stmt, self),
        }
    }

    /// Reject escapes and unknown consumers; shared methods and already-planned helper parameters may observe aliases.
    fn expr(&mut self, expr: &mut IrExpr) {
        if !self.uses(expr) {
            return;
        }
        let contract = self.contracts.get(&(expr.span.start, expr.span.end)).copied();
        let borrowed_result = contains_reference(&expr.ty);
        match &mut expr.kind {
            IrExprKind::Match { scrutinee, arms } => self.classify(scrutinee, arms),
            IrExprKind::Block { .. }
            | IrExprKind::If { .. }
            | IrExprKind::Loop { .. }
            | IrExprKind::BinOp { .. }
            | IrExprKind::UnaryOp { .. }
            | IrExprKind::Struct { .. }
            | IrExprKind::Try(_) => visit::walk_expr(expr, self),
            IrExprKind::MethodCall { receiver, args, .. } if self.alias(receiver) => {
                if borrowed_result || !contract.is_some_and(|fact| fact.shared && !fact.returns_receiver_borrow) {
                    self.valid = false;
                    return;
                }
                self.observed = true;
                for arg in args {
                    self.expr(&mut arg.expr);
                }
            }
            IrExprKind::Call {
                func,
                args,
                callable_signature,
                ..
            } => {
                if self.uses(func) {
                    self.valid = false;
                    return;
                }
                for (index, arg) in args.iter_mut().enumerate() {
                    if !self.uses(&arg.expr) {
                        continue;
                    }
                    if self.alias(&arg.expr) {
                        let param = callable_signature.as_ref().and_then(|signature| match &arg.name {
                            Some(name) => signature.params.iter().find(|p| &p.name == name),
                            None => signature.params.get(index),
                        });
                        if param.is_some_and(|p| matches!(p.ty, IrType::Ref(_)))
                            && callable_signature
                                .as_ref()
                                .is_some_and(|sig| !contains_reference(&sig.return_type))
                        {
                            self.observed = true;
                        } else {
                            self.valid = false;
                        }
                    } else {
                        self.expr(&mut arg.expr);
                    }
                }
            }
            _ => self.valid = false,
        }
    }
}

/// Gather bound names structurally; spelling does not determine whether a payload is borrowed.
fn pattern_names(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Var(name) => names.push(name.clone()),
        Pattern::Enum { fields, .. } | Pattern::Tuple(fields) | Pattern::Or(fields) => {
            for field in fields {
                pattern_names(field, names);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, field) in fields {
                pattern_names(field, names);
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

/// Do not authorize a helper call that can return a borrowed or unresolved value carrying the candidate out.
fn contains_reference(ty: &IrType) -> bool {
    match ty {
        IrType::Bool
        | IrType::Int
        | IrType::Float
        | IrType::Numeric(_)
        | IrType::Decimal { .. }
        | IrType::Unit
        | IrType::String
        | IrType::Enum(_)
        | IrType::Struct(_) => false,
        IrType::Option(inner) | IrType::List(inner) => contains_reference(inner),
        IrType::Result(ok, err) | IrType::Dict(ok, err) => contains_reference(ok) || contains_reference(err),
        IrType::Tuple(items) | IrType::NamedGeneric(_, items) => items.iter().any(contains_reference),
        _ => true,
    }
}

/// Rewrite bindings only after the complete body has proven one shared root and no escaping aliases.
struct BorrowBinding<'a> {
    aliases: &'a HashSet<String>,
}

impl Visitor for BorrowBinding<'_> {
    /// Alias destinations must carry the same borrow shape as their initializer and later reassignments.
    fn stmt(&mut self, stmt: &mut IrStmt) {
        match &mut stmt.kind {
            IrStmtKind::Let { name, ty, .. }
            | IrStmtKind::Assign {
                target: AssignTarget::Var { name, ty },
                ..
            } if self.aliases.contains(name) => {
                borrow_type(ty);
            }
            _ => {}
        }
        visit::walk_stmt(stmt, self);
    }
    /// Pattern-bound descendants already have native reference shape; typed uses retain that fact without cloning.
    fn expr(&mut self, expr: &mut IrExpr) {
        if matches!(&expr.kind, IrExprKind::Var { name, .. } if self.aliases.contains(name)) {
            borrow_type(&mut expr.ty);
        }
        visit::walk_expr(expr, self);
    }
}

/// Add exactly one shared-reference wrapper, preserving an already borrowed payload.
fn borrow_type(ty: &mut IrType) {
    if !matches!(ty, IrType::Ref(_) | IrType::RefMut(_)) {
        *ty = IrType::Ref(Box::new(ty.clone()));
    }
}

/// Publish declaration signatures to direct call sites before ownership and Clone-bound planning.
struct Calls<'a> {
    signatures: &'a HashMap<String, FunctionSignature>,
}

impl Visitor for Calls<'_> {
    /// Follow exact lowered callee identity, never a same-spelled method or imported alias.
    fn expr(&mut self, expr: &mut IrExpr) {
        if let IrExprKind::Call {
            func,
            callable_signature,
            ..
        } = &mut expr.kind
            && let IrExprKind::Var { name, .. } = &func.kind
            && let Some(signature) = self.signatures.get(name)
        {
            *callable_signature = Some(signature.clone());
        }
        visit::walk_expr(expr, self);
    }
}
