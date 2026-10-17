//! Const-checker values and selected required SDK slots survive the private Body IR boundary.

use std::collections::BTreeSet;

use super::{bir, body_named, build, build_body_ir_module_v0};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, build_surface};

const SOURCE: &str = "pub type Wide = newtype u128:\n    def bits(self) -> u128:\n        return self.0\n\npub const MAXIMUM: Wide = Wide(340282366920938463463374607431768211455)\n\npub def maximum_bits() -> u128:\n    return MAXIMUM.bits()\n";

/// Collect a real checked declaration under a package identity, without fabricating a constant or layout.
fn checked_constant_module() -> Result<(bir::BodyIrModule, CanonicalSymbolId), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_current_package_identity(Some("wide_constants".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let identity = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "MAXIMUM")
        .cloned()
        .ok_or("checked MAXIMUM identity missing")?;
    Ok((
        build_body_ir_module_v0(&program, &module_path, checker.type_info()),
        identity,
    ))
}

/// An accepted full-width constructor constant must survive under its exact declaration identity.
#[test]
fn checked_full_width_constructor_constant_is_retained() -> Result<(), Box<dyn std::error::Error>> {
    let (module, identity) = checked_constant_module()?;
    let declaration = module
        .constant_declarations
        .iter()
        .find(|declaration| declaration.canonical == identity)
        .ok_or("checked constructor constant was omitted")?;
    assert_eq!(declaration.canonical, identity);
    let bir::Constant::NumericNewtype(value) = &declaration.value else {
        return Err("constant lost its checked newtype value".into());
    };
    assert!(matches!(
        value.value,
        bir::TypedNumericConstant::Unsigned { value: u128::MAX, .. }
    ));
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Wide")
        .ok_or("owner missing")?;
    assert!(value.matches_layout(owner));
    assert_eq!(value.constructor.canonical.as_ref(), Some(&owner.canonical));
    assert_eq!(
        value.constructor.direct_declaration_id.as_ref(),
        Some(&owner.direct_declaration_id)
    );
    let bytes = incan_semantics_core::executable_representation::encode_module(&module)?;
    assert_eq!(
        incan_semantics_core::executable_representation::decode_module(&bytes)?,
        module
    );
    Ok(())
}

/// Publication must retain the owner requirement and refuse a value whose declaring layout is absent.
#[test]
fn constructor_constant_publication_requires_its_nominal_layout() -> Result<(), Box<dyn std::error::Error>> {
    let (mut module, identity) = checked_constant_module()?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == "Wide")
        .map(|declaration| declaration.canonical.clone())
        .ok_or("checked Wide layout missing")?;
    let public = BTreeSet::from([identity.clone(), owner.clone()]);
    let bytes = build_surface(&[module.clone()], "wide_constants", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&identity)? else {
        return Err("checked constructor constant was not published".into());
    };
    assert!(requirements.contains(&owner));
    assert_eq!(reader.fragment(&identity)?.identity(), Some(&identity));
    module.nominal_declarations.clear();
    let missing = build_surface(&[module], "wide_constants", "0.1.0", &public, &BTreeSet::new())?;
    assert!(!SurfaceReader::open(&missing)?.covers(&identity));
    Ok(())
}

/// A nongeneric required SDK slot retains the selected native declaration with its exact Self argument.
#[test]
fn required_reader_slot_retains_checked_native_signature() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.io import BinaryReader, IoError\n\ndef probe[R with BinaryReader](reader: R, size: int) -> Result[bytes, IoError]:\n    return reader.read_bytes(size)\n",
        &["required_reader"],
    )?;
    let body = body_named(&module, "probe")?;
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "read_bytes" => Some(target),
            _ => None,
        })
        .ok_or("checked required slot missing")?;
    let declaration = target
        .native_trait_delegation
        .as_ref()
        .ok_or("required slot native declaration omitted")?;
    assert_eq!(
        declaration.canonical,
        target.canonical.clone().ok_or("slot canonical absent")?
    );
    assert_eq!(
        declaration.rust_path().as_deref(),
        Some("incan_stdlib_system::io::BinaryReader::read_bytes")
    );
    assert_eq!(declaration.type_parameters, ["Self"]);
    assert_eq!(
        declaration.native_type_arguments,
        [incan_semantics_core::IncanType::TypeVar("Self".into())]
    );
    assert!(target.has_valid_native_trait_selection());
    Ok(())
}

