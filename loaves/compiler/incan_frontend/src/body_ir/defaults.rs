//! Lowering a declared parameter default into the form a direct consumer can supply for an omitted argument.

use super::*;

/// Prove that a declaration-owned default constructs only fresh literal data, including checked enum variants.
/// Publication preserves this proof without source syntax; calls, parameter reads and writes to existing storage
/// cannot cross a supplied argument's observable evaluation.
pub(super) fn is_literal_default(default: &bir::CallableParamDefault) -> bool {
    let bir::CallableParamDefault::Source(computation) = default else {
        return false;
    };
    let mut initialized = HashSet::new();
    for statement in &computation.stmts {
        let bir::StatementKind::Assign { place, rvalue } = &statement.kind else {
            return false;
        };
        let bir::PlaceRoot::Local(local) = place.root else {
            return false;
        };
        if !place.projection.is_empty() || initialized.contains(&local) || !literal_default_rvalue(rvalue, &initialized)
        {
            return false;
        }
        initialized.insert(local);
    }
    literal_default_operand(&computation.result, &initialized)
}

/// Admit literal containers and checked enum variants whose payloads refer only to constants or fresh temporaries.
/// Variant construction has no user callable; retained enum targets supply its declaration-owned layout.
fn literal_default_rvalue(value: &bir::Rvalue, initialized: &HashSet<bir::LocalId>) -> bool {
    match value {
        bir::Rvalue::Use(operand) => literal_default_operand(operand, initialized),
        bir::Rvalue::FieldlessEnumVariant(_) | bir::Rvalue::ValueEnumVariant(_) => true,
        bir::Rvalue::Aggregate(
            bir::AggregateKind::List
            | bir::AggregateKind::Set
            | bir::AggregateKind::Tuple
            | bir::AggregateKind::EnumVariant(_),
            elements,
        ) => elements.iter().all(|element| {
            matches!(element, bir::ArgumentElement::One(operand) if literal_default_operand(operand, initialized))
        }),
        bir::Rvalue::Dict(entries) => entries.iter().all(|entry| {
            matches!(entry, bir::DictEntry::Pair(key, value)
                if literal_default_operand(key, initialized) && literal_default_operand(value, initialized))
        }),
        _ => false,
    }
}

/// Accept immutable constant data and fresh whole temporaries, excluding parameter reads and projections.
fn literal_default_operand(value: &bir::Operand, initialized: &HashSet<bir::LocalId>) -> bool {
    match value {
        bir::Operand::Constant(_) => true,
        bir::Operand::Place(value) => {
            value.place.projection.is_empty()
                && matches!(value.place.root, bir::PlaceRoot::Local(local) if initialized.contains(&local))
        }
    }
}

/// Retain each admitted model or class's field defaults in a declaration-owned frame, evaluated only for omitted slots.
pub(super) fn attach_nominal_field_defaults(
    program: &ast::Program,
    declarations: &mut [bir::NominalDeclaration],
    facts: &BodyIrLoweringFacts<'_, '_>,
) {
    for source in &program.declarations {
        let (name, fields) = match &source.node {
            ast::Declaration::Model(model) => (&model.name, &model.fields),
            ast::Declaration::Class(class) => (&class.name, &class.fields),
            _ => continue,
        };
        let Some(declaration) = declarations.iter_mut().find(|value| value.name == *name) else {
            continue;
        };
        if !declaration.has_field_defaults {
            continue;
        }

        // ---- Declaration-local frame ----
        let span = hir_span(source.span);
        let mut builder = BodyBuilder::new(
            facts,
            IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Unit),
        );
        builder.type_parameters = declaration.type_parameters.clone();
        let scope = builder.new_scope(None, span);

        // ---- Canonical field slots and deferred computations ----
        let mut params = Vec::new();
        for (field, ty) in fields.iter().zip(&declaration.field_types) {
            let local =
                builder.declare_new_local(field.node.name.clone(), ty.clone(), scope, hir_span(field.span), &[]);
            builder.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            params.push(bir::CallableParam {
                local,
                kind: IncanCallableParamKind::Normal,
                name: field.node.name.clone(),
                ty: ty.clone(),
                span: hir_span(field.span),
                default: builder.lower_callable_default(field.node.default.as_ref(), scope),
                mutable: false,
                native_owned: false,
            });
        }

        // ---- Owned frame metadata; construction selects computations, never this empty block ----
        if builder
            .locals
            .iter()
            .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
        {
            builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        }
        let named_type_identities =
            builder.finish_type_identities(facts.type_info.declarations.named_type_identities.clone());
        work::built_body(&declaration.direct_declaration_id, "field_defaults");
        declaration.field_default_body = Some(Box::new(bir::Body {
            trait_default_instantiation: None,
            nominal_instantiation: Vec::new(),
            type_parameters: Vec::new(),
            decl_id: declaration.direct_declaration_id.clone(),
            direct_call_id: declaration.direct_declaration_id.clone(),
            canonical: Some(declaration.canonical.clone()),
            name: declaration.name.clone(),
            span,
            return_type: IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Unit),
            // Field slots carry no checker callable-representation proof, so consumers refuse function-typed fields.
            callable_representation: None,
            named_type_identities,
            param_locals: params.iter().map(|param| param.local).collect(),
            params,
            locals: builder.locals,
            scopes: builder.scopes,
            block: bir::Block {
                scope,
                stmts: Vec::new(),
            },
            runtime_requirements: builder.runtime_requirements,
            panic_facts: builder.panic_facts,
            is_async: false,
            extern_delegation: None,
            native_attributes: Vec::new(),
        }));
    }
}

