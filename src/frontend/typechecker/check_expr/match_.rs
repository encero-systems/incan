//! Check `match` expressions, patterns, and exhaustiveness.
//!
//! This module validates `match` expressions by type-checking each arm, binding pattern variables,
//! and ensuring exhaustiveness for enums, `Result`, and `Option`.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use incan_core::lang::surface::constructors;
use incan_core::lang::surface::constructors::ConstructorId;
use incan_core::lang::types::collections::{self, CollectionTypeId};

use super::TypeChecker;

impl TypeChecker {
    /// Type-check a `match` expression and return its resolved type.
    pub(in crate::frontend::typechecker::check_expr) fn check_match(
        &mut self,
        subject: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        _span: Span,
    ) -> ResolvedType {
        let subject_ty = self.check_expr(subject);

        self.check_match_exhaustiveness(&subject_ty, arms, _span);

        let mut arm_types = Vec::new();

        for arm in arms {
            self.symbols.enter_scope(ScopeKind::Block);
            self.check_pattern(&arm.node.pattern, &subject_ty);

            let arm_ty = match &arm.node.body {
                MatchBody::Expr(e) => self.check_expr(e),
                MatchBody::Block(stmts) => {
                    for stmt in stmts {
                        self.check_statement(stmt);
                    }
                    ResolvedType::Unit
                }
            };
            arm_types.push(arm_ty);

            self.symbols.exit_scope();
        }

        arm_types.first().cloned().unwrap_or(ResolvedType::Unit)
    }

