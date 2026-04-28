//! Rust boundary matching, Rust call validation, and coercion metadata recording.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, Span};
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, TypeInfo};
use crate::frontend::typechecker::helpers::collection_type_id;
use crate::frontend::typechecker::{RustArgCoercionInfo, RustArgCoercionKind};
use incan_core::interop::{CoercionPolicy, RustFunctionSig, admitted_builtin_coercion};
use incan_core::lang::types::collections::CollectionTypeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustArgBoundaryMatch {
    /// Argument already matches the Rust parameter shape; no lowering-time adapter is required.
    Exact,
    /// Argument is admissible if lowering inserts a boundary coercion/adapter.
    Coercion(RustArgCoercionKind),
    /// Argument cannot satisfy the Rust parameter shape.
    NoMatch,
}

impl TypeChecker {
    fn rusttype_boundary_match(&self, arg_ty: &ResolvedType, target_ty: &ResolvedType) -> Option<RustArgCoercionKind> {
        if let ResolvedType::Named(type_name) = arg_ty
            && let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(type_name)
            && newtype.is_rusttype
        {
            if self.types_compatible(&newtype.underlying, target_ty) {
                return Some(RustArgCoercionKind::RustTypeUnwrap);
            }
            if newtype.has_interop {
                return Some(RustArgCoercionKind::RustTypeInterop);
            }
        }

        // RFC 041 (`from S ...`) edges convert non-rusttype arguments into a rusttype surface. Type checking marks
        // these as interop-capable when the target is a rusttype with declared interop, and lowering picks the concrete
        // adapter edge.
        if let ResolvedType::Named(type_name) = target_ty
            && let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(type_name)
            && newtype.is_rusttype
            && newtype.has_interop
        {
            return Some(RustArgCoercionKind::RustTypeInterop);
        }

        None
    }