/// Retain each admitted model or class's method partials as frames of their target methods whose defaults are the
/// presets (see [`bir::MethodPartialDefaults`]).
///
/// The frame is the target method lowered with an empty block and each preset as its parameter's default, so a preset
/// is evaluated exactly as a source default is, only where a call omits it. A partial whose target or checked identity
/// is unavailable retains no frame; a call that omits its preset then refuses at lowering.
pub(super) fn attach_method_partial_defaults(
    program: &ast::Program,
    declarations: &mut [bir::NominalDeclaration],
    facts: &BodyIrLoweringFacts<'_, '_>,
) {
    for source in &program.declarations {
        let (name, methods, aliases, partials) = match &source.node {
            ast::Declaration::Model(model) => (
                &model.name,
                &model.methods,
                &model.method_aliases,
                &model.method_partials,
            ),
            ast::Declaration::Class(class) => (
                &class.name,
                &class.methods,
                &class.method_aliases,
                &class.method_partials,
            ),
            _ => continue,
        };
        let Some(declaration) = declarations.iter_mut().find(|value| value.name == *name) else {
            continue;
        };
        for partial in partials {
            if let Some(frame) = method_partial_frame(declaration, methods, aliases, partial, facts) {
                declaration.method_partial_defaults.push(frame);
            }
        }
    }
}