    /// Type-check a pattern against an expected type, defining bindings in the current scope.
    fn check_pattern(&mut self, pattern: &Spanned<Pattern>, expected_ty: &ResolvedType) {
        match &pattern.node {
            Pattern::Wildcard => {}
            Pattern::Binding(name) => {
                self.symbols.define(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: expected_ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: pattern.span,
                    scope: 0,
                });
            }
            Pattern::Literal(_) => {}
            Pattern::Constructor(name, sub_patterns) => {
                if let Some(cid) = constructors::from_str(name.as_str()) {
                    match cid {
                        ConstructorId::Ok => {
                            if let ResolvedType::Generic(type_name, args) = expected_ty
                                && type_name == collections::as_str(CollectionTypeId::Result)
                                && !args.is_empty()
                            {
                                let mut positional = None;
                                for arg in sub_patterns {
                                    match arg {
                                        PatternArg::Positional(pat) => {
                                            positional = Some(pat);
                                            break;
                                        }
                                        PatternArg::Named(_, pat) => {
                                            self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                                        }
                                    }
                                }
                                if let Some(pat) = positional {
                                    self.check_pattern(pat, &args[0]);
                                }
                                return;
                            }
                        }
                        ConstructorId::Err => {
                            if let ResolvedType::Generic(type_name, args) = expected_ty
                                && type_name == collections::as_str(CollectionTypeId::Result)
                                && args.len() >= 2
                            {
                                let mut positional = None;
                                for arg in sub_patterns {
                                    match arg {
                                        PatternArg::Positional(pat) => {
                                            positional = Some(pat);
                                            break;
                                        }
                                        PatternArg::Named(_, pat) => {
                                            self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                                        }
                                    }
                                }
                                if let Some(pat) = positional {
                                    self.check_pattern(pat, &args[1]);
                                }
                                return;
                            }
                        }
                        ConstructorId::Some => {
                            if let ResolvedType::Generic(type_name, args) = expected_ty
                                && type_name == collections::as_str(CollectionTypeId::Option)
                                && !args.is_empty()
                            {
                                let mut positional = None;
                                for arg in sub_patterns {
                                    match arg {
                                        PatternArg::Positional(pat) => {
                                            positional = Some(pat);
                                            break;
                                        }
                                        PatternArg::Named(_, pat) => {
                                            self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                                        }
                                    }
                                }
                                if let Some(pat) = positional {
                                    self.check_pattern(pat, &args[0]);
                                }
                                return;
                            }
                        }
                        ConstructorId::None => {
                            return;
                        }
                    }
                }

                let ctor_name = if name.contains("::") {
                    name.split("::").last().unwrap_or(name)
                } else {
                    name.as_str()
                };

                let model_or_class_fields = match expected_ty {
                    ResolvedType::Named(type_name) if ctor_name == type_name => self
                        .lookup_type_info(type_name)
                        .and_then(|type_info| match type_info {
                            TypeInfo::Model(model_info) => Some(model_info.fields.clone()),
                            TypeInfo::Class(class_info) => Some(class_info.fields.clone()),
                            _ => None,
                        })
                        .map(|fields| (type_name, fields)),
                    _ => None,
                };

                if let Some((type_name, fields)) = model_or_class_fields {
                    let mut provided = HashSet::new();
                    for arg in sub_patterns {
                        match arg {
                            PatternArg::Positional(pat) => {
                                self.errors
                                    .push(errors::positional_pattern_not_supported(type_name, pat.span));
                            }
                            PatternArg::Named(field_name, pat) => {
                                let Some((canonical_name, info)) =
                                    self.resolve_field_info(&fields, field_name, true, true)
                                else {
                                    self.errors.push(errors::missing_field(type_name, field_name, pat.span));
                                    continue;
                                };

                                if !provided.insert(canonical_name.clone()) {
                                    self.errors.push(errors::duplicate_pattern_field(
                                        type_name,
                                        canonical_name.as_str(),
                                        pat.span,
                                    ));
                                    continue;
                                }
                                self.check_pattern(pat, &info.ty);
                            }
                        }
                    }
                    return;
                }

                let variant_name = ctor_name;

                let positional_count = sub_patterns
                    .iter()
                    .filter(|a| matches!(a, PatternArg::Positional(_)))
                    .count();

                let rust_resolution =
                    self.rust_enum_constructor_payload_types(expected_ty, name.as_str(), positional_count);
                let field_types: Option<Vec<ResolvedType>> = self
                    .symbols
                    .lookup(variant_name)
                    .and_then(|id| self.symbols.get(id))
                    .and_then(|sym| {
                        if let SymbolKind::Variant(info) = &sym.kind {
                            if self.match_variant_symbol_applies_to_scrutinee(expected_ty, info, positional_count) {
                                Some(info.fields.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .or_else(|| match rust_resolution.as_ref() {
                        Some(RustEnumPatternResolution::PayloadTypes(fields)) => Some(fields.clone()),
                        Some(RustEnumPatternResolution::QualifierMismatch) | None => None,
                    });

                match field_types {
                    Some(fields) => {
                        self.check_constructor_subpatterns_enum_like(
                            name.as_str(),
                            sub_patterns,
                            Some(fields.as_slice()),
                        );
                    }
                    None => {
                        let permissive = self.match_subject_allows_unknown_rust_enum_payloads(
                            expected_ty,
                            name.as_str(),
                            rust_resolution.as_ref(),
                        );
                        if !permissive && !matches!(expected_ty, ResolvedType::Unknown) {
                            self.errors.push(errors::unknown_match_constructor_pattern(
                                name.as_str(),
                                &expected_ty.to_string(),
                                pattern.span,
                            ));
                        }
                        self.check_constructor_subpatterns_enum_like(name.as_str(), sub_patterns, None);
                    }
                }
            }
            Pattern::Tuple(sub_patterns) => {
                if let ResolvedType::Tuple(elem_types) = expected_ty {
                    for (pat, elem_ty) in sub_patterns.iter().zip(elem_types.iter()) {
                        self.check_pattern(pat, elem_ty);
                    }
                }
            }
        }
    }

    /// Positional sub-patterns for enum-like constructor patterns: known payload types per index, or all
    /// [`ResolvedType::Unknown`] when `known_fields` is `None` (Rust interop best-effort).
    fn check_constructor_subpatterns_enum_like(
        &mut self,
        ctor_label: &str,
        sub_patterns: &[PatternArg],
        known_fields: Option<&[ResolvedType]>,
    ) {
        let mut idx = 0usize;
        for arg in sub_patterns {
            match arg {
                PatternArg::Positional(pat) => {
                    if let Some(fields) = known_fields {
                        if let Some(field_ty) = fields.get(idx) {
                            self.check_pattern(pat, field_ty);
                        }
                    } else {
                        self.check_pattern(pat, &ResolvedType::Unknown);
                    }
                    idx += 1;
                }
                PatternArg::Named(_, pat) => {
                    self.errors
                        .push(errors::named_pattern_not_supported(ctor_label, pat.span));
                }
            }
        }
    }

    /// When constructor payload types are missing, only Rust-backed match subjects use permissive
    /// [`ResolvedType::Unknown`] payload checking, and only when the written constructor does not already prove the
    /// pattern is invalid (for example an explicit mismatched rusttype qualifier).
    fn match_subject_allows_unknown_rust_enum_payloads(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        rust_resolution: Option<&RustEnumPatternResolution>,
    ) -> bool {
        let is_rust_backed = match expected_ty {
            ResolvedType::RustPath(_) => true,
            ResolvedType::Named(type_name) => self.lookup_type_info(type_name).is_some_and(|info| {
                matches!(
                    info,
                    TypeInfo::Newtype(nt) if nt.is_rusttype && matches!(&nt.underlying, ResolvedType::RustPath(_))
                )
            }),
            _ => false,
        };
        if !is_rust_backed {
            return false;
        }

        match rust_resolution {
            Some(RustEnumPatternResolution::PayloadTypes(_)) => true,
            Some(RustEnumPatternResolution::QualifierMismatch) => false,
            None => !pattern_full_name.contains("::"),
        }
    }

    /// Whether a [`SymbolKind::Variant`] from the symbol table actually describes this match scrutinee.
    ///
    /// Rust metadata and library manifests register variant names (e.g. `Root`) at module scope. A `rusttype` alias
    /// such as `PlanRel` uses a **different** Incan name than the backing Rust enum (`Sender`), so we must not let an
    /// unrelated `Root` stub with empty payload metadata shadow [`Self::rust_enum_constructor_payload_types`], or
    /// payload bindings in the pattern are never registered and the arm body sees `Unknown symbol`.
    fn match_variant_symbol_applies_to_scrutinee(
        &self,
        expected_ty: &ResolvedType,
        info: &VariantInfo,
        positional_count: usize,
    ) -> bool {
        if positional_count > info.fields.len() {
            return false;
        }
        match expected_ty {
            ResolvedType::Named(type_name) => info.enum_name == *type_name,
            // Scrutinee is a bare Rust path: module-level variant symbols are Incan-/manifest-scoped names.
            ResolvedType::RustPath(_) => false,
            _ => false,
        }
    }

    /// Tuple-variant payload types for `match` patterns on Rust-backed enum surfaces.
    ///
    /// Incan registers [`SymbolKind::Variant`] for source/manifest enums; imported Rust enums and prost-style oneofs
    /// are usually spelled as a `rusttype` / [`TypeInfo::Newtype`] wrapper over [`ResolvedType::RustPath`]. For a
    /// single positional sub-pattern, the payload type is `RustPath("{backing}::{variant}")`, consistent with Rust
    /// field/member path composition in `check_expr::access`. Multiple positional patterns without precise metadata use
    /// [`ResolvedType::Unknown`] per slot.
    ///
    /// When the scrutinee is already [`ResolvedType::RustPath`], any `Type::Variant` prefix in the pattern is not
    /// validated against that path (unlike [`ResolvedType::Named`] rusttypes, where the prefix must match the Incan
    /// type name). Payload typing still uses `{scrutinee_rust_path}::{variant}`.
    fn rust_enum_constructor_payload_types(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        positional_count: usize,
    ) -> Option<RustEnumPatternResolution> {
        let (enum_qualifier_opt, variant_segment) = match pattern_full_name.rsplit_once("::") {
            Some((e, v)) => (Some(e), v),
            None => (None, pattern_full_name),
        };

        let base_rust_path: String = match expected_ty {
            ResolvedType::Named(type_name) => {
                if let Some(q) = enum_qualifier_opt
                    && q != type_name.as_str()
                {
                    return Some(RustEnumPatternResolution::QualifierMismatch);
                }
                let info = self.lookup_type_info(type_name)?;
                match info {
                    TypeInfo::Newtype(nt) if nt.is_rusttype => match &nt.underlying {
                        ResolvedType::RustPath(p) => p.clone(),
                        _ => return None,
                    },
                    _ => return None,
                }
            }
            ResolvedType::RustPath(p) => p.clone(),
            _ => return None,
        };

        Some(RustEnumPatternResolution::payloads(match positional_count {
            0 => vec![],
            1 => vec![ResolvedType::RustPath(format!("{base_rust_path}::{variant_segment}"))],
            n => (0..n).map(|_| ResolvedType::Unknown).collect(),
        }))
    }

    /// Check that a match expression covers all possible cases.
    ///
    /// For enums, `Result`, and `Option`, verifies every variant is handled. Wildcards
    /// (`_`) satisfy all remaining cases. Emits a [`non_exhaustive_match`](errors::non_exhaustive_match)
    /// error if patterns are missing.
    fn check_match_exhaustiveness(&mut self, subject_ty: &ResolvedType, arms: &[Spanned<MatchArm>], span: Span) {
        let variants = if let ResolvedType::Named(name) = subject_ty {
            match self.lookup_type_info(name) {
                Some(TypeInfo::Enum(enum_info)) => Some(enum_info.variants.clone()),
                _ => None,
            }
        } else if subject_ty.is_result() || subject_ty.is_option() {
            if subject_ty.is_result() {
                Some(vec![
                    constructors::as_str(ConstructorId::Ok).to_string(),
                    constructors::as_str(ConstructorId::Err).to_string(),
                ])
            } else {
                Some(vec![
                    constructors::as_str(ConstructorId::Some).to_string(),
                    constructors::as_str(ConstructorId::None).to_string(),
                ])
            }
        } else {
            None
        };

        if let Some(all_variants) = variants {
            let mut covered: HashSet<String> = HashSet::new();
            let mut has_wildcard = false;

            for arm in arms {
                match &arm.node.pattern.node {
                    Pattern::Wildcard | Pattern::Binding(_) => {
                        has_wildcard = true;
                    }
                    Pattern::Literal(Literal::None) if subject_ty.is_option() => {
                        covered.insert(constructors::as_str(ConstructorId::None).to_string());
                    }
                    Pattern::Constructor(name, _) => {
                        let variant_name = if name.contains("::") {
                            name.split("::").last().unwrap_or(name).to_string()
                        } else {
                            name.clone()
                        };
                        covered.insert(variant_name);
                    }
                    _ => {}
                }
            }

            if !has_wildcard {
                let missing: Vec<String> = all_variants.iter().filter(|v| !covered.contains(*v)).cloned().collect();

                if !missing.is_empty() {
                    self.errors.push(errors::non_exhaustive_match(&missing, span));
                }
            }
        }
    }
}

enum RustEnumPatternResolution {
    PayloadTypes(Vec<ResolvedType>),
    QualifierMismatch,
}

impl RustEnumPatternResolution {
    fn payloads(fields: Vec<ResolvedType>) -> Self {
        Self::PayloadTypes(fields)
    }
}