    /// Render an Incan type into the canonical boundary vocabulary used by interop coercion policy lookup.
    ///
    /// Returns `None` for shapes not covered by the builtin boundary coercion matrix.
    fn incan_boundary_type_display(arg_ty: &ResolvedType) -> Option<String> {
        match arg_ty {
            ResolvedType::Int => Some("int".to_string()),
            ResolvedType::Float => Some("float".to_string()),
            ResolvedType::Bool => Some("bool".to_string()),
            ResolvedType::Str => Some("str".to_string()),
            ResolvedType::FrozenStr => Some("FrozenStr".to_string()),
            ResolvedType::Bytes => Some("bytes".to_string()),
            ResolvedType::FrozenBytes => Some("FrozenBytes".to_string()),
            ResolvedType::Unit => Some("unit".to_string()),
            ResolvedType::FrozenList(elem) => {
                let elem_display = Self::incan_boundary_type_display(elem)?;
                Some(format!("FrozenList[{elem_display}]"))
            }
            ResolvedType::FrozenDict(key, value) => {
                let key_display = Self::incan_boundary_type_display(key)?;
                let value_display = Self::incan_boundary_type_display(value)?;
                Some(format!("FrozenDict[{key_display}, {value_display}]"))
            }
            ResolvedType::FrozenSet(elem) => {
                let elem_display = Self::incan_boundary_type_display(elem)?;
                Some(format!("FrozenSet[{elem_display}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Option) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("Option[{inner}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && args.len() == 2 =>
            {
                let ok = Self::incan_boundary_type_display(&args[0])?;
                let err = Self::incan_boundary_type_display(&args[1])?;
                Some(format!("Result[{ok}, {err}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("List[{inner}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                let key = Self::incan_boundary_type_display(&args[0])?;
                let value = Self::incan_boundary_type_display(&args[1])?;
                Some(format!("Dict[{key}, {value}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Set) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("Set[{inner}]"))
            }
            ResolvedType::Tuple(elems) => {
                let mut rendered = Vec::with_capacity(elems.len());
                for elem in elems {
                    rendered.push(Self::incan_boundary_type_display(elem)?);
                }
                Some(format!("Tuple[{}]", rendered.join(", ")))
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => Self::incan_boundary_type_display(inner),
            _ => None,
        }
    }

    /// Whether a Rust type display string belongs to the builtin boundary coercion matrix.
    ///
    /// This intentionally includes fully-qualified std/core/alloc spellings that rust-analyzer may emit.
    fn is_builtin_rust_boundary_display(rust_ty: &str) -> bool {
        matches!(
            rust_ty,
            "bool"
                | "f32"
                | "f64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "String"
                | "std::string::String"
                | "&str"
                | "Vec<u8>"
                | "std::vec::Vec<u8>"
                | "&[u8]"
                | "()"
        ) || rust_ty.starts_with("Option<")
            || rust_ty.starts_with("std::option::Option<")
            || rust_ty.starts_with("core::option::Option<")
            || rust_ty.starts_with("Result<")
            || rust_ty.starts_with("std::result::Result<")
            || rust_ty.starts_with("core::result::Result<")
            || rust_ty.starts_with("Vec<")
            || rust_ty.starts_with("std::vec::Vec<")
            || rust_ty.starts_with("alloc::vec::Vec<")
            || rust_ty.starts_with("HashMap<")
            || rust_ty.starts_with("std::collections::HashMap<")
            || rust_ty.starts_with("std::collections::hash_map::HashMap<")
            || rust_ty.starts_with("HashSet<")
            || rust_ty.starts_with("std::collections::HashSet<")
            || rust_ty.starts_with("std::collections::hash_set::HashSet<")
            || (rust_ty.starts_with('(') && rust_ty.ends_with(')'))
    }

    /// Classify whether an Incan argument type can satisfy a Rust parameter boundary.
    ///
    /// This first tries builtin coercion-matrix matches, then resolved-type compatibility, then rusttype-specific
    /// boundary adapters.
    fn rust_arg_boundary_match(&self, arg_ty: &ResolvedType, rust_param_ty: &str) -> RustArgBoundaryMatch {
        let normalized = rust_param_ty.replace(' ', "");
        if let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
            && Self::is_builtin_rust_boundary_display(normalized.as_str())
        {
            return match admitted_builtin_coercion(incan_display.as_str(), normalized.as_str()) {
                Some(CoercionPolicy::Exact) => RustArgBoundaryMatch::Exact,
                Some(policy) => RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy)),
                None => RustArgBoundaryMatch::NoMatch,
            };
        }
        let target_ty = self.resolved_type_from_rust_display(normalized.as_str());
        if self.types_compatible(arg_ty, &target_ty) {
            return RustArgBoundaryMatch::Exact;
        }
        if let Some(kind) = self.rusttype_boundary_match(arg_ty, &target_ty) {
            return RustArgBoundaryMatch::Coercion(kind);
        }
        if let Some(incan_name) = Self::incan_boundary_type_display(arg_ty)
            && let Some(policy) = admitted_builtin_coercion(incan_name.as_str(), normalized.as_str())
        {
            return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy));
        }
        RustArgBoundaryMatch::NoMatch
    }

    fn rust_lookup_probe_boundary_match(&self, arg_ty: &ResolvedType, target_ty: &ResolvedType) -> bool {
        match (arg_ty, target_ty) {
            (ResolvedType::Str | ResolvedType::FrozenStr, ResolvedType::Ref(inner)) => {
                matches!(inner.as_ref(), ResolvedType::Str)
            }
            (ResolvedType::Bytes | ResolvedType::FrozenBytes, ResolvedType::Ref(inner)) => {
                matches!(inner.as_ref(), ResolvedType::Bytes)
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub(in crate::frontend::typechecker) fn rust_arg_matches_boundary(
        &self,
        arg_ty: &ResolvedType,
        rust_param_ty: &str,
    ) -> bool {
        !matches!(
            self.rust_arg_boundary_match(arg_ty, rust_param_ty),
            RustArgBoundaryMatch::NoMatch
        )
    }

    /// Validate a Rust method call (`receiver.method(...)`) against metadata and record required arg coercions.
    ///
    /// The receiver is already validated by access resolution; this function validates only post-receiver parameters.
    pub(in crate::frontend::typechecker) fn validate_rust_method_call(
        &mut self,
        callable_display: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
    ) -> ResolvedType {
        let params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };

        if arg_types.len() != params.len() {
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                arg_types.len(),
                span,
            ));
            return self.resolved_type_from_rust_display(sig.return_type.as_str());
        }

        for ((arg, arg_ty), param) in args.iter().zip(arg_types.iter()).zip(params.iter()) {
            let arg_expr = Self::call_arg_expr(arg);
            let normalized = param.type_display.replace(' ', "");
            let target_ty = self.resolved_type_from_rust_display(normalized.as_str());
            if preserves_lookup_arg_shape && self.rust_lookup_probe_boundary_match(arg_ty, &target_ty) {
                continue;
            }
            match self.rust_arg_boundary_match(arg_ty, param.type_display.as_str()) {
                RustArgBoundaryMatch::Exact => {}
                RustArgBoundaryMatch::Coercion(kind) => {
                    self.type_info.rust_arg_coercions.insert(
                        (arg_expr.span.start, arg_expr.span.end),
                        RustArgCoercionInfo {
                            rust_target_type: normalized,
                            target_type: target_ty,
                            kind,
                        },
                    );
                }
                RustArgBoundaryMatch::NoMatch => {
                    self.errors.push(errors::type_mismatch(
                        param.type_display.as_str(),
                        &arg_ty.to_string(),
                        arg_expr.span,
                    ));
                }
            }
        }

        self.resolved_type_from_rust_display(sig.return_type.as_str())
    }

    /// Validate a direct Rust function call (`rust::path::item(...)`) and record boundary coercions.
    pub(in crate::frontend::typechecker::check_expr::calls) fn validate_rust_function_call(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        let arg_types = self.check_call_arg_types(args);
        if arg_types.len() != sig.params.len() {
            self.errors
                .push(errors::builtin_arity(path, sig.params.len(), arg_types.len(), span));
            return self.resolved_type_from_rust_display(sig.return_type.as_str());
        }

        for ((arg, arg_ty), param) in args.iter().zip(arg_types.iter()).zip(sig.params.iter()) {
            let arg_expr = Self::call_arg_expr(arg);
            let normalized = param.type_display.replace(' ', "");
            let target_ty = self.resolved_type_from_rust_display(normalized.as_str());
            match self.rust_arg_boundary_match(arg_ty, param.type_display.as_str()) {
                RustArgBoundaryMatch::Exact => {}
                RustArgBoundaryMatch::Coercion(kind) => {
                    self.type_info.rust_arg_coercions.insert(
                        (arg_expr.span.start, arg_expr.span.end),
                        RustArgCoercionInfo {
                            rust_target_type: normalized,
                            target_type: target_ty,
                            kind,
                        },
                    );
                }
                RustArgBoundaryMatch::NoMatch => {
                    self.errors.push(errors::type_mismatch(
                        param.type_display.as_str(),
                        &arg_ty.to_string(),
                        arg_expr.span,
                    ));
                }
            }
        }

        self.resolved_type_from_rust_display(sig.return_type.as_str())
    }
}

#[cfg(test)]
mod validate_rust_function_call_tests {
    use super::TypeChecker;
    use crate::frontend::ast::{CallArg, Expr, IntLiteral, Literal, Span, Spanned};
    use crate::frontend::symbols::{NewtypeInfo, ResolvedType, Symbol, SymbolKind, TypeInfo, VariableInfo};
    use incan_core::interop::{RustFunctionSig, RustParam};
    use std::collections::HashMap;

    #[test]
    fn zero_parameter_rust_sig_rejects_extra_arguments() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            params: Vec::new(),
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::nop", &sig, &args, span);

        assert!(
            checker.errors.iter().any(|e| {
                e.message.contains("rust::crate::nop()")
                    && e.message.contains("expects 0 argument")
                    && e.message.contains("got 1")
            }),
            "expected builtin_arity for 0-param Rust call with 1 arg, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn zero_parameter_rust_sig_allows_no_arguments() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            params: Vec::new(),
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::nop", &sig, &[], span);

        assert!(
            checker.errors.is_empty(),
            "expected no errors for arity match, got {:?}",
            checker.errors
        );
    }

    #[test]
    fn too_few_arguments_reports_arity_before_param_zip() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            params: vec![
                RustParam {
                    name: Some("a".to_string()),
                    type_display: "i64".to_string(),
                },
                RustParam {
                    name: Some("b".to_string()),
                    type_display: "i64".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::f", &sig, &args, span);

        assert!(
            checker
                .errors
                .iter()
                .any(|e| e.message.contains("expects 2 argument") && e.message.contains("got 1")),
            "expected arity error when call has fewer args than Rust params, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_arg_boundary_accepts_structural_list_to_vec() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);
        assert!(checker.rust_arg_matches_boundary(&arg_ty, "Vec<String>"));
    }

    #[test]
    fn rust_arg_boundary_accepts_structural_option_int_to_option_i64() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Generic("Option".to_string(), vec![ResolvedType::Int]);
        assert!(checker.rust_arg_matches_boundary(&arg_ty, "Option<i64>"));
    }

    #[test]
    fn rust_arg_boundary_accepts_structural_frozen_dict_to_hash_map() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::FrozenDict(Box::new(ResolvedType::Str), Box::new(ResolvedType::Float));
        assert!(checker.rust_arg_matches_boundary(&arg_ty, "std::collections::HashMap<&str, f32>"));
    }

    #[test]
    fn rust_arg_boundary_rejects_structural_list_str_to_vec_i64() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);
        assert!(!checker.rust_arg_matches_boundary(&arg_ty, "Vec<i64>"));
    }

    #[test]
    fn rust_method_call_rejects_missing_required_arguments_after_receiver() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("pattern".to_string()),
                    type_display: "&str".to_string(),
                },
            ],
            return_type: "bool".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call("rust::regex::Regex.is_match", &sig, &[], &[], false, span);

        assert!(
            checker.errors.iter().any(
                |e| e.message.contains("rust::regex::Regex.is_match()") && e.message.contains("expects 1 argument")
            ),
            "expected arity diagnostic for missing method arg, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_method_call_rejects_type_mismatch_after_receiver() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("pattern".to_string()),
                    type_display: "&str".to_string(),
                },
            ],
            return_type: "bool".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(123))), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Int];

        let _ = checker.validate_rust_method_call("rust::regex::Regex.is_match", &sig, &args, &arg_types, false, span);

        assert!(
            checker
                .errors
                .iter()
                .any(|e| e.message.contains("&str") && e.message.contains("int")),
            "expected type mismatch diagnostic for method arg, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_lookup_preserving_method_accepts_string_probe_for_ref_string_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("key".to_string()),
                    type_display: "&String".to_string(),
                },
            ],
            return_type: "Option<&i64>".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("the".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Str];

        let _ = checker.validate_rust_method_call(
            "rust::std::collections::HashMap.get",
            &sig,
            &args,
            &arg_types,
            true,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected lookup-preserving rust method call to stay permissive for string probes, errors={:?}",
            checker.errors
        );
        assert!(
            checker.type_info.rust_arg_coercions.is_empty(),
            "expected lookup-preserving rust method call to preserve arg shape without coercion, got {:?}",
            checker.type_info.rust_arg_coercions
        );
    }

    #[test]
    fn rust_arg_boundary_accepts_rusttype_from_interop_target() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        checker.symbols.define(Symbol {
            name: "Email".to_string(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: Vec::new(),
                is_rusttype: true,
                has_interop: true,
                underlying: ResolvedType::Named("RustString".to_string()),
                method_rebindings: HashMap::new(),
                methods: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        assert!(
            checker.rust_arg_matches_boundary(&ResolvedType::Str, "Email"),
            "expected `str` to be admitted for rusttype target boundary via `from` interop edge hint"
        );
    }

    #[test]
    fn rust_function_call_accepts_string_for_borrowed_string_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&String".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_string", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected borrowed String boundary to admit Incan str, errors={:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .rust_arg_coercions
                .contains_key(&(span.start, span.end)),
            "expected rust arg coercion metadata for borrowed String boundary"
        );
    }

    #[test]
    fn rust_function_call_accepts_concrete_borrowed_rust_path_param_without_ref_unknown_diagnostic() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "plan".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::RustPath("demo::Plan".to_string()),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Ident("plan".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&demo::Plan".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::consume_plan", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected concrete borrowed Rust path boundary to typecheck, errors={:?}",
            checker.errors
        );
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_function_call_matches_reexported_borrowed_param_via_definition_path()
    -> Result<(), Box<dyn std::error::Error>> {
        use incan_core::interop::{RustItemKind, RustItemMetadata, RustTypeInfo, RustVisibility};
        let mut checker = TypeChecker::new();
        let tmp = tempfile::tempdir()?;
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"incan_test_rust_identity\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        let manifest_dir = tmp.path().to_path_buf();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker.rust_inspect_cache.insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "datafusion_substrait::substrait::proto::Plan".to_string(),
                definition_path: Some("substrait::proto::Plan".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    methods: vec![],
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )?;
        let cached = checker
            .rust_item_metadata_for_path("rust::datafusion_substrait::substrait::proto::Plan")
            .ok_or_else(|| std::io::Error::other("expected cached metadata for rust::datafusion_substrait::..."))?;
        assert_eq!(
            cached.definition_path.as_deref(),
            Some("substrait::proto::Plan"),
            "expected cached metadata definition path to resolve through re-export"
        );
        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "plan".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::RustPath("rust::datafusion_substrait::substrait::proto::Plan".to_string()),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        let arg_expr = Spanned::new(Expr::Ident("plan".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&substrait::proto::Plan".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let _ = checker.validate_rust_function_call("rust::consume_plan", &sig, &args, span);
        assert!(
            checker.errors.is_empty(),
            "expected re-exported borrowed Rust path boundary to typecheck, errors={:?}",
            checker.errors
        );
        Ok(())
    }

    #[test]
    fn validate_rust_function_call_records_interop_coercion_for_rusttype_target() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "Email".to_string(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: Vec::new(),
                is_rusttype: true,
                has_interop: true,
                underlying: ResolvedType::Named("RustString".to_string()),
                method_rebindings: HashMap::new(),
                methods: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Literal(Literal::String("alice@example.com".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "Email".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::takes_email", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected rusttype interop boundary to avoid type mismatch, errors={:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .rust_arg_coercions
                .contains_key(&(span.start, span.end)),
            "expected rust arg coercion metadata for rusttype target boundary"
        );
    }

    #[test]
    fn validate_rust_method_call_records_interop_coercion_for_rusttype_target() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        checker.symbols.define(Symbol {
            name: "Email".to_string(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: Vec::new(),
                is_rusttype: true,
                has_interop: true,
                underlying: ResolvedType::Named("RustString".to_string()),
                method_rebindings: HashMap::new(),
                methods: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Literal(Literal::String("alice@example.com".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Str];
        let sig = RustFunctionSig {
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("value".to_string()),
                    type_display: "Email".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call(
            "rust::demo::EmailService.set_email",
            &sig,
            &args,
            &arg_types,
            false,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected rusttype interop boundary to avoid type mismatch, errors={:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .rust_arg_coercions
                .contains_key(&(span.start, span.end)),
            "expected rust arg coercion metadata for rust method boundary"
        );
    }
}
