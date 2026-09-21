//! Lowering results that emission has to honor: the lowering tests that need an emission pass or plan.

use incan_ir::decl::IrDeclKind;
use incan_ir::expr::{BinOp, IrExprKind};
use incan_ir::lower::AstLowering;
use incan_ir::stmt::{AssignTarget, IrStmtKind};
use incan_ir::types::{IR_UNION_TYPE_NAME, IrType, manifest_type_ref_from_ir};

use crate::conversions::{BinOpEmitKind, determine_binop_plan};
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{ast, lexer, parser};

#[test]
fn foreign_union_keeps_its_producer_wrapper_identity() -> Result<(), Box<dyn std::error::Error>> {
    use incan_frontend::library_manifest::TypeRef;
    let program = incan_frontend::parser::parse(
        &incan_frontend::lexer::lex("pub type Answer = Product | int\n").map_err(|error| format!("{error:?}"))?,
    )
    .map_err(|error| format!("{error:?}"))?;
    let ast::Declaration::TypeAlias(alias) = &program.declarations[0].node else {
        return Err("type alias absent".into());
    };
    // This is the production declaration-lowering path for an already-imported local Product binding.
    let producer = AstLowering::new().lower_type(&alias.target.node);
    let mut published = TypeRef::Applied {
        name: IR_UNION_TYPE_NAME.into(),
        origin: None,
        args: vec![
            TypeRef::Named {
                name: "Product".into(),
                origin: None,
            },
            TypeRef::Named {
                name: "int".into(),
                origin: None,
            },
        ],
    };
    let legacy = AstLowering::new().lower_pub_manifest_type_ref("pricing", &published);
    assert_eq!(
        legacy.union_type_name(),
        producer.union_type_name(),
        "legacy local alias spelling preserved the declaring wrapper"
    );
    let origin = incan_frontend::library_manifest::NominalTypeOriginExport {
        provider: incan_frontend::provider::ProviderIdentity {
            name: "catalog".into(),
            version: "1.2.3".into(),
            digest: "a".repeat(64),
            feature_projection: Default::default(),
        },
        canonical: incan_frontend::library_manifest::CanonicalIdentityExport {
            namespace: incan_frontend::library_manifest::CanonicalIdentityNamespaceExport::OrdinaryLexical,
            origin: incan_frontend::library_manifest::CanonicalIdentityOriginExport::Package {
                library: "catalog".into(),
                module_path: vec!["lib".into()],
            },
            declaration_name: "Product".into(),
            kind: "model".into(),
            declaration_span: incan_frontend::library_manifest::CanonicalIdentitySpanExport { start: 0, end: 20 },
        },
    };
    if let TypeRef::Applied { args, .. } = &mut published {
        args[0] = TypeRef::Named {
            name: "Product".into(),
            origin: Some(origin.clone()),
        };
    }
    let mut facts = incan_frontend::typechecker::TypeCheckInfo::default();
    facts.declarations.foreign_pub_type_remappings.insert(
        "pricing".into(),
        std::collections::HashMap::from([(
            origin.binding_key(),
            "pub::pricing::__incan_provider_rust::catalog::Product".into(),
        )]),
    );
    // Obtain the wrapper from real emission, then publish the exact ordered table entry with checked leaf origins.
    let lowered_program = AstLowering::new().lower_program(&program)?;
    let mut emitter = crate::IrEmitter::new(&lowered_program.function_registry);
    let _ = emitter.emit_program(&lowered_program)?;
    let definitions = emitter.emitted_native_union_types();
    let (wrapper, emitted) = definitions.iter().next().ok_or("emitter produced no union")?;
    let native = incan_frontend::library_manifest::NativeUnionExport {
        owner: incan_frontend::library_manifest::NativeUnionOwnerExport::ContainingArtifact,
        rust_name: wrapper.clone(),
        local_nominals: Default::default(),
        members: emitted
            .union_members()
            .ok_or("emitted union has no members")?
            .iter()
            .map(|member| {
                manifest_type_ref_from_ir(member).map(|ty| {
                    incan_frontend::library_manifest::with_checked_type_origins(
                        ty,
                        &std::collections::BTreeMap::from([("Product".into(), origin.clone())]),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        checked_projection: None,
    };
    let mut manifest = incan_frontend::library_manifest::LibraryManifest::new("pricing", "1.0.0");
    manifest.contract_metadata.native_unions.push(native.clone());
    manifest
        .exports
        .type_aliases
        .push(incan_frontend::library_manifest::TypeAliasExport {
            name: "Answer".into(),
            type_params: Vec::new(),
            target: TypeRef::NativeUnion(native.clone()),
        });
    manifest
        .exports
        .functions
        .push(incan_frontend::library_manifest::FunctionExport {
            name: "echo".into(),
            emitted_name: None,
            type_params: Vec::new(),
            params: vec![incan_frontend::library_manifest::ParamExport {
                name: "value".into(),
                ty: TypeRef::NativeUnion(native.clone()),
                kind: Default::default(),
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::NativeUnion(native.clone()),
            is_async: false,
        });
    let artifact = incan_frontend::library_manifest_index::LibraryArtifactMetadata::from_crate_root(
        "pricing",
        "pricing",
        std::path::Path::new("/checked/pricing"),
    );
    let index =
        incan_frontend::library_manifest_index::LibraryManifestIndex::from_entries(std::collections::HashMap::from([
            (
                "pricing".into(),
                incan_frontend::library_manifest_index::LibraryManifestIndexEntry::Loaded {
                    manifest: Box::new(manifest.clone()),
                    metadata: artifact.clone(),
                },
            ),
        ]));
    let record = incan_frontend::provider::ProviderRecord {
        identity: incan_frontend::provider::ProviderIdentity {
            name: "pricing".into(),
            version: "1.0.0".into(),
            digest: "b".repeat(64),
            feature_projection: Default::default(),
        },
        provenance: incan_frontend::provider::ProviderProvenance::ProjectDependency {
            dependency_key: "pricing".into(),
            manifest_path: artifact.manifest_path.clone(),
        },
        authority: incan_frontend::provider::NamespaceAuthority::ProjectDependency {
            dependency_key: "pricing".into(),
        },
        namespace_claims: Default::default(),
        available: true,
        enabled: true,
        manifest: Some(std::sync::Arc::new(manifest)),
        artifact: Some(artifact),
        implementation_facets: Vec::new(),
    };
    let plan = incan_frontend::provider::ProviderPlan::new(index, vec![record], [])?;
    facts
        .declarations
        .named_type_origins
        .insert("pub::stock::Product".into(), origin.clone());
    let mut facade = incan_frontend::library_manifest::LibraryManifest::new("facade", "1.0.0");
    let aliases = ["Answer", "echo"]
        .into_iter()
        .map(|name| {
            incan_frontend::api_metadata::ApiDeclaration::Alias(incan_frontend::api_metadata::ApiAlias {
                name: name.into(),
                anchor: incan_frontend::api_metadata::SourceAnchor {
                    id: name.into(),
                    span: incan_frontend::api_metadata::SourceSpan { start: 0, end: 1 },
                },
                target_path: vec!["pub".into(), "pricing".into(), name.into()],
                is_public: true,
                projected_function: None,
                projected_type: None,
            })
        })
        .collect();
    facade.contract_metadata.api = Some(incan_frontend::api_metadata::CheckedApiMetadataPackage {
        schema_version: incan_frontend::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![incan_frontend::api_metadata::CheckedApiMetadata {
            schema_version: incan_frontend::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION,
            derivable_traits: Vec::new(),
            module_path: vec!["lib".into()],
            declarations: aliases,
        }],
        public_namespaces: Vec::new(),
    });
    facade.exports.aliases = ["Answer", "echo"]
        .into_iter()
        .map(|name| incan_frontend::library_manifest::AliasExport {
            name: name.into(),
            target_path: vec!["pub".into(), "pricing".into(), name.into()],
            projected_function: None,
            projected_type: None,
        })
        .collect();
    crate::emit::native_unions::preserve_native_aliases(&mut facade, Some(&plan))?;
    assert!(
        facade.contract_metadata.native_unions.is_empty(),
        "a facade must not claim a local native definition"
    );
    let forwarded = facade.exports.aliases[0]
        .projected_type
        .as_ref()
        .ok_or("facade lost its type alias")?;
    let TypeRef::NativeUnion(forwarded) = forwarded else {
        return Err("facade erased native type carrier".into());
    };
    let expected_owner = incan_frontend::provider::ProviderIdentity {
        name: "pricing".into(),
        version: "1.0.0".into(),
        digest: "b".repeat(64),
        feature_projection: Default::default(),
    };
    assert_eq!(
        forwarded.owner,
        incan_frontend::library_manifest::NativeUnionOwnerExport::SelectedArtifact(expected_owner.clone())
    );
    assert_eq!(forwarded.rust_name, *wrapper);
    assert_eq!(
        facade.exports.aliases[1]
            .projected_function
            .as_ref()
            .ok_or("facade lost its callable")?
            .params[0]
            .ty,
        TypeRef::NativeUnion(forwarded.clone())
    );
    let mut wrong_owner = forwarded.clone();
    let mut other_artifact = expected_owner;
    other_artifact.digest = "c".repeat(64);
    wrong_owner.owner = incan_frontend::library_manifest::NativeUnionOwnerExport::SelectedArtifact(other_artifact);
    assert!(plan.public_native_union_projection("pricing", &wrong_owner).is_err());
    facts.declarations.named_type_origins.insert(
        "pub::pricing::__incan_provider_rust::catalog::Product".into(),
        origin.clone(),
    );
    let mut lowering = AstLowering::new_with_type_info(facts);
    lowering.set_provider_plan(Some(std::sync::Arc::new(plan.clone())));
    let published = TypeRef::NativeUnion(native.clone());
    let consumer = lowering.lower_pub_manifest_type_ref("pricing", &published);
    assert!(lowering.metadata_errors.borrow().is_empty());
    assert_eq!(consumer.union_type_name(), producer.union_type_name());
    assert_eq!(consumer.rust_name(), format!("::pricing::{wrapper}"));
    let product_index = emitted
        .union_members()
        .ok_or("missing emitted members")?
        .iter()
        .position(|member| matches!(member, IrType::Struct(name) if name == "Product"))
        .ok_or("missing Product variant")?;
    assert_eq!(
        consumer.union_variant_index_for_member(&IrType::Struct("stock::Product".into())),
        Some(product_index)
    );
    assert_eq!(
        incan_ir::types::isinstance_union_variant_indices(&consumer, &IrType::Struct("stock::Product".into())),
        Some(vec![product_index])
    );
    let mut forged = native;
    forged.members.reverse();
    assert!(plan.public_native_union_projection("pricing", &forged).is_err());
    forged.rust_name = "__IncanUnion_not_emitted".into();
    assert!(plan.public_native_union_projection("pricing", &forged).is_err());
    Ok(())
}

#[test]
fn rust_string_slice_import_add_assign_preserves_owned_rhs_issue896() -> Result<(), String> {
    let source = r#"
from rust::incan_std_core::strings import str_slice_byte_range

def concat_slice(text: str) -> str:
    mut out = ""
    out += str_slice_byte_range(text, 0, 1)
    return out
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    let program = lowering
        .lower_program(&ast)
        .map_err(|err| format!("lowering failed: {err:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == "concat_slice" => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected lowered function `concat_slice`".to_string())?;
    let value = function
        .body
        .iter()
        .find_map(|statement| match &statement.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Var { name, .. },
                value,
            } if name == "out" && matches!(value.kind, IrExprKind::BinOp { .. }) => Some(value),
            _ => None,
        })
        .ok_or_else(|| format!("expected lowered `out += ...` assignment, got {:?}", function.body))?;
    let IrExprKind::BinOp { op, left, right } = &value.kind else {
        return Err(format!("expected binary addition, got {:?}", value.kind));
    };

    assert_eq!(*op, BinOp::Add);
    assert_eq!(left.ty, IrType::String);
    assert_eq!(right.ty, IrType::String);
    let BinOpEmitKind::StdlibCall { path, borrow_args } = determine_binop_plan(op, left, right).emit else {
        return Err("owned Rust string RHS should lower through a stdlib call".to_string());
    };
    assert_eq!(path.to_string(), "incan_std_core :: strings :: str_concat");
    assert!(borrow_args);
    Ok(())
}
