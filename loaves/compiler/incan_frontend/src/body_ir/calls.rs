//! Lowering for calls: argument planning and binding, callable identity, nominal construction, method calls.

use super::args::*;
use super::primitives::*;
use super::*;
use incan_lang::lang::builtins::BuiltinFnId;
use incan_lang::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_lang::lang::surface::{dict_methods, list_methods, set_methods};
use incan_semantics_core::SymbolOrigin;

/// Put a fully bound two-slot call's single-value arguments in declared slot order.
///
/// `None` unless each slot is supplied exactly once by one written value and nothing is left to a default.
fn slot_ordered_pair(elements: Vec<bir::ArgumentElement>, binding: &bir::ArgumentBinding) -> Option<[bir::Operand; 2]> {
    let bir::ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = binding
    else {
        return None;
    };
    if !defaulted_slots.is_empty() || arguments.len() != 2 || elements.len() != 2 {
        return None;
    }
    let mut ordered: [Option<bir::Operand>; 2] = [None, None];
    for (element, bound) in elements.into_iter().zip(arguments) {
        let bir::ArgumentElement::One(operand) = element else {
            return None;
        };
        let slot = ordered.get_mut(bound.slot)?;
        if slot.replace(operand).is_some() {
            return None;
        }
    }
    let [Some(first), Some(second)] = ordered else {
        return None;
    };
    Some([first, second])
}

/// Read receiver exclusivity from the canonical builtin collection registry, never from a source method's spelling.
fn collection_method_changes_receiver(identity: &CanonicalSymbolId) -> bool {
    if identity.origin != incan_semantics_core::SymbolOrigin::Builtin {
        return false;
    }
    let Some((family, member)) = identity.declaration_name.split_once('.') else {
        return false;
    };
    match family {
        "List" => list_methods::from_str(member).is_some_and(list_methods::changes_receiver),
        "Set" => set_methods::from_str(member).is_some_and(set_methods::changes_receiver),
        "Dict" => dict_methods::from_str(member).is_some_and(dict_methods::changes_receiver),
        _ => false,
    }
}

/// The type a `Type.member(..)` call is made on, when its receiver identifier names a type rather than a value.
///
/// A nominal type in the ordinary namespace (model, class, newtype, rusttype, enum, alias or trait) is that type; a
/// generic parameter (`T.default()` under a `Default` bound) is the type variable, so a trait's associated function
/// called on it keeps the type that selects the implementation. Anything else is a value, and `None`.
fn named_static_receiver_type(name: &str, identity: &CanonicalSymbolId) -> Option<IncanType> {
    if identity.kind == SemanticSourceTargetKind::GenericBinder {
        return Some(IncanType::TypeVar(name.to_owned()));
    }
    (identity.namespace == incan_semantics_core::SymbolNamespace::OrdinaryLexical
        && matches!(
            identity.kind,
            SemanticSourceTargetKind::Model
                | SemanticSourceTargetKind::Class
                | SemanticSourceTargetKind::Newtype
                | SemanticSourceTargetKind::Rusttype
                | SemanticSourceTargetKind::Enum
                | SemanticSourceTargetKind::TypeAlias
                | SemanticSourceTargetKind::Trait
        ))
    .then(|| IncanType::Named(name.to_owned()))
}

