//! A class fills a Rust trait it adopts with its own methods; a class a linked Loaf declares keeps the trait
//! implementation that Loaf compiled.

use incan_mir_plan_native::{NativeTraitAdoption, NativeTraitMethod};
use incan_semantics_core::body_ir::{BodyIrModule, TraitAdoption, TraitImplementation};
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use super::bodies::empty_body;
use super::model_types::nominal;
use crate::classes::function_name;
use crate::native_traits::native_trait_adoptions;

const REFUSAL: &str = "unsupported Body IR Rust trait method without its body";

/// An item of the Rust crate `tiny`, as the checker names a Rust trait and its method slots.
fn native(path: &[&str], kind: SemanticSourceTargetKind, namespace: SymbolNamespace) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace,
        origin: SymbolOrigin::RustCrate(path.iter().map(|segment| (*segment).to_owned()).collect()),
        declaration_name: path.last().map_or_else(String::new, |name| (*name).to_owned()),
        kind,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 0),
    }
}

/// The class `Holder` and its method `read`, declared under `origin`.
fn holder(origin: SymbolOrigin) -> (CanonicalSymbolId, CanonicalSymbolId) {
    let owner = CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: origin.clone(),
        declaration_name: "Holder".into(),
        kind: SemanticSourceTargetKind::Class,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 100),
    };
    let method = CanonicalSymbolId {
        namespace: SymbolNamespace::Member,
        origin,
        declaration_name: "read".into(),
        kind: SemanticSourceTargetKind::Method,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(40, 90),
    };
    (owner, method)
}

/// A module holding `owner`, which adopts the Rust trait `tiny::Source` and fills its `read` slot with `method`. The
/// module holds no body: each test adds the bodies its unit has.
fn adopting_module(owner: &CanonicalSymbolId, method: &CanonicalSymbolId, native_item: &str) -> BodyIrModule {
    let slot = native(
        &["tiny", "Source", "read"],
        SemanticSourceTargetKind::Method,
        SymbolNamespace::Member,
    );
    let mut declaration = nominal(owner.clone(), "Holder", &[]);
    declaration.native_item = native_item.into();
    declaration.trait_adoptions.push(TraitAdoption {
        trait_identity: native(
            &["tiny", "Source"],
            SemanticSourceTargetKind::Trait,
            SymbolNamespace::OrdinaryLexical,
        ),
        arguments: Vec::new(),
        methods: Some(vec![slot.clone()]),
        fields: Vec::new(),
        primitive_adapter: None,
    });
    let mut module = BodyIrModule::declarations_only("app".into());
    module.nominal_declarations.push(declaration);
    module.trait_implementations.push(TraitImplementation {
        owner: owner.clone(),
        method: slot,
        implementation: method.clone(),
        slot_arguments: Vec::new(),
        type_arguments: Vec::new(),
    });
    module
}

/// A class this unit declares implements the trait with the planned function of its own method.
#[test]
fn a_class_fills_a_rust_trait_with_its_own_method() -> Result<(), String> {
    let (owner, method) = holder(SymbolOrigin::Module(vec!["app".into()]));
    let mut module = adopting_module(&owner, &method, "");
    let mut body = empty_body("read");
    body.canonical = Some(method);
    module.bodies.push(body);
    let function = function_name(&module, &module.bodies[0]);
    let expected = vec![NativeTraitAdoption {
        path: "tiny::Source".into(),
        methods: vec![NativeTraitMethod {
            name: "read".into(),
            function,
        }],
    }];
    assert!(native_trait_adoptions(&module, 0)? == expected);
    Ok(())
}

/// A class this unit declares, whose method filling the slot has no body here, is refused rather than left without an
/// implementation.
#[test]
fn a_class_of_this_unit_without_its_method_body_is_refused() {
    let (owner, method) = holder(SymbolOrigin::Module(vec!["app".into()]));
    let module = adopting_module(&owner, &method, "");
    assert!(native_trait_adoptions(&module, 0).err().as_deref() == Some(REFUSAL));
}

/// A class a linked Loaf declares, such as the project's shared library a test unit links, is that Loaf's compiled
/// item: the Loaf already implements the trait, and its method bodies became linked calls, so this unit plans no
/// second implementation (Oven's tests naming `PreparedBodyRecordSource`, which adopts the Rust `RecordSource`).
#[test]
fn a_linked_loafs_class_keeps_its_compiled_trait_implementation() -> Result<(), String> {
    let (owner, method) = holder(SymbolOrigin::Package {
        library: "app_project_library".into(),
        module_path: vec!["holder".into()],
    });
    let module = adopting_module(&owner, &method, "app_project_library::holder::Holder");
    assert!(native_trait_adoptions(&module, 0)?.is_empty());
    Ok(())
}
