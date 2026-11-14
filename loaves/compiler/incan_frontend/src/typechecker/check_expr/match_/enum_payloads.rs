//! Constructor subpatterns and Incan, Rust and native enum payload types.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    /// Collect variable bindings defined while checking one isolated alternation alternative.
    pub(super) fn collect_pattern_bindings_since(&self, start: usize) -> HashMap<String, PatternBinding> {
        self.symbols
            .all_symbols()
            .iter()
            .skip(start)
            .filter_map(|symbol| match &symbol.kind {
                SymbolKind::Variable(info) => Some((
                    symbol.name.clone(),
                    PatternBinding {
                        ty: info.ty.clone(),
                        span: symbol.span,
                    },
                )),
                _ => None,
            })
            .collect()
    }

    /// Check positional payloads against their ordered types and named native payloads against metadata field
    /// identities. Only legacy positional patterns without metadata retain Unknown types; named fields never guess.
    pub(super) fn check_constructor_subpatterns_enum_like(
        &mut self,
        ctor_label: &str,
        sub_patterns: &[PatternArg],
        known_fields: Option<&[ResolvedType]>,
        rust_backed: bool,
        rust_resolution: Option<&RustEnumPatternResolution>,
    ) {
        let mut idx = 0usize;
        let mut provided = HashSet::new();
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
                PatternArg::Named(name, pat) => {
                    if rust_backed {
                        if !provided.insert(name.node.clone()) {
                            self.errors
                                .push(errors::duplicate_pattern_field(ctor_label, &name.node, name.span));
                            continue;
                        }
                        let selected = match rust_resolution {
                            Some(RustEnumPatternResolution::PayloadTypes { fields, names, .. }) => names
                                .iter()
                                .position(|field| field == &name.node)
                                .and_then(|slot| fields.get(slot)),
                            _ => None,
                        };
                        if let Some(field) = selected {
                            self.check_pattern(pat, field);
                        } else {
                            self.errors
                                .push(errors::missing_field(ctor_label, &name.node, name.span));
                        }
                    } else {
                        self.errors
                            .push(errors::named_pattern_not_supported(ctor_label, pat.span));
                    }
                }
            }
        }
    }

    /// Recognize metadata-backed native subjects, directly or through a rusttype wrapper. Legacy positional
    /// fallbacks may remain permissive while checking, but a missing fact never grants executable layout authority.
    pub(super) fn pattern_subject_is_rust_backed(&self, expected_ty: &ResolvedType) -> bool {
        let (expected_ty, _) = borrowed_pattern_subject(expected_ty);
        match expected_ty {
            ResolvedType::RustPath(_) => true,
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => {
                self.lookup_type_info(type_name).is_some_and(|info| {
                    matches!(
                        info,
                        TypeInfo::Newtype(nt) if nt.is_rusttype && matches!(&nt.underlying, ResolvedType::RustPath(_))
                    )
                })
            }
            _ => false,
        }
    }

    /// When constructor payload types are missing, only Rust-backed match subjects use permissive
    /// [`ResolvedType::Unknown`] payload checking, and only when the written constructor does not already prove the
    /// pattern is invalid (for example an explicit mismatched rusttype qualifier).
    pub(super) fn match_subject_allows_unknown_rust_enum_payloads(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        rust_resolution: Option<&RustEnumPatternResolution>,
    ) -> bool {
        if !self.pattern_subject_is_rust_backed(expected_ty) {
            return false;
        }

        match rust_resolution {
            Some(RustEnumPatternResolution::PayloadTypes { .. }) => true,
            Some(RustEnumPatternResolution::QualifierMismatch) => false,
            None => !pattern_full_name.contains("::"),
        }
    }

    /// Payload types for a source-defined enum variant, using the enum type's own metadata under the subject's type
    /// arguments.
    ///
    /// Qualified patterns such as `Color.Red` should not depend on a module-level `Red` symbol being importable or
    /// winning same-scope shadowing. The scrutinee already tells us which enum is being matched, so resolve the
    /// variant from that enum's table. The caller compares the pattern's sub-pattern count with the payload count.
    pub(super) fn incan_enum_constructor_payload_types(
        &mut self,
        expected_ty: &ResolvedType,
        variant_name: &str,
        enum_qualifier_opt: Option<&str>,
    ) -> Option<Vec<ResolvedType>> {
        let enum_name = match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => type_name,
            _ => return None,
        };
        if let Some(qualifier) = enum_qualifier_opt {
            if !self.pattern_type_spelling_names_subject(qualifier, enum_name) {
                return None;
            }
            let segments = qualifier.split("::").map(str::to_owned).collect::<Vec<_>>();
            let resolved = self.resolve_qualified_name(&segments).ok()?;
            let ResolvedNamespaceMemberKind::Symbol(SymbolKind::Type(TypeInfo::Enum(enum_info))) = resolved.kind else {
                return None;
            };
            return self.source_enum_variant_payload_types(&enum_info, expected_ty, variant_name);
        }
        let Some(TypeInfo::Enum(enum_info)) = self.lookup_semantic_type_info(enum_name) else {
            return None;
        };
        self.source_enum_variant_payload_types(enum_info, expected_ty, variant_name)
    }

    /// Select one source enum variant's payloads and substitute the matched subject's type arguments.
    fn source_enum_variant_payload_types(
        &self,
        enum_info: &EnumInfo,
        expected_ty: &ResolvedType,
        variant_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let canonical_variant = enum_info
            .variant_aliases
            .get(variant_name)
            .map(String::as_str)
            .unwrap_or(variant_name);
        if !enum_info.variants.iter().any(|variant| variant == canonical_variant) {
            return None;
        }
        // A generic enum's payloads are typed under the subject's type arguments, as a generic record's fields are.
        let substitutions = match expected_ty {
            ResolvedType::Generic(_, type_args) => type_param_subst_map(&enum_info.type_params, type_args),
            _ => HashMap::new(),
        };
        Some(
            enum_info
                .variant_fields
                .get(canonical_variant)
                .map(|fields| {
                    fields
                        .iter()
                        .map(|field| {
                            let field = substitute_resolved_type(field, &substitutions);
                            self.explicit_ownership_value_type(&field).unwrap_or(field)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )
    }

    /// Whether a pattern qualifier names a rusttype other than the rusttype being matched. Wrappers over one Rust enum
    /// share its identity path, yet each is its own type, so another wrapper's qualifier does not name the subject.
    pub(super) fn qualifier_is_other_rusttype(&mut self, qualifier: &str, subject: &ResolvedType) -> bool {
        let (ResolvedType::Named(subject_name) | ResolvedType::Generic(subject_name, _)) = subject else {
            return false;
        };
        let qualifier_is_rusttype = self
            .lookup_type_info(qualifier)
            .is_some_and(|info| matches!(info, TypeInfo::Newtype(nt) if nt.is_rusttype));
        let subject_is_rusttype = self
            .lookup_type_info(subject_name)
            .is_some_and(|info| matches!(info, TypeInfo::Newtype(nt) if nt.is_rusttype));
        qualifier_is_rusttype
            && subject_is_rusttype
            && !self.pattern_type_spelling_names_subject(qualifier, subject_name)
    }

    /// Select a native variant from complete metadata and retain its exact storage layout once. Positional payloads
    /// inherit the subject's reference binding mode; named payloads keep their native storage type for checked
    /// copy/clone reads. Signatures and patterns share the checked public owner, so written field order and reexport
    /// spellings never establish a second layout or identity.
    pub(super) fn rust_enum_constructor_payload_types(
        &mut self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        span: Span,
    ) -> Option<RustEnumPatternResolution> {
        let (expected_ty, borrow) = borrowed_pattern_subject(expected_ty);
        let path = self.rust_canonical_path_for_receiver_type(expected_ty)?;
        let (qualifier, variant) = pattern_full_name
            .rsplit_once("::")
            .map_or((None, pattern_full_name), |(owner, variant)| (Some(owner), variant));
        if let Some(qualifier) = qualifier {
            let selected = self
                .rust_canonical_path_for_receiver_type(&ResolvedType::Named(qualifier.to_owned()))
                .unwrap_or_else(|| qualifier.to_owned());
            if self.rust_native_type_identity_path(&selected) != self.rust_native_type_identity_path(&path)
                || self.qualifier_is_other_rusttype(qualifier, expected_ty)
            {
                return Some(RustEnumPatternResolution::QualifierMismatch);
            }
        }
        let metadata = self.rust_item_metadata_for_complete_type(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let layout = self.checked_rust_enum_layout(&path, &info)?;
        let (_, _, names) = layout.variants.iter().find(|(name, _, _)| name == variant)?;
        let names = names.clone();
        let fields = self
            .native_enum_payload_types(expected_ty, &path, variant)?
            .into_iter()
            .map(|field| {
                if names.is_empty() {
                    borrowed_pattern_payload(field, borrow)
                } else {
                    field
                }
            })
            .collect();
        // Option and Result keep their intrinsic constructor identities and storage.
        let ancestral = incan_lang::interop::ancestral_rust_path(&path);
        if matches!(ancestral.as_str(), "core::option::Option" | "core::result::Result") {
            let constructor = constructors::from_str(variant)?;
            self.type_info.record_resolved_identity(
                span,
                crate::symbols::canonical_builtin_constructor_identity(constructor),
            );
            return Some(RustEnumPatternResolution::payloads(fields));
        }
        let public_path = self.rust_public_type_path(&path);
        let (identity, _) = incan_semantics_core::body_ir::native_enum_variant(&public_path, variant)?;
        self.type_info.record_resolved_identity(span, identity);
        self.type_info.rust.checked_enums.insert(public_path, layout);
        Some(RustEnumPatternResolution::PayloadTypes { fields, names })
    }

    /// Return the payload types of native enum `variant` instantiated with `column`'s owner arguments, completing
    /// the arguments the column leaves out from their declared defaults.
    ///
    /// Pattern checking and coverage both read payload types here, so a defaulted owner parameter (`C` in
    /// `ControlFlow[B, C = ()]`) closes the same way for both.
    pub(in crate::typechecker::check_expr) fn native_enum_payload_types(
        &self,
        column: &ResolvedType,
        path: &str,
        variant: &str,
    ) -> Option<Vec<ResolvedType>> {
        let metadata = self.rust_item_metadata_for_complete_type(path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let layout = self.checked_rust_enum_layout(path, info)?;
        let (_, fields, _) = layout.variants.iter().find(|(name, _, _)| name == variant)?;
        let mut arguments = self.rust_owner_type_args(column, path)?;
        if !self.complete_rust_owner_arguments(info, &mut arguments) {
            return None;
        }
        let substitution = type_param_subst_map(&info.type_params, &arguments);
        Some(
            fields
                .iter()
                .map(|field| substitute_resolved_type(field, &substitution))
                .collect(),
        )
    }
}
