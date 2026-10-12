//! SDK type dependencies retain declaring identities even without consumer imports.

use super::*;
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::SymbolOrigin;

/// The SDK's error layout names its Codec field type in its own module, independently of consumer bindings.
#[test]
fn stdlib_layout_fields_retain_nested_type_identities() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "compression".to_owned(), "_core".to_owned()];
    let mut identity = cache
        .lookup_identity(&path, "CompressionError")
        .ok_or("missing error identity")?;
    identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_compression".to_owned(),
        module_path: path[1..].to_vec(),
    };
    let mut info = TypeCheckInfo::default();
    info.declarations
        .named_type_identities
        .insert("CompressionError".to_owned(), identity);
    let identities = library_type_identities(&info);
    assert!(
        identities.iter().any(|identity| identity.declaration_name == "Codec"),
        "a nested SDK layout must retain Codec: {identities:?}"
    );
    Ok(())
}

/// A checked source-module binding to a generic SDK model retains its owner binder and field types.
#[test]
fn stdlib_generic_model_retains_declared_type_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "collections".to_owned()];
    let identity = cache.lookup_identity(&path, "Deque").ok_or("missing deque identity")?;
    let mut info = TypeCheckInfo::default();
    info.declarations
        .named_type_identities
        .insert("Deque".to_owned(), identity);
    let declarations = library_nominal_declarations(&info);
    let deque = declarations
        .iter()
        .find(|declaration| declaration.name == "Deque")
        .ok_or("missing SDK deque")?;
    assert_eq!(deque.type_parameters, vec!["T"]);
    assert_eq!(deque.type_parameter_count, 1);
    assert!(matches!(deque.canonical.origin, SymbolOrigin::Package { .. }));
    assert_eq!(deque.fields, vec!["items"]);
    Ok(())
}