/// Keep written nominal arguments in source order; spreads have no checked field-slot binding and remain refused.
fn nominal_argument_expressions<'args>(
    name: &str,
    args: &'args [ast::CallArg],
) -> Result<Vec<&'args ast::Spanned<ast::Expr>>, String> {
    args.iter()
        .map(|argument| match argument {
            ast::CallArg::Positional(value) | ast::CallArg::Named(_, value) => Ok(value),
            ast::CallArg::PositionalUnpack(_) => {
                Err(format!("construction of `{name}` with a positional argument spread"))
            }
            ast::CallArg::KeywordUnpack(_) => Err(format!("construction of `{name}` with a keyword argument spread")),
        })
        .collect()
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// The type a `Type.member(..)` call is made on, when its receiver is a type rather than a value.
    ///
    /// A bare identifier follows [`named_static_receiver_type`]. An explicitly instantiated nominal (`Box[int]`) is
    /// the checker's applied type for the whole receiver, so an associated call closes the owner's type parameters at
    /// exactly those arguments; an application the checker did not type as that nominal is a value receiver.
    fn static_receiver_type(&self, recv: &ast::Spanned<ast::Expr>, call_span: ast::Span) -> Option<IncanType> {
        if let Some(ty) = self
            .type_info
            .body_ir_newtypes
            .static_receiver_type(call_span, self.type_info)
        {
            return Some(self.checked_type(ty));
        }
        match &recv.node {
            ast::Expr::Ident(name) => named_static_receiver_type(name, self.type_info.resolved_identity(recv.span)?),
            ast::Expr::Index(owner, _) => {
                let ast::Expr::Ident(name) = &owner.node else {
                    return None;
                };
                named_static_receiver_type(name, self.type_info.resolved_identity(owner.span)?)?;
                match self.resolve_ty(recv.span) {
                    IncanType::Generic { base, args } if base == *name => Some(IncanType::Generic { base, args }),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Rewrite an argument bound to a `mut` parameter (#2022). A `mut` parameter of non-`Copy` type writes through to
    /// the caller, as the emitted route's `&mut T` does, so the argument is borrowed mutably instead of copied or
    /// moved: the caller keeps owning it (an operand that had selected a move is un-moved, so the caller still
    /// drops it) and sees the callee's changes. A `Copy` argument stays a copy, matching the emitted route's
    /// by-value `mut` scalar, and a compiler temporary keeps its move, since no caller can observe writes to it.
    pub(super) fn borrow_for_mut_parameter(&mut self, operand: bir::Operand) -> bir::Operand {
        let bir::Operand::Place(mut source) = operand else {
            return operand;
        };
        let is_temporary = source
            .place
            .local_id()
            .and_then(|local| self.locals.get(local.index()))
            .is_some_and(|decl| matches!(decl.origin, bir::LocalOrigin::Temporary));
        if is_temporary
            || !matches!(
                source.fact,
                bir::OwnershipFact::Move | bir::OwnershipFact::Clone | bir::OwnershipFact::Borrow
            )
        {
            return bir::Operand::Place(source);
        }
        if source.fact == bir::OwnershipFact::Move
            && source.place.projection.is_empty()
            && let Some(local) = source.place.local_id()
        {
            self.moved_out.remove(&local);
        }
        source.fact = bir::OwnershipFact::MutBorrow;
        source.last_use = false;
        bir::Operand::Place(source)
    }

    /// Retain the caller's owner when a checked method-chain input passes its shared receiver reference.
    fn share_argument_slots(
        &mut self,
        operands: &mut [bir::ArgumentElement],
        binding: &bir::ArgumentBinding,
        shared: &[usize],
    ) {
        let bir::ArgumentBinding::Resolved { arguments, .. } = binding else {
            return;
        };
        for (element, argument) in operands.iter_mut().zip(arguments) {
            if !shared.contains(&argument.slot) {
                continue;
            }
            let bir::ArgumentElement::One(bir::Operand::Place(read)) = element else {
                continue;
            };
            if read.fact == bir::OwnershipFact::Move
                && read.place.projection.is_empty()
                && let Some(local) = read.place.local_id()
            {
                self.moved_out.remove(&local);
            }
            read.fact = bir::OwnershipFact::Borrow;
            read.last_use = false;
        }
    }

    /// Lower planned call arguments in written source order, then place them into declaration-slot order.
    ///
    /// Both orders are part of the source contract and they differ whenever a caller writes named arguments out of
    /// declaration order. Argument expressions are therefore lowered here strictly left to right, so the emitted
    /// statement sequence observes written evaluation order, while the returned operand vector is in declaration
    /// order and the returned [`bir::ArgumentBinding`] records which slot each operand fills and where it was
    /// written. A declaration slot the call site never supplied becomes a defaulted slot rather than an operand:
    /// this call site evaluates nothing for it, so it has no ownership fact to record and the default's computation
    /// stays owned by the declaration.
    ///
    /// Because ownership is decided during that written-order pass, each operand's [`bir::OwnershipFact`] and
    /// last-use marker are sequenced by `written_position` and **not** by operand index -- see
    /// [`bir::ArgumentBinding`]'s own docs, which state the invariant a consumer has to honor.
    ///
    /// `mut_slots` marks, by declaration slot, which parameters are declared `mut`; an operand for such a slot goes
    /// through [`Self::borrow_for_mut_parameter`]. A slot past its end, or an empty slice for a surface with no
    /// declared parameters, is treated as not `mut`. `slot_types` retains numeric carrier/member conversions at checked
    /// by-value parameter boundaries; absent slot types use the recorded value destination, as constructor fields do.
    pub(super) fn lower_planned_args(
        &mut self,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        slot_count: usize,
        mut_slots: &[bool],
        slot_types: &[IncanType],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        // Both planners derive their slots from the same declaration surface they report the count of, so an
        // out-of-range slot is unreachable. It is still refused rather than skipped: dropping the operand while
        // leaving the statements that computed it in `out` would produce a silently wrong call, which is the worst
        // failure mode this node has.
        if let Some((slot, _)) = planned.iter().find(|(slot, _)| *slot >= slot_count) {
            return Err(format!(
                "argument bound to declaration slot {slot} outside the callee's {slot_count} declared slots"
            ));
        }
        let mut lowered: Vec<Option<(bir::Operand, usize)>> = (0..slot_count).map(|_| None).collect();
        for (written_position, (slot, expr)) in planned.iter().enumerate() {
            let mut operand = if mut_slots.get(*slot).copied().unwrap_or(false) {
                self.lower_expr_to_operand(expr, scope, out)
            } else {
                self.lower_boundary_value(expr, slot_types.get(*slot), scope, out)
            };
            if mut_slots.get(*slot).copied().unwrap_or(false) {
                operand = self.borrow_for_mut_parameter(operand);
            } else {
                operand = self.owned_parameter_operand(expr, operand);
            }
            if let Some(entry) = lowered.get_mut(*slot) {
                *entry = Some((operand, written_position));
            }
        }

        let mut operands = Vec::with_capacity(planned.len());
        let mut arguments = Vec::with_capacity(planned.len());
        let mut defaulted_slots = Vec::new();
        for (slot, entry) in lowered.into_iter().enumerate() {
            match entry {
                Some((operand, written_position)) => {
                    operands.push(operand);
                    arguments.push(bir::BoundArgument { slot, written_position });
                }
                None => defaulted_slots.push(slot),
            }
        }
        Ok((
            operands,
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            },
        ))
    }

    /// Resolve the declaration surface and exact local identity for a direct named call.
    ///
    /// A direct declaration identity names only an executable body physically represented by this module. Imports
    /// retain their selected canonical identity and checked signature for project assembly to resolve elsewhere.
    /// Unresolved targets stay explicit refusals. Every unshadowed core builtin has a registry identity; individual
    /// consumers still admit only their documented subset, such as `range` for counting-loop lowering.
    ///
    /// Overloads are why this is resolved per call site rather than per name. `function_bindings` is keyed by bare
    /// source name, so for two same-name declarations it holds only one of them; binding a call against the wrong
    /// overload's parameter *names* would silently reorder its arguments, turning an honest refusal into a wrong
    /// answer. The typechecker already records which overload it selected for this call span, so this follows that
    /// decision to the declaration and reads that declaration's own signature. If a name is overloaded but no
    /// selection was recorded, this fails closed rather than picking one.
    pub(super) fn declared_slots_for_direct_call(
        &self,
        name: &str,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Result<DirectCallDeclaration, String> {
        let declarations = &self.type_info.declarations;
        // A same-module alias keeps the selected function's identity; use that declaration's retained spans rather
        // than treating the alias spelling as an imported function with no executable body.
        let selected = self.type_info.resolved_identity(callee_span);
        let declaration_name = selected
            .filter(|identity| {
                matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial
                )
            })
            .filter(|identity| {
                incan_semantics_core::canonical_module_identity(identity).as_deref() == Some(self.module_identity)
            })
            .map_or(name, |identity| identity.declaration_name.as_str());
        let local_declarations = self.local_function_declarations.get(declaration_name);
        let Some(local_declarations) = local_declarations else {
            return self.external_call_declaration(name, callee_span, call_span);
        };
        let is_overloaded = local_declarations.len() > 1;

        if is_overloaded {
            let Some(selected) = self.type_info.resolved_identity(callee_span) else {
                return Err(format!(
                    "call to overloaded function `{name}` without a resolved canonical declaration"
                ));
            };
            let selected_span = local_declarations.iter().find(|candidate_span| {
                declarations
                    .function_bindings_by_span
                    .get(&(candidate_span.start, candidate_span.end))
                    .and_then(|binding| binding.identity.as_ref())
                    .is_some_and(|identity| identity == selected)
            });
            let Some(selected_span) = selected_span else {
                return Err(format!(
                    "call to overloaded function `{name}` whose canonical declaration is not present in this module"
                ));
            };
            let Some(binding) = declarations
                .function_bindings_by_span
                .get(&(selected_span.start, selected_span.end))
            else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration has no checked signature"
                ));
            };
            return Ok(DirectCallDeclaration {
                is_async: binding.is_async,
                slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: Some(CompilerNodeId::declaration_span(
                    self.module_identity,
                    selected_span.start,
                    selected_span.end,
                )),
                builtin: None,
                // The identity anchors to the declaration span, so the selected overload is as nameable as any other
                // declaration; it is the *spelling* that cannot separate them, and the spelling is not the identity.
                // Consumed from the declaration's checked binding — minted once by the typechecker at the
                // declaration site, never re-derived here from module path plus spelling.
                canonical: binding.identity.clone(),
            });
        }

        let [declaration_span] = local_declarations.as_slice() else {
            return Err(format!(
                "direct call to `{name}` has no unambiguous same-module declaration identity"
            ));
        };
        let Some(binding) = declarations
            .function_bindings_by_span
            .get(&(declaration_span.start, declaration_span.end))
        else {
            return Err(format!(
                "same-module declaration `{name}` has no checked callable signature"
            ));
        };
        let canonical = self.type_info.resolved_identity(callee_span).cloned();
        if canonical.as_ref() != binding.identity.as_ref() {
            return Err(format!(
                "direct call to `{name}` does not retain the selected canonical declaration"
            ));
        }
        Ok(DirectCallDeclaration {
            is_async: binding.is_async,
            slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id: Some(CompilerNodeId::declaration_span(
                self.module_identity,
                declaration_span.start,
                declaration_span.end,
            )),
            builtin: None,
            // Consumed from the declaration's checked binding — minted once by the typechecker at the declaration
            // site, never re-derived here from module path plus spelling.
            canonical,
        })
    }

    /// Bind a call's arguments against a declared parameter surface, falling back to positional lowering when there
    /// is none to bind against.
    ///
    /// A checked rest-bearing surface packs explicit values into owned list/dictionary operands in declaration
    /// order, after evaluating written values in source order. Checked fixed components expand before pack planning;
    /// unretained pack elements and passing modes stay refused by name. An absent declaration surface retains
    /// unresolved argument elements instead; it must not claim a computed slot map.
    pub(super) fn bind_declared_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let Some(slots) = declared else {
            let elements = self.lower_spread_capable_args(args, scope, out);
            return Ok((elements, bir::ArgumentBinding::UnresolvedPositional));
        };
        if fixed_spreads::has_checked_tuple_spread(self.type_info, args) {
            return self.bind_checked_tuple_spreads(callee, &slots, args, scope, out);
        }
        // A spread whose shape the typechecker proved is an ordinary fixed-arity call in disguise: `add(*(1, 2))`
        // really is `add(1, 2)`. Expanding it here means it binds through the same declaration-slot planner as any
        // other call, instead of being pushed onto the runtime-arity path it does not belong on.
        let expanded: Vec<ast::CallArg> = args
            .iter()
            .flat_map(|arg| match expand_shaped_spread(self.type_info, arg) {
                Some(expansion) => expansion,
                None => vec![arg.clone()],
            })
            .collect();
        if slots.iter().any(|slot| slot.kind != IncanCallableParamKind::Normal) {
            return self.bind_rest_args(callee, &slots, &expanded, scope, out);
        }
        let planned = plan_declared_args(callee, &slots, &expanded)?;
        let (operands, binding) = self
            .lower_planned_args(
                &planned,
                slots.len(),
                &slots.iter().map(|slot| slot.is_mut).collect::<Vec<bool>>(),
                &slots.iter().map(|slot| slot.ty.clone()).collect::<Vec<IncanType>>(),
                scope,
                out,
            )
            .map_err(|description| format!("{callee}: {description}"))?;
        Ok((fixed_elements(operands), binding))
    }

    /// Retain a nonlocal callable's checked signature by the exact selected identity, including imported overloads.
    ///
    /// An overload-set spelling cannot select a signature. Its canonical call-site identity must match a retained
    /// candidate; absent proof keeps the binding unresolved rather than borrowing another overload's slots.
    /// Constructor syntax keeps the static method selected at the full call span instead of the nominal identity
    /// selected at the callee token, using that method's checked parameter surface.
    fn external_call_declaration(
        &self,
        name: &str,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Result<DirectCallDeclaration, String> {
        if let Some(factory) = self.checked_nominal_factory_declaration(callee_span, call_span) {
            return Ok(factory);
        }
        let canonical = self.type_info.resolved_identity(callee_span).cloned();
        let builtin = self.type_info.resolved_builtin_call(call_span);
        let canonical_builtin_id = canonical.as_ref().and_then(canonical_builtin);
        if builtin != canonical_builtin_id && (builtin.is_some() || canonical_builtin_id.is_some()) {
            return Err(format!(
                "checked builtin call `{name}` does not retain its canonical builtin identity"
            ));
        }
        let signature = match self.type_info.function_overloads(name) {
            Some(candidates) => canonical.as_ref().and_then(|selected| {
                candidates
                    .iter()
                    .find(|candidate| candidate.identity.as_ref() == Some(selected))
                    .map(|candidate| (candidate.info.is_async, candidate.info.params.as_slice()))
            }),
            None => self
                .type_info
                .declarations
                .function_bindings
                .get(name)
                .map(|binding| (binding.is_async, binding.params.as_slice())),
        };
        Ok(DirectCallDeclaration {
            is_async: signature.is_some_and(|(is_async, _)| is_async),
            slots: signature.map(|(_, parameters)| {
                self.type_info
                    .body_ir_call_site_callable_params(call_span)
                    .unwrap_or(parameters)
                    .iter()
                    .map(DeclaredSlot::from_checked_param)
                    .collect()
            }),
            direct_call_id: None,
            builtin,
            canonical,
        })
    }

    /// Keep the static factory the checker selected for constructor syntax, independently of the named type.
    ///
    /// The callee span names the nominal owner; the full call span selects its executable method and substituted
    /// parameters. Reading the former as a function would lose that selection. Ordinary functions and unresolved
    /// nominal calls keep their existing path, and no source spelling supplies a factory identity.
    fn checked_nominal_factory_declaration(
        &self,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Option<DirectCallDeclaration> {
        let owner = self.type_info.resolved_identity(callee_span)?;
        if !matches!(
            owner.kind,
            SemanticSourceTargetKind::Model
                | SemanticSourceTargetKind::Class
                | SemanticSourceTargetKind::Newtype
                | SemanticSourceTargetKind::Enum
        ) {
            return None;
        }
        let factory = self.type_info.resolved_identity(call_span)?;
        if factory.kind != SemanticSourceTargetKind::Method {
            return None;
        }
        let parameters = self.type_info.call_site_callable_params(call_span)?;
        Some(DirectCallDeclaration {
            is_async: false,
            slots: Some(parameters.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id: None,
            builtin: None,
            canonical: Some(factory.clone()),
        })
    }

    /// Retain an applied factory owner from the checked construction result, independently of method type arguments.
    ///
    /// A selected static factory returning its own generic nominal carries that owner's arguments on the associated
    /// call. A wrapper result, ordinary function, absent factory selection or mismatched nominal cannot supply them.
    /// This consumes existing checked facts only; the legacy inference tables remain unchanged.
    fn checked_nominal_factory_owner(&self, callee_span: ast::Span, call_span: ast::Span) -> Option<IncanType> {
        self.checked_nominal_factory_declaration(callee_span, call_span)?;
        let owner = self.type_info.resolved_identity(callee_span)?;
        let result = self.resolve_ty(call_span);
        match &result {
            IncanType::Generic { base, .. } if base == &owner.declaration_name => Some(result),
            _ => None,
        }
    }

    /// Retain checked explicit or inferred call-site type arguments, refusing unresolved explicit arguments.
    ///
    /// Type arguments are part of a call's resolved identity, so Body IR takes the typechecker's
    /// monomorphized selection rather than re-lowering the written AST type nodes -- which is also the only way a
    /// `_` placeholder resolves to a real type instead of an unknown. A call that wrote type arguments the
    /// typechecker did not resolve is refused by name rather than represented with a guess.
    pub(super) fn call_site_type_arguments(
        &self,
        span: ast::Span,
        type_args: &[ast::Spanned<ast::Type>],
    ) -> Result<Vec<IncanType>, String> {
        if let Some(application) = self
            .type_info
            .declarations
            .body_ir_decorator_applications
            .get(&(span.start, span.end))
            && !application.factory_type_args.is_empty()
        {
            return Ok(application
                .factory_type_args
                .iter()
                .map(|ty| {
                    let ty = crate::resolved_type_subst::substitute_resolved_type(ty, &application.type_bindings);
                    self.checked_type(&ty)
                })
                .collect());
        }
        // A retained raw owner instance has no method binders; its owner arguments live in the receiver type.
        if type_args.is_empty()
            && self
                .type_info
                .body_ir_newtypes
                .callable_params(span, self.type_info)
                .is_some()
        {
            return Ok(Vec::new());
        }
        let Some(resolved) = self
            .type_info
            .calls
            .body_ir_call_type_arguments
            .get(&(span.start, span.end))
            .or_else(|| {
                self.type_info
                    .calls
                    .call_site_monomorph_type_args
                    .get(&(span.start, span.end))
            })
        else {
            if type_args.is_empty() {
                return Ok(Vec::new());
            }
            return Err("call with unresolved explicit type arguments".to_string());
        };
        Ok(resolved.iter().map(|ty| self.checked_type(ty)).collect())
    }

    /// Select the declaration identity a checked construction builds.
    ///
    /// An ordinary callee resolves to that declaration. A classmethod's `cls(...)` resolves its callee to the `cls`
    /// receiver binding instead, while checking types the construction as the owner; the owner's declaration then
    /// comes from the checker's module-scope nominal bindings for that type, never from the callee's spelling.
    fn constructed_declaration_identity(&self, callee_span: ast::Span, span: ast::Span) -> Option<CanonicalSymbolId> {
        if let Some(owner) = self.type_info.body_ir_native_constructor_identity(span) {
            return Some(owner.clone());
        }
        let identity = self.type_info.resolved_identity(callee_span)?;
        if identity.kind != SemanticSourceTargetKind::Receiver || identity.declaration_name != "cls" {
            return Some(identity.clone());
        }
        let owner = match self.resolve_ty(span) {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => return None,
        };
        self.type_info.declarations.named_type_identities.get(&owner).cloned()
    }

    /// Whether field `slot` of the source-local model or class `owner` declares a default built only from literals and
    /// builtin empty constructors ([`ast::Spanned::is_literal_construction`]), which evaluates to an equal value and
    /// does nothing else. A field with no default, another default, or an owner this module does not retain answers
    /// `false`.
    fn field_default_is_literal(&self, owner: &str, slot: usize) -> bool {
        let Some(declaration) = self.local_nominal_declarations.get(owner) else {
            return false;
        };
        self.function_default_sources
            .get(owner)
            .filter(|fields| fields.len() == declaration.fields.len())
            .and_then(|fields| fields.get(slot))
            .and_then(|field| field.default.as_ref())
            .is_some_and(|default| {
                default.is_literal_construction(&|span| self.type_info.resolved_collection_constructor(span).is_some())
            })
    }

    /// Lower a value the checker coerces into a newtype where it is used (RFC 017).
    ///
    /// Source-local nongeneric chains evaluate the input once and retain every selected hook or predicate before its
    /// canonical wrapper. Aggregate-field error collection and unretained owners remain specifically refused.
    pub(super) fn lower_newtype_coercion(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        coercion: &ValidatedNewtypeCoercionInfo,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.lower_newtype_ingress(expr, coercion, scope, out)
    }

    /// Refuse a supplied computation that would precede an observable omitted default. Canonically matched local
    /// literal defaults and declaration-owned, statement-free copy constants have no source effect to reorder.
    /// The checked local source or declaring frame supplies this evidence; absent or unproven defaults stay refused.
    fn construction_interleaves_default_effects(
        &self,
        canonical: Option<&CanonicalSymbolId>,
        owner: &str,
        supplied_slots: &[usize],
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
    ) -> bool {
        if supplied_slots.windows(2).any(|pair| pair[0] > pair[1]) {
            return false;
        }
        let declaration = canonical.and_then(|identity| {
            self.local_nominal_declarations
                .values()
                .find(|value| &value.canonical == identity)
        });
        let defaults = declaration.and_then(|declaration| declaration.field_default_body.as_deref());
        let local_source = self
            .local_nominal_declarations
            .get(owner)
            .filter(|source| Some(&source.canonical) == canonical);
        planned.iter().any(|(slot, expression)| {
            !matches!(expression.node, ast::Expr::Literal(_))
                && (0..*slot).any(|prior| {
                    !supplied_slots.contains(&prior)
                        && !(local_source.is_some() && self.field_default_is_literal(owner, prior))
                        && defaults
                            .and_then(|frame| frame.params.get(prior))
                            .is_none_or(|parameter| {
                                !parameter.ty.abi_v0_facts().ownership.is_trivially_copy()
                                    || !matches!(&parameter.default,
                                    bir::CallableParamDefault::Source(computation)
                                        if computation.stmts.is_empty()
                                            && matches!(computation.result, bir::Operand::Constant(_)))
                            })
                })
        })
    }

    /// Lower a `model`/`class` or plain-newtype construction into a [`bir::AggregateKind::Constructor`] aggregate.
    ///
    /// Models/classes bind named fields; a newtype binds its single positional argument to tuple slot zero. Lowering
    /// consumes the typechecker's own recorded decision
    /// ([`TypeCheckInfo::constructor_field_binding`](crate::typechecker::TypeCheckInfo::constructor_field_binding))
    /// rather than re-resolving field aliases or rediscovering declared field order, both of which live in the
    /// symbol table this stage deliberately cannot reach. Operands are emitted in declared field order while the
    /// argument expressions are lowered in written source order, exactly as for a call.
    pub(super) fn lower_nominal_construction(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        callee_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let Some(field_binding) = self.type_info.body_ir_constructor_field_binding(span).cloned() else {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        };
        self.lower_nominal_construction_with_slots(
            name,
            args,
            span,
            callee_span,
            scope,
            out,
            (field_binding.field_count, &field_binding.argument_slots),
        )
    }

    /// Materialize a construction from checker-selected slots or a complete canonical field-identity binding.
    ///
    /// Both producers preserve written evaluation order and the same selected owner. This boundary never binds a
    /// source field name: the supplied slot layout must already belong to that owner's canonical declaration.
    #[allow(clippy::too_many_arguments)]
    fn lower_nominal_construction_with_slots(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        callee_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
        layout: (usize, &[usize]),
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let (field_count, argument_slots) = layout;
        let written_exprs = match nominal_argument_expressions(name, args) {
            Ok(values) => values,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        if written_exprs.len() != argument_slots.len() {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let planned: Vec<(usize, &ast::Spanned<ast::Expr>)> =
            argument_slots.iter().copied().zip(written_exprs).collect();
        let canonical = self.constructed_declaration_identity(callee_span, span);
        // Supplied operands precede deferred defaults; only retained effect-free defaults may cross that boundary.
        if self.construction_interleaves_default_effects(canonical.as_ref(), name, argument_slots, &planned) {
            return self.unsupported_operand(
                format!("model field-default evaluation interleaved with supplied arguments on `{name}`"),
                scope,
                hir_span_value,
                out,
            );
        }
        let single_field =
            self.single_field_checked_ingress(canonical.as_ref(), &planned, field_count, scope, hir_span_value, out);
        let lowered = match single_field {
            Some(value) => Ok((vec![value], bir::ArgumentBinding::resolved_positional(1))),
            None => self.lower_planned_args(&planned, field_count, &[], &[], scope, out),
        };
        let (operands, binding) = match lowered {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(
                    format!("construction of `{name}`: {description}"),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };
        let ty = self.resolve_ty(span);
        // A constructor field binding proves argument slots, but not that this constructor names one of the plain
        // models admitted to this lowering context. Preserve the selected declaration's identity and layout together;
        // imported aliases use their original canonical identity, while absent or unsupported context retains
        // neither fact and is refused without guessing from `name`.
        let (direct_declaration_id, canonical_field_layout) =
            self.nominal_constructor_layout(canonical.as_ref(), field_count);
        if let Some(checked) =
            self.checked_newtype_construction(name, canonical.as_ref(), &operands, &ty, scope, hir_span_value, out)
        {
            return checked;
        }
        let operands = self.constrain_nominal_arguments(name, canonical.as_ref(), operands, scope, hir_span_value, out);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(
                bir::AggregateKind::Constructor(Box::new(bir::ConstructorTarget {
                    name: name.to_string(),
                    canonical,
                    direct_declaration_id,
                    canonical_field_layout,
                    binding,
                })),
                fixed_elements(operands),
            ),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Carry only the selected declaration's canonical field order; argument arity alone cannot name a layout.
    ///
    /// This retains the existing aggregate boundary for aliases and absent declarations. Validation ingress must
    /// separately prove field types, hook identity, and failure strategy before it evaluates any supplied input.
    fn nominal_constructor_layout(
        &self,
        canonical: Option<&CanonicalSymbolId>,
        field_count: usize,
    ) -> (Option<CompilerNodeId>, Option<Vec<String>>) {
        self.local_nominal_declarations
            .values()
            .find(|declaration| declaration.fields.len() == field_count && canonical == Some(&declaration.canonical))
            .map(|declaration| {
                (
                    Some(declaration.direct_declaration_id.clone()),
                    Some(declaration.fields.clone()),
                )
            })
            .unwrap_or((None, None))
    }

    /// Bind fully supplied module constructor fields by checked identities against the same retained owner.
    ///
    /// No source spelling, default, generic field or spread supplies execution authority here. Missing, duplicate or
    /// mismatched identities leave construction refused without altering the constructor table legacy consumes.
    fn supplied_module_constructor_slots(
        &self,
        canonical: &CanonicalSymbolId,
        args: &[ast::CallArg],
    ) -> Option<Vec<usize>> {
        let declaration = self
            .local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == canonical)?;
        if args.is_empty()
            || args.len() != declaration.fields.len()
            || declaration.field_identities.len() != declaration.fields.len()
            || !declaration.type_parameters.is_empty()
        {
            return None;
        }
        let mut slots = Vec::with_capacity(args.len());
        for argument in args {
            let ast::CallArg::Named(field, _) = argument else {
                return None;
            };
            let identity = self.type_info.resolved_identity(field.span)?;
            let slot = declaration
                .field_identities
                .iter()
                .position(|field| field == identity)?;
            if slots.contains(&slot) {
                return None;
            }
            slots.push(slot);
        }
        Some(slots)
    }

    /// Lower call arguments positionally, admitting spreads, for a call whose arity is not statically known.
    ///
    /// Every argument keeps its written form: a positional value, a named value, or a spread. None of them can be
    /// resolved to a declared slot here, because a spread supplies an unknown number of arguments at runtime —
    /// which is exactly why the resulting call records [`bir::ArgumentBinding::UnresolvedPositional`] rather than a
    /// slot map asserting a binding nobody checked. A name is preserved on its element rather than discarded, so a
    /// later consumer can still bind it once the arity is known.
    pub(super) fn lower_spread_capable_args(
        &mut self,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::ArgumentElement> {
        let mut elements = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => {
                    elements.push(bir::ArgumentElement::One(self.lower_expr_to_operand(expr, scope, out)));
                }
                ast::CallArg::PositionalUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out));
                }
                ast::CallArg::KeywordUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out));
                }
                ast::CallArg::Named(name, expr) => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    elements.push(bir::ArgumentElement::Named {
                        name: name.node.clone(),
                        operand,
                    });
                }
            }
        }
        elements
    }

    /// Return the exact retained target for a qualified local fieldless normal-enum member, if safe to materialize.
    ///
    /// A bare type-name receiver and source-local registry membership are both required. This leaves ordinary forms
    /// not represented by the registry as generic field accesses that the direct executor visibly refuses, while
    /// preserving exact declaration identities for the one bounded unit-variant carrier profile.
    pub(super) fn local_fieldless_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
        access_span: ast::Span,
    ) -> Option<bir::FieldlessEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name)
            || !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let declaration = self
            .local_fieldless_enum_declarations
            .values()
            .find(|declaration| self.type_info.resolved_identity(base.span) == Some(&declaration.canonical))?;
        if self.type_info.resolved_identity(base.span) != Some(&declaration.canonical) {
            return None;
        }
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        if self.type_info.resolved_identity(access_span) != Some(&variant.canonical) {
            return None;
        }
        Some(bir::FieldlessEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            enum_canonical: declaration.canonical.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            variant_canonical: variant.canonical.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Return the checked enum variant a `Type.Variant` spelling selects, for any enum the narrower fieldless and
    /// value-enum targets do not cover.
    ///
    /// The base must be an unshadowed type name, and the checker must have resolved `variant_span` to a variant: the
    /// spelling alone never selects one. `variant_span` is the member access for `Shape.Empty` and the call for
    /// `Shape.Circle(r)`, which is where the checker records the selection. The returned target binds no payload yet.
    pub(super) fn checked_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        _variant_name: &str,
        variant_span: ast::Span,
    ) -> Option<bir::EnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name)
            || !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let variant_canonical = self
            .type_info
            .resolved_identity(variant_span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Variant)?
            .clone();
        Some(bir::EnumVariantTarget {
            enum_name: enum_name.clone(),
            variant_name: variant_canonical.declaration_name.clone(),
            enum_canonical: self.type_info.resolved_identity(base.span).cloned(),
            variant_canonical,
            binding: bir::ArgumentBinding::resolved_positional(0),
        })
    }

    /// Construct `Type.Variant(payload)` as an [`bir::AggregateKind::EnumVariant`], binding the payload against the
    /// variant's checked parameters when the checker recorded them and positionally otherwise. Retained payload
    /// layouts provide ordinary parameter slots with their exact types when no callable-parameter record is present.
    #[allow(clippy::too_many_arguments)]
    fn lower_enum_variant_call(
        &mut self,
        mut target: bir::EnumVariantTarget,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect())
            .or_else(|| {
                self.enum_payload_types(&target.variant_canonical, &self.resolve_ty(span))
                    .map(|fields| {
                        fields
                            .into_iter()
                            .map(|ty| DeclaredSlot {
                                name: None,
                                has_default: false,
                                is_partial_preset: false,
                                kind: IncanCallableParamKind::Normal,
                                ty,
                                is_mut: false,
                            })
                            .collect()
                    })
            });
        let what = format!("enum variant `{}.{}`", target.enum_name, target.variant_name);
        let (operands, binding) = match self.bind_declared_args(&what, declared, args, scope, out) {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        target.binding = binding;
        let ty = self.resolve_ty(span);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::EnumVariant(Box::new(target)), operands),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Return the exact retained target for a qualified local RFC 032 value-enum member, if this spelling is safe to
    /// materialize directly.
    ///
    /// The source-local registry is deliberately the only lookup used here. A function-local binding wins over a
    /// same-spelling declaration, and any import, alias, ordinary enum, payload member, or behavior-bearing enum is
    /// absent from the registry. The resulting rvalue stores both declaration identities for runtime revalidation;
    /// it does not make the spelling itself an execution authority.
    pub(super) fn local_value_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
        access_span: ast::Span,
    ) -> Option<bir::ValueEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name) {
            return None;
        }
        if !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName)) {
            return None;
        }
        let declaration = self
            .local_value_enum_declarations
            .values()
            .find(|declaration| self.type_info.resolved_identity(base.span) == Some(&declaration.canonical))?;
        if self.type_info.resolved_identity(base.span) != Some(&declaration.canonical) {
            return None;
        }
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        if self.type_info.resolved_identity(access_span) != Some(&variant.canonical) {
            return None;
        }
        Some(bir::ValueEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            enum_canonical: declaration.canonical.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            variant_canonical: variant.canonical.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Lower one compiler-owned `isinstance` call from the typechecker's retained builtin and target facts.
    fn lower_checked_isinstance_call(
        &mut self,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let call_span = hir_span(span);
        let [ast::CallArg::Positional(value), ast::CallArg::Positional(target_expr)] = args else {
            return self.unsupported_operand(
                "isinstance outside the positional checked-target profile".to_string(),
                scope,
                call_span,
                out,
            );
        };
        if !type_args.is_empty() {
            return self.unsupported_operand(
                "isinstance with explicit call-site type arguments".to_string(),
                scope,
                call_span,
                out,
            );
        }
        let Some(target) = self.type_info.isinstance_target(span) else {
            return self.unsupported_operand(
                "isinstance without checked target evidence".to_string(),
                scope,
                hir_span(target_expr.span),
                out,
            );
        };
        let target_span = hir_span(target.span);
        let target_span = if target_span.start >= call_span.start && target_span.end <= call_span.end {
            target_span
        } else {
            call_span
        };
        let value_ty = self.resolve_ty(value.span);
        let value = self.lower_expr_to_operand(value, scope, out);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(
            bir::Rvalue::IsInstance {
                value,
                value_ty,
                target: bir::IsInstanceTarget {
                    ty: semantic_type_from_resolved(&target.ty),
                    canonical: target.canonical.clone(),
                    span: target_span,
                },
            },
            ty,
            scope,
            call_span,
            out,
        )
    }

    /// The intrinsic `Result` variant a call of `callee` at `span` constructs: `Ok` or `Err` spelled as a bare name
    /// that no local, same-module function or source target binds. A same-spelled source binding stays an ordinary
    /// call that refuses unless its own direct callable facts are available; the direct runtime never resolves a
    /// constructor name dynamically.
    pub(super) fn intrinsic_result_constructor(
        &self,
        callee: &ast::Spanned<ast::Expr>,
        span: ast::Span,
    ) -> Option<bir::ResultVariantKind> {
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        if self.bindings.contains_key(name)
            || self.local_function_declarations.contains_key(name)
            || self.type_info.source_target(span).is_some()
        {
            return None;
        }
        result_variant_kind(name)
    }

    /// Settle each `Ok(...)` or `Err(...)` argument of the call at `span` to the parameter type the checker resolved
    /// for it, which carries the side the constructor leaves open as the call settled it (see
    /// [`Self::settle_open_result_value`]). A call with no resolved parameters, a rest parameter, or arguments that do
    /// not bind settles nothing; binding the arguments for the call itself still reports those.
    pub(super) fn settle_open_call_arguments(&mut self, span: ast::Span, args: &[ast::CallArg]) {
        let type_info = self.type_info;
        let Some(params) = type_info.body_ir_call_site_callable_params(span) else {
            return;
        };
        let slots: Vec<DeclaredSlot> = params.iter().map(DeclaredSlot::from_checked_param).collect();
        let Ok(planned) = plan_declared_args("call", &slots, args) else {
            return;
        };
        for (slot, argument) in planned {
            if let Some(param) = params.get(slot) {
                let destination = self.checked_type(&param.ty);
                self.settle_open_result_value(argument, &destination);
            }
        }
    }

    /// Lower a call to a locally held callable value, a nominal construction, or a direct named function.
    ///
    /// A bare identifier that resolves to one of this body's locals is deliberately a
    /// [`bir::CallableTarget::Local`] call: it carries the local read's ownership fact, so a closure's lexical
    /// environment is not lost by pretending the identifier were a declaration. Its callable signature also
    /// enforces the stored value's fixed callable contract before any call arguments are lowered. An identifier the
    /// typechecker resolved to a `model`/`class` construction lowers to a constructor aggregate instead of a call
    /// (see [`Self::lower_nominal_construction`]) -- construction is not invocation, and representing it as a call
    /// would invite a consumer to execute it as one. Any other bare identifier remains a direct
    /// [`bir::Callee::Function`] call.
    ///
    /// Every one of those paths binds its arguments through the same [`plan_declared_args`] planner and records the
    /// result as a [`bir::ArgumentBinding`], so named, out-of-order, and defaulted spellings resolve identically
    /// regardless of how the callee was reached. A direct call whose signature the typechecker did not resolve
    /// (notably a builtin) still lowers its arguments faithfully, recording
    /// [`bir::ArgumentBinding::UnresolvedPositional`], and refuses only a named spelling it cannot bind without one.
    /// Argument spreads lower as [`bir::ArgumentElement::Spread`] elements, since a spread has no declared slot to
    /// bind to by construction. A checked callable expression is evaluated into a local target before its arguments;
    /// missing signatures and unretained type applications remain explicit unsupported forms.
    pub(super) fn lower_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if let Some(value) = self.lower_testing_raises_call(callee, type_args, args, span, scope, out) {
            return value;
        }
        if let Some(value) = self.lower_checked_length_call(args, span, scope, out) {
            return value;
        }
        if let Some(error) = self.lower_validation_error_construction(callee, type_args, args, span, scope, out) {
            return error;
        }
        if self.type_info.resolved_builtin_call(span) == Some(BuiltinFnId::IsInstance) {
            return self.lower_checked_isinstance_call(type_args, args, span, scope, out);
        }
        self.settle_open_call_arguments(span, args);
        let ast::Expr::Ident(name) = &callee.node else {
            return self.lower_evaluated_callable_call(callee, type_args, args, span, scope, out);
        };
        let name = name.clone();

        // The checked constructor identity distinguishes this conversion from a same-spelled source callable.
        if type_args.is_empty()
            && let [ast::CallArg::Positional(source)] = args
        {
            match self.type_info.resolved_collection_constructor(span) {
                Some(CollectionTypeId::Set) => return self.lower_set_constructor_source(source, span, scope, out),
                Some(CollectionTypeId::List) => return self.lower_list_constructor_source(source, span, scope, out),
                Some(CollectionTypeId::Dict) => return self.lower_dict_constructor_source(source, span, scope, out),
                _ => {}
            }
        }

        // A retained zero-argument constructor fact distinguishes builtin construction from a same-spelled source
        // callable. Only empty List/Set/Dict construction is admitted here; iterable conversions stay on their
        // existing path.
        if args.is_empty()
            && type_args.is_empty()
            && let Some(constructor) = self.type_info.resolved_collection_constructor(span)
        {
            match constructor {
                CollectionTypeId::List => {
                    return self.lower_aggregate(bir::AggregateKind::List, &[], span, scope, out);
                }
                CollectionTypeId::Set => {
                    return self.lower_aggregate(bir::AggregateKind::Set, &[], span, scope, out);
                }
                CollectionTypeId::Dict => return self.lower_dict(&[], span, scope, out),
                _ => {}
            }
        }

        // A recorded constructor field binding is the typechecker's own statement that this spelling constructs a
        // nominal value, which is what distinguishes `P(x=1)` from a call to a function that happens to be named
        // `P`. A construction may carry call-site type arguments (`Box[int]()` is accepted), but the typechecker
        // records no monomorphization for them, and the constructed value's own type already carries the resolved
        // arguments -- so this deliberately does not duplicate them on the constructor target rather than claiming
        // construction cannot be generic.
        if self.type_info.body_ir_constructor_field_binding(span).is_some() {
            return self.lower_nominal_construction(&name, args, span, callee.span, scope, out);
        }

        // `Ok` and `Err` are intrinsic Result constructors, not ordinary direct calls. Retain that checked
        // distinction explicitly: a same-spelled source binding (a local callable, local function, or imported
        // target) must remain on the normal call path and refuse unless its own direct callable facts are available.
        // The direct runtime never resolves a constructor name dynamically.
        if type_args.is_empty()
            && let Some(kind) = self.intrinsic_result_constructor(callee, span)
        {
            let result_ty = self.resolve_ty(span);
            let Some((ok_type, error_type)) = result_type_parts(&result_ty) else {
                return self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` without a resolved Result carrier"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let [ast::CallArg::Positional(payload)] = args else {
                return self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` requires one positional payload"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let operand = self.lower_expr_to_operand(payload, scope, out);
            // A Result owns its payload, so a generic or nominal field follows the same clone boundary as a value
            // argument.
            let payload = self.owned_parameter_operand(payload, operand);
            let recorded = self
                .type_info
                .expressions
                .expression_type_identities
                .get(&(span.start, span.end))
                .cloned()
                .unwrap_or_default();
            // A side its destination settled can name leaves the checker's own constructor type does not.
            let canonical_types = if self.settled_types.contains_key(&(span.start, span.end)) {
                self.settled_result_identities(&result_ty, &recorded)
            } else {
                recorded
            };
            return self.push_assign_temp(
                bir::Rvalue::ResultVariant(bir::ResultVariant {
                    kind,
                    payload,
                    ok_type: ok_type.clone(),
                    error_type: error_type.clone(),
                    canonical_types,
                }),
                result_ty,
                scope,
                hir_span_value,
                out,
            );
        }

        if name == "Some"
            && !self.bindings.contains_key(&name)
            && !self.local_function_declarations.contains_key(&name)
            && self
                .type_info
                .resolved_identity(callee.span)
                .is_some_and(|identity| identity.origin == SymbolOrigin::Builtin && identity.declaration_name == "Some")
            && type_args.is_empty()
            && matches!(self.resolve_ty(span), IncanType::Generic { ref base, ref args } if base == "Option" && args.len() == 1)
            && let [ast::CallArg::Positional(payload)] = args
        {
            let payload = self.lower_expr_to_operand(payload, scope, out);
            return self.push_assign_temp(
                bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, fixed_elements(vec![payload])),
                self.resolve_ty(span),
                scope,
                hir_span_value,
                out,
            );
        }

        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        // Calling a value whose type adopts `__call__` (a `Callable1` adopter, or a type parameter bounded by one) is a
        // call of that method, as the checker resolved it; the value is the receiver, borrowed like any receiver.
        if let Some(dispatch) = self.type_info.resolved_operator_call(span)
            && dispatch.kind == ResolvedOperatorKind::Call
        {
            let method = dispatch.method.clone();
            let receiver_place = self.lower_expr_to_place(callee, scope, out);
            let receiver = bir::Operand::place(receiver_place, bir::OwnershipFact::Borrow, false);
            let declared: Option<Vec<DeclaredSlot>> = self
                .type_info
                .body_ir_call_site_callable_params(span)
                .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
            let (mut operands, binding) =
                match self.bind_declared_args(&format!("`{method}` of `{name}`"), declared, args, scope, out) {
                    Ok(bound) => bound,
                    Err(description) => {
                        return self.unsupported_operand(description, scope, hir_span_value, out);
                    }
                };
            let mut call_args = Vec::with_capacity(operands.len() + 1);
            call_args.push(bir::ArgumentElement::One(receiver));
            call_args.append(&mut operands);
            let ty = self.resolve_ty(span);
            return self.push_call_temp(
                bir::Callee::Method(bir::MethodTarget {
                    name: method,
                    intrinsic_carrier: None,
                    reflection: None,
                    sdk_trait: None,
                    canonical: self.type_info.resolved_identity(span).cloned(),
                    type_args: resolved_type_args,
                    binding,
                }),
                call_args,
                ty,
                scope,
                hir_span_value,
                false,
                out,
            );
        }

        if let Some(&local) = self.bindings.get(&name) {
            let local_ty = self.locals[local.index()].ty.clone();
            let IncanType::Function { params, return_type: _ } = local_ty else {
                return self.unsupported_operand(
                    format!("call to non-callable local `{name}`"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let slots: Vec<DeclaredSlot> = params.iter().map(DeclaredSlot::from_semantic_param).collect();

            // Source evaluation observes the callable value before its arguments. The target read also performs the
            // one ownership/last-use decision for that lexical environment, which `CallableTarget::Local` preserves
            // for a later executor instead of re-deriving it from the local's source spelling.
            let place = bir::Place::from_local(local);
            let (fact, last_use) = self.ownership_fact_for_place(&place, &self.locals[local.index()].ty.clone());
            let (mut operands, binding) =
                match self.bind_declared_args(&format!("local callable `{name}`"), Some(slots), args, scope, out) {
                    Ok(bound) => bound,
                    Err(description) => {
                        return self.unsupported_operand(description, scope, hir_span_value, out);
                    }
                };
            let shared = params
                .iter()
                .enumerate()
                .filter_map(|(index, parameter)| matches!(parameter.ty, IncanType::Ref(_)).then_some(index))
                .collect::<Vec<_>>();
            self.share_argument_slots(&mut operands, &binding, &shared);
            let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
                operand: bir::PlaceOperand { place, fact, last_use },
                binding,
            }));
            let ty = self.resolve_ty(span);
            return self.push_call_temp(callee, operands, ty, scope, hir_span_value, false, out);
        }

        // A name that resolves to a nominal type but has no recorded field binding is a construction the checker
        // declined to bind (a duplicate or unknown field). Refusing it as a call to an unknown function would name
        // the wrong construct entirely.
        if self.type_info.declarations.class_layouts.contains_key(&name)
            || self.type_info.declarations.model_field_visibilities.contains_key(&name)
        {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let declaration = match self.declared_slots_for_direct_call(&name, callee.span, span) {
            Ok(declaration) => declaration,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        // A provider operation is selected by the canonical identity this call already resolved to, never by the
        // spelling written here (#1213). The lookup therefore sits after resolution rather than in place of it, and
        // a callee with no proven identity finds nothing rather than being guessed at by name.
        if let Some(operation) = declaration.canonical.clone()
            && let Some(record) = self.provider_operation_record(&operation)
        {
            return self.lower_provider_operation(&name, &operation, record, declaration, args, span, scope, out);
        }

        let is_async = declaration.is_async;
        if let Some(identity) = &declaration.canonical {
            self.settle_declared_enum_arguments(identity, args);
        }
        let shared_receiver = declaration.canonical.as_ref().is_some_and(|identity| {
            incan_semantics_core::canonical_module_identity(identity).as_deref() == Some(self.module_identity)
                && self
                    .type_info
                    .declarations
                    .method_decorator_receiver_slots
                    .get(&(identity.declaration_span.start, identity.declaration_span.end))
                    .is_some_and(|slot| {
                        !slot.mutable && slot.role == crate::typechecker::MethodDecoratorReceiverRole::Replacement
                    })
        });
        let legacy_enumerate_argument = legacy_enumerate_argument(declaration.builtin, args);
        let args = legacy_enumerate_argument.as_deref().unwrap_or(args);
        let (mut operands, binding) =
            match self.bind_declared_args(&format!("function `{name}`"), declaration.slots, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
        if declaration.builtin == Some(BuiltinFnId::Print) {
            self.display_type_parameter_arguments(args, &mut operands, scope, out);
        }
        if shared_receiver {
            self.share_argument_slots(&mut operands, &binding, &[0]);
        }

        // The checked expression type is the eventual output for immediately awaited source calls. Body IR
        // represents construction and awaiting separately, so their intervening local must own the future.
        let checked_return = self.resolve_ty(span);
        // A published signature may refine an open carrier, but it cannot override a checked provenance refusal.
        let returned = if matches!(checked_return, IncanType::Unsupported(_)) {
            checked_return
        } else {
            declaration
                .canonical
                .as_ref()
                .and_then(|identity| self.published_callable_returns.get(identity))
                .cloned()
                .unwrap_or(checked_return)
        };
        let ty = if is_async {
            IncanType::Generic {
                base: "Awaitable".to_string(),
                args: vec![returned],
            }
        } else {
            returned
        };
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: self.checked_nominal_factory_owner(callee.span, span),
                name,
                direct_call_id: declaration.direct_call_id,
                canonical: declaration.canonical,
                builtin: declaration.builtin,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Give each positional `print`/`println` argument whose checked type is a type parameter its display text as
    /// generic code spells it, through the type argument's own display ([`bir::FormatStyle::TypeParameterDisplay`]).
    ///
    /// Legacy prints such a value with `{}`, so a closed instance must not apply the formatting a concrete value of
    /// that type receives (an integral float keeps no fractional part); the argument becomes that one-part formatted
    /// string. Arguments of any other type, and calls whose written arguments do not line up one-to-one with their
    /// lowered elements, are left as lowered.
    fn display_type_parameter_arguments(
        &mut self,
        args: &[ast::CallArg],
        operands: &mut [bir::ArgumentElement],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        if args.len() != operands.len() {
            return;
        }
        for (arg, element) in args.iter().zip(operands.iter_mut()) {
            let ast::CallArg::Positional(expr) = arg else {
                continue;
            };
            if !matches!(self.resolve_ty(expr.span), IncanType::TypeVar(_)) {
                continue;
            }
            let bir::ArgumentElement::One(operand) = element else {
                continue;
            };
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("fstring".to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            let displayed = self.push_assign_temp(
                bir::Rvalue::Format(vec![bir::FormatPart::Expr {
                    operand: Box::new(operand.clone()),
                    style: bir::FormatStyle::TypeParameterDisplay,
                }]),
                IncanType::Primitive(IncanPrimitiveType::Str),
                scope,
                hir_span(expr.span),
                out,
            );
            *element = bir::ArgumentElement::One(displayed);
        }
    }

    /// Return the typechecker's callable type for a closure or local partial value that Body IR constructs itself.
    ///
    /// Local partials use the typechecker's canonical full signature with overrideable preset-default slots, so the
    /// binding, its [`bir::Rvalue::Closure`], and a later [`Self::lower_call`] share one arity/default contract.
    pub(super) fn callable_value_ty(&self, expr: &ast::Spanned<ast::Expr>) -> Option<IncanType> {
        match &expr.node {
            ast::Expr::Closure(_, _) | ast::Expr::Partial(_) => Some(self.resolve_ty(expr.span)),
            _ => None,
        }
    }

    /// Lower a method-shaped explicit builtin call from the typechecker's retained builtin selection.
    ///
    /// `std.builtins.len(value)` reaches the AST as a method call, but the namespace is not a runtime receiver. The
    /// typechecker records the selected [`BuiltinFnId`] at the full call span. Consuming that closed fact here keeps
    /// the call distinct from a user-defined function or method with the same spelling and avoids evaluating the
    /// namespace as a value.
    #[allow(clippy::too_many_arguments)]
    fn lower_checked_builtin_method_call(
        &mut self,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if let Some(value) = self.lower_checked_length_call(args, span, scope, out) {
            return Some(value);
        }
        let builtin = self.type_info.resolved_builtin_call(span)?;
        if builtin == BuiltinFnId::IsInstance {
            return None;
        }
        let call_span = hir_span(span);
        let Some(canonical) = crate::symbols::canonical_builtin_function_identity(builtin) else {
            return Some(self.unsupported_operand(
                "checked builtin call has no canonical registry identity".to_string(),
                scope,
                call_span,
                out,
            ));
        };
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return Some(self.unsupported_operand(description, scope, call_span, out));
            }
        };
        let declared = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let display_name = canonical.declaration_name.clone();
        let legacy_enumerate_argument = legacy_enumerate_argument(Some(builtin), args);
        let args = legacy_enumerate_argument.as_deref().unwrap_or(args);
        let (operands, binding) =
            match self.bind_declared_args(&format!("builtin `{display_name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return Some(self.unsupported_operand(description, scope, call_span, out));
                }
            };
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: display_name,
                direct_call_id: None,
                canonical: Some(canonical),
                builtin: Some(builtin),
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            call_span,
            false,
            out,
        ))
    }

    /// Preserve the checker's selected length hook as an ordinary method call, evaluating its receiver once.
    ///
    /// Builtin lists and strings have no user operator selection and retain their builtin calls. An imported model's
    /// hook keeps the checked canonical method identity so the SDK call collector can retain its signature.
    fn lower_checked_length_call(
        &mut self,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if self.type_info.resolved_builtin_call(span) != Some(BuiltinFnId::Len) {
            return None;
        }
        let dispatch = self.type_info.resolved_operator_call(span)?;
        if dispatch.kind != ResolvedOperatorKind::Len {
            return None;
        }
        let method = dispatch.method.clone();
        let [ast::CallArg::Positional(receiver) | ast::CallArg::Named(_, receiver)] = args else {
            return None;
        };
        Some(self.lower_operator_dispatch(&method, receiver, &[], self.resolve_ty(span), span, scope, out))
    }

    /// Lower `list.repeat(value, count)` as the [`bir::HelperOp::ListRepeat`] call the typechecker resolved.
    ///
    /// The receiver is the builtin `list` type surface rather than a value, so nothing is read from it. The checker
    /// records the helper's member identity on the call and marks the receiver a type name, and both are required:
    /// a local binding that shadows `list` keeps its ordinary method call. Arguments bind to the helper's `value` and
    /// `count` slots by position or name, are evaluated in written order, and are passed in slot order.
    #[allow(clippy::too_many_arguments)]
    fn lower_list_repeat_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let helper_id = BuiltinCollectionHelperId::ListRepeat;
        let ast::Expr::Ident(receiver) = &recv.node else {
            return None;
        };
        if receiver != collection_helpers::receiver(helper_id)
            || name != collection_helpers::member(helper_id)
            || self.bindings.contains_key(receiver)
            || !matches!(self.type_info.ident_kind(recv.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let identity = crate::symbols::canonical_builtin_member_identity(
            collections::as_str(CollectionTypeId::List),
            collection_helpers::member(helper_id),
        );
        if self.type_info.resolved_identity(span) != Some(&identity) {
            return None;
        }
        let call_span = hir_span(span);
        if !type_args.is_empty() {
            let description = "list.repeat with explicit type arguments".to_string();
            return Some(self.unsupported_operand(description, scope, call_span, out));
        }
        let slots = Vec::from(["value", "count"].map(|slot| DeclaredSlot {
            name: Some(slot.to_string()),
            has_default: false,
            is_partial_preset: false,
            kind: IncanCallableParamKind::Normal,
            ty: IncanType::Unknown,
            is_mut: false,
        }));
        let (operands, binding) = match self.bind_declared_args("builtin `list.repeat`", Some(slots), args, scope, out)
        {
            Ok(bound) => bound,
            Err(description) => {
                return Some(self.unsupported_operand(description, scope, call_span, out));
            }
        };
        let Some([value, count]) = slot_ordered_pair(operands, &binding) else {
            let description = "list.repeat without one value and one count".to_string();
            return Some(self.unsupported_operand(description, scope, call_span, out));
        };
        let helper = bir::HelperOp::ListRepeat;
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        self.panic_facts.push(bir::PanicFact {
            span: call_span,
            reason: bir::PanicReason::HelperMayPanic(helper),
        });
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            bir::Callee::Helper(helper),
            fixed_elements(vec![value, count]),
            ty,
            scope,
            call_span,
            true,
            out,
        ))
    }

    /// Lower a module-qualified function call or nominal construction using its selected declaration.
    ///
    /// A module binding is a namespace, not a value. `identity_provider.imported_path(17)` reaches the AST as a
    /// method call, but there is no receiver to read, and treating the qualifier as a place refuses at a name that
    /// never named one. The typechecker has already resolved this call to exactly one declaration; consuming that
    /// identity keeps the call on the direct path instead of re-deriving a target from the two spellings the source
    /// happens to join with a dot. The receiver's own identity decides whether this applies, so a local variable
    /// that happens to share a module's name still lowers as an ordinary method call.
    ///
    /// A selected nominal owner constructs an aggregate instead of invoking a function. Its checked field slots, or
    /// a complete set of checked field identities matching that owner's retained layout, determine the binding.
    /// Missing layout or identity evidence remains an explicit refusal.
    ///
    /// Returns `None` when the receiver is not a module, leaving every other receiver shape untouched.
    #[allow(clippy::too_many_arguments)]
    fn lower_module_qualified_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if !self
            .type_info
            .resolved_identity(recv.span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Module)
        {
            return None;
        }
        let hir_span_value = hir_span(span);

        // Without the call's own resolved identity there is no target: the module qualifier names where to look,
        // not what was found. Refusing here rather than falling through to the method path reports the construct
        // the source actually wrote.
        let Some(canonical) = self.type_info.resolved_identity(span).cloned() else {
            return Some(self.unsupported_operand(
                format!("module-qualified call `{name}` without a resolved canonical declaration"),
                scope,
                hir_span_value,
                out,
            ));
        };
        if matches!(
            canonical.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Newtype
        ) {
            if self.type_info.body_ir_constructor_field_binding(span).is_some() {
                return Some(self.lower_nominal_construction(name, args, span, span, scope, out));
            }
            let Some(slots) = self.supplied_module_constructor_slots(&canonical, args) else {
                return Some(self.unsupported_operand(
                    format!("module-qualified construction of `{name}` without a checked canonical field binding"),
                    scope,
                    hir_span_value,
                    out,
                ));
            };
            return Some(self.lower_nominal_construction_with_slots(
                name,
                args,
                span,
                span,
                scope,
                out,
                (slots.len(), &slots),
            ));
        }
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return Some(self.unsupported_operand(description, scope, hir_span_value, out));
            }
        };
        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let (operands, binding) =
            match self.bind_declared_args(&format!("function `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return Some(self.unsupported_operand(description, scope, hir_span_value, out));
                }
            };
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            // The declaration lives in another module, so it has no span identity in this one. `canonical` is the
            // fact that survives the boundary, and it is the only one a consumer may dispatch on here.
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: name.to_string(),
                direct_call_id: None,
                canonical: Some(canonical),
                builtin: None,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        ))
    }

    /// The checked result type of the method call at `span`.
    ///
    /// In the standard library's own trait defaults, a call on an open `Self` receiver keeps its checked type open for
    /// legacy; its slot's declared result, which the checker retains for Body IR only, is the call's type instead.
    pub(super) fn method_call_result_type(&self, span: ast::Span) -> IncanType {
        let checked = self.resolve_ty(span);
        if !matches!(checked, IncanType::Unknown) {
            return checked;
        }
        self.type_info
            .traits
            .self_method_result_types
            .get(&(span.start, span.end))
            .map_or(checked, |declared| self.checked_type(declared))
    }

    /// Complete a caller-open receiver from one retained source nominal and its physical checked method signature.
    ///
    /// Existing call identities remain authoritative. A generic owner, overloaded member, or method without an
    /// ordinary immutable instance contract stays refused; selecting a concrete source method here does not infer
    /// runtime dispatch or reconstruct a canonical identity from its spelling.
    fn retained_receiver_method_binding(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> Option<&'type_info crate::typechecker::FunctionBindingInfo> {
        if self.type_info.resolved_identity(span).is_some() {
            return None;
        }
        let recorded = self
            .type_info
            .expr_type(receiver.span)
            .map(|ty| self.checked_type(ty))?;
        if !self.leaves_type_open(&recorded) {
            return None;
        }
        let IncanType::Named(owner_name) = self.resolve_ty(receiver.span) else {
            return None;
        };
        let owner = self.type_info.declarations.named_type_identities.get(&owner_name)?;
        self.local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == owner && declaration.type_parameters.is_empty())?;
        let selected = collect::checked_slot_method_name(self.type_info, owner, name);
        let mut candidates = self
            .local_receiver_methods
            .iter()
            .filter(|method| &method.owner == owner && method.name == selected);
        let method = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        let facts: &'type_info TypeCheckInfo = self.type_info;
        let binding = facts
            .declarations
            .method_bindings_by_span
            .get(&(method.span.start, method.span.end))?;
        let identity = binding.identity.as_ref()?;
        (identity.kind == SemanticSourceTargetKind::Method).then_some(binding)
    }

    /// Lower a method call `recv.name(args)` to a [`bir::Callee::Method`] call, with the receiver prepended to
    /// `args[0]` as a shared or exclusive borrow operand (see the inline comment on the receiver-borrow decision
    /// below).
    ///
    /// Argument binding goes through the same [`plan_declared_args`] planner every other call shape uses, against
    /// the typechecker's own rest-aware call-site signature for this span -- which already has the receiver's
    /// generic arguments substituted, so a generic method's slots are concrete here. The receiver is deliberately
    /// outside the recorded binding: its slots index the method's declared parameters, so a consumer reads
    /// `args[0]` as the receiver and `args[1..]` as the bound arguments. A method call whose signature the
    /// typechecker did not record still lowers positional arguments faithfully and refuses only the spellings it
    /// cannot bind — a named spelling with no spread beside it — matching [`Self::lower_call`]'s treatment of an
    /// unresolved direct callee. Spread arguments lower here too, after the receiver.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if self.type_info.resolved_builtin_call(span) == Some(BuiltinFnId::IsInstance) {
            return self.lower_checked_isinstance_call(type_args, args, span, scope, out);
        }
        self.settle_open_call_arguments(span, args);
        if let Some(lowered) = self.lower_checked_builtin_method_call(type_args, args, span, scope, out) {
            return lowered;
        }
        if let Some(lowered) = self.lower_list_repeat_call(recv, name, type_args, args, span, scope, out) {
            return lowered;
        }
        if type_args.is_empty()
            && let Some(lowered) = self.lower_text_codec_call(recv, args, span, scope, out)
        {
            return lowered;
        }
        if let Some(lowered) = self.lower_module_qualified_call(recv, name, type_args, args, span, scope, out) {
            return lowered;
        }
        if self.static_receiver_type(recv, span).is_some() {
            return self.lower_static_method_call(recv, name, type_args, args, span, scope, out);
        }
        if let Some(sum) = self.lower_newtype_sum(recv, name, type_args, args, span, scope, out) {
            return sum;
        }
        let retained_binding = self.retained_receiver_method_binding(recv, name, span);
        let helper = match self.checked_string_helper_for_method_call(recv, name, span) {
            Ok(helper) => helper,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(_) => {
                return self.unsupported_operand(
                    "method call with unresolved explicit type arguments".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };

        // A call on an open `Self` receiver in a standard-library trait default binds against its slot's declared
        // parameters, which the checker retains for Body IR only.
        let native_member = super::runtime_nominals::member(self.type_info, &self.resolve_ty(recv.span), name);
        let declared: Option<Vec<DeclaredSlot>> = self
            .native_source_method_slots(span)
            .or_else(|| {
                self.type_info
                    .traits
                    .self_method_params
                    .get(&(span.start, span.end))
                    .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect())
            })
            .or_else(|| {
                retained_binding.map(|binding| binding.params.iter().map(DeclaredSlot::from_checked_param).collect())
            })
            // Published task members take only their receiver. The frozen checker leaves their external signature
            // open, but the guarded SDK contract supplies an empty ordinary parameter list for binding.
            .or_else(|| native_member.as_ref().map(|_| Vec::new()));

        let retained_method = self.retained_alias_method(recv, name, type_args, args, span);
        let declared = declared.or_else(|| retained_method.as_ref().map(|_| Vec::new()));

        let intrinsic_carrier = self.checked_intrinsic_carrier(recv, name);
        let receiver_operand = self.lower_method_receiver(recv, name, intrinsic_carrier.as_deref(), span, scope, out);

        let (mut arg_operands, binding) =
            match self.bind_declared_args(&format!("method `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
        arg_operands = self.retain_list_probe_carrier(recv, args, arg_operands, span, scope, out);

        // The receiver is `args[0]` and is never spliced, so it is always a single-value element.
        let mut call_args = Vec::with_capacity(arg_operands.len() + 1);
        call_args.push(bir::ArgumentElement::One(receiver_operand));
        call_args.append(&mut arg_operands);
        self.retain_result_call_channels(span, recv.span, &call_args);
        let checked = native_member.as_ref().map_or_else(
            || {
                retained_method.as_ref().map_or_else(
                    || self.method_call_result_type(span),
                    |method| method.return_type.clone(),
                )
            },
            |member| member.return_type.clone(),
        );
        let ty = match retained_binding {
            Some(binding) if self.leaves_type_open(&checked) => self.checked_type(&binding.return_type),
            _ => checked,
        };
        if let Some(helper) = helper {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            return self.push_call_temp(
                bir::Callee::Helper(helper),
                call_args,
                ty,
                scope,
                hir_span_value,
                false,
                out,
            );
        }
        self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                name: name.to_string(),
                intrinsic_carrier,
                reflection: self.checked_reflection_operation(span),
                sdk_trait: self.type_info.resolved_identity(span).and_then(|identity| {
                    super::library_calls::trait_dispatch(
                        identity,
                        self.type_info.resolved_method_call(span).map(|call| &call.dispatch),
                    )
                }),
                // A standard slot reached through the stdlib source cache is named by its provider identity, the one
                // the program's implementation records and the SDK's published bodies carry.
                canonical: native_member
                    .map(|member| member.canonical)
                    .or_else(|| {
                        self.type_info
                            .traits
                            .provider_slot_identities
                            .get(&(span.start, span.end))
                            .or_else(|| self.type_info.resolved_identity(span))
                            .or_else(|| {
                                self.type_info
                                    .traits
                                    .inferred_method_identities
                                    .get(&(span.start, span.end))
                            })
                            .or_else(|| {
                                self.type_info
                                    .traits
                                    .self_method_identities
                                    .get(&(span.start, span.end))
                            })
                            .or_else(|| retained_binding.and_then(|binding| binding.identity.as_ref()))
                            .cloned()
                    })
                    .or_else(|| retained_method.map(|method| method.canonical)),
                type_args: resolved_type_args,
                binding,
            }),
            call_args,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Retain the existing checked standard-carrier method profile once for both receiver evaluation and dispatch.
    /// User-defined methods and other carriers keep their canonical source method path.
    fn checked_intrinsic_carrier(&self, receiver: &ast::Spanned<ast::Expr>, name: &str) -> Option<String> {
        match self.resolve_ty(receiver.span) {
            IncanType::Generic { base, .. }
                if (base == "Option" && matches!(name, "is_some" | "is_none" | "unwrap" | "unwrap_or"))
                    || (base == "Result" && matches!(name, "is_ok" | "is_err" | "unwrap" | "unwrap_or")) =>
            {
                Some(base)
            }
            _ => None,
        }
    }

    /// Read consuming frame carriers before their defaults and retain ordinary receivers as borrowed places. Global
    /// carriers stay delayed: legacy evaluates a static wrapper's arguments before locking and cloning storage. A
    /// checked static decorator factory keeps the checker's selected mut-self fact.
    fn lower_method_receiver(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        intrinsic_carrier: Option<&str>,
        call_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if intrinsic_carrier.is_some() && matches!(name, "unwrap" | "unwrap_or") {
            return self.lower_consumed_carrier_receiver(receiver, scope, out);
        }
        if let ast::Expr::Field(base, member) = &receiver.node
            && self
                .local_value_enum_variant_target(base, member, receiver.span)
                .is_some()
        {
            return self.lower_expr_to_operand(receiver, scope, out);
        }
        let place = self.lower_expr_to_place(receiver, scope, out);
        let key = (call_span.start, call_span.end);
        let mutable = self
            .type_info
            .resolved_identity(call_span)
            .is_some_and(collection_method_changes_receiver)
            || (self
                .type_info
                .declarations
                .body_ir_decorator_static_receivers
                .contains_key(&key)
                && self.type_info.calls.mutable_receiver_method_calls.contains(&key));
        let fact = if mutable {
            bir::OwnershipFact::MutBorrow
        } else {
            bir::OwnershipFact::Borrow
        };
        bir::Operand::place(place, fact, false)
    }

    /// Materialize a consuming local, field or mutable-parameter carrier under its checked ownership facts, so later
    /// argument effects cannot change its read. A field cannot move out of its owner, so the retained value is a clone,
    /// as legacy's consumed-receiver policy requires. An enclosing local read inside a loop stays owned for the next
    /// iteration even at its final textual occurrence. Global reads retain their static access boundary instead.
    fn lower_consumed_carrier_receiver(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let ty = self.resolve_ty(receiver.span);
        let mut value = self.lower_expr_to_operand(receiver, scope, out);
        if let bir::Operand::Place(read) = &mut value {
            if read.place.global().is_some() {
                read.fact = bir::OwnershipFact::Borrow;
                return value;
            }
            if read.fact == bir::OwnershipFact::Borrow {
                read.fact = bir::OwnershipFact::Clone;
            }
            if read.fact == bir::OwnershipFact::Move
                && !self.loop_break_targets.is_empty()
                && let Some(local) = read.place.local_id()
                && self.locals[local.index()].scope != scope
            {
                read.fact = bir::OwnershipFact::Clone;
                read.last_use = false;
                self.moved_out.remove(&local);
            }
        }
        self.push_assign_temp(bir::Rvalue::Use(value), ty, scope, hir_span(receiver.span), out)
    }

    /// Invoke a checked callable expression through its evaluated value, retaining source evaluation order.
    ///
    /// The expression's retained function signature owns argument binding. A call result or closure expression is an
    /// owned value evaluated before its arguments. Reads of existing callable storage, explicit type applications,
    /// rest parameters and open carriers remain named refusals; no declaration is selected by source spelling.
    fn lower_evaluated_callable_call(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if !matches!(
            expression.node,
            ast::Expr::Call(..) | ast::Expr::MethodCall(..) | ast::Expr::Closure(..)
        ) {
            return self.unsupported_operand(
                "callable expression without a retained owned evaluation".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let ty = self.resolve_ty(expression.span);
        let IncanType::Function { params, .. } = &ty else {
            return self.unsupported_operand(
                "callable expression without a checked function signature".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if !type_args.is_empty() || self.leaves_type_open(&ty) {
            return self.unsupported_operand(
                "callable expression with an unretained type application".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        if params
            .iter()
            .any(|parameter| parameter.kind != IncanCallableParamKind::Normal)
        {
            return self.unsupported_operand(
                "callable expression with rest parameters".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let slots = params.iter().map(DeclaredSlot::from_semantic_param).collect::<Vec<_>>();
        let planned = match plan_declared_args("callable expression", &slots, args) {
            Ok(planned) => planned,
            Err(description) => return self.unsupported_operand(description, scope, hir_span_value, out),
        };

        // ---- Context: freeze the callable before evaluating its arguments ----
        let callable = self.lower_expr_to_operand(expression, scope, out);
        let callable = self.push_assign_temp(bir::Rvalue::Use(callable), ty, scope, hir_span(expression.span), out);
        let bir::Operand::Place(operand) = callable else {
            return self.unsupported_operand(
                "callable expression without a retained value".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };

        // ---- Context: bind the physical arguments and invoke that evaluated value ----
        let (arguments, binding) = match self.lower_planned_args(
            &planned,
            slots.len(),
            &slots.iter().map(|slot| slot.is_mut).collect::<Vec<_>>(),
            &slots.iter().map(|slot| slot.ty.clone()).collect::<Vec<IncanType>>(),
            scope,
            out,
        ) {
            Ok(bound) => bound,
            Err(description) => return self.unsupported_operand(description, scope, hir_span_value, out),
        };
        let mut arguments = fixed_elements(arguments);
        let shared = slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| matches!(slot.ty, IncanType::Ref(_)).then_some(index))
            .collect::<Vec<_>>();
        self.share_argument_slots(&mut arguments, &binding, &shared);
        let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
            operand,
            binding,
        }));
        self.push_call_temp(
            callee,
            arguments,
            self.resolve_ty(span),
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Whether the member read at `span` is a computed property the checker resolved, rather than a stored field.
    pub(super) fn is_property_read(&self, span: ast::Span) -> bool {
        self.type_info
            .resolved_identity(span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Property)
    }

    /// Retain reflection only when the checked canonical member belongs to the compiler-generated owner table.
    /// Generic-binder ownership selects string results before substitution; concrete ownership selects typed values.
    fn checked_reflection_operation(&self, span: ast::Span) -> Option<bir::ReflectionOperation> {
        let identity = self.type_info.resolved_identity(span)?;
        let owner = self
            .type_info
            .calls
            .compiler_generated_member_identities
            .get(identity)?;
        let generic = owner.kind == SemanticSourceTargetKind::GenericBinder;
        match (identity.declaration_name.as_str(), generic) {
            ("__field_value__", false) => Some(bir::ReflectionOperation::TypedValue),
            ("__field_items__", false) => Some(bir::ReflectionOperation::TypedItems),
            ("__field_value__", true) => Some(bir::ReflectionOperation::StringValue),
            ("__field_items__", true) => Some(bir::ReflectionOperation::StringItems),
            _ => None,
        }
    }

    /// Retain a checked builtin list lookup's numeric element carrier even when its legacy signature table is absent.
    ///
    /// The canonical builtin member and list-method registry authorize the single probe slot. The receiver's checked
    /// element type supplies its destination; a user method with the same spelling and unshaped spreads stay unchanged.
    fn retain_list_probe_carrier(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        mut elements: Vec<bir::ArgumentElement>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::ArgumentElement> {
        let Some(identity) = self.type_info.resolved_identity(span) else {
            return elements;
        };
        if identity.origin != SymbolOrigin::Builtin {
            return elements;
        }
        let Some(member) = identity.declaration_name.strip_prefix("List.") else {
            return elements;
        };
        if !matches!(
            list_methods::from_str(member),
            Some(
                list_methods::ListMethodId::Count
                    | list_methods::ListMethodId::Contains
                    | list_methods::ListMethodId::Index
            )
        ) {
            return elements;
        }
        let [argument] = args else { return elements };
        let expression = match argument {
            ast::CallArg::Positional(expression) | ast::CallArg::Named(_, expression) => expression,
            _ => return elements,
        };
        if elements.len() != 1 {
            return elements;
        }
        let receiver_type = self.resolve_ty(receiver.span);
        let Some([element_type]) = collection_member_types(&receiver_type, CollectionTypeId::List) else {
            return elements;
        };
        match elements.pop() {
            Some(bir::ArgumentElement::One(operand)) => elements.push(bir::ArgumentElement::One(
                self.retain_numeric_destination(expression, operand, element_type, scope, out),
            )),
            Some(other) => elements.push(other),
            None => {}
        }
        elements
    }

    /// Lower `Type.method(args)`, a call to a type's static method, as a direct call to the method's body.
    ///
    /// A static method has no receiver, so it is called exactly like a function: the checker's selected method is
    /// the canonical identity, and when that method is declared in this module its declaration span is the
    /// `direct_call_id` the method's own [`bir::Body`] carries. A selection that is not a method (an enum variant
    /// constructor, say, or a member the checker did not resolve) refuses rather than being called by its spelling.
    #[allow(clippy::too_many_arguments)]
    fn lower_static_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if let Some(target) = self.checked_enum_variant_target(recv, name, span) {
            return self.lower_enum_variant_call(target, args, span, scope, out);
        }
        let spelling = match &recv.node {
            ast::Expr::Ident(owner) => format!("{owner}.{name}"),
            _ => name.to_string(),
        };
        let canonical = self
            .type_info
            .resolved_identity(span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Method)
            .cloned();
        let Some(canonical) = canonical else {
            return self.unsupported_operand(
                format!("static member `{spelling}` without a resolved method declaration"),
                scope,
                hir_span_value,
                out,
            );
        };
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(_) => {
                return self.unsupported_operand(
                    "method call with unresolved explicit type arguments".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };
        let direct_call_id = self
            .type_info
            .declarations
            .method_bindings_by_span
            .iter()
            .find(|(_, binding)| binding.identity.as_ref() == Some(&canonical))
            .map(|((start, end), _)| CompilerNodeId::declaration_span(self.module_identity, *start, *end));
        let declared = self.native_source_method_slots(span);
        let (operands, binding) =
            match self.bind_declared_args(&format!("static method `{spelling}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: self.static_receiver_type(recv, span),
                name: spelling,
                direct_call_id,
                canonical: Some(canonical),
                builtin: None,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Use an atomic accepted owner instance before ordinary shared signature facts, preserving the same binder.
    fn native_source_method_slots(&self, span: ast::Span) -> Option<Vec<DeclaredSlot>> {
        self.type_info
            .body_ir_newtypes
            .callable_params(span, self.type_info)
            .or_else(|| self.type_info.body_ir_call_site_callable_params(span))
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect())
    }

    /// Return the helper operation selected by a checked string-method call, or a source-span-preserving refusal.
    ///
    /// Body IR only maps the retained [`StringMethodId`](incan_lang::lang::surface::string_methods::StringMethodId)
    /// to a [`bir::HelperOp`]. The source registry is consulted solely to validate that an admitted spelling still
    /// agrees with the checked identity, so a missing or corrupted fact cannot turn a raw method name into runtime
    /// dispatch.
    fn checked_string_helper_for_method_call(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> Result<Option<bir::HelperOp>, String> {
        let checked = self.type_info.resolved_string_helper_call(span);
        let runtime_string_receiver = matches!(
            self.resolve_ty(receiver.span),
            IncanType::Primitive(IncanPrimitiveType::Str)
        );
        if !runtime_string_receiver {
            return match checked {
                Some(_) => Err("checked string helper identity has a non-string receiver".to_string()),
                None => Ok(None),
            };
        }
        let source = incan_lang::lang::surface::string_methods::from_str(name);
        match checked {
            Some(method) => {
                let Some(helper) = bir::HelperOp::for_selected_string_method(method) else {
                    return Err(format!("unadmitted checked string helper identity `{method:?}`"));
                };
                if source != Some(method) {
                    return Err("checked string helper identity does not match the source method".to_string());
                }
                Ok(Some(helper))
            }
            None if source.and_then(bir::HelperOp::for_selected_string_method).is_some() => {
                Err("selected string helper call is missing its checked string helper identity".to_string())
            }
            None => Ok(None),
        }
    }
}

/// Project a compiler-owned builtin id only from its retained RFC 120 identity.
///
/// The call-site spelling is deliberately absent from this function. A same-spelled source declaration, import, or
/// alias cannot become a builtin by textual resemblance, while a canonical builtin alias retains the registry's
/// declaration name and therefore still projects to the same [`BuiltinFnId`].
fn canonical_builtin(identity: &CanonicalSymbolId) -> Option<incan_lang::lang::builtins::BuiltinFnId> {
    (identity.origin == incan_semantics_core::SymbolOrigin::Builtin
        && identity.namespace == incan_semantics_core::SymbolNamespace::OrdinaryLexical
        && identity.kind == SemanticSourceTargetKind::Builtin
        && identity.scope_discriminant.is_none()
        && identity.declaration_span == HirSourceSpan::new(0, 0))
    .then(|| incan_lang::lang::builtins::from_str(&identity.declaration_name))
    .flatten()
}

/// Retain the sole operand frozen legacy passes to canonical `enumerate`, including a written positional spread.
///
/// Legacy's builtin emission directly enumerates the outer operand instead of unpacking its elements into arguments.
/// Only the checker's canonical builtin identity authorizes this profile; user callables keep ordinary spread binding.
fn legacy_enumerate_argument(builtin: Option<BuiltinFnId>, args: &[ast::CallArg]) -> Option<Vec<ast::CallArg>> {
    if builtin != Some(BuiltinFnId::Enumerate) {
        return None;
    }
    match args {
        [ast::CallArg::PositionalUnpack(source)] => Some(vec![ast::CallArg::Positional(source.clone())]),
        _ => None,
    }
}
