//! Declaration lowering for AST to IR conversion.
//!
//! This module handles lowering of all declaration types: functions, models, classes, enums, newtypes, traits, and
//! imports.
//!
//! The logic is split across submodules by declaration kind; all methods live on `impl AstLowering`.

mod classes;
mod default_consts;
mod enums;
mod functions;
mod helpers;
mod imports;
mod method_partials;
mod methods;
mod models;
mod newtypes;
mod traits;

pub(in crate::lower) use functions::callable_docstring;

use super::super::decl::{
    IrDecl, IrDeclKind, IrImportOrigin, IrImportQualifier, IrInteropAdapterKind, IrInteropDirection, IrInteropEdge,
    Visibility,
};
use super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind};
use super::super::types::IrType;
use super::super::{IrSpan, TypedExpr};
use super::AstLowering;
use super::errors::LoweringError;
use incan_frontend::ast;
use incan_lang::lang::decorators::{self, DecoratorId};

/// Which physical spelling a checked source method carries after lowering.
///
/// See [`AstLowering::checked_method_spelling`] for where each one comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckedMethodSpelling {
    /// The declaration's own name: the projection was declined.
    Source,
    /// The compiled provider's canonical projection: a provider plan serves the declaring module.
    Provider,
    /// The source-stdlib projection: the declaring module is compiled alongside this program.
    Projected,
}

impl AstLowering {
    /// Map frontend visibility (`pub` / private) to IR visibility for Rust emission.
    pub(in crate::lower) fn map_visibility(vis: incan_frontend::ast::Visibility) -> Visibility {
        match vis {
            incan_frontend::ast::Visibility::Private => Visibility::Private,
            incan_frontend::ast::Visibility::Public => Visibility::Public,
        }
    }

    /// Map callable visibility, keeping private source stdlib helpers visible across generated sibling modules.
    pub(in crate::lower) fn map_callable_visibility(&self, vis: incan_frontend::ast::Visibility) -> Visibility {
        let mapped = Self::map_visibility(vis);
        if mapped == Visibility::Private
            && self
                .current_source_module_name
                .as_deref()
                .is_some_and(|name| name.starts_with("__incan_std.") || name.starts_with("std."))
        {
            return Visibility::Crate;
        }
        mapped
    }

    /// Map nominal type visibility, keeping private source stdlib helper types aligned with crate-visible helpers.
    pub(in crate::lower) fn map_type_visibility(&self, vis: incan_frontend::ast::Visibility) -> Visibility {
        let mapped = Self::map_visibility(vis);
        if mapped == Visibility::Private
            && self
                .current_source_module_name
                .as_deref()
                .is_some_and(|name| name.starts_with("__incan_std.") || name.starts_with("std."))
        {
            return Visibility::Crate;
        }
        mapped
    }