/// Lower one method partial's frame with its retained owner arguments, presets as defaults and an empty block.
/// The receiver is the same checked owner template the target method uses; layout specialization closes both together.
/// The selected physical method must belong to that exact owner origin and declaration extent; package publication
/// changes the origin without changing this ownership relation.
fn method_partial_frame(
    owner: &bir::NominalDeclaration,
    methods: &[ast::Spanned<ast::MethodDecl>],
    aliases: &[ast::Spanned<ast::MethodAliasDecl>],
    partial: &ast::Spanned<ast::MethodPartialDecl>,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> Option<bir::MethodPartialDefaults> {
    let method = method_partial_target(methods, aliases, &partial.node)?;
    let target = facts
        .type_info
        .declarations
        .method_bindings_by_span
        .get(&method.span)?
        .identity
        .clone()?;
    if target.kind != SemanticSourceTargetKind::Method
        || target.namespace != incan_semantics_core::SymbolNamespace::Member
        || target.origin != owner.canonical.origin
        || target.declaration_span.start < owner.canonical.declaration_span.start
        || target.declaration_span.end > owner.canonical.declaration_span.end
    {
        return None;
    }
    let mut frame_source = method.node.clone();
    frame_source.body = Some(Vec::new());
    for preset in &partial.node.args {
        let parameter = frame_source
            .params
            .iter_mut()
            .find(|parameter| parameter.node.name == preset.name)?;
        parameter.node.default = Some(preset.value.clone());
    }
    let receiver = if owner.type_parameters.is_empty() {
        IncanType::Named(owner.name.clone())
    } else {
        IncanType::Generic {
            base: owner.name.clone(),
            args: owner.type_parameters.iter().cloned().map(IncanType::TypeVar).collect(),
        }
    };
    let frame = lower_method_body(&frame_source, method.span, &owner.name, &receiver, facts)?;
    Some(bir::MethodPartialDefaults {
        name: partial.node.name.clone(),
        span: hir_span(partial.span),
        target,
        frame: Box::new(frame),
    })
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower one source-declared default into a deferred Body-IR computation.
    ///
    /// The ordinary function body may not contain this computation: source defaults run only when the matching
    /// parameter is omitted. While lowering it, callable-local bindings are hidden because the legacy path
    /// materializes source defaults while assembling call arguments, before the callee frame is bound. A default
    /// therefore becomes a closed Body-IR computation or a tagged refusal: a callable-local or other external
    /// source read, every explicitly unsupported Body-IR form, and a default without a usable canonical type fact
    /// refuse at the default expression's own span. The final condition is deliberately fail-closed: Body IR may
    /// not make an unchecked source default executable by reconstructing source semantics. This leaves a direct
    /// consumer no reason to consult AST/HIR/typechecker state or legacy execution.
    pub(super) fn lower_callable_default(
        &mut self,
        default_expr: Option<&ast::Spanned<ast::Expr>>,
        scope: bir::ScopeId,
    ) -> bir::CallableParamDefault {
        let Some(default_expr) = default_expr else {
            return bir::CallableParamDefault::Required;
        };

        let locals_len = self.locals.len();
        let scopes_len = self.scopes.len();
        let runtime_requirements_len = self.runtime_requirements.len();
        let panic_facts_len = self.panic_facts.len();
        let next_local = self.next_local;
        let next_scope = self.next_scope;
        let saved_remaining_reads = self.remaining_reads.clone();
        let saved_moved_out = self.moved_out.clone();
        let saved_materialized_range_locals = self.materialized_range_locals.clone();
        let saved_bindings = std::mem::take(&mut self.bindings);
        let saved_identity_bindings = std::mem::take(&mut self.identity_bindings);
        let saved_external_locals = std::mem::take(&mut self.external_locals);
        let mut stmts = Vec::new();
        let result = self.lower_expr_to_operand(default_expr, scope, &mut stmts);
        let mut unresolved_names: Vec<String> = self.external_locals.keys().cloned().collect();
        unresolved_names.sort();
        self.bindings = saved_bindings;
        self.identity_bindings = saved_identity_bindings;
        self.external_locals = saved_external_locals;

        // Expression lowering refuses every validated-newtype coercion; a default names that gap in its own terms.
        let refusal = self
            .type_info
            .validated_newtype_coercion(default_expr.span)
            .is_some()
            .then(|| {
                (
                    hir_span(default_expr.span),
                    "default requires a validated-newtype coercion Body IR does not yet represent".to_string(),
                )
            })
            .or_else(|| first_unsupported_default_statement(&stmts))
            .or_else(|| {
                (!unresolved_names.is_empty()).then(|| {
                    (
                        hir_span(default_expr.span),
                        format!(
                            "default reads Body-IR-external name(s): {}",
                            unresolved_names.join(", ")
                        ),
                    )
                })
            })
            .or_else(|| {
                matches!(
                    self.resolve_ty(default_expr.span),
                    IncanType::Unknown | IncanType::Never
                )
                .then(|| {
                    (
                        hir_span(default_expr.span),
                        "default expression lacks a usable typecheck fact".to_string(),
                    )
                })
            });
        if let Some((span, description)) = refusal {
            self.locals.truncate(locals_len);
            self.scopes.truncate(scopes_len);
            self.runtime_requirements.truncate(runtime_requirements_len);
            self.panic_facts.truncate(panic_facts_len);
            self.next_local = next_local;
            self.next_scope = next_scope;
            self.remaining_reads = saved_remaining_reads;
            self.moved_out = saved_moved_out;
            self.materialized_range_locals = saved_materialized_range_locals;
            return bir::CallableParamDefault::Unsupported { span, description };
        }

        self.materialized_range_locals = saved_materialized_range_locals;
        bir::CallableParamDefault::Source(Box::new(bir::DefaultComputation {
            span: hir_span(default_expr.span),
            stmts,
            result,
        }))
    }

    // ---- Expressions ----
}

/// Return the first explicitly unsupported default statement, preserving the source span a direct consumer must
/// show when it refuses an omitted argument.
///
/// [`BodyBuilder::unsupported_operand`] records every unsupported expression as a
/// [`bir::StatementKind::Unsupported`] statement. Defaults can also nest executable statement sequences inside
/// control-flow, race arms, closures, generators, and match arms, so the scan walks each such sequence before the
/// deferred computation becomes callable metadata.
pub(super) fn first_unsupported_default_statement(stmts: &[bir::Statement]) -> Option<(HirSourceSpan, String)> {
    stmts.iter().find_map(first_unsupported_default_statement_inner)
}
/// Inspect one statement and each rvalue shape that owns a nested executable statement sequence.
pub(super) fn first_unsupported_default_statement_inner(statement: &bir::Statement) -> Option<(HirSourceSpan, String)> {
    match &statement.kind {
        bir::StatementKind::Unsupported { description } => Some((statement.span, description.clone())),
        bir::StatementKind::Assign { rvalue, .. } => first_unsupported_default_rvalue(rvalue),
        bir::StatementKind::If {
            then_block, else_block, ..
        } => first_unsupported_default_statement(&then_block.stmts).or_else(|| {
            else_block
                .as_ref()
                .and_then(|block| first_unsupported_default_statement(&block.stmts))
        }),
        bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
            first_unsupported_default_statement(&body.stmts)
        }
        bir::StatementKind::Race { arms, .. } => arms
            .iter()
            .find_map(|arm| first_unsupported_default_statement(&arm.body.stmts)),
        _ => None,
    }
}
/// Inspect an rvalue's deferred executable parts without treating its explicit operands as source syntax to rebuild.
pub(super) fn first_unsupported_default_rvalue(rvalue: &bir::Rvalue) -> Option<(HirSourceSpan, String)> {
    match rvalue {
        bir::Rvalue::Closure { body, .. } => first_unsupported_default_statement(&body.stmts),
        bir::Rvalue::Generator { body, .. } => first_unsupported_default_statement(&body.stmts),
        bir::Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
            first_unsupported_default_statement(&arm.guard_stmts)
                .or_else(|| first_unsupported_default_statement(&arm.body_stmts))
        }),
        _ => None,
    }
}