/// Independently corrupting an actual value's owner, address, slot or width must never grant materialization.
#[test]
fn numeric_newtype_value_requires_exact_owner_address_slot_and_width() -> Result<(), Box<dyn std::error::Error>> {
    let (module, identity) = checked_constant_module()?;
    let declaration = module
        .constant_declarations
        .iter()
        .find(|value| value.canonical == identity)
        .ok_or("constant missing")?;
    let bir::Constant::NumericNewtype(value) = &declaration.value else {
        return Err("newtype value missing".into());
    };
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Wide")
        .ok_or("owner missing")?;
    assert!(value.matches_layout(owner));
    let mut wrong = value.clone();
    wrong.constructor.canonical = None;
    assert!(!wrong.matches_layout(owner));
    let mut wrong = value.clone();
    wrong.constructor.direct_declaration_id = None;
    assert!(!wrong.matches_layout(owner));
    let mut wrong = value.clone();
    wrong.constructor.binding = bir::ArgumentBinding::resolved_positional(0);
    assert!(!wrong.matches_layout(owner));
    let mut wrong = value.clone();
    wrong.constructor.canonical_field_layout = Some(vec!["1".into()]);
    assert!(!wrong.matches_layout(owner));
    let mut wrong = value.clone();
    wrong.value = bir::TypedNumericConstant::Unsigned {
        kind: incan_lang::lang::types::numerics::NumericTypeId::U8,
        value: 256,
    };
    assert!(!wrong.matches_layout(owner));
    let mut wrong_owner = owner.clone();
    wrong_owner.type_parameters.push("T".into());
    assert!(!value.matches_layout(&wrong_owner));
    Ok(())
}

/// The declaring boundary retains both ends of each tested unsigned width without narrowing their values.
#[test]
fn numeric_newtype_constants_preserve_unsigned_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    for (width, maximum) in [
        ("u8", "255"),
        ("u64", "18446744073709551615"),
        ("u128", "340282366920938463463374607431768211455"),
    ] {
        let source = format!(
            "pub type Bits = newtype {width}\n\npub const LOW: Bits = Bits(0)\npub const HIGH: Bits = Bits({maximum})\n"
        );
        let module = build(&source, &["constant_boundaries"])?;
        assert_eq!(module.constant_declarations.len(), 2, "{width}");
        for declaration in &module.constant_declarations {
            let bir::Constant::NumericNewtype(value) = &declaration.value else {
                return Err("boundary value lost".into());
            };
            let bir::TypedNumericConstant::Unsigned { value, .. } = value.value else {
                return Err("boundary kind lost".into());
            };
            let expected = if declaration.canonical.declaration_name == "LOW" {
                0
            } else {
                maximum.parse::<u128>()?
            };
            assert_eq!(value, expected);
        }
    }
    Ok(())
}

/// A foreign package or changed physical slot cannot acquire the native signature of an actual checked reader call.
#[test]
fn required_reader_slot_rejects_changed_canonical_selection() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.io import BinaryReader, IoError\n\ndef probe[R with BinaryReader](reader: R, size: int) -> Result[bytes, IoError]:\n    return reader.read_bytes(size)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["required_reader".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &["required_reader".into()], checker.type_info());
    let body = body_named(&module, "probe")?;
    let (target, span) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "read_bytes" => Some((target, statement.span)),
            _ => None,
        })
        .ok_or("checked required slot missing")?;
    let span = crate::ast::Span {
        start: span.start,
        end: span.end,
    };
    assert!(crate::body_ir::library_calls::selected_native_trait(checker.type_info(), span, target).is_some());
    let mut wrong_span = target.clone();
    wrong_span
        .canonical
        .as_mut()
        .ok_or("canonical missing")?
        .declaration_span
        .start += 1;
    assert!(crate::body_ir::library_calls::selected_native_trait(checker.type_info(), span, &wrong_span).is_none());
    let mut foreign = target.clone();
    foreign.canonical.as_mut().ok_or("canonical missing")?.origin = incan_semantics_core::SymbolOrigin::Package {
        library: "foreign_system".into(),
        module_path: vec!["io".into()],
    };
    assert!(crate::body_ir::library_calls::selected_native_trait(checker.type_info(), span, &foreign).is_none());
    Ok(())
}