    /// Lower a declaration to IR.
    ///
    /// # Parameters
    ///
    /// * `decl` - The AST declaration to lower
    ///
    /// # Returns
    ///
    /// The corresponding IR declaration.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError` if the declaration cannot be lowered.
    pub(in crate::lower) fn lower_declaration(
        &mut self,
        decl: &ast::Declaration,
        span: ast::Span,
    ) -> Result<IrDecl, LoweringError> {
        let kind = match decl {
            ast::Declaration::Function(f) => IrDeclKind::Function(self.lower_function(f)?),
            ast::Declaration::Const(c) => {
                if c.name == "__derives__" {
                    return Err(LoweringError {
                        message: "internal __derives__ metadata is not emitted".to_string(),
                        span: IrSpan::default(),
                    });
                }
                let value = self.lower_expr_spanned(&c.value)?;
                // RFC 008: In const context, annotations imply frozen/static types.
                // Prefer frozen annotation if present; otherwise use the initializer type.
                let ty = if let Some(ann) = &c.ty {
                    self.lower_const_annotation_type(&ann.node)
                } else if !matches!(value.ty, IrType::Unknown) {
                    value.ty.clone()
                } else {
                    IrType::Unknown
                };
                // A private const a parameter default names is reached from the default's call sites, which lie
                // outside this module.
                let visibility = match c.visibility {
                    ast::Visibility::Public => Visibility::Public,
                    ast::Visibility::Private if self.const_is_named_by_a_default(&c.name) => Visibility::Public,
                    ast::Visibility::Private => Visibility::Private,
                };
                IrDeclKind::Const {
                    visibility,
                    name: c.name.clone(),
                    ty,
                    value,
                }
            }
            ast::Declaration::Static(s) => {
                let mut value = self.lower_expr_spanned(&s.value)?;
                self.rewrite_checked_registry_entry_subject(&s.name, s.value.span, &mut value)?;
                let visibility = match s.visibility {
                    ast::Visibility::Public => Visibility::Public,
                    ast::Visibility::Private => Visibility::Private,
                };
                IrDeclKind::Static {
                    visibility,
                    name: s.name.clone(),
                    provenance: super::super::decl::IrStaticProvenance::Source(
                        self.emitted_static_identity(&s.name, span)?,
                    ),
                    ty: self.lower_type(&s.ty.node),
                    value,
                }
            }
            ast::Declaration::Model(m) => {
                let struct_ir = self.lower_model(m)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Class(c) => {
                let struct_ir = self.lower_class(c)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Enum(e) => {
                let enum_ir = self.lower_enum(e)?;
                // Register enum name for type resolution
                self.enum_names
                    .insert(enum_ir.name.clone(), IrType::Enum(enum_ir.name.clone()));
                IrDeclKind::Enum(enum_ir)
            }
            ast::Declaration::TypeAlias(a) => IrDeclKind::TypeAlias {
                visibility: self.map_type_visibility(a.visibility),
                name: a.name.clone(),
                type_params: self.lower_type_params(&a.type_params),
                ty: self.lower_type(&a.target.node),
                is_rusttype: false,
                interop_edges: Vec::new(),
            },
            ast::Declaration::Alias(a) => {
                let (target_path, target_origin, target_qualifier) = self.alias_reexport_target(&a.target.segments);
                IrDeclKind::SymbolAlias {
                    visibility: Self::map_visibility(a.visibility),
                    name: a.name.clone(),
                    target_path,
                    target_canonical: self.emitted_symbol_alias_target_identity(&a.name, span)?,
                    target_origin,
                    target_qualifier,
                }
            }
            ast::Declaration::Partial(_) => {
                return Err(LoweringError {
                    message: "Partial callable presets are not lowered by this syntax-only slice".to_string(),
                    span: IrSpan::default(),
                });
            }
            ast::Declaration::Newtype(n) => {
                if n.is_rusttype {
                    let interop_edges = self.lower_interop_edges(&n.interop_edges)?;
                    return Ok(IrDecl::new(IrDeclKind::TypeAlias {
                        visibility: Self::map_visibility(n.visibility),
                        name: n.name.clone(),
                        type_params: self.lower_type_params(&n.type_params),
                        ty: self.lower_type(&n.underlying.node),
                        is_rusttype: true,
                        interop_edges,
                    })
                    .with_span(span.into()));
                }
                // Note: newtype checked construction hook selection is done in `lower_program` when we see the full
                // newtype declaration.
                let struct_ir = self.lower_newtype(n)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Import(i) => self.lower_import(i, span)?,
            ast::Declaration::Trait(t) => IrDeclKind::Trait(self.lower_trait(t)?),
            ast::Declaration::TestModule(_) => {
                return Err(LoweringError {
                    message: "Test modules are not lowered to production IR".to_string(),
                    span: IrSpan::default(),
                });
            }
            ast::Declaration::VocabBlock(_) => {
                return Err(LoweringError {
                    message: "raw vocabulary declarations must be desugared before IR lowering".to_string(),
                    span: IrSpan::default(),
                });
            }
            ast::Declaration::Capability(_) => {
                // A capability declares authority, not code. It carries no body to lower and emits no Rust item;
                // consumers reach it through its checked identity, not through IR.
                return Err(LoweringError {
                    message: "capability declarations are not lowered to IR".to_string(),
                    span: IrSpan::default(),
                });
            }
            ast::Declaration::Docstring(_) => {
                // Skip docstrings in codegen
                return Err(LoweringError {
                    message: "Docstrings are not lowered to IR".to_string(),
                    span: IrSpan::default(),
                });
            }
        };
        Ok(IrDecl::new(kind).with_span(span.into()))
    }

    /// Materialize the canonical source identity for a frontend-approved explicit registry entry.
    ///
    /// `RegistrySubject.current_unit()` and `.package()` are ordinary typed source constructors. The placeholders in
    /// their source implementations must not escape into loaded runtime entries, however: the frontend has already
    /// fixed their subject kind and this lowering step supplies the compilation boundary's canonical identity. No
    /// runtime discovery or Rust-side registry implementation is involved.
    ///
    /// The entry call and its subject constructor are verified by the identities the checker recorded at their spans,
    /// not by the spelling lowering gave them: one checked method has three admissible spellings (see
    /// [`Self::checked_method_spelling`]), and which one a call carries depends on how the standard library is
    /// provided to this compilation. The checked constructor is then spelled the way the placeholder was, so it names
    /// a slot that exists under the same provision (#1713).
    fn rewrite_checked_registry_entry_subject(
        &self,
        entry_name: &str,
        value_span: ast::Span,
        value: &mut TypedExpr,
    ) -> Result<(), LoweringError> {
        let Some(entry) = self.type_info.as_ref().and_then(|info| {
            info.registry
                .explicit_entries
                .iter()
                .find(|entry| entry.entry_name == entry_name)
        }) else {
            return Ok(());
        };

        let qualified_name = match entry.subject_kind {
            incan_semantics_core::SemanticRegistrySubjectKind::CompilationUnit => self
                .current_source_module_name
                .clone()
                .unwrap_or_else(|| "<unknown-compilation-unit>".to_string()),
            incan_semantics_core::SemanticRegistrySubjectKind::Package => self
                .registry_package_identity
                .clone()
                .unwrap_or_else(|| "<unpackaged>".to_string()),
            incan_semantics_core::SemanticRegistrySubjectKind::Function
            | incan_semantics_core::SemanticRegistrySubjectKind::Method => {
                return Err(LoweringError {
                    message: "checked explicit registry entries only support compilation-unit and package subjects"
                        .to_string(),
                    span: IrSpan::default(),
                });
            }
        };
        let subject_span = ast::Span::new(entry.subject_span.0, entry.subject_span.1);
        let resolved_identity = |span: ast::Span| self.type_info.as_ref().and_then(|info| info.resolved_identity(span));
        if resolved_identity(value_span) != Some(&entry.entry_method_identity) {
            return Err(LoweringError {
                message: "checked registry entry lowering no longer matches its frontend-approved Registry.entry"
                    .to_string(),
                span: value_span.into(),
            });
        }
        if resolved_identity(subject_span) != Some(&entry.subject_constructor_identity) {
            return Err(LoweringError {
                message: "checked registry entry subject no longer matches its frontend-approved artifact".to_string(),
                span: subject_span.into(),
            });
        }

        let IrExprKind::MethodCall {
            receiver, method, args, ..
        } = &mut value.kind
        else {
            return Err(LoweringError {
                message: "checked registry entry lowering expected registry.entry(...)".to_string(),
                span: value_span.into(),
            });
        };
        if self
            .checked_method_spelling(method, &entry.entry_method_identity, value_span, &receiver.ty)
            .is_none()
        {
            return Err(LoweringError {
                message: "checked registry entry lowering no longer matches its frontend-approved Registry.entry"
                    .to_string(),
                span: value_span.into(),
            });
        }
        let Some(subject) = args.iter_mut().find(|arg| arg.name.as_deref() == Some("subject")) else {
            return Err(LoweringError {
                message: "checked registry entry lowering expected a named subject argument".to_string(),
                span: value_span.into(),
            });
        };
        let IrExprKind::MethodCall {
            receiver: subject_receiver,
            method: subject_method,
            args: subject_args,
            ..
        } = &mut subject.expr.kind
        else {
            return Err(LoweringError {
                message: "checked registry entry lowering expected a RegistrySubject constructor".to_string(),
                span: subject_span.into(),
            });
        };
        let Some(spelling) = self.checked_method_spelling(
            subject_method,
            &entry.subject_constructor_identity,
            subject_span,
            &subject_receiver.ty,
        ) else {
            return Err(LoweringError {
                message: "checked registry entry subject no longer matches its frontend-approved artifact".to_string(),
                span: subject_span.into(),
            });
        };
        let Some(checked_constructor) = self.spell_checked_method(
            spelling,
            &entry.checked_constructor_identity,
            subject_span,
            &subject_receiver.ty,
        ) else {
            return Err(LoweringError {
                message: "checked registry entry subject has no spelling for its checked constructor under this standard-library provision".to_string(),
                span: subject_span.into(),
            });
        };
        *subject_method = checked_constructor;
        *subject_args = vec![IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr: TypedExpr::new(IrExprKind::String(qualified_name), IrType::String),
        }];
        Ok(())
    }

    /// Classify the physical spelling lowering gave one checked source method, or `None` when it is none of them.
    ///
    /// `project_resolved_method_target` (`lower/expr`) can hand the same checked call one of three spellings: the
    /// declaration's own name when the projection is declined (a package-owned method reached from outside its
    /// build), the compiled provider's canonical projection when a provider plan serves the standard library
    /// (`incan run`), or the source-stdlib projection when the standard library is compiled alongside (the in-process
    /// codegen tests). A guard that pins one of them refuses the others; this names which one is in effect so the
    /// checked constructor can follow it.
    fn checked_method_spelling(
        &self,
        lowered: &str,
        identity: &incan_semantics_core::CanonicalSymbolId,
        call_span: ast::Span,
        receiver_ty: &IrType,
    ) -> Option<CheckedMethodSpelling> {
        [
            CheckedMethodSpelling::Source,
            CheckedMethodSpelling::Provider,
            CheckedMethodSpelling::Projected,
        ]
        .into_iter()
        .find(|spelling| {
            self.spell_checked_method(*spelling, identity, call_span, receiver_ty)
                .is_some_and(|candidate| candidate == lowered)
        })
    }

    /// Spell one checked source method the way `spelling` would, or `None` when that provision does not name it.
    ///
    /// The provider projection is looked up by declaration name in the provider's API, using the span of a call that
    /// resolves into the same source module; that is how the compiler-reserved `_checked_*` constructors, which no
    /// source call ever names, get the spelling of the placeholder call beside them.
    fn spell_checked_method(
        &self,
        spelling: CheckedMethodSpelling,
        identity: &incan_semantics_core::CanonicalSymbolId,
        call_span: ast::Span,
        receiver_ty: &IrType,
    ) -> Option<String> {
        match spelling {
            CheckedMethodSpelling::Source => Some(identity.declaration_name.clone()),
            CheckedMethodSpelling::Provider => {
                self.compiled_provider_method_reference_name(call_span, receiver_ty, &identity.declaration_name)
            }
            CheckedMethodSpelling::Projected => Some(Self::emitted_source_identity_name(identity, true)),
        }
    }

    /// Resolve the path that should be used when emitting a module-level alias declaration.
    ///
    /// A source alias can target a local import binding, but generated Rust public re-exports must point at the
    /// imported item path itself. Expression lowering still keeps the source binding for ordinary calls.
    fn alias_reexport_target(
        &self,
        segments: &[String],
    ) -> (Vec<String>, Option<IrImportOrigin>, Option<IrImportQualifier>) {
        if let [target] = segments
            && let Some(imported) = self.imported_alias_targets.get(target)
        {
            return (
                imported.path.clone(),
                Some(imported.origin.clone()),
                Some(imported.qualifier),
            );
        }
        (segments.to_vec(), None, None)
    }

    fn lower_interop_edges(
        &mut self,
        edges: &[ast::Spanned<ast::InteropEdgeDecl>],
    ) -> Result<Vec<IrInteropEdge>, LoweringError> {
        edges
            .iter()
            .map(|edge| {
                let direction = match edge.node.direction {
                    ast::InteropDirection::From => IrInteropDirection::From,
                    ast::InteropDirection::Into => IrInteropDirection::Into,
                };
                let adapter_kind = match edge.node.adapter_kind {
                    ast::InteropAdapterKind::Via => IrInteropAdapterKind::Via,
                    ast::InteropAdapterKind::Try => IrInteropAdapterKind::Try,
                };
                Ok(IrInteropEdge {
                    direction,
                    ty: self.lower_type(&edge.node.ty.node),
                    adapter_kind,
                    adapter: self.lower_expr_spanned(&edge.node.adapter)?,
                })
            })
            .collect()
    }

    /// RFC 023: Check if a decorator list contains `@rust.extern`.
    ///
    /// Used during lowering to mark functions whose body is provided by a Rust backing module. Uses `from_segments` on
    /// the full decorator path (e.g. `["rust", "extern"]`) since the `name` field only stores the last segment.
    pub(in crate::lower) fn has_rust_extern_decorator(decorators_list: &[ast::Spanned<ast::Decorator>]) -> bool {
        decorators_list
            .iter()
            .any(|d| decorators::from_segments(&d.node.path.segments) == Some(DecoratorId::RustExtern))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IrProgram;
    use incan_frontend::typechecker::{RegistryExplicitEntryInfo, TypeChecker};
    use incan_frontend::{lexer, parser};

    /// The explicit-entry program of #1713, with both subject kinds.
    const REGISTRY_SUBJECTS: &str = r#"
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind

@derive(Clone, Eq)
type CapabilityId = newtype str

@derive(Descriptor)
model CapabilitySpec:
    title: str

pub static capabilities: Registry[CapabilityId, CapabilitySpec] = Registry.define(
    subjects=[SubjectKind.CompilationUnit, SubjectKind.Package],
)

pub static logging_capability: RegistryEntry[CapabilityId, CapabilitySpec] = capabilities.entry(
    key=CapabilityId("std.logging"),
    subject=RegistrySubject.current_unit(),
    descriptor=CapabilitySpec(title="Structured logging"),
)

pub static package_capability: RegistryEntry[CapabilityId, CapabilitySpec] = capabilities.entry(
    key=CapabilityId("std.registry"),
    subject=RegistrySubject.package(),
    descriptor=CapabilitySpec(title="Typed declaration registries"),
)
"#;

    /// Check and lower the program as the compilation unit `main` of the package `probe`.
    fn lower_registry_subjects() -> Result<(ast::Program, AstLowering, IrProgram), String> {
        let tokens = lexer::lex(REGISTRY_SUBJECTS).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        lowering.set_current_source_module_name(Some("main".to_string()));
        lowering.set_registry_package_identity(Some("probe".to_string()));
        let ir_program = lowering
            .lower_program(&program)
            .map_err(|errors| format!("lowering failed: {errors:?}"))?;
        Ok((program, lowering, ir_program))
    }

    /// Return the source declaration of the named static.
    fn static_declaration<'a>(program: &'a ast::Program, name: &str) -> Result<&'a ast::StaticDecl, String> {
        program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                ast::Declaration::Static(static_decl) if static_decl.name == name => Some(static_decl),
                _ => None,
            })
            .ok_or_else(|| format!("missing static `{name}`"))
    }

    /// Return the checked explicit entry recorded for the named static.
    fn explicit_entry(lowering: &AstLowering, name: &str) -> Result<RegistryExplicitEntryInfo, String> {
        lowering
            .type_info
            .as_ref()
            .and_then(|info| {
                info.registry
                    .explicit_entries
                    .iter()
                    .find(|entry| entry.entry_name == name)
            })
            .cloned()
            .ok_or_else(|| format!("the checker recorded no explicit entry for `{name}`"))
    }

    /// Return the subject argument's constructor spelling and positional string arguments.
    fn subject_constructor(value: &TypedExpr) -> Result<(String, Vec<String>), String> {
        let IrExprKind::MethodCall { args, .. } = &value.kind else {
            return Err("the entry value is not a method call".to_string());
        };
        let subject = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("subject"))
            .ok_or("the entry call has no named subject argument")?;
        let IrExprKind::MethodCall { method, args, .. } = &subject.expr.kind else {
            return Err("the subject is not a constructor call".to_string());
        };
        let strings = args
            .iter()
            .map(|arg| match &arg.expr.kind {
                IrExprKind::String(text) => Ok(text.clone()),
                other => Err(format!("unexpected subject argument {other:?}")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((method.clone(), strings))
    }

    /// Rewrite one static's freshly lowered value after respelling its entry call and subject constructor the way
    /// a declined projection would, which is the spelling `incan run` hands the guard for a package-owned method.
    fn rewrite_with_spellings(
        lowering: &mut AstLowering,
        program: &ast::Program,
        name: &str,
        entry_spelling: &str,
        subject_spelling: &str,
    ) -> Result<Result<TypedExpr, LoweringError>, String> {
        let static_decl = static_declaration(program, name)?;
        let mut value = lowering
            .lower_expr_spanned(&static_decl.value)
            .map_err(|error| format!("lowering the entry value failed: {error:?}"))?;
        {
            let IrExprKind::MethodCall { method, args, .. } = &mut value.kind else {
                return Err("the entry value is not a method call".to_string());
            };
            *method = entry_spelling.to_string();
            let subject = args
                .iter_mut()
                .find(|arg| arg.name.as_deref() == Some("subject"))
                .ok_or("the entry call has no named subject argument")?;
            let IrExprKind::MethodCall { method, .. } = &mut subject.expr.kind else {
                return Err("the subject is not a constructor call".to_string());
            };
            *method = subject_spelling.to_string();
        }
        let outcome = lowering
            .rewrite_checked_registry_entry_subject(name, static_decl.value.span, &mut value)
            .map(|()| value);
        Ok(outcome)
    }

    /// With the standard library compiled alongside, the entry call carries the source-stdlib projection and the
    /// checked constructor is spelled the same way, carrying the compilation unit or package identity.
    #[test]
    fn explicit_registry_entry_follows_the_source_stdlib_projection_issue1713() -> Result<(), String> {
        let (_, lowering, ir_program) = lower_registry_subjects()?;
        for (name, expected_identity) in [("logging_capability", "main"), ("package_capability", "probe")] {
            let entry = explicit_entry(&lowering, name)?;
            let value = ir_program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.kind {
                    IrDeclKind::Static {
                        name: static_name,
                        value,
                        ..
                    } if static_name == name => Some(value),
                    _ => None,
                })
                .ok_or_else(|| format!("no lowered static `{name}`"))?;
            let projected_placeholder =
                AstLowering::emitted_source_identity_name(&entry.subject_constructor_identity, true);
            let projected_checked =
                AstLowering::emitted_source_identity_name(&entry.checked_constructor_identity, true);
            assert_ne!(projected_placeholder, projected_checked);
            assert_eq!(
                lowering.checked_method_spelling(
                    &projected_placeholder,
                    &entry.subject_constructor_identity,
                    ast::Span::new(entry.subject_span.0, entry.subject_span.1),
                    &IrType::Struct("RegistrySubject".to_string()),
                ),
                Some(CheckedMethodSpelling::Projected)
            );
            assert_eq!(
                subject_constructor(value)?,
                (projected_checked, vec![expected_identity.to_string()])
            );
        }
        Ok(())
    }

    /// A call whose projection was declined keeps its source spelling; the checked constructor follows it, so the
    /// substituted call names the slot that exists under that provision (#1713).
    #[test]
    fn explicit_registry_entry_follows_a_declined_projection_issue1713() -> Result<(), String> {
        let (program, mut lowering, _) = lower_registry_subjects()?;
        let value = rewrite_with_spellings(&mut lowering, &program, "logging_capability", "entry", "current_unit")?
            .map_err(|error| format!("the source spelling was refused: {error:?}"))?;
        assert_eq!(
            subject_constructor(&value)?,
            ("_checked_current_unit".to_string(), vec!["main".to_string()])
        );

        let value = rewrite_with_spellings(&mut lowering, &program, "package_capability", "entry", "package")?
            .map_err(|error| format!("the source spelling was refused: {error:?}"))?;
        assert_eq!(
            subject_constructor(&value)?,
            ("_checked_package".to_string(), vec!["probe".to_string()])
        );
        Ok(())
    }

    /// A spelling that is none of the three admissible ones is drift, and still refused with the guard's message.
    #[test]
    fn explicit_registry_entry_refuses_an_unknown_spelling_issue1713() -> Result<(), String> {
        let (program, mut lowering, _) = lower_registry_subjects()?;
        let refused = rewrite_with_spellings(
            &mut lowering,
            &program,
            "logging_capability",
            "entry",
            "current_unit_v2",
        )?;
        let Err(error) = refused else {
            return Err("an unknown subject spelling was accepted".to_string());
        };
        assert!(
            error
                .message
                .contains("subject no longer matches its frontend-approved artifact"),
            "unexpected message: {}",
            error.message
        );

        let refused = rewrite_with_spellings(
            &mut lowering,
            &program,
            "logging_capability",
            "entry_v2",
            "current_unit",
        )?;
        let Err(error) = refused else {
            return Err("an unknown entry spelling was accepted".to_string());
        };
        assert!(
            error
                .message
                .contains("no longer matches its frontend-approved Registry.entry"),
            "unexpected message: {}",
            error.message
        );
        Ok(())
    }
}