/// The actual UUID publication context retains every numeric constant, including its maximum-width payload.
#[test]
fn standard_uuid_source_retains_its_checked_constants() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/data/src/uuid.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["uuid".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("incan_stdlib_data".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    for name in [
        "NIL",
        "MAX",
        "NAMESPACE_DNS",
        "NAMESPACE_URL",
        "NAMESPACE_OID",
        "NAMESPACE_X500",
    ] {
        assert!(
            module
                .constant_declarations
                .iter()
                .any(|value| value.canonical.declaration_name == name),
            "actual UUID constant absent: {name}"
        );
    }
    Ok(())
}

/// The full standard-source reader producer retains the required native slot used by its own adapter body.
#[test]
fn standard_reader_source_retains_its_required_slot() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/system/src/io.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["io".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("incan_stdlib_system".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let targets = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "read_bytes" => Some(target),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!targets.is_empty(), "actual adapter call absent");
    assert!(
        targets.iter().any(|target| target.native_trait_delegation.is_some()),
        "standard required-slot signature absent"
    );
    Ok(())
}

/// The complete standard UUID producer publishes methods as public API, so facade executable membership retains them.
#[test]
fn public_newtype_methods_belong_to_its_executable_surface() -> Result<(), Box<dyn std::error::Error>> {
    let (manifest, methods, _) = selected_uuid_method_surface()?;
    let public = crate::library_manifest::published_layout::public_executable_identities(&manifest);
    for method in methods {
        assert!(public.contains(&method), "published newtype method omitted: {method:?}");
    }
    Ok(())
}

/// API metadata cannot expose newtype members when the complete owning identity is absent or belongs to another
/// package.
#[test]
fn newtype_facade_requires_its_exact_exported_owner() -> Result<(), Box<dyn std::error::Error>> {
    let (mut manifest, methods, owner) = selected_uuid_method_surface()?;
    assert!(
        manifest.exports.newtypes.is_empty(),
        "fixture must exercise API facade membership"
    );
    for corruption in 0..4 {
        let mut corrupted = manifest.clone();
        for export in &mut corrupted.contract_metadata.identity_graph.exports {
            let Some(identity) = &mut export.canonical else {
                continue;
            };
            if identity.hydrate().as_ref() != Some(&owner) {
                continue;
            }
            match corruption {
                0 => identity.kind = "class".into(),
                1 => identity.declaration_span.start += 1,
                2 => {
                    if let crate::library_manifest::CanonicalIdentityOriginExport::Package { library, .. } =
                        &mut identity.origin
                    {
                        *library = "foreign_data".into();
                    }
                }
                _ => {
                    identity.origin = crate::library_manifest::CanonicalIdentityOriginExport::Package {
                        library: "incan_stdlib_data".into(),
                        module_path: vec!["foreign_module".into()],
                    }
                }
            }
        }
        let public = crate::library_manifest::published_layout::public_executable_identities(&corrupted);
        assert!(
            methods.iter().all(|method| !public.contains(method)),
            "corruption {corruption}"
        );
    }
    manifest.contract_metadata.identity_graph.exports.retain(|export| {
        export
            .canonical
            .as_ref()
            .and_then(|identity| identity.hydrate())
            .as_ref()
            != Some(&owner)
    });
    let public = crate::library_manifest::published_layout::public_executable_identities(&manifest);
    assert!(methods.iter().all(|method| !public.contains(method)));
    Ok(())
}

/// Build the complete unchanged standard source's public manifest, retaining its facade API independently of direct
/// export records so these tests do not depend on a machine-local SDK.
fn selected_uuid_method_surface() -> Result<
    (
        crate::library_manifest::LibraryManifest,
        Vec<CanonicalSymbolId>,
        CanonicalSymbolId,
    ),
    Box<dyn std::error::Error>,
> {
    let source = include_str!("../../../../../stdlib/data/src/uuid.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["uuid".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("incan_stdlib_data".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
    let mut manifest =
        crate::library_manifest::LibraryManifest::from_checked_exports("incan_stdlib_data", "0.6", &exports);
    manifest.contract_metadata.api = Some(crate::api_metadata::CheckedApiMetadataPackage {
        schema_version: crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![crate::api_metadata::collect_checked_api_metadata(
            &program, &checker, path,
        )],
        public_namespaces: Vec::new(),
    });
    manifest.exports.newtypes.clear();
    let api = manifest.contract_metadata.api.as_ref().ok_or("API absent")?;
    let module = api
        .modules
        .iter()
        .find(|module| module.module_path == ["uuid"])
        .ok_or("UUID API module absent")?;
    let value = module
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            crate::api_metadata::ApiDeclaration::Newtype(value) if value.name == "UUID" => Some(value),
            _ => None,
        })
        .ok_or("public UUID API absent")?;
    let methods = value
        .methods
        .iter()
        .map(|method| {
            method
                .canonical
                .as_ref()
                .and_then(|identity| identity.hydrate())
                .ok_or("checked method identity absent")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert!(!methods.is_empty());
    let owner = manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter_map(|export| export.canonical.as_ref()?.hydrate())
        .find(|identity| {
            identity.declaration_name == "UUID"
                && identity.kind == incan_semantics_core::SemanticSourceTargetKind::Newtype
        })
        .ok_or("exported UUID owner absent")?;
    Ok((manifest, methods, owner))
}

/// Declaring-module generic trait calls retain their existing source-body path; required nongeneric reader slots are
/// the separate native-signature extension.
#[test]
fn standard_generic_trait_calls_keep_their_existing_selection() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/core/src/derives/collection.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["derives".to_string(), "collection".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("incan_stdlib_core".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let mut selected = 0;
    for body in &module.bodies {
        for statement in &body.block.stmts {
            let bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } = &statement.kind
            else {
                continue;
            };
            let span = crate::ast::Span {
                start: statement.span.start,
                end: statement.span.end,
            };
            let Some(dispatch) = checker.type_info().resolved_method_call(span) else {
                continue;
            };
            let crate::typechecker::ResolvedMethodDispatch::Trait {
                module_path: Some(path),
                type_args,
                ..
            } = &dispatch.dispatch
            else {
                continue;
            };
            if path.first().is_some_and(|part| part == "std") || type_args.is_empty() {
                continue;
            }
            selected += 1;
            assert!(
                target.native_trait_delegation.is_none(),
                "generic source selection changed in {}: {target:?}",
                body.name
            );
        }
    }
    assert!(selected > 0, "actual standard generic source calls absent");
    Ok(())
}

/// Collect the original diverging runtime error call without supplying fabricated Rust metadata or changing legacy.
fn checked_error_helper_source(source: &str) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("error_helper_probe".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// A checked canonical core helper must publish its existing ValueError operation instead of an unknown result local.
#[test]
fn canonical_diverging_error_helper_is_portable() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_error_helper_source(
        "from rust::incan_std_core::errors import raise_value_error\n\npub def fail(message: str) -> bytes:\n    return raise_value_error(message)\n",
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "fail")
        .ok_or("fail absent")?;
    assert!(body.block.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Call {callee: bir::Callee::Helper(helper), destination: None, ..} if helper.as_str() == "raise_value_error"
    )), "canonical diverging operation absent: {body:#?}");
    assert!(
        body.locals
            .iter()
            .all(|local| local.ty != incan_semantics_core::IncanType::Unknown)
    );
    let identity = body.canonical.clone().ok_or("root identity absent")?;
    let bytes = build_surface(
        &[module],
        "error_helper_probe",
        "0.1",
        &BTreeSet::from([identity.clone()]),
        &BTreeSet::new(),
    )?;
    assert!(SurfaceReader::open(&bytes)?.covers(&identity));
    Ok(())
}

/// An equal-spelled foreign Rust function cannot acquire the compiler-owned runtime error operation.
#[test]
fn foreign_diverging_helper_spelling_grants_no_runtime_operation() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_error_helper_source(
        "from rust::foreign_core::errors import raise_value_error\n\npub def fail(message: str) -> bytes:\n    return raise_value_error(message)\n",
    )?;
    assert!(module.bodies.iter().flat_map(|body| &body.block.stmts).all(|statement| !matches!(&statement.kind,
        bir::StatementKind::Call {callee: bir::Callee::Helper(helper), ..} if helper.as_str() == "raise_value_error"
    )));
    Ok(())
}

/// Real Rust metadata checks a lazy import without recording a source identity. The private Body IR projection must
/// retain the validated foreign path, including aliases, while leaving legacy reference facts unchanged.
#[cfg(feature = "rust_inspect")]
#[test]
fn metadata_checked_diverging_helper_retains_its_exact_call_path() -> Result<(), Box<dyn std::error::Error>> {
    use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};
    for (crate_name, portable) in [("incan_std_core", true), ("foreign_core", false)] {
        let source = format!(
            "from rust::{crate_name}::errors import raise_value_error as fail_core\n\npub def fail(message: str) -> bytes:\n    return fail_core(message)\n"
        );
        let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let directory = tempfile::tempdir()?;
        std::fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = \"error_metadata_probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let mut checker = TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(directory.path().to_path_buf());
        let path = format!("{crate_name}::errors::raise_value_error");
        checker.rust_inspect_cache.insert_test_item(
            directory.path(),
            RustItemMetadata {
                canonical_path: path.clone(),
                definition_path: Some(path),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![RustParam {
                        name: Some("message".into()),
                        type_display: "&str".into(),
                    }],
                    return_type: "!".into(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )?;
        checker.set_current_module_path(Some(vec!["lib".into()]));
        checker.set_current_package_identity(Some("error_metadata_probe".into()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let legacy = checker.type_info().references.resolved_identities.clone();
        let module = build_body_ir_module_v0(&program, &["lib".into()], checker.type_info());
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "fail")
            .ok_or("fail absent")?;
        let retained = body.block.stmts.iter().any(|statement| {
            matches!(
                &statement.kind,
                bir::StatementKind::Call {
                    callee: bir::Callee::Helper(bir::HelperOp::RaiseValueError),
                    destination: None,
                    ..
                }
            )
        });
        assert_eq!(retained, portable, "wrong metadata-selected helper: {body:#?}");
        assert_eq!(checker.type_info().references.resolved_identities, legacy);
        if portable {
            for corrupted in [false, true] {
                let mut facts = checker.type_info().clone();
                if corrupted {
                    for path in facts.rust.body_ir_diverging_import_calls.values_mut() {
                        path[0] = "foreign_core".into();
                    }
                } else {
                    facts.rust.body_ir_diverging_import_calls.clear();
                }
                let missing = build_body_ir_module_v0(&program, &["lib".into()], &facts);
                assert!(
                    missing
                        .bodies
                        .iter()
                        .flat_map(|body| &body.block.stmts)
                        .all(|statement| !matches!(
                            &statement.kind,
                            bir::StatementKind::Call {
                                callee: bir::Callee::Helper(bir::HelperOp::RaiseValueError),
                                ..
                            }
                        )),
                    "absent or foreign private proof granted the helper"
                );
            }
        }
    }
    Ok(())
}

/// Publish the complete unchanged UUID producer and require the actual layout/constant closure, not just retained
/// values or membership in a public API registry.
///
/// The smaller value-enum regression below separately covers the generated getter that this closure requires.
#[test]
#[ignore = "Requires the complete data-package source/dependency context; authoritative verification uses the real SDK publisher"]
fn standard_uuid_constant_closure_is_publishable() -> Result<(), Box<dyn std::error::Error>> {
    let sources = [
        (
            include_str!("../../../../../stdlib/data/src/collections.incn"),
            vec!["collections".to_string()],
        ),
        (
            include_str!("../../../../../stdlib/data/src/uuid.incn"),
            vec!["uuid".to_string()],
        ),
    ];
    let mut checked = Vec::new();
    let mut public = BTreeSet::new();
    for (source, path) in sources {
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_standard_library_source(true);
        checker.set_current_module_path(Some(path.clone()));
        checker.set_current_package_identity(Some("incan_stdlib_data".into()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
        let manifest =
            crate::library_manifest::LibraryManifest::from_checked_exports("incan_stdlib_data", "0.5.0", &exports);
        public.extend(crate::library_manifest::published_layout::public_executable_identities(
            &manifest,
        ));
        checked.push((program, path, checked_standard_package_facts(checker.type_info())));
    }
    let initial = checked
        .iter()
        .map(|(program, path, facts)| build_body_ir_module_v0(program, path, facts))
        .collect::<Vec<_>>();
    let mut modules = checked
        .iter()
        .map(|(program, path, facts)| {
            crate::body_ir::build_body_ir_module_v0_with_executable_context(program, path, facts, &initial)
        })
        .collect::<Vec<_>>();
    for module in &mut modules {
        for implementation in &mut module.trait_implementations {
            for identity in [
                &mut implementation.owner,
                &mut implementation.method,
                &mut implementation.implementation,
            ] {
                if let Some(published) = crate::body_ir::library_nominals::sdk_identity(identity) {
                    *identity = published;
                }
            }
        }
    }
    let bytes = build_surface(&modules, "incan_stdlib_data", "0.5.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    for (identity, coverage) in &reader.index().declarations {
        if [
            "UUID",
            "MAX",
            "NIL",
            "ordinal_bytes",
            "__str__",
            "to_bytes",
            "from_ordinal_bytes",
        ]
        .contains(&identity.declaration_name.as_str())
        {
            eprintln!("UUID closure {}: {coverage:?}", identity.declaration_name);
        }
    }
    for name in [
        "UUID",
        "NIL",
        "MAX",
        "NAMESPACE_DNS",
        "NAMESPACE_URL",
        "NAMESPACE_OID",
        "NAMESPACE_X500",
    ] {
        let identity = public
            .iter()
            .find(|identity| identity.declaration_name == name)
            .ok_or("public UUID root absent")?;
        assert!(
            reader.covers(identity),
            "actual UUID root {name} uncovered: {:?}",
            reader.index().coverage(identity)
        );
    }
    Ok(())
}

/// A generated value-enum getter executes through its checked owner, without a separately declared method body.
#[test]
fn generated_value_enum_getter_publishes_its_owner_context() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "pub enum Code(str):\n    Bad = \"bad\"\n\n    def as_str(self) -> str:\n        return self.value()\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["lib".into()]));
    checker.set_current_package_identity(Some("value_getter".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &["lib".into()], checker.type_info());
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "as_str")
        .ok_or("as_str absent")?;
    let root = body.canonical.clone().ok_or("as_str identity absent")?;
    let mut getter = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => target.canonical.clone(),
            _ => None,
        })
        .ok_or("generated getter absent")?;
    if let incan_semantics_core::SymbolOrigin::Module(module_path) = &getter.origin {
        getter.origin = incan_semantics_core::SymbolOrigin::Package {
            library: "value_getter".into(),
            module_path: module_path.clone(),
        };
    }
    let mut owner = module
        .value_enum_declarations
        .first()
        .ok_or("value enum absent")?
        .canonical
        .clone();
    if let incan_semantics_core::SymbolOrigin::Module(module_path) = &owner.origin {
        owner.origin = incan_semantics_core::SymbolOrigin::Package {
            library: "value_getter".into(),
            module_path: module_path.clone(),
        };
    }
    let public = BTreeSet::from([root.clone()]);
    let bytes = build_surface(
        std::slice::from_ref(&module),
        "value_getter",
        "0.1",
        &public,
        &BTreeSet::new(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        reader.covers(&root),
        "generated getter stranded public body: {:?}",
        reader.index().coverage(&root)
    );
    assert!(
        matches!(reader.index().execution_coverage(&getter), Ok(DeclarationCoverage::TypeContext {owner: actual}) if actual == &owner)
    );
    assert!(matches!(
        reader.index().execution_coverage(&owner),
        Ok(DeclarationCoverage::Covered { .. })
    ));
    assert!(
        reader.index().coverage(&getter).is_err(),
        "generated getters must not become public resolution roots"
    );
    assert!(
        !reader.covers(&owner),
        "a private execution owner must not become a public export"
    );
    for corruption in 0..8 {
        let mut changed = module.clone();
        if corruption == 0 {
            changed.value_enum_declarations.clear();
        } else if corruption == 1 {
            changed.value_enum_declarations[0].canonical.declaration_span.end -= 1;
        } else if corruption == 5 {
            changed
                .value_enum_declarations
                .push(changed.value_enum_declarations[0].clone());
        } else if corruption == 6 {
            changed.value_enum_declarations[0].canonical.kind = incan_semantics_core::SemanticSourceTargetKind::Class;
        } else if corruption == 7 {
            changed.value_enum_declarations[0].canonical.origin = incan_semantics_core::SymbolOrigin::Builtin;
        } else {
            for body in &mut changed.bodies {
                for statement in &mut body.block.stmts {
                    if let bir::StatementKind::Call {
                        callee: bir::Callee::Method(target),
                        ..
                    } = &mut statement.kind
                    {
                        let identity = target.canonical.as_mut().ok_or("getter identity absent")?;
                        match corruption {
                            2 => identity.declaration_span.end -= 1,
                            3 => identity.namespace = incan_semantics_core::SymbolNamespace::OrdinaryLexical,
                            _ => identity.declaration_name = "other".into(),
                        }
                    }
                }
            }
        }
        let bytes = build_surface(&[changed], "value_getter", "0.1", &public, &BTreeSet::new())?;
        assert!(
            !SurfaceReader::open(&bytes)?.covers(&root),
            "corrupt getter authority {corruption} admitted"
        );
    }
    Ok(())
}

/// Source-backed unit checking leaves imported standard references as modules. Use the existing catalog ownership
/// projection on cloned checked identity facts, matching SDK package origins without touching any numeric payload,
/// source declaration, or legacy checker state. Production consumers retain the provider-plan authority boundary.
fn checked_standard_package_facts(original: &crate::typechecker::TypeCheckInfo) -> crate::typechecker::TypeCheckInfo {
    let mut facts = original.clone();
    for identity in facts
        .declarations
        .named_type_identities
        .values_mut()
        .chain(facts.declarations.resolved_import_identities.values_mut())
        .chain(facts.references.resolved_identities.values_mut())
        .chain(facts.traits.method_identities.values_mut())
        .chain(facts.traits.provider_slot_identities.values_mut())
        .chain(facts.traits.inferred_method_identities.values_mut())
        .chain(facts.traits.self_method_identities.values_mut())
    {
        if let Some(published) = crate::body_ir::library_nominals::sdk_identity(identity) {
            *identity = published;
        }
    }
    for adoption in facts.declarations.body_ir_trait_adoptions.values_mut().flatten() {
        if let Some(published) = crate::body_ir::library_nominals::sdk_identity(&adoption.trait_identity) {
            adoption.trait_identity = published;
        }
        if let Some(methods) = &mut adoption.methods {
            for method in methods {
                if let Some(published) = crate::body_ir::library_nominals::sdk_identity(method) {
                    *method = published;
                }
            }
        }
    }
    facts
}

/// The unchanged Core iterator producers must preserve their complete publication closure, including the callable
/// trait headers their private adapters depend on, before another SDK is built.
#[test]
fn standard_core_iterator_closures_are_publishable() -> Result<(), Box<dyn std::error::Error>> {
    let sources = [
        (
            include_str!("../../../../../stdlib/core/src/traits/callable.incn"),
            vec!["traits".to_string(), "callable".to_string()],
        ),
        (
            include_str!("../../../../../stdlib/core/src/derives/collection.incn"),
            vec!["derives".to_string(), "collection".to_string()],
        ),
    ];
    let mut modules = Vec::new();
    let mut public = BTreeSet::new();
    for (source, path) in sources {
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_standard_library_source(true);
        checker.set_current_module_path(Some(path.clone()));
        checker.set_current_package_identity(Some("incan_stdlib_core".into()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
        let manifest =
            crate::library_manifest::LibraryManifest::from_checked_exports("incan_stdlib_core", "0.6", &exports);
        public.extend(crate::library_manifest::published_layout::public_executable_identities(
            &manifest,
        ));
        let facts = checked_standard_package_facts(checker.type_info());
        modules.push(build_body_ir_module_v0(&program, &path, &facts));
    }
    let bytes = build_surface(&modules, "incan_stdlib_core", "0.6", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    let mut roots = 0;
    for body in modules.iter().flat_map(|module| &module.bodies) {
        if ["take", "filter", "map", "map_err", "enumerate"].contains(&body.name.as_str()) {
            let identity = body.canonical.as_ref().ok_or("standard root identity absent")?;
            roots += 1;
            assert!(
                reader.covers(identity),
                "standard iterator root {} uncovered: {:?}",
                body.name,
                reader.index().coverage(identity)
            );
        }
    }
    assert!(roots >= 8, "standard iterator defaults absent");
    Ok(())
}

/// A checked immutable newtype constant is materialized before borrowing its method receiver.
#[test]
fn constructor_constant_method_receiver_has_local_storage() -> Result<(), Box<dyn std::error::Error>> {
    let (module, _) = checked_constant_module()?;
    let body = body_named(&module, "maximum_bits")?.render_snapshot();
    assert!(!body.contains("@const:"), "immutable receiver remained global: {body}");
    assert!(body.contains("const<newtype>"), "receiver value missing: {body}");
    Ok(())
}
